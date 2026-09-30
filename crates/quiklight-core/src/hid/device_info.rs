//! Разбор ответа `readDeviceInfo` (action 130, RB-кадр).
//!
//! Смещения полей — **от начала кадра** (0-based, от байта `'R'`), как в
//! оригинале quicklight-linux PROTOCOL.md (см. `docs/PROTOCOL.md`, «Ответ
//! readDeviceInfo»). ВАЖНО: это не смещения payload — payload начинается
//! с кадра offset 5.

/// Информация об устройстве, извлечённая из ответа на `readDeviceInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Идентификатор устройства (кадр[5..8]).
    pub device_id: [u8; 3],
    /// Настроенный размер экрана (кадр `[8]`).
    pub screen_size: u8,
    /// Число LED ленты (кадр `[11]`).
    pub led_count: u8,
    /// UUID устройства (кадр[12..20]).
    pub uuid: [u8; 8],
    /// Версия прошивки: major, minor, patch (кадр[21..24]).
    pub firmware: [u8; 3],
}

impl DeviceInfo {
    /// Версия прошивки строкой вида `major.minor.patch`.
    pub fn firmware_string(&self) -> String {
        format!(
            "{}.{}.{}",
            self.firmware[0], self.firmware[1], self.firmware[2]
        )
    }

    /// UUID шестнадцатеричной строкой.
    pub fn uuid_string(&self) -> String {
        self.uuid.iter().map(|b| format!("{b:02X}")).collect()
    }
}

/// Запасное число LED, если ответ устройства короче ожидаемого
/// (стандартная лента 27" — 63 LED; см. `EdgeLayout::default_for`).
pub const DEFAULT_LED_COUNT: u8 = 63;

/// Смещение идентификатора устройства (`кадр[5..8]`).
const OFF_DEVICE_ID: usize = 5;
/// Смещение настроенного размера экрана (`кадр[8]`).
const OFF_SCREEN_SIZE: usize = 8;
/// Смещение числа LED (`кадр[11]`).
const OFF_LED_COUNT: usize = 11;
/// Смещение UUID (`кадр[12..20]`).
const OFF_UUID: usize = 12;
/// Смещение версии прошивки (`кадр[21..24]`).
const OFF_FIRMWARE: usize = 21;
/// Минимальная длина кадра для разбора: заголовок + id + action + device_id + screen_size.
const MIN_FRAME_LEN: usize = OFF_SCREEN_SIZE + 1;

/// Прочитать `N` байт по смещению, подставляя нули для отсутствующего хвоста.
fn tail_or_zero<const N: usize>(frame: &[u8], offset: usize) -> [u8; N] {
    std::array::from_fn(|i| frame.get(offset + i).copied().unwrap_or(0))
}

/// Разобрать RB-ответ на `readDeviceInfo`.
///
/// `frame` — полный RB-кадр (с заголовком и checksum), без report ID.
/// Смещения — от начала кадра, как в оригинале quicklight-linux.
///
/// Возвращает `None`, если кадр слишком короткий — по PROTOCOL.md смещения
/// допускают несовпадение длины, поэтому парсер прощает отсутствие хвостовых
/// полей (uuid/firmware), но требует хотя бы [`MIN_FRAME_LEN`] байт кадра.
pub fn parse_device_info(frame: &[u8]) -> Option<DeviceInfo> {
    if frame.len() < MIN_FRAME_LEN {
        return None;
    }

    Some(DeviceInfo {
        device_id: tail_or_zero(frame, OFF_DEVICE_ID),
        screen_size: frame[OFF_SCREEN_SIZE],
        led_count: frame
            .get(OFF_LED_COUNT)
            .copied()
            .unwrap_or(DEFAULT_LED_COUNT),
        uuid: tail_or_zero(frame, OFF_UUID),
        firmware: tail_or_zero(frame, OFF_FIRMWARE),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Собрать RB-кадр с произвольным payload для тестов парсера.
    fn rb_response(payload: &[u8]) -> Vec<u8> {
        // 'R' 'B' size id action payload checksum
        let mut frame = vec![0x52, 0x42, (6 + payload.len()) as u8, 2, 0x82];
        frame.extend_from_slice(payload);
        let sum: u8 = frame.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        frame.push(sum);
        frame
    }

    #[test]
    fn parses_full_info() {
        // Поля лежат на frame-относительных смещениях (как в quicklight-linux):
        // кадр[5..8] id, кадр[8] screen size, кадр[11] LED, кадр[12..20] uuid,
        // кадр[21..24] прошивка. В payload это индексы 0,3,6,7,16.
        let mut payload = vec![0u8; 19];
        payload[0..3].copy_from_slice(&[0xAA, 0xBB, 0xCC]); // device id
        payload[3] = 27; // screen size 27"
        payload[6] = 63; // 63 LED
        payload[7..15].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]); // uuid
        payload[16..19].copy_from_slice(&[1, 8, 0]); // firmware 1.8.0

        let frame = rb_response(&payload);
        let info = parse_device_info(&frame).expect("должен распарситься");
        assert_eq!(info.device_id, [0xAA, 0xBB, 0xCC]);
        assert_eq!(info.screen_size, 27);
        assert_eq!(info.led_count, 63);
        assert_eq!(info.uuid, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(info.firmware, [1, 8, 0]);
        assert_eq!(info.firmware_string(), "1.8.0");
        assert_eq!(info.uuid_string(), "0102030405060708");
    }

    #[test]
    fn parses_real_device_response() {
        // Реальный ответ панели (USB): 52 42 19 02 82 00 15 00 18 01 00 33
        // CD AB 94 69 72 BD D3 D2 E5 01 08 02 CB — проверено на железе.
        let frame: Vec<u8> = vec![
            0x52, 0x42, 0x19, 0x02, 0x82, 0x00, 0x15, 0x00, 0x18, 0x01, 0x00, 0x33, 0xCD, 0xAB,
            0x94, 0x69, 0x72, 0xBD, 0xD3, 0xD2, 0xE5, 0x01, 0x08, 0x02, 0xCB,
        ];
        let info = parse_device_info(&frame).expect("должен распарситься");
        assert_eq!(info.device_id, [0x00, 0x15, 0x00]);
        assert_eq!(info.screen_size, 0x18); // 24"
        assert_eq!(info.led_count, 0x33); // 51 LED
        assert_eq!(info.uuid, [0xCD, 0xAB, 0x94, 0x69, 0x72, 0xBD, 0xD3, 0xD2]);
        assert_eq!(info.firmware, [1, 8, 2]);
        assert_eq!(info.firmware_string(), "1.8.2");
    }

    #[test]
    fn short_frame_returns_none() {
        assert!(parse_device_info(&[0x52, 0x42, 0x05]).is_none());
        assert!(parse_device_info(&[]).is_none());
    }

    #[test]
    fn missing_tail_fields_default() {
        // Только кадр[5..9] (id + screen size), без led_count/uuid/firmware
        let mut payload = vec![0u8; 4];
        payload[0..3].copy_from_slice(&[1, 2, 3]);
        payload[3] = 24;
        let frame = rb_response(&payload);
        let info = parse_device_info(&frame).expect("должен распарситься");
        assert_eq!(info.led_count, DEFAULT_LED_COUNT); // дефолт
        assert_eq!(info.uuid, [0; 8]);
        assert_eq!(info.firmware, [0; 3]);
    }
}
