//! HID-транспорт: поиск устройства QuikLight, открытие interface 0,
//! последовательная запись отчётов, команды с ответом.
//!
//! См. `docs/PROTOCOL.md` — «Передача по HID» и «Идентификация устройства».

use std::time::Duration;

use hidapi::{HidApi, HidDevice};

use crate::protocol::actions::{action, set_all_black, turn_off};
use crate::protocol::packet::{chunk_to_hid_reports, sc_frame, HID_REPORT_SIZE};

/// Vendor ID панели QuikLight: `0x1A86` (QinHeng/WCH).
pub const VENDOR_ID: u16 = 0x1A86;
/// Product ID панели: `0xFE07`.
pub const PRODUCT_ID: u16 = 0xFE07;

/// Задержка между записью и чтением для команд с ответом (мс).
pub const RESPONSE_DELAY_MS: u64 = 200;
/// Таймаут чтения для команд с ответом (мс).
pub const READ_TIMEOUT_MS: i32 = 500;

/// Сколько раз повторять action 151 при выключении (PROTOCOL.md, п. 3).
pub const TURN_OFF_REPEATS: u32 = 4;
/// Пауза между повторами выключения (мс).
pub const TURN_OFF_INTERVAL_MS: u64 = 20;

/// Ошибка работы с устройством.
#[derive(Debug)]
pub enum DeviceError {
    /// Устройство не найдено на USB.
    NotFound,
    /// Ошибка инициализации HID API.
    Init(String),
    /// Ошибка открытия устройства.
    Open(String),
    /// Ошибка записи HID-отчёта.
    Write(String),
    /// Ошибка чтения HID-отчёта.
    Read(String),
    /// Ответ на команду не пришёл за таймаут.
    Timeout,
    /// Ответ пришёл, но не похож на ожидаемый кадр.
    BadResponse,
    /// Попытка записать пустой кадр.
    EmptyFrame,
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::NotFound => write!(f, "устройство QuikLight (0x1A86:0xFE07) не найдено"),
            DeviceError::Init(e) => write!(f, "ошибка инициализации HID: {e}"),
            DeviceError::Open(e) => write!(f, "ошибка открытия устройства: {e}"),
            DeviceError::Write(e) => write!(f, "ошибка записи: {e}"),
            DeviceError::Read(e) => write!(f, "ошибка чтения: {e}"),
            DeviceError::Timeout => write!(f, "таймаут ожидания ответа"),
            DeviceError::BadResponse => write!(f, "некорректный ответ устройства"),
            DeviceError::EmptyFrame => write!(f, "пустой кадр"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// Следующий id сессии: монотонно растёт, после 255 возвращается к 1.
fn next_session(current: u8) -> u8 {
    match current {
        255 => 1,
        _ => current + 1,
    }
}

/// Открытое устройство QuikLight.
///
/// Все записи идут через `&mut self` — поэтому чанки одного кадра никогда
/// не перемежаются чужими (требование PROTOCOL.md).
pub struct Device {
    /// HID-устройство (interface 0 — канал управления).
    device: HidDevice,
    /// Счётчик сессии (id кадров). Стартует с 2, как в оригинале.
    session_id: u8,
}

impl Device {
    /// Найти и открыть устройство QuikLight (1a86:fe07) на **interface 0**.
    ///
    /// Важно: у панели два HID-интерфейса. Interface 0 — управление,
    /// interface 1 (keyboard) — сенсорные кнопки, команды цвета игнорирует.
    ///
    /// На macOS `hidapi` возвращает каждый интерфейс отдельной записью, и
    /// «первая попавшаяся» может оказаться keyboard. Это не только бесполезно,
    /// но и приводит к TCC-блокировке (`privilege violation`, Input Monitoring).
    /// Поэтому явно выбираем запись с `interface_number == 0` (поле структуры);
    /// если система не предоставила номер (бывает −1) — берём первую как фолбэк.
    pub fn open_first(api: &HidApi) -> Result<Self, DeviceError> {
        let candidates: Vec<_> = api
            .device_list()
            .filter(|d| d.vendor_id() == VENDOR_ID && d.product_id() == PRODUCT_ID)
            .collect();

        if candidates.is_empty() {
            return Err(DeviceError::NotFound);
        }

        let device_info = candidates
            .iter()
            .find(|d| d.interface_number() == 0)
            .or_else(|| candidates.first())
            .copied()
            .ok_or(DeviceError::NotFound)?;

        let device = device_info
            .open_device(api)
            .map_err(|e| DeviceError::Open(e.to_string()))?;
        Ok(Self {
            device,
            session_id: 2,
        })
    }

    /// Следующий id сессии (монотонно растёт, в u8 оборачивается).
    pub fn next_session_id(&mut self) -> u8 {
        let id = self.session_id;
        self.session_id = next_session(self.session_id);
        id
    }

    /// Погасить подсветку: action 151 ×4 с паузами + «всё чёрным» для старых
    /// прошивок (см. `docs/PROTOCOL.md`, «Подводные камни», п. 2–3).
    ///
    /// Единый сценарий выключения для приложения, `hidprobe` и `sync_probe`.
    pub fn turn_off_all(&mut self) -> Result<(), DeviceError> {
        for _ in 0..TURN_OFF_REPEATS {
            let id = self.next_session_id();
            self.write_frame(&turn_off(id))?;
            std::thread::sleep(Duration::from_millis(TURN_OFF_INTERVAL_MS));
        }
        let id = self.next_session_id();
        self.write_frame(&set_all_black(id))
    }

    /// Отправить SC-кадр screen sync (action 128) с payload цветов RGB.
    ///
    /// Единственная точка сборки SC-кадра: маркер `'S','C'`, 16-битный размер
    /// и порядок `[R,G,B, ...]` заданы в `docs/PROTOCOL.md`.
    pub fn send_screen_sync(&mut self, payload: &[u8]) -> Result<(), DeviceError> {
        let id = self.next_session_id();
        let frame = sc_frame(id, action::SET_SCREEN_SYNC, payload).with_checksum();
        self.write_frame(&frame)
    }

    /// Записать готовый кадр (RB/SC) в устройство, разбивая на HID-отчёты.
    pub fn write_frame(&mut self, frame: &[u8]) -> Result<(), DeviceError> {
        if frame.is_empty() {
            return Err(DeviceError::EmptyFrame);
        }
        let reports = chunk_to_hid_reports(frame);
        for report in &reports {
            self.device
                .write(report)
                .map_err(|e| DeviceError::Write(e.to_string()))?;
        }
        Ok(())
    }

    /// Записать кадр и прочитать ответ (команды с ожиданием ответа).
    ///
    /// По PROTOCOL.md: писать → подождать ~200 мс → читать с таймаутом 500 мс.
    ///
    /// Возвращает **сырой** буфер чтения. Важно: `hidapi` на macOS не включает
    /// report ID в принимаемые данные, а на Linux/Windows — включает. Поэтому
    /// первый байт здесь не срезается: вызывающий должен найти маркер кадра
    /// (`'R','B'` / `'S','C'`) через [`Self::parse_rb_response`].
    pub fn write_with_response(&mut self, frame: &[u8]) -> Result<Vec<u8>, DeviceError> {
        self.write_frame(frame)?;
        std::thread::sleep(std::time::Duration::from_millis(RESPONSE_DELAY_MS));

        let mut buf = [0u8; HID_REPORT_SIZE];
        let n = self
            .device
            .read_timeout(&mut buf, READ_TIMEOUT_MS)
            .map_err(|e| DeviceError::Read(e.to_string()))?;

        if n == 0 {
            return Err(DeviceError::Timeout);
        }
        // НЕ срезаем первый байт: на macOS это был бы потерянный 'R'/'S'.
        // Вызывающий находит маркер кадра (parse_rb_response).
        Ok(buf[..n].to_vec())
    }

    /// Отбросить мусорные байты до начала RB-кадра.
    ///
    /// Иногда панель присылает лишние байты перед кадром. Ищем пару 'R','B'.
    pub fn parse_rb_response(buf: &[u8]) -> Result<Vec<u8>, DeviceError> {
        let start = buf
            .windows(2)
            .position(|w| w == *b"RB")
            .ok_or(DeviceError::BadResponse)?;
        Ok(buf[start..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_increments() {
        assert_eq!(next_session(2), 3);
        assert_eq!(next_session(254), 255);
    }

    #[test]
    fn session_id_wraps_to_one() {
        // после 255 — возврат к 1, а не к 0
        assert_eq!(next_session(255), 1);
    }

    #[test]
    fn parse_rb_response_strips_garbage() {
        // Linux/Windows: report ID 0x00 идёт первым, кадр — после него
        let buf = [0x00, 0x00, 0x52, 0x42, 0x06, 0x02, 0x97, 0x33];
        let parsed = Device::parse_rb_response(&buf).expect("должен найти RB");
        assert_eq!(parsed, &buf[2..]);
    }

    #[test]
    fn parse_rb_response_macos_no_report_id() {
        // macOS: hidapi не отдаёт report ID — кадр начинается сразу с 'R'
        let buf = [0x52, 0x42, 0x06, 0x02, 0x97, 0x33];
        let parsed = Device::parse_rb_response(&buf).expect("должен найти RB");
        assert_eq!(parsed, &buf[..]);
    }

    #[test]
    fn parse_rb_response_no_marker() {
        let buf = [0x00, 0x01, 0x02, 0x03];
        assert!(Device::parse_rb_response(&buf).is_err());
    }
}
