//! Извлечение цветов краёв экрана из BGRA-кадра и постобработка.
//!
//! Конвейер (совместим с SyncLight/SyncRGB `extractor.rs`):
//! 1. кадр → сетка `cols×rows`, средний цвет каждой ячейки (с даунсемплингом);
//! 2. из сетки берутся края (верх/низ/лево/право), чёрный край заменяется
//!    ближайшей внутренней строкой;
//! 3. цвета краёв → LED (`led_mapper`);
//! 4. постобработка на LED: гамма → насыщенность → light compression;
//! 5. временно́е сглаживание (среднее по последним N кадрам).

use std::collections::VecDeque;

use super::led_mapper::{map_to_leds, EdgeColors, EdgeLayout};

/// Число кадров усреднения при включённом сглаживании (как в SyncRGB).
pub const SMOOTHING_FRAMES: usize = 10;

/// Порог «чёрного» края для замены внутренней строкой (канал < порога).
const BLACK_THRESHOLD: u8 = 10;

pub use crate::color::Rgb;

/// Размер сетки по соотношению сторон экрана (как SyncLight `getAspectRatio`).
pub fn aspect_grid(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (16, 9);
    }
    let ratio = width as f32 / height as f32;
    let close = |target: f32| (ratio - target).abs() < 0.05;
    if close(16.0 / 9.0) || close(21.0 / 9.0) {
        (16, 9)
    } else if close(16.0 / 10.0) {
        (8, 5)
    } else if close(4.0 / 3.0) {
        (4, 3)
    } else {
        (16, 9)
    }
}

/// Средний цвет каждой ячейки сетки `cols×rows` из BGRA-кадра.
///
/// `data` — пиксели BGRA (B, G, R, A), `pitch` — байт на строку, `width`/`height`
/// — размер кадра в пикселях. Даунсемплинг внутри ячейки — шаг не меньше 4 px.
/// Возвращает `cols*rows` цветов, индексация построчно (row-major).
pub fn capture_grid(
    data: &[u8],
    pitch: u32,
    width: u32,
    height: u32,
    cols: u32,
    rows: u32,
) -> Vec<Rgb> {
    if width == 0 || height == 0 || cols == 0 || rows == 0 {
        return Vec::new();
    }
    let cell_w = width / cols;
    let cell_h = height / rows;
    let step = 4u32.max(cell_w / 8).max(cell_h / 8).max(1);

    let mut grid = Vec::with_capacity((cols * rows) as usize);

    for row in 0..rows {
        for col in 0..cols {
            let x0 = col * cell_w;
            let y0 = row * cell_h;
            let x1 = ((col + 1) * cell_w).min(width);
            let y1 = ((row + 1) * cell_h).min(height);

            let mut r_sum = 0u64;
            let mut g_sum = 0u64;
            let mut b_sum = 0u64;
            let mut count = 0u64;

            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    let offset = (y * pitch + x * 4) as usize;
                    if offset + 2 < data.len() {
                        b_sum += data[offset] as u64;
                        g_sum += data[offset + 1] as u64;
                        r_sum += data[offset + 2] as u64;
                        count += 1;
                    }
                    x += step;
                }
                y += step;
            }

            if count > 0 {
                let avg = |sum: u64| (sum.checked_div(count).unwrap_or(0)) as u8;
                grid.push((avg(r_sum), avg(g_sum), avg(b_sum)));
            } else {
                grid.push((0, 0, 0));
            }
        }
    }
    grid
}

fn is_black(c: Rgb) -> bool {
    c.0 < BLACK_THRESHOLD && c.1 < BLACK_THRESHOLD && c.2 < BLACK_THRESHOLD
}

fn row(grid: &[Rgb], cols: u32, r: u32) -> &[Rgb] {
    let start = (r * cols) as usize;
    &grid[start..start + cols as usize]
}

/// Края экрана из сетки, с заменой чёрного края внутренней строкой.
///
/// Возвращает цвета в «экранном» порядке (см. `led_mapper::EdgeColors`):
/// лево/право сверху вниз, верх/низ слева направо.
pub fn extract_border_colors(grid: &[Rgb], cols: u32, rows: u32) -> EdgeColors {
    if grid.is_empty() || cols == 0 || rows == 0 {
        return EdgeColors {
            left: vec![],
            top: vec![],
            right: vec![],
            bottom: vec![],
        };
    }

    // Верх: строка 0; если вся чёрная — строка 1.
    let top1 = row(grid, cols, 0);
    let top: &[Rgb] = if top1.iter().all(|c| is_black(*c)) && rows > 1 {
        row(grid, cols, 1)
    } else {
        top1
    };

    // Низ: строка rows-1; если чёрная — строка rows-2.
    let bot1 = row(grid, cols, rows - 1);
    let bottom: &[Rgb] = if bot1.iter().all(|c| is_black(*c)) && rows > 2 {
        row(grid, cols, rows - 2)
    } else {
        bot1
    };

    let left: Vec<Rgb> = (0..rows).map(|r| grid[(r * cols) as usize]).collect();
    let right: Vec<Rgb> = (0..rows)
        .map(|r| grid[(r * cols + cols - 1) as usize])
        .collect();

    EdgeColors {
        left,
        top: top.to_vec(),
        right,
        bottom: bottom.to_vec(),
    }
}

/// Применить гамма-коррекцию к цвету: `c' = 255 * (c/255)^gamma`.
pub fn apply_gamma(c: Rgb, gamma: f32) -> Rgb {
    if (gamma - 1.0).abs() < f32::EPSILON {
        return c;
    }
    let g = gamma.clamp(0.1, 5.0);
    let f = |v: u8| {
        let norm = (v as f32 / 255.0).powf(g);
        (norm * 255.0).round().clamp(0.0, 255.0) as u8
    };
    (f(c.0), f(c.1), f(c.2))
}

/// Поднять/ослабить насыщенность через HSL: `s' = clamp(s * factor)`.
pub fn apply_saturation(c: Rgb, factor: f32) -> Rgb {
    if (factor - 1.0).abs() < f32::EPSILON {
        return c;
    }
    let (h, s, l) = rgb_to_hsl(c);
    let s2 = (s * factor).clamp(0.0, 1.0);
    hsl_to_rgb(h, s2, l)
}

/// Light compression: если сумма каналов > 255 — нормализовать так, чтобы
/// сумма стала 255 (яркость ужимается, оттенок сохраняется).
pub fn apply_light_compression(c: Rgb) -> Rgb {
    let sum = c.0 as u16 + c.1 as u16 + c.2 as u16;
    if sum <= 255 || sum == 0 {
        return c;
    }
    let k = 255.0 / sum as f32;
    let f = |v: u8| ((v as f32 * k).round().clamp(0.0, 255.0)) as u8;
    (f(c.0), f(c.1), f(c.2))
}

fn rgb_to_hsl(c: Rgb) -> (f32, f32, f32) {
    let r = c.0 as f32 / 255.0;
    let g = c.1 as f32 / 255.0;
    let b = c.2 as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;

    (h, s, l)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    if s.abs() < 1e-6 {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);
    (
        (r * 255.0).round().clamp(0.0, 255.0) as u8,
        (g * 255.0).round().clamp(0.0, 255.0) as u8,
        (b * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/// Настройки постобработки и сглаживания.
#[derive(Debug, Clone, Copy)]
pub struct PostProcessConfig {
    /// Гамма (1.0 = без коррекции).
    pub gamma: f32,
    /// Множитель насыщенности (1.0 = без изменений).
    pub saturation: f32,
    /// Light compression: ужать суммарную яркость > 255.
    pub light_compression: bool,
    /// Временно́е сглаживание (среднее по последним N кадрам).
    pub smoothing: bool,
}

impl Default for PostProcessConfig {
    fn default() -> Self {
        Self {
            gamma: 1.0,
            saturation: 1.0,
            light_compression: false,
            smoothing: true,
        }
    }
}

/// Извлекатель цветов экрана: хранит сетку и историю для сглаживания.
pub struct ColorExtractor {
    cols: u32,
    rows: u32,
    config: PostProcessConfig,
    history: VecDeque<Vec<Rgb>>,
}

impl ColorExtractor {
    pub fn new(config: PostProcessConfig) -> Self {
        Self {
            cols: 0,
            rows: 0,
            config,
            history: VecDeque::with_capacity(SMOOTHING_FRAMES + 1),
        }
    }

    pub fn set_config(&mut self, config: PostProcessConfig) {
        self.config = config;
    }

    /// Полный конвейер: BGRA-кадр → цвета LED в порядке проводов.
    pub fn extract(
        &mut self,
        data: &[u8],
        pitch: u32,
        width: u32,
        height: u32,
        layout: &EdgeLayout,
    ) -> Vec<Rgb> {
        // Сетка пересчитывается при смене разрешения.
        let (c, r) = aspect_grid(width, height);
        if (c, r) != (self.cols, self.rows) {
            self.cols = c;
            self.rows = r;
        }

        let grid = capture_grid(data, pitch, width, height, self.cols, self.rows);
        let edges = extract_border_colors(&grid, self.cols, self.rows);
        let mapped = map_to_leds(&edges, layout);

        // Постобработка на каждом LED.
        let processed: Vec<Rgb> = mapped
            .iter()
            .map(|c| {
                let mut c = *c;
                c = apply_gamma(c, self.config.gamma);
                c = apply_saturation(c, self.config.saturation);
                if self.config.light_compression {
                    c = apply_light_compression(c);
                }
                c
            })
            .collect();

        // Временно́е сглаживание.
        if self.config.smoothing {
            self.history.push_back(processed);
            if self.history.len() > SMOOTHING_FRAMES {
                self.history.pop_front();
            }
            self.averaged()
        } else {
            processed
        }
    }

    fn averaged(&self) -> Vec<Rgb> {
        let n = self.history.len();
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return self.history[0].clone();
        }
        let led_count = self.history[0].len();
        (0..led_count)
            .map(|i| {
                let mut r = 0u32;
                let mut g = 0u32;
                let mut b = 0u32;
                for frame in &self.history {
                    if i < frame.len() {
                        r += frame[i].0 as u32;
                        g += frame[i].1 as u32;
                        b += frame[i].2 as u32;
                    }
                }
                (
                    (r / n as u32) as u8,
                    (g / n as u32) as u8,
                    (b / n as u32) as u8,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Синтетический BGRA-кадр: вся область одного цвета.
    fn solid_frame(width: u32, height: u32, (r, g, b): Rgb) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            data.extend_from_slice(&[b, g, r, 255]);
        }
        data
    }

    #[test]
    fn aspect_ratio_16_9() {
        assert_eq!(aspect_grid(1920, 1080), (16, 9));
        assert_eq!(aspect_grid(2560, 1440), (16, 9));
        assert_eq!(aspect_grid(3440, 1440), (16, 9)); // 21:9 → тоже 16×9, как в SyncRGB
        assert_eq!(aspect_grid(2560, 1600), (8, 5)); // 16:10
        assert_eq!(aspect_grid(1440, 1080), (4, 3));
        assert_eq!(aspect_grid(0, 0), (16, 9));
    }

    #[test]
    fn grid_average_solid_color() {
        // 640×360, сетка 2×2, кадр весь зелёный (BGRA: G=200).
        let frame = solid_frame(640, 360, (0, 200, 0));
        let grid = capture_grid(&frame, 640 * 4, 640, 360, 2, 2);
        assert_eq!(grid.len(), 4);
        for c in grid {
            assert_eq!(c, (0, 200, 0));
        }
    }

    #[test]
    fn grid_average_mixed() {
        // 80×80, сетка 1×1: верхняя половина красная (BGR=0,0,200), нижняя синяя
        // (BGR=200,0,0). Шаг даунсемплинга = max(4, 10, 10) = 10 → ровно 8×8
        // выборок, поровну красных и синих → среднее (100, 0, 100).
        let mut frame = vec![0u8; 80 * 80 * 4];
        for y in 0..80 {
            for x in 0..80 {
                let i = ((y * 80 + x) * 4) as usize;
                if y < 40 {
                    frame[i..i + 3].copy_from_slice(&[0, 0, 200]); // BGR = 0,0,200 → R
                } else {
                    frame[i..i + 3].copy_from_slice(&[200, 0, 0]); // BGR = 200,0,0 → B
                }
                frame[i + 3] = 255;
            }
        }
        let grid = capture_grid(&frame, 320, 80, 80, 1, 1);
        assert_eq!(grid[0], (100, 0, 100)); // (200+0)/2 по R и B
    }

    #[test]
    fn border_colors_order_and_fallback() {
        // Сетка 3×3: верх чёрный, всё остальное белое.
        // Верх должен замениться второй строкой (белой).
        let mut grid = vec![(0, 0, 0); 9];
        for c in grid.iter_mut().skip(3) {
            *c = (255, 255, 255);
        }
        let edges = extract_border_colors(&grid, 3, 3);
        assert_eq!(edges.top.len(), 3);
        assert!(
            edges.top.iter().all(|c| *c == (255, 255, 255)),
            "верх не чёрный: {:?}",
            edges.top
        );
        assert_eq!(edges.left.len(), 3);
        assert_eq!(edges.right.len(), 3);
        assert_eq!(edges.bottom.len(), 3);
        // Левый/правый края: строка 0 — чёрная, но fallback для них не делаем (как SyncRGB).
        assert_eq!(edges.left[0], (0, 0, 0));
        assert_eq!(edges.right[0], (0, 0, 0));
    }

    #[test]
    fn gamma_midpoint() {
        // gamma=2.0: 0.5^2 = 0.25 → ~64.
        assert_eq!(apply_gamma((128, 128, 128), 2.0), (64, 64, 64));
        // gamma=1.0 — без изменений.
        assert_eq!(apply_gamma((128, 90, 200), 1.0), (128, 90, 200));
    }

    #[test]
    fn saturation_boost_and_reduce() {
        // Серый (s=0) остаётся серым при любом множителе.
        assert_eq!(apply_saturation((100, 100, 100), 2.0), (100, 100, 100));
        // Насыщенный красный (255,0,0): s=1 → умножение на 1.5 не даёт выхода за [0,255].
        let boosted = apply_saturation((255, 0, 0), 1.5);
        assert_eq!(boosted.0, 255);
        assert!(boosted.1 < 10 && boosted.2 < 10, "got {boosted:?}");
        // Ослабление: (255, 128, 128) → менее насыщенный (ближе к серому).
        let reduced = apply_saturation((255, 128, 128), 0.0);
        let avg = (reduced.0 as u16 + reduced.1 as u16 + reduced.2 as u16) / 3;
        assert_eq!(reduced, (avg as u8, avg as u8, avg as u8));
    }

    #[test]
    fn light_compression_scales_sum() {
        let c = (200, 100, 50); // сумма 350 > 255
        let out = apply_light_compression(c);
        let sum = out.0 as u16 + out.1 as u16 + out.2 as u16;
        assert!(sum <= 255, "sum={sum}");
        // Пропорции сохраняются: 200:100 = 2:1 → out.0 = 2 * out.1.
        assert_eq!(out.0 as u32, out.1 as u32 * 2);
        // Точное значение: 200*255/350 = 145.7 → 146; 100*255/350 = 72.9 → 73.
        assert_eq!(out, (146, 73, 36));
        // Уже в пределах 255 — без изменений.
        assert_eq!(apply_light_compression((100, 100, 50)), (100, 100, 50));
    }

    #[test]
    fn extract_solid_screen_all_leds_same_color() {
        // Весь экран красный → все LED красные.
        let frame = solid_frame(1920, 1080, (255, 0, 0));
        let mut ex = ColorExtractor::new(PostProcessConfig {
            gamma: 1.0,
            saturation: 1.0,
            light_compression: false,
            smoothing: false,
        });
        let layout = EdgeLayout::default_for(51);
        let colors = ex.extract(&frame, 1920 * 4, 1920, 1080, &layout);
        assert_eq!(colors.len(), 51);
        for c in colors {
            assert_eq!(c, (255, 0, 0));
        }
    }

    #[test]
    fn extract_left_edge_blue_top_green() {
        // 320×180, сетка 16×9. Левый столбец ячеек — синий, верхний ряд — зелёный.
        // Кадр: левая полоса 20 px — синяя; верхняя полоса 20 px — зелёная; остальное чёрное.
        let (w, h) = (320u32, 180u32);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let (r, g, b) = if x < 20 {
                    (0, 0, 255) // синий
                } else if y < 20 {
                    (0, 255, 0) // зелёный
                } else {
                    (0, 0, 0)
                };
                frame[i..i + 3].copy_from_slice(&[b, g, r]);
                frame[i + 3] = 255;
            }
        }
        let mut ex = ColorExtractor::new(PostProcessConfig::default());
        let layout = EdgeLayout {
            left: 4,
            top: 8,
            right: 0,
            bottom: 0,
        };
        let colors = ex.extract(&frame, w * 4, w, h, &layout);
        assert_eq!(colors.len(), 12); // 4 + 8
                                      // Первые 4 LED — левый край (снизу вверх): должен быть синим (кроме, возможно,
                                      // верхнего угла, где пересекается с зелёным).
        let left_leds = &colors[0..4];
        for c in left_leds {
            assert!(c.2 > c.0 && c.2 >= c.1, "ожидаем синеву, got {c:?}");
        }
        // Следующие 8 — верх: зелёные.
        let top_leds = &colors[4..12];
        for c in top_leds {
            assert!(c.1 > c.0 && c.1 >= c.2, "ожидаем зелень, got {c:?}");
        }
    }

    #[test]
    fn smoothing_averages_frames() {
        let mut ex = ColorExtractor::new(PostProcessConfig {
            gamma: 1.0,
            saturation: 1.0,
            light_compression: false,
            smoothing: true,
        });
        let layout = EdgeLayout {
            left: 1,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let (w, h) = (320u32, 180u32);

        let frame_with = |color: Rgb| -> Vec<u8> {
            let mut f = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    f[i..i + 3].copy_from_slice(&[color.2, color.1, color.0]);
                    f[i + 3] = 255;
                }
            }
            f
        };

        // 5 кадров: 4 тёмных (10,0,0) и 1 яркий (250,0,0).
        for _ in 0..4 {
            ex.extract(&frame_with((10, 0, 0)), w * 4, w, h, &layout);
        }
        let bright = ex.extract(&frame_with((250, 0, 0)), w * 4, w, h, &layout);
        // Среднее по 5 кадрам: (4*10 + 250)/5 = 58.
        assert_eq!(bright[0].0, 58);
    }
}
