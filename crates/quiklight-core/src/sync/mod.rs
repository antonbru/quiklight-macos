//! Screen sync: захват экрана и превращение его краёв в цвета LED.
//!
//! - `screen_capturer` — захват краёв экрана через ScreenCaptureKit (objc2);
//! - `color_extractor` — из BGRA-кадра: сетка → края → LED, гамма/насыщенность/
//!   light compression/сглаживание;
//! - `led_mapper` — раскладка краёв на физические LED в порядке проводов.

pub mod color_extractor;
pub mod led_mapper;
pub mod screen_capturer;
