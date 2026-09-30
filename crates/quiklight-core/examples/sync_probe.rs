//! Диагностика SC-каналов screen sync на реальном железе (action 0x80).
//!
//! Отправляет на панель два градиентных SC-кадра подряд (проверка многочанкового
//! разбиения 51 LED × 3 байта = 153 байта payload → 3 HID-отчёта), затем гасит.
//!
//! Запуск: `cargo run -p quiklight-core --example sync_probe`
//!
//! Ожидание: лента показывает радужный градиент (первый кадр), затем инвертированный
//! (второй кадр), после чего гаснет.

use hidapi::HidApi;
use quiklight_core::hid::device::Device;
use quiklight_core::protocol::actions::{turn_off, LED_END};
use quiklight_core::protocol::packet::sc_frame;

fn main() {
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("sync_probe: ошибка HID API: {e}");
            std::process::exit(3);
        }
    };
    let mut dev = match Device::open_first(&api) {
        Ok(dev) => dev,
        Err(e) => {
            eprintln!("sync_probe: панель не найдена: {e}");
            std::process::exit(3);
        }
    };

    // Число LED — с устройства.
    let id = dev.next_session_id();
    let info = dev
        .write_with_response(&quiklight_core::protocol::actions::read_device_info(id))
        .ok()
        .and_then(|raw| Device::parse_rb_response(&raw).ok())
        .and_then(|frame| quiklight_core::hid::device_info::parse_device_info(&frame));
    let led_count = info.as_ref().map(|i| i.led_count as usize).unwrap_or(51);
    println!(
        "sync_probe: LED = {led_count}, прошивка = {}",
        info.map(|i| i.firmware_string())
            .unwrap_or_else(|| "?".into())
    );

    // Градиент: цвет плавно идёт по кругу (красный → зелёный → синий → красный).
    let grad = |phase: f32| -> Vec<u8> {
        let mut payload = Vec::with_capacity(led_count * 3);
        for i in 0..led_count {
            let t = (i as f32 / led_count as f32 + phase) % 1.0;
            // HSV→RGB по сегментам (без библиотек): конвертируем h=t через 6-секторную формулу.
            let h = t * 360.0;
            let (r, g, b) = hsv(h, 1.0, 1.0);
            payload.extend_from_slice(&[r, g, b]);
        }
        payload
    };

    let send = |dev: &mut Device, payload: &[u8]| {
        let id = dev.next_session_id();
        let frame = sc_frame(id, 0x80, payload).with_checksum();
        dev.write_frame(&frame)
    };

    // Кадр 1: градиент от красного.
    let p1 = grad(0.0);
    println!(
        "sync_probe: шлю SC-кадр 1 ({} байт payload, {} HID-отчётов)",
        p1.len(),
        (frame_len(&p1) + 62) / 63
    );
    send(&mut dev, &p1).expect("кадр 1");

    std::thread::sleep(std::time::Duration::from_secs(2));

    // Кадр 2: инвертированный градиент — проверка, что панель принимает обновления.
    let p2 = grad(0.5);
    println!("sync_probe: шлю SC-кадр 2");
    send(&mut dev, &p2).expect("кадр 2");

    std::thread::sleep(std::time::Duration::from_secs(2));

    // Гасим: 0x97 ×4 + чёрный (совместимость со старыми прошивками).
    for _ in 0..4 {
        let id = dev.next_session_id();
        dev.write_frame(&turn_off(id)).expect("off");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let id = dev.next_session_id();
    let black = quiklight_core::protocol::actions::set_section_led(id, &[(1, 0, 0, 0, LED_END)]);
    dev.write_frame(&black).expect("black");
    println!("sync_probe: готово, подсветка выключена");
}

/// Полная длина SC-кадра (для оценки числа HID-отчётов).
fn frame_len(payload: &[u8]) -> usize {
    6 + payload.len() + 1 // 'S' 'C' size(2) id action payload checksum
}

/// Простой HSV→RGB (h в градусах 0..360).
fn hsv(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
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
