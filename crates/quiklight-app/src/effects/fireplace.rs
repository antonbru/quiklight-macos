//! Свой эффект «Камин»: резко мерцающее пламя по нижней части ленты.
//!
//! Чистая математика без AppKit/HID: раскладка LED → высота над нижним краем
//! монитора → цвет пламени в момент времени `t`. Движок (`engine`) лишь
//! вызывает [`payload`] и отправляет результат на панель.

use quiklight_core::sync::led_mapper::EdgeLayout;

/// Высота каждого LED над нижним краем монитора, 0..1, в порядке проводов:
/// левый край снизу вверх → верх → правый сверху вниз → низ.
pub fn led_heights(layout: &EdgeLayout) -> Vec<f64> {
    let mut heights = Vec::with_capacity(layout.total() as usize);

    // Левый край: LED 0 — нижний угол (0), последний — верхний (1).
    let left = layout.left as usize;
    for k in 0..left {
        heights.push(if left > 1 {
            k as f64 / (left - 1) as f64
        } else {
            0.0
        });
    }
    // Верх — вся сторона на высоте 1.
    heights.extend(std::iter::repeat_n(1.0, layout.top as usize));
    // Правый край: LED 0 — верхний угол (1), последний — нижний (0).
    let right = layout.right as usize;
    for k in 0..right {
        heights.push(if right > 1 {
            1.0 - k as f64 / (right - 1) as f64
        } else {
            0.0
        });
    }
    // Низ — вся сторона на высоте 0.
    heights.extend(std::iter::repeat_n(0.0, layout.bottom as usize));

    heights
}

/// Собрать payload кадра пламени `[R,G,B, ...]` для раскладки `layout`
/// в момент времени `t` (секунды с начала эффекта).
pub fn payload(layout: &EdgeLayout, t: f64) -> Vec<u8> {
    let heights = led_heights(layout);
    let mut out = Vec::with_capacity(heights.len() * 3);
    for (i, &h) in heights.iter().enumerate() {
        // Фаза каждого LED своя — огонь «дышит» по ленте, а не мигает вся сразу.
        let phase = i as f64 * 0.8;
        let (r, g, b) = fire_color(h, t, phase);
        out.extend_from_slice(&[r, g, b]);
    }
    out
}

/// Цвет пламени для LED на высоте `h` (0 = низ монитора, 1 = верх) в момент `t`.
///
/// Огонь «рваный», как живой: мерцание скачет резко, несколько раз в секунду,
/// а не плывёт плавно; тело огня остаётся ярко-оранжевым (не уходит в тёмное).
pub fn fire_color(h: f64, t: f64, phase: f64) -> (u8, u8, u8) {
    // Пламя резко затухает с высотой — горит нижняя часть ленты.
    let falloff = (1.0 - h).powf(1.8);
    // Резкое мерцание 0.6..1.0: группы соседних LED вспыхивают и гаснут.
    let flick = 0.6 + 0.4 * flame_flicker(t, phase);
    // Медленное «дыхание» всего огня (0.7..1.0).
    let breathe = 0.85 + 0.15 * (t * 1.4 + phase * 0.7).sin();
    // Редкие языки: вспышка, тянущаяся вверх (огонь «выстреливает»).
    let tongue = if flame_flicker(t, phase * 0.31) > 0.86 {
        (1.0 - h).powf(0.8)
    } else {
        0.0
    };
    let mut heat = (falloff * flick * breathe + 0.35 * tongue).clamp(0.0, 1.0);
    // Слегка приподнимаем середину, чтобы переход был сочнее.
    heat = heat.powf(0.9);
    fire_gradient(heat)
}

/// Резкий мерцающий шум 0..1. Значение скачет каждый ~90 мс (ячейка времени),
/// а соседние LED объединены в группы по ~2 — «языки» пламени, а не белый шум.
fn flame_flicker(t: f64, phase: f64) -> f64 {
    const CELL_S: f64 = 0.09;
    let cell = (t / CELL_S).floor();
    // Крупный «язык»: группа соседних LED мигает вместе.
    let group = (phase / 1.5).floor();
    let coarse = hash01(group * 7.13, cell);
    // Мелкая рябь внутри группы.
    let fine = hash01(phase * 3.1, cell * 0.71);
    0.65 * coarse + 0.35 * fine
}

/// Детерминированный псевдослучайный [0, 1) по двум ключам (GLSL-хэш на sin).
fn hash01(a: f64, b: f64) -> f64 {
    let n = (a * 12.9898 + b * 78.233).sin() * 43758.5453;
    n - n.floor()
}

/// Градиент огня: нагрев 0..1 → цвет от тёмных углей до яркого оранжевого ядра.
pub fn fire_gradient(heat: f64) -> (u8, u8, u8) {
    const STOPS: &[(f64, (u8, u8, u8))] = &[
        (0.00, (20, 4, 0)),      // тлеющие угли
        (0.18, (120, 30, 0)),    // тёмно-красный
        (0.40, (220, 70, 0)),    // красно-оранжевый
        (0.65, (255, 120, 10)),  // яркий оранжевый
        (0.85, (255, 175, 45)),  // жёлто-оранжевый
        (1.00, (255, 228, 120)), // горячее ядро
    ];
    let heat = heat.clamp(0.0, 1.0);
    let mut i = 0;
    while i + 1 < STOPS.len() && heat > STOPS[i + 1].0 {
        i += 1;
    }
    let (t0, (r0, g0, b0)) = STOPS[i];
    let (t1, (r1, g1, b1)) = STOPS[i + 1];
    let span = t1 - t0;
    let k = if span <= f64::EPSILON {
        0.0
    } else {
        ((heat - t0) / span).clamp(0.0, 1.0)
    };
    (
        (f64::from(r0) + (f64::from(r1) - f64::from(r0)) * k).round() as u8,
        (f64::from(g0) + (f64::from(g1) - f64::from(g0)) * k).round() as u8,
        (f64::from(b0) + (f64::from(b1) - f64::from(b0)) * k).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn led_heights_follow_wire_order() {
        let layout = EdgeLayout {
            left: 3,
            top: 2,
            right: 3,
            bottom: 2,
        };
        let h = led_heights(&layout);
        assert_eq!(h.len(), 10);
        // Левый край: снизу вверх 0 → 0.5 → 1.
        assert_eq!(&h[0..3], &[0.0, 0.5, 1.0]);
        // Верх: вся сторона на высоте 1.
        assert_eq!(&h[3..5], &[1.0, 1.0]);
        // Правый край: сверху вниз 1 → 0.5 → 0.
        assert_eq!(&h[5..8], &[1.0, 0.5, 0.0]);
        // Низ: вся сторона на высоте 0.
        assert_eq!(&h[8..10], &[0.0, 0.0]);
    }

    #[test]
    fn led_heights_single_led_edges() {
        // Сторона из одного LED не должна делить на ноль.
        let layout = EdgeLayout {
            left: 1,
            top: 1,
            right: 1,
            bottom: 0,
        };
        let h = led_heights(&layout);
        assert_eq!(h, vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn payload_matches_layout() {
        let layout = EdgeLayout {
            left: 2,
            top: 1,
            right: 0,
            bottom: 0,
        };
        // 3 LED → 9 байт (RGB).
        assert_eq!(payload(&layout, 0.5).len(), 9);
    }

    #[test]
    fn fire_gradient_heat_endpoints() {
        assert_eq!(fire_gradient(0.0), (20, 4, 0));
        assert_eq!(fire_gradient(1.0), (255, 228, 120));
        // Значения за границами не ломают градиент.
        assert_eq!(fire_gradient(-1.0), fire_gradient(0.0));
        assert_eq!(fire_gradient(2.0), fire_gradient(1.0));
    }

    #[test]
    fn fire_gradient_grows_toward_white() {
        // Чем горячее — тем ярче (монотонность по каналам R и G).
        let (r1, g1, _) = fire_gradient(0.3);
        let (r2, g2, _) = fire_gradient(0.8);
        assert!(r2 >= r1 && g2 >= g1);
    }

    #[test]
    fn fire_bottom_hot_top_dark() {
        // У верха монитора пламени нет — только тлеющие угли.
        assert_eq!(fire_color(1.0, 3.7, 1.2), (20, 4, 0));
        // У низа — всегда огненная палитра (яркий оранжевый, r ≥ g ≥ b).
        for t in [0.0, 0.5, 1.3, 2.2] {
            for phase in [0.0, 0.8, 2.1] {
                let (r, g, b) = fire_color(0.0, t, phase);
                assert!(r >= g && g >= b, "t={t} phase={phase}: ({r},{g},{b})");
                assert!(
                    r >= 200,
                    "у низа пламя должно быть ярко-оранжевым: t={t} phase={phase} → ({r},{g},{b})"
                );
            }
        }
    }

    #[test]
    fn fire_flickers_over_time() {
        // Пламя должно мерцать, а не быть статичной картинкой.
        let mut changed = 0;
        for h in [0.0, 0.2, 0.4, 0.6] {
            for phase in [0.0, 1.5, 3.1] {
                let a = fire_color(h, 1.0, phase);
                let b = fire_color(h, 1.0 + 0.3, phase);
                if a != b {
                    changed += 1;
                }
            }
        }
        assert!(
            changed >= 6,
            "пламя почти не мерцает: изменений {changed}/12"
        );
    }
}
