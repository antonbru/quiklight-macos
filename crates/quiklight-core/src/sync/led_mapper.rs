//! Маппинг цветов краёв экрана на физические LED ленты QuikLight.
//!
//! Экран семплируется сеткой (`color_extractor`), края дают сегментные цвета,
//! а здесь они раскладываются по LED в порядке проводов устройства:
//! левый край снизу вверх → верх слева направо → правый сверху вниз →
//! низ справа налево (порядок SyncLight/SyncRGB при `reverse=false`).

/// Раскладка ленты по краям: сколько LED на каждой стороне.
///
/// Стандарт для 27" ленты — 17/29/17 (63 LED, bottom = 0), см. quiklight-x11.
/// Фактическое число LED читается из `readDeviceInfo` (у нашей панели — 51).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeLayout {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}

impl EdgeLayout {
    /// Полное число LED ленты.
    pub fn total(&self) -> u16 {
        self.left + self.top + self.right + self.bottom
    }

    /// Раскладка по умолчанию под фактическое число LED: сохраняет пропорции
    /// эталонной раскладки 17/29/17 (без нижнего края). Используется, когда
    /// пользователь не задал раскладку вручную.
    ///
    /// Пропорции считаются по долям эталона, остаток распределяется по кругу
    /// (left → top → right), чтобы сумма совпала с `led_count` в точности.
    pub fn default_for(led_count: u16) -> Self {
        if led_count == 0 {
            return Self { left: 0, top: 0, right: 0, bottom: 0 };
        }
        // Эталон: 17 лево, 29 верх, 17 право (всего 63).
        let mut layout = Self {
            left: (led_count * 17 / 63).max(1),
            top: (led_count * 29 / 63).max(1),
            right: (led_count * 17 / 63).max(1),
            bottom: 0,
        };
        // Остаток от целочисленного деления — по одному LED на сторону по кругу.
        let mut remainder = led_count as i64 - layout.total() as i64;
        while remainder > 0 {
            if layout.left > 0 {
                layout.left += 1;
                remainder -= 1;
            }
            if remainder > 0 && layout.top > 0 {
                layout.top += 1;
                remainder -= 1;
            }
            if remainder > 0 && layout.right > 0 {
                layout.right += 1;
                remainder -= 1;
            }
        }
        layout
    }
}

/// Цвета краёв экрана: по одному цвету на сегмент сетки.
///
/// Векторы идут в «экранном» порядке (не в порядке проводов):
/// - `left`, `right` — сверху вниз;
/// - `top`, `bottom` — слева направо.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeColors {
    pub left: Vec<(u8, u8, u8)>,
    pub top: Vec<(u8, u8, u8)>,
    pub right: Vec<(u8, u8, u8)>,
    pub bottom: Vec<(u8, u8, u8)>,
}

impl EdgeColors {
    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.top.is_empty() && self.right.is_empty() && self.bottom.is_empty()
    }
}

/// Разложить цвета краёв на LED в порядке проводов устройства.
///
/// Порядок: левый край снизу вверх → верх слева направо → правый сверху вниз →
/// низ справа налево (соответствует `setSyncScreen`/`setSectionLED` в SyncLight).
///
/// Каждая сторона интерполируется с числа сегментов сетки на число LED этой
/// стороны (`layout`). Если LED на стороне нет — её цвета пропускаются.
pub fn map_to_leds(edges: &EdgeColors, layout: &EdgeLayout) -> Vec<(u8, u8, u8)> {
    let mut out = Vec::with_capacity(layout.total() as usize);

    // Левый край: LED идут от нижнего угла вверх → реверсируем экранный порядок.
    if layout.left > 0 {
        let mut col = edges.left.clone();
        col.reverse();
        out.extend(interpolate(&col, layout.left as usize));
    }
    // Верх: слева направо, как на экране.
    if layout.top > 0 {
        out.extend(interpolate(&edges.top, layout.top as usize));
    }
    // Правый край: сверху вниз, как на экране.
    if layout.right > 0 {
        out.extend(interpolate(&edges.right, layout.right as usize));
    }
    // Низ: LED идут от правого угла влево → реверсируем.
    if layout.bottom > 0 {
        let mut col = edges.bottom.clone();
        col.reverse();
        out.extend(interpolate(&col, layout.bottom as usize));
    }

    out
}

/// Линейная интерполяция одной стороны: `src` сегментных цветов → `dst` LED.
///
/// Каждый LED берёт цвет ближайшего по позиции сегмента (как в SyncRGB
/// `map_to_leds`: `src_idx = (i + 0.5) * src_len / dst_len`).
pub fn interpolate(src: &[(u8, u8, u8)], dst: usize) -> Vec<(u8, u8, u8)> {
    if dst == 0 {
        return Vec::new();
    }
    if src.is_empty() {
        return vec![(0, 0, 0); dst];
    }
    if src.len() == 1 || dst == 1 {
        return vec![src[0]; dst];
    }

    let src_len = src.len() as f32;
    let dst_len = dst as f32;
    (0..dst)
        .map(|i| {
            let src_idx = ((i as f32 + 0.5) * src_len / dst_len) as usize;
            src[src_idx.min(src.len() - 1)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_63() {
        // Эталонная раскладка 17/29/17 для 63 LED.
        let l = EdgeLayout::default_for(63);
        assert_eq!((l.left, l.top, l.right, l.bottom), (17, 29, 17, 0));
        assert_eq!(l.total(), 63);
    }

    #[test]
    fn default_layout_51_preserves_proportions() {
        // Наша панель: 51 LED. Пропорции должны сохраниться, сумма — 51.
        let l = EdgeLayout::default_for(51);
        assert_eq!(l.total(), 51);
        assert_eq!(l.bottom, 0);
        // 17/63*51 ≈ 13.8 → 13/14; 29/63*51 ≈ 23.5 → 23/24.
        assert!((13..=14).contains(&l.left), "left={}", l.left);
        assert!((23..=24).contains(&l.top), "top={}", l.top);
        assert!((13..=14).contains(&l.right), "right={}", l.right);
        assert_eq!(l.left + l.top + l.right, 51);
    }

    #[test]
    fn default_layout_zero() {
        let l = EdgeLayout::default_for(0);
        assert_eq!(l.total(), 0);
    }

    #[test]
    fn interpolate_expands() {
        // 2 сегмента → 4 LED: 0,0,1,1 (ближайший по позиции).
        let src = vec![(255, 0, 0), (0, 0, 255)];
        let out = interpolate(&src, 4);
        assert_eq!(out, vec![(255, 0, 0), (255, 0, 0), (0, 0, 255), (0, 0, 255)]);
    }

    #[test]
    fn interpolate_contracts() {
        // 4 сегмента → 2 LED: формула src_idx = (i+0.5)*src_len/dst_len даёт
        // сегменты 1 и 3 (как в SyncRGB map_to_leds).
        let src = vec![(1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0)];
        let out = interpolate(&src, 2);
        assert_eq!(out, vec![(2, 0, 0), (4, 0, 0)]);
    }

    #[test]
    fn interpolate_empty_src_is_black() {
        assert_eq!(interpolate(&[], 3), vec![(0, 0, 0); 3]);
    }

    #[test]
    fn wire_order_matches_synclight() {
        // Лента 1+2+1 (без низа): левый край 1 LED (нижний угол), верх 2, правый 1.
        let layout = EdgeLayout { left: 1, top: 2, right: 1, bottom: 0 };
        let edges = EdgeColors {
            left: vec![(1, 0, 0)],        // экранный порядок: сверху вниз
            top: vec![(0, 2, 0), (0, 3, 0)], // слева направо
            right: vec![(4, 0, 0)],
            bottom: vec![],
        };
        // Порядок проводов: левый снизу вверх (реверс) → верх → правый.
        let out = map_to_leds(&edges, &layout);
        assert_eq!(out, vec![(1, 0, 0), (0, 2, 0), (0, 3, 0), (4, 0, 0)]);
    }

    #[test]
    fn wire_order_reverses_left_and_bottom() {
        // Проверка реверса левого и нижнего краёв.
        let layout = EdgeLayout { left: 3, top: 0, right: 0, bottom: 2 };
        let edges = EdgeColors {
            left: vec![(1, 0, 0), (2, 0, 0), (3, 0, 0)], // экран: верх→низ
            top: vec![],
            right: vec![],
            bottom: vec![(4, 0, 0), (5, 0, 0)], // экран: лево→право
        };
        let out = map_to_leds(&edges, &layout);
        // Левый: низ→верх = 3,2,1; низ: право→лево = 5,4.
        assert_eq!(out, vec![(3, 0, 0), (2, 0, 0), (1, 0, 0), (5, 0, 0), (4, 0, 0)]);
    }

    #[test]
    fn total_matches_out_len() {
        let layout = EdgeLayout::default_for(51);
        let edges = EdgeColors {
            left: vec![(1, 0, 0); 4],
            top: vec![(0, 1, 0); 16],
            right: vec![(0, 0, 1); 4],
            bottom: vec![],
        };
        let out = map_to_leds(&edges, &layout);
        assert_eq!(out.len(), 51);
    }
}
