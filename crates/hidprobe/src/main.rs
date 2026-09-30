//! hidprobe — CLI-утилита для проверки LED-панели QuikLight (фаза 0).
//!
//! Команды:
//! - `hidprobe info`            — readDeviceInfo (MAC, число LED, прошивка)
//! - `hidprobe color R G B`     — вся лента одним цветом (0-255)
//! - `hidprobe brightness N`    — яркость 0-100
//! - `hidprobe off`             — выключить (0x97 + чёрный цвет, повтор 4×)
//! - `hidprobe effect T I`      — встроенный эффект: tipo, index (0-6)
//! - `hidprobe speed N`         — скорость эффектов 0-100 (инверсия в протоколе)
//!
//! Без аргументов — печать справки.

use hidapi::HidApi;
use quiklight_core::hid::device::{Device, DeviceError};
use quiklight_core::hid::device_info::parse_device_info;
use quiklight_core::protocol::actions::{
    read_device_info, set_brightness, set_dynamic_speed, set_led_effect, set_section_led, LED_END,
};

/// Коды выхода для CLI.
mod exit {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 2;
    pub const DEVICE: i32 = 3;
}

/// Ошибка CLI: неверные аргументы или сбой устройства.
#[derive(Debug)]
enum CliError {
    /// Неверные аргументы — код выхода 2.
    Usage(String),
    /// Ошибка работы с устройством — код выхода 3.
    Device(DeviceError),
}

impl From<DeviceError> for CliError {
    fn from(e: DeviceError) -> Self {
        CliError::Device(e)
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "{m}"),
            CliError::Device(e) => write!(f, "{e}"),
        }
    }
}

impl CliError {
    /// Код выхода процесса для этой ошибки.
    fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => exit::USAGE,
            CliError::Device(_) => exit::DEVICE,
        }
    }
}

/// Разобрать один числовой аргумент 0..=100 (для brightness/speed).
fn parse_percent(args: &[String], name: &str) -> Result<u8, CliError> {
    let Some(v) = args.first().and_then(|s| s.parse::<u8>().ok()) else {
        return Err(CliError::Usage(format!("{name} — нужно число 0-100")));
    };
    if v > 100 {
        return Err(CliError::Usage(format!(
            "{name} должен быть 0-100, получено {v}"
        )));
    }
    Ok(v)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(run(&args[1..]));
}

fn run(args: &[String]) -> i32 {
    if args.is_empty() {
        print_usage();
        return exit::OK;
    }

    let api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            eprintln!("hidprobe: ошибка инициализации HID API: {e}");
            return exit::DEVICE;
        }
    };

    let result = match args[0].as_str() {
        "info" => cmd_info(&api),
        "color" => cmd_color(&api, &args[1..]),
        "brightness" => cmd_brightness(&api, &args[1..]),
        "off" => cmd_off(&api),
        "effect" => cmd_effect(&api, &args[1..]),
        "speed" => cmd_speed(&api, &args[1..]),
        "help" | "--help" | "-h" => {
            print_usage();
            return exit::OK;
        }
        other => {
            eprintln!("hidprobe: неизвестная команда `{other}`");
            print_usage();
            return exit::USAGE;
        }
    };

    match result {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("hidprobe: {e}");
            e.exit_code()
        }
    }
}

fn print_usage() {
    println!(
        r#"Проверка панели QuikLight (0x1A86:0xFE07)

Использование:
  hidprobe info                  — информация об устройстве (MAC, LED, прошивка)
  hidprobe color <R> <G> <B>     — вся лента одним цветом (0-255)
  hidprobe brightness <0-100>    — яркость
  hidprobe off                   — выключить подсветку
  hidprobe effect <tipo> <idx>   — встроенный эффект (tipo 2|3, idx 0-6)
  hidprobe speed <0-100>         — скорость эффектов (инвертируется)
  hidprobe help                  — эта справка
"#
    );
}

fn open_device(api: &HidApi) -> Result<Device, DeviceError> {
    Device::open_first(api)
}

fn cmd_info(api: &HidApi) -> Result<(), CliError> {
    let mut dev = open_device(api)?;
    let id = dev.next_session_id();
    let frame = read_device_info(id);
    let raw = dev.write_with_response(&frame)?;
    println!("ОТВЕТ (raw, {} байт): {}", raw.len(), hex(&raw));
    let response = Device::parse_rb_response(&raw)?;

    println!("ОТВЕТ (RB-кадр): {}", hex(&response));

    match parse_device_info(&response) {
        Some(info) => {
            println!(
                "  device_id    : {:02X} {:02X} {:02X}",
                info.device_id[0], info.device_id[1], info.device_id[2]
            );
            println!("  screen_size  : {} дюймов", info.screen_size);
            println!("  LED          : {}", info.led_count);
            println!("  UUID         : {}", info.uuid_string());
            println!("  прошивка     : {}", info.firmware_string());
        }
        None => {
            eprintln!("  предупреждение: ответ не распознан (см. raw выше).");
            eprintln!("  Ожидаем RB-кадр: 'R' 'B' size id 0x82 payload... checksum");
        }
    }
    Ok(())
}

fn parse_rgb(args: &[String]) -> Option<(u8, u8, u8)> {
    if args.len() != 3 {
        return None;
    }
    let parse = |s: &str| -> Option<u8> {
        let v: i32 = s.parse().ok()?;
        if !(0..=255).contains(&v) {
            return None;
        }
        Some(v as u8)
    };
    let r = parse(&args[0])?;
    let g = parse(&args[1])?;
    let b = parse(&args[2])?;
    Some((r, g, b))
}

fn cmd_color(api: &HidApi, args: &[String]) -> Result<(), CliError> {
    let Some((r, g, b)) = parse_rgb(args) else {
        return Err(CliError::Usage(
            "color — нужно три числа 0-255: color <R> <G> <B>".into(),
        ));
    };
    let mut dev = open_device(api)?;
    let id = dev.next_session_id();
    let frame = set_section_led(id, &[(1, r, g, b, LED_END)]);
    dev.write_frame(&frame)?;
    println!("Цвет RGB({r}, {g}, {b}) отправлен (вся лента).");
    Ok(())
}

fn cmd_brightness(api: &HidApi, args: &[String]) -> Result<(), CliError> {
    let val = parse_percent(args, "brightness")?;
    let mut dev = open_device(api)?;
    let id = dev.next_session_id();
    let frame = set_brightness(id, val);
    let _response = dev.write_with_response(&frame)?;
    println!("Яркость {val} установлена.");
    Ok(())
}

fn cmd_off(api: &HidApi) -> Result<(), CliError> {
    let mut dev = open_device(api)?;
    // 0x97 ×4 + чёрный — общий сценарий из ядра (PROTOCOL.md, п. 2–3).
    dev.turn_off_all()?;
    println!("Подсветка выключена (0x97 ×4 + чёрный).");
    Ok(())
}

fn cmd_effect(api: &HidApi, args: &[String]) -> Result<(), CliError> {
    let (tipo, idx) = match args {
        [t, i] => (t.parse::<u8>().ok(), i.parse::<u8>().ok()),
        _ => (None, None),
    };
    let Some((tipo, idx)) = tipo.zip(idx) else {
        return Err(CliError::Usage(
            "effect — нужно `effect <tipo> <idx>`, tipo 2|3, idx 0-6".into(),
        ));
    };
    if tipo != 2 && tipo != 3 {
        return Err(CliError::Usage(format!(
            "tipo должен быть 2 (динамика) или 3 (звук), получено {tipo}"
        )));
    }
    if idx > 6 {
        return Err(CliError::Usage(format!(
            "индекс эффекта 0-6, получено {idx}"
        )));
    }
    let mut dev = open_device(api)?;
    let id = dev.next_session_id();
    let frame = set_led_effect(id, tipo, idx);
    let _response = dev.write_with_response(&frame)?;
    println!("Эффект tipo={tipo}, index={idx} включён.");
    Ok(())
}

fn cmd_speed(api: &HidApi, args: &[String]) -> Result<(), CliError> {
    let val = parse_percent(args, "speed")?;
    let mut dev = open_device(api)?;
    let id = dev.next_session_id();
    // set_dynamic_speed уже инвертирует: пользовательская скорость → устройству.
    let frame = set_dynamic_speed(id, val);
    let _response = dev.write_with_response(&frame)?;
    println!(
        "Скорость {val} установлена (на устройство ушло {}).",
        100 - val
    );
    Ok(())
}

/// Вывести байты hex-строкой для отладки.
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
