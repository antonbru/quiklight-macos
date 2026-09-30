//! Захват экрана через ScreenCaptureKit (objc2) для screen sync.
//!
//! Используется `SCScreenshotManager` (периодический захват, macOS 14.0+):
//! на каждом кадре запрашивается CGImage главного дисплея, который рисуется
//! в BGRA-буфер известного формата и отдаётся вызывающему.
//!
//! На macOS 13 (где `SCScreenshotManager` отсутствует) возвращается
//! [`CaptureError::Unsupported`] — статичные режимы при этом работают.

use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::AnyThread;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};

/// Захваченный кадр в формате BGRA (B, G, R, A по байтам, непремноженный).
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// Число байт на строку (может быть больше `width * 4` — выравнивание).
    pub pitch: u32,
    /// Пиксели BGRA, длина `pitch * height`.
    pub data: Vec<u8>,
}

/// Ошибки захвата экрана.
#[derive(Debug)]
pub enum CaptureError {
    /// `SCScreenshotManager` недоступен — screen sync требует macOS 14+.
    Unsupported,
    /// Нет разрешения «Запись экрана» (TCC) или ошибка ScreenCaptureKit.
    ScreenCapture(String),
    /// Не найден ни один дисплей для захвата.
    NoDisplay,
    /// Ответ ScreenCaptureKit не пришёл за таймаут.
    Timeout,
    /// Прочие ошибки.
    Failed(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Unsupported => {
                write!(f, "screen sync требует macOS 14+ (SCScreenshotManager)")
            }
            CaptureError::ScreenCapture(e) => write!(f, "ScreenCaptureKit: {e}"),
            CaptureError::NoDisplay => write!(f, "нет доступных дисплеев для захвата"),
            CaptureError::Timeout => write!(f, "таймаут захвата экрана"),
            CaptureError::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Захватчик главного дисплея.
pub struct ScreenCapturer {
    filter: Retained<SCContentFilter>,
    config: Retained<SCStreamConfiguration>,
}

/// Максимальная сторона захватываемого кадра в пикселях: для ambilight хватает
/// маленького разрешения — меньше CPU и памяти.
const MAX_CAPTURE_DIMENSION: u32 = 480;
/// Таймаут на первый ответ SCShareableContent (первый вызов может быть долгим).
const CONTENT_TIMEOUT: Duration = Duration::from_secs(10);
/// Таймаут на захват одного кадра.
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

impl ScreenCapturer {
    /// Создать захватчик главного дисплея (блокирующий вызов).
    ///
    /// Может потребовать разрешения «Запись экрана» — при отсутствии возвращает
    /// [`CaptureError::ScreenCapture`] с описанием.
    pub fn new() -> Result<Self, CaptureError> {
        // SCScreenshotManager появился в macOS 14.0 — на более старых версиях
        // класс отсутствует, и screen sync невозможен (см. `docs/PROTOCOL.md`).
        if AnyClass::get(c"SCScreenshotManager").is_none() {
            return Err(CaptureError::Unsupported);
        }

        let display = main_display()?;
        let (w, h) = scaled_size(unsafe { display.width() }, unsafe { display.height() });

        // Фильтр: главный дисплей целиком, без исключённых окон.
        let filter = unsafe {
            let excluded = NSArray::<objc2_screen_capture_kit::SCWindow>::new();
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &excluded,
            )
        };
        // Конфигурация: уменьшенное разрешение, без курсора.
        let config = unsafe {
            let cfg = SCStreamConfiguration::new();
            cfg.setWidth(w as usize);
            cfg.setHeight(h as usize);
            cfg.setShowsCursor(false);
            cfg
        };

        Ok(Self { filter, config })
    }

    /// Синхронно захватить один кадр экрана в формате BGRA.
    pub fn capture_frame(&self) -> Result<CapturedFrame, CaptureError> {
        let (tx, rx) = channel();
        unsafe {
            let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
                let _ = tx.send(cgimage_to_frame(image, error));
            });
            SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                &self.filter,
                &self.config,
                Some(&block),
            );
        }
        match rx.recv_timeout(FRAME_TIMEOUT) {
            Ok(result) => result,
            Err(_) => Err(CaptureError::Timeout),
        }
    }
}

/// Получить главный дисплей через SCShareableContent (блокирующий вызов).
fn main_display() -> Result<Retained<SCDisplay>, CaptureError> {
    let (tx, rx): (Sender<Result<Retained<SCDisplay>, CaptureError>>, _) = channel();
    unsafe {
        let block = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                let _ = tx.send(extract_display(content, error));
            },
        );
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true,
            false,
            &block,
        );
    }
    match rx.recv_timeout(CONTENT_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err(CaptureError::Timeout),
    }
}

/// Достать первый дисплей из ответа SCShareableContent.
unsafe fn extract_display(
    content: *mut SCShareableContent,
    error: *mut NSError,
) -> Result<Retained<SCDisplay>, CaptureError> {
    if !error.is_null() {
        return Err(map_nserror(unsafe { &*error }));
    }
    let Some(content) = (unsafe { content.as_ref() }) else {
        return Err(CaptureError::Failed("SCShareableContent вернул nil".into()));
    };
    let displays = content.displays();
    match displays.firstObject() {
        Some(display) => Ok(display),
        None => Err(CaptureError::NoDisplay),
    }
}

/// Превратить CGImage из ScreenCaptureKit в BGRA-буфер.
///
/// CGImage рисуется в CGBitmapContext с известным форматом (BGRA,
/// premultiplied-first, little-endian) — так мы не зависим от внутренней
/// раскладки пикселей, которую отдаёт ScreenCaptureKit.
unsafe fn cgimage_to_frame(
    image: *mut CGImage,
    error: *mut NSError,
) -> Result<CapturedFrame, CaptureError> {
    if !error.is_null() {
        return Err(map_nserror(unsafe { &*error }));
    }
    let Some(cg) = (unsafe { image.as_ref() }) else {
        return Err(CaptureError::Failed(
            "ScreenCaptureKit вернул пустой CGImage".into(),
        ));
    };

    let w = CGImage::width(Some(cg));
    let h = CGImage::height(Some(cg));
    if w == 0 || h == 0 {
        return Err(CaptureError::Failed("CGImage нулевого размера".into()));
    }

    let bytes_per_row = w * 4;
    let mut buf = vec![0u8; bytes_per_row * h];

    let colorspace = CGColorSpace::new_device_rgb()
        .ok_or_else(|| CaptureError::Failed("не удалось создать device RGB".into()))?;
    let bitmap_info =
        CGBitmapInfo(CGImageAlphaInfo::PremultipliedFirst.0) | CGBitmapInfo::ByteOrder32Host;
    let ctx = CGBitmapContextCreate(
        buf.as_mut_ptr() as *mut _,
        w,
        h,
        8,
        bytes_per_row,
        Some(&colorspace),
        bitmap_info.0,
    )
    .ok_or_else(|| CaptureError::Failed("не удалось создать битовый контекст".into()))?;

    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&ctx), rect, Some(cg));

    Ok(CapturedFrame {
        width: w as u32,
        height: h as u32,
        pitch: bytes_per_row as u32,
        data: buf,
    })
}

/// Уменьшить размер захвата: большая сторона → `MAX_CAPTURE_DIMENSION`, аспект
/// сохраняется.
fn scaled_size(width: isize, height: isize) -> (u32, u32) {
    let w = width.max(1) as f64;
    let h = height.max(1) as f64;
    let scale = (MAX_CAPTURE_DIMENSION as f64 / w.max(h)).min(1.0);
    let nw = (w * scale).round().max(1.0) as u32;
    let nh = (h * scale).round().max(1.0) as u32;
    (nw, nh)
}

/// Извлечь текст ошибки NSError (localizedDescription).
fn map_nserror(error: &NSError) -> CaptureError {
    CaptureError::ScreenCapture(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_size_preserves_aspect() {
        // 4K → 480px по большой стороне.
        let (w, h) = scaled_size(3840, 2160);
        assert_eq!((w, h), (480, 270));
        // Экран меньше 480 не увеличивается.
        let (w, h) = scaled_size(400, 300);
        assert_eq!((w, h), (400, 300));
        // Портрет.
        let (w, h) = scaled_size(1080, 1920);
        assert_eq!((w, h), (270, 480));
        // Нулевые размеры не паникуют.
        let (w, h) = scaled_size(0, 0);
        assert!(w >= 1 && h >= 1);
    }
}
