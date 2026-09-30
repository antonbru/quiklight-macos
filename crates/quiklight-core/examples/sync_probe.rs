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
use quiklight_core::color::hsv_to_rgb;
use quiklight_core::hid::device::Device;
use quiklight_core::hid::device_info::parse_device_info;
use quiklight_core::protocol::actions::read_device_info;

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
        .write_with_response(&read_device_info(id))
        .ok()
        .and_then(|raw| Device::parse_rb_response(&raw).ok())
        .and_then(|frame| parse_device_info(&frame));
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
            let (r, g, b) = hsv_to_rgb(t * 360.0, 1.0, 1.0);
            payload.extend_from_slice(&[r, g, b]);
        }
        payload
    };

    let send = |dev: &mut Device, payload: &[u8]| dev.send_screen_sync(payload);

    // Кадр 1: градиент от красного.
    let p1 = grad(0.0);
    println!(
        "sync_probe: шлю SC-кадр 1 ({} байт payload, {} HID-отчётов)",
        p1.len(),
        frame_len(&p1).div_ceil(63)
    );
    send(&mut dev, &p1).expect("кадр 1");

    std::thread::sleep(std::time::Duration::from_secs(2));

    // Кадр 2: инвертированный градиент — проверка, что панель принимает обновления.
    let p2 = grad(0.5);
    println!("sync_probe: шлю SC-кадр 2");
    send(&mut dev, &p2).expect("кадр 2");

    std::thread::sleep(std::time::Duration::from_secs(2));

    // Гасим: 0x97 ×4 + чёрный (совместимость со старыми прошивками).
    dev.turn_off_all().expect("off");
    println!("sync_probe: готово, подсветка выключена");
}

/// Полная длина SC-кадра (для оценки числа HID-отчётов).
fn frame_len(payload: &[u8]) -> usize {
    6 + payload.len() + 1 // 'S' 'C' size(2) id action payload checksum
}
