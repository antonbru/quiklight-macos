//! Коды действий (action) и типизированные команды протокола QuikLight.
//!
//! См. `docs/PROTOCOL.md` — таблица действий и раздел «Цвет — действие 134».

use crate::protocol::packet::rb_frame;

/// Коды действий протокола RB/SC.
#[allow(dead_code)]
pub mod action {
    /// 128 (0x80) — screen sync: цвета экрана, кадр SC.
    pub const SET_SCREEN_SYNC: u8 = 0x80;
    /// 129 — запись информации устройства.
    pub const WRITE_DEVICE_INFO: u8 = 0x81;
    /// 130 (0x82) — чтение информации устройства (есть ответ).
    pub const READ_DEVICE_INFO: u8 = 0x82;
    /// 131 — чтение UUID устройства (есть ответ).
    pub const READ_DEVICE_UUID: u8 = 0x83;
    /// 133 (0x85) — встроенный эффект (есть ответ).
    pub const SET_LED_EFFECT: u8 = 0x85;
    /// 134 (0x86) — цвет/секции ленты.
    pub const SET_SECTION_LED: u8 = 0x86;
    /// 135 (0x87) — яркость 0..100 (есть ответ).
    pub const SET_BRIGHTNESS: u8 = 0x87;
    /// 137 — авто-выключение (есть ответ).
    pub const SET_AUTO_OFF: u8 = 0x89;
    /// 138 (0x8A) — скорость эффектов 5..100 (есть ответ).
    pub const SET_DYNAMIC_SPEED: u8 = 0x8A;
    /// 139 — чувствительность звука (есть ответ).
    pub const SET_SOUND_SENSITIVITY: u8 = 0x8B;
    /// 145 — внешнее аудио.
    pub const SET_EXTERNAL_AUDIO: u8 = 0x91;
    /// 147 — открыть URL.
    pub const SET_OPEN_URL: u8 = 0x93;
    /// 149 — количество LED.
    pub const SET_LAMPS_AMOUNT: u8 = 0x95;
    /// 150 — белая яркость.
    pub const SET_WHITE_BRIGHT: u8 = 0x96;
    /// 151 (0x97) — выключить подсветку (есть ответ).
    pub const TURN_OFF_LIGHT: u8 = 0x97;
    /// 152 (0x98) — ритм компьютера.
    pub const SET_COMPUTER_RHYTHM: u8 = 0x98;
}

/// Значение `254` = «до конца ленты» (не зависит от фактического числа LED).
pub const LED_END: u8 = 0xFE;

/// Построить RB-кадр «погасить всё» (action 134, payload `[1, 0, 0, 0, 254]`).
///
/// Нужен для совместимости со старыми прошивками (< 1.8.0), которые не знают
/// action 151: см. `docs/PROTOCOL.md`, «Подводные камни», п. 2.
pub fn set_all_black(id: u8) -> Vec<u8> {
    set_section_led(id, &[(1, 0, 0, 0, LED_END)])
}

/// Построить RB-кадр «выключить подсветку» (action 151, payload пустой).
pub fn turn_off(id: u8) -> Vec<u8> {
    rb_frame(id, action::TURN_OFF_LIGHT, &[]).with_checksum()
}

/// Построить RB-кадр «яркость» (action 135).
///
/// Аргумент `brightness` — **0..100**. Протокол не принимает значения больше 100;
/// минимум в SyncRGB ограничен 5, но панель принимает 0..100.
pub fn set_brightness(id: u8, brightness: u8) -> Vec<u8> {
    assert!(
        brightness <= 100,
        "яркость должна быть в 0..100, получено {brightness}"
    );
    rb_frame(id, action::SET_BRIGHTNESS, &[brightness]).with_checksum()
}

/// Построить RB-кадр «встроенный эффект» (action 133, payload `tipo, índice`).
pub fn set_led_effect(id: u8, effect_type: u8, index: u8) -> Vec<u8> {
    rb_frame(id, action::SET_LED_EFFECT, &[effect_type, index]).with_checksum()
}

/// Построить RB-кадр «скорость эффектов» (action 138).
///
/// ⚠️ **Скорость инвертирована**: в UI слайдер растёт вправо → на устройство
/// уходит `100 - slider`. См. PROTOCOL.md, «Подводные камни», п.1.
///
/// Принимает «пользовательскую» скорость 0..100 и сам инвертирует.
pub fn set_dynamic_speed(id: u8, user_speed: u8) -> Vec<u8> {
    assert!(
        user_speed <= 100,
        "скорость должна быть в 0..100, получено {user_speed}"
    );
    let device_speed = 100_u8.saturating_sub(user_speed);
    rb_frame(id, action::SET_DYNAMIC_SPEED, &[device_speed]).with_checksum()
}

/// Построить RB-кадр «чтение информации устройства» (action 130, payload пустой).
pub fn read_device_info(id: u8) -> Vec<u8> {
    rb_frame(id, action::READ_DEVICE_INFO, &[]).with_checksum()
}

/// Построить payload для `setSectionLED` (action 134) из групп
/// `(led_начало, R, G, B, led_конец)`.
///
/// LED нумеруются с 1; `led_конец == 254` значит «до конца ленты».
pub fn section_led_payload(groups: &[(u8, u8, u8, u8, u8)]) -> Vec<u8> {
    assert!(
        groups.len() <= 49,
        "кадр ≤ 255 байт → максимум ~49 групп, получено {}",
        groups.len()
    );
    let mut payload = Vec::with_capacity(groups.len() * 5);
    for &(start, r, g, b, end) in groups {
        debug_assert!(start >= 1, "LED нумеруются с 1");
        payload.extend_from_slice(&[start, r, g, b, end]);
    }
    payload
}

/// Построить RB-кадр «цвет» (action 134) из групп секций.
pub fn set_section_led(id: u8, groups: &[(u8, u8, u8, u8, u8)]) -> Vec<u8> {
    let payload = section_led_payload(groups);
    rb_frame(id, action::SET_SECTION_LED, &payload).with_checksum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_50() {
        // Эталон из PROTOCOL.md (пересчитан программно формулой):
        // 52 42 07 02 87 32 56
        assert_eq!(
            set_brightness(2, 50),
            vec![0x52, 0x42, 0x07, 0x02, 0x87, 0x32, 0x56]
        );
    }

    #[test]
    fn turn_off_light() {
        // 52 42 06 02 97 33
        assert_eq!(turn_off(2), vec![0x52, 0x42, 0x06, 0x02, 0x97, 0x33]);
    }

    #[test]
    fn all_red() {
        // 52 42 0B 02 86 01 FF 00 00 FE 25
        assert_eq!(
            set_section_led(2, &[(1, 0xFF, 0x00, 0x00, LED_END)]),
            vec![0x52, 0x42, 0x0B, 0x02, 0x86, 0x01, 0xFF, 0x00, 0x00, 0xFE, 0x25]
        );
    }

    #[test]
    fn read_info() {
        // Пустой payload → size = 6 (формула из PROTOCOL.md: полный размер кадра).
        // В PROTOCOL.md ошибочно указан 0x05 — см. предупреждение в доке.
        assert_eq!(
            read_device_info(2),
            vec![0x52, 0x42, 0x06, 0x02, 0x82, 0x1E]
        );
    }

    #[test]
    fn speed_inverted() {
        // Пользовательская скорость 30 → устройству 70.
        let frame = set_dynamic_speed(2, 30);
        assert_eq!(&frame[..6], &[0x52, 0x42, 0x07, 0x02, 0x8A, 70]);
    }

    #[test]
    fn set_all_black_frame() {
        // «Погасить всё» = payload [1,0,0,0,254] в кадре action 134.
        // checksum пересчитан программно формулой из PROTOCOL.md.
        assert_eq!(
            set_all_black(2),
            vec![0x52, 0x42, 0x0B, 0x02, 0x86, 0x01, 0x00, 0x00, 0x00, 0xFE, 0x26]
        );
    }
}
