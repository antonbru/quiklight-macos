//! Общие цветовые утилиты: тип [`Rgb`] и конверсии цветовых пространств.
//!
//! Модуль не зависит от HID/UI — используется и ядром (`sync`), и примерами
//! (`examples/sync_probe.rs`), чтобы формулы цвета жили в одном месте.

/// Цвет в sRGB: каналы `(R, G, B)`, каждый 0..=255.
pub type Rgb = (u8, u8, u8);

/// HSV → RGB.
///
/// - `h` — тон в градусах; значение вне `0..360` берётся по модулю;
/// - `s` — насыщенность `0.0..=1.0`;
/// - `v` — значение (яркость) `0.0..=1.0`.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h as i32).rem_euclid(360) / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), (0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), (0, 0, 255));
    }

    #[test]
    fn hsv_zero_saturation_is_gray() {
        assert_eq!(hsv_to_rgb(200.0, 0.0, 0.5), (128, 128, 128));
    }

    #[test]
    fn hsv_wraps_hue() {
        assert_eq!(hsv_to_rgb(360.0, 1.0, 1.0), hsv_to_rgb(0.0, 1.0, 1.0));
        assert_eq!(hsv_to_rgb(-120.0, 1.0, 1.0), hsv_to_rgb(240.0, 1.0, 1.0));
    }
}
