//! Настройки приложения QuikLight — общие для UI (`app`) и фонового потока
//! (`engine`). Здесь же — производные значения (эффективное число LED,
//! раскладка, интервал кадров, конфиг постобработки).

use quiklight_core::hid::device_info::DEFAULT_LED_COUNT;
use quiklight_core::sync::color_extractor::PostProcessConfig;
use quiklight_core::sync::led_mapper::EdgeLayout;

/// Настройки приложения.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Число LED ленты; **0 = авто** (брать из `readDeviceInfo`, запасной
    /// [`DEFAULT_LED_COUNT`]).
    pub led_count: u16,
    /// Ручная раскладка по краям; если сумма 0 — автоматическая 17/29/17
    /// под эффективное число LED (см. `EdgeLayout::default_for`).
    pub layout: EdgeLayout,
    /// Частота screen sync, кадров/с (20..=60).
    pub fps: u8,
    /// Гамма (1.0 = без коррекции).
    pub gamma: f32,
    /// Насыщенность (1.0 = без изменений).
    pub saturation: f32,
    /// Временно́е сглаживание кадров.
    pub smoothing: bool,
    /// Light compression (ужать яркость > 255).
    pub light_compression: bool,
    /// Выключать подсветку при выходе из приложения.
    pub turn_off_on_exit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            led_count: 0, // авто — из устройства
            layout: EdgeLayout {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            fps: 30,
            gamma: 1.0,
            saturation: 1.0,
            smoothing: true,
            light_compression: false,
            turn_off_on_exit: true,
        }
    }
}

impl Settings {
    /// Эффективное число LED: ручное, если задано, иначе из устройства
    /// ([`DEFAULT_LED_COUNT`] — запасной).
    pub fn led_count_effective(&self, device_led: Option<u16>) -> u16 {
        if self.led_count > 0 {
            self.led_count
        } else {
            device_led
                .filter(|&n| n > 0)
                .unwrap_or(u16::from(DEFAULT_LED_COUNT))
        }
    }

    /// Эффективная раскладка: ручная, если задана, иначе авто-пропорции
    /// под эффективное число LED.
    pub fn layout_effective(&self, device_led: Option<u16>) -> EdgeLayout {
        if self.layout.total() > 0 {
            self.layout
        } else {
            EdgeLayout::default_for(self.led_count_effective(device_led))
        }
    }

    /// Интервал кадра screen sync в миллисекундах.
    pub fn frame_interval_ms(&self) -> u64 {
        let fps = self.fps.clamp(20, 60) as u64;
        (1000 / fps).clamp(16, 50)
    }

    /// Конфиг постобработки для `ColorExtractor`.
    pub fn postprocess(&self) -> PostProcessConfig {
        PostProcessConfig {
            gamma: self.gamma,
            saturation: self.saturation,
            light_compression: self.light_compression,
            smoothing: self.smoothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn led_count_auto_falls_back_to_device() {
        let s = Settings::default(); // led_count = 0 → авто
        assert_eq!(s.led_count_effective(Some(51)), 51);
        assert_eq!(s.led_count_effective(None), 63); // запасной DEFAULT_LED_COUNT
        assert_eq!(s.led_count_effective(Some(0)), 63);
    }

    #[test]
    fn led_count_manual_wins() {
        let s = Settings {
            led_count: 100,
            ..Settings::default()
        };
        assert_eq!(s.led_count_effective(Some(51)), 100);
    }

    #[test]
    fn layout_auto_uses_device_led() {
        let s = Settings::default();
        let l = s.layout_effective(Some(63));
        assert_eq!((l.left, l.top, l.right, l.bottom), (17, 29, 17, 0));
        let l51 = s.layout_effective(Some(51));
        assert_eq!(l51.total(), 51);
    }

    #[test]
    fn layout_manual_wins() {
        let s = Settings {
            layout: EdgeLayout {
                left: 5,
                top: 10,
                right: 5,
                bottom: 0,
            },
            ..Settings::default()
        };
        assert_eq!(s.layout_effective(Some(51)).total(), 20);
    }

    #[test]
    fn frame_interval_clamped() {
        let mut s = Settings {
            fps: 30,
            ..Settings::default()
        };
        assert_eq!(s.frame_interval_ms(), 33);
        s.fps = 60;
        assert_eq!(s.frame_interval_ms(), 16);
        s.fps = 5; // ниже минимума → 20 fps → 50 мс
        assert_eq!(s.frame_interval_ms(), 50);
        s.fps = 200; // выше максимума → 60 fps → 16 мс
        assert_eq!(s.frame_interval_ms(), 16);
    }
}
