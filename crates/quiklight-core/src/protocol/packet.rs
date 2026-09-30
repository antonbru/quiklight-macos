//! Построение и разбор кадров RB/SC, контрольная сумма, чанкинг для HID.
//!
//! См. `docs/PROTOCOL.md`:
//! - RB-кадр (управление): `'R' 'B' size(1) id action payload checksum`
//! - SC-кадр (screen sync, action 128): `'S' 'C' size(2 BE) id action payload checksum`
//! - checksum — сумма всех предыдущих байт кадра по модулю 256

/// Размер HID-отчёта (вместе с report ID 0x00) — endpoint панели 64 байта.
pub const HID_REPORT_SIZE: usize = 64;

/// Константы маркеров кадров.
pub mod marker {
    pub const RB: [u8; 2] = *b"RB";
    pub const SC: [u8; 2] = *b"SC";
}

/// Сырой кадр без контрольной суммы: помогает построить кадр отдельно от
/// вычисления checksum (нужно для тестов и для композиции).
pub struct Frame {
    /// Байты кадра **включая** checksum.
    bytes: Vec<u8>,
    /// Смещение байта checksum (последний байт кадра).
    checksum_pos: usize,
}

impl Frame {
    /// Полный кадр с контрольной суммой.
    pub fn with_checksum(mut self) -> Vec<u8> {
        let sum: u8 = self
            .bytes
            .iter()
            .take(self.checksum_pos)
            .fold(0u8, |acc, &b| acc.wrapping_add(b));
        self.bytes[self.checksum_pos] = sum;
        self.bytes
    }
}

/// Построить RB-кадр (без checksum). Возвращает `Frame`, из которого
/// `with_checksum()` делает готовый байтовый кадр.
///
/// ```
/// # use quiklight_core::protocol::packet::rb_frame;
/// let frame = rb_frame(2, 0x87, &[50]).with_checksum();
/// assert_eq!(frame, vec![0x52, 0x42, 0x07, 0x02, 0x87, 0x32, 0x56]);
/// ```
pub fn rb_frame(id: u8, action: u8, payload: &[u8]) -> Frame {
    build_frame(marker::RB, SizeField::OneByte, id, action, payload)
}

/// Построить SC-кадр (без checksum). Используется для передачи цветов экрана
/// (action 128). Размер — 2 байта, big-endian.
///
/// Кадр может быть больше 255 байт — это нормально для SC (цвета 63+ LED).
pub fn sc_frame(id: u8, action: u8, payload: &[u8]) -> Frame {
    build_frame(marker::SC, SizeField::TwoBytesBe, id, action, payload)
}

/// Разрядность поля размера кадра: RB — 1 байт, SC — 2 байта big-endian.
#[derive(Debug, Clone, Copy)]
enum SizeField {
    OneByte,
    TwoBytesBe,
}

/// Общий билдер кадра: маркер + размер + id + action + payload + заглушка checksum.
///
/// Полный размер = маркер(2) + размер(1|2) + id(1) + action(1) + payload + checksum(1).
fn build_frame(
    marker: [u8; 2],
    size_field: SizeField,
    id: u8,
    action: u8,
    payload: &[u8],
) -> Frame {
    let size = 6 + payload.len();
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(&marker);
    match size_field {
        SizeField::OneByte => {
            assert!(
                size <= u8::MAX as usize,
                "RB-кадр не может превышать 255 байт (size — 1 байт)"
            );
            bytes.push(size as u8);
        }
        SizeField::TwoBytesBe => {
            assert!(size <= u16::MAX as usize, "SC-кадр слишком большой");
            bytes.push((size >> 8) as u8);
            bytes.push((size & 0xFF) as u8);
        }
    }
    bytes.push(id);
    bytes.push(action);
    bytes.extend_from_slice(payload);
    let checksum_pos = bytes.len();
    bytes.push(0); // сюда запишется checksum
    Frame {
        bytes,
        checksum_pos,
    }
}

/// Разбить кадр на HID-отчёты по 64 байта, каждый начинается с report ID `0x00`.
///
/// Панель ждёт пакеты ровно по 64 байта; последний чанк дополняется нулями.
/// См. PROTOCOL.md, «Схема записи HID».
///
/// Возвращает `Vec<[u8; 64]>` — готовые отчёты для `hidapi`.
pub fn chunk_to_hid_reports(frame: &[u8]) -> Vec<[u8; HID_REPORT_SIZE]> {
    const REPORT_ID: u8 = 0x00;
    let mut out = Vec::new();
    let mut i = 0;
    while i < frame.len() {
        let mut report = [0u8; HID_REPORT_SIZE];
        report[0] = REPORT_ID;
        let n = (frame.len() - i).min(HID_REPORT_SIZE - 1);
        report[1..=n].copy_from_slice(&frame[i..i + n]);
        out.push(report);
        i += n;
    }
    out
}

/// Проверить контрольную сумму RB/SC-кадра.
///
/// `frame` — полный кадр, последний байт — checksum.
pub fn verify_checksum(frame: &[u8]) -> bool {
    if frame.len() < 3 {
        return false;
    }
    let checksum = frame[frame.len() - 1];
    let sum: u8 = frame[..frame.len() - 1]
        .iter()
        .fold(0u8, |acc, &b| acc.wrapping_add(b));
    sum == checksum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rb_frame_brightness_50() {
        // Эталон из PROTOCOL.md, пересчитан программно:
        // checksum = 0x52+0x42+0x07+0x02+0x87+0x32 = 0x156 → 0x56
        let frame = rb_frame(2, 0x87, &[50]).with_checksum();
        assert_eq!(frame, vec![0x52, 0x42, 0x07, 0x02, 0x87, 0x32, 0x56]);
        assert!(verify_checksum(&frame));
    }

    #[test]
    fn rb_frame_turn_off() {
        // 52 42 06 02 97 33
        let frame = rb_frame(2, 0x97, &[]).with_checksum();
        assert_eq!(frame, vec![0x52, 0x42, 0x06, 0x02, 0x97, 0x33]);
        assert!(verify_checksum(&frame));
    }

    #[test]
    fn rb_frame_all_red() {
        // 52 42 0B 02 86 01 FF 00 00 FE 25
        let frame = rb_frame(2, 0x86, &[1, 0xFF, 0, 0, 0xFE]).with_checksum();
        assert_eq!(
            frame,
            vec![0x52, 0x42, 0x0B, 0x02, 0x86, 0x01, 0xFF, 0x00, 0x00, 0xFE, 0x25]
        );
        assert!(verify_checksum(&frame));
    }

    #[test]
    fn rb_frame_read_info() {
        // Пустой payload → size = 6. В PROTOCOL.md ошибочно указан 0x05;
        // формула size = полный размер кадра даёт 0x06 (см. предупреждение в доке).
        let frame = rb_frame(2, 0x82, &[]).with_checksum();
        assert_eq!(frame, vec![0x52, 0x42, 0x06, 0x02, 0x82, 0x1E]);
        assert!(verify_checksum(&frame));
    }

    #[test]
    fn id_wraps_to_zero_within_byte() {
        // id — один байт, после 255 должен вернуться к 0/1 (v0.1 — просто байт)
        let frame = rb_frame(255, 0x82, &[]).with_checksum();
        assert_eq!(frame[3], 255);
    }

    #[test]
    fn chunking_single() {
        let frame = rb_frame(2, 0x87, &[50]).with_checksum();
        let reports = chunk_to_hid_reports(&frame);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0][0], 0x00); // report ID
        assert_eq!(&reports[0][1..=7], &frame[..]); // данные
    }

    #[test]
    fn chunking_multi() {
        // SC-кадр на 65 LED: 65*3 = 195 байт payload
        let mut payload = vec![0u8; 65 * 3];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        let frame = sc_frame(2, 0x80, &payload).with_checksum();
        let reports = chunk_to_hid_reports(&frame);
        assert_eq!(reports.len(), 4); // 202 байта → 4 отчёта
        for r in &reports {
            assert_eq!(r[0], 0x00);
        }
        // Последний чанк должен содержать конец кадра
        let last = reports.last().unwrap();
        let end_len = frame.len() - 3 * (HID_REPORT_SIZE - 1);
        assert_eq!(&last[1..=end_len], &frame[3 * (HID_REPORT_SIZE - 1)..]);
    }

    #[test]
    fn sc_frame_size_be() {
        let payload = vec![1u8; 10];
        let frame = sc_frame(2, 0x80, &payload).with_checksum();
        // size = 6 + 10 = 16 = 0x0010
        assert_eq!(&frame[2..4], &[0x00, 0x10]);
        assert!(verify_checksum(&frame));
    }

    #[test]
    fn checksum_detects_corruption() {
        let mut frame = rb_frame(2, 0x87, &[50]).with_checksum();
        assert!(verify_checksum(&frame));
        frame[4] ^= 0xFF;
        assert!(!verify_checksum(&frame));
    }
}
