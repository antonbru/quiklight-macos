//! Фоновый поток приложения QuikLight: владеет устройством (HID) и выполняет
//! режимы — выключено / статичный цвет / эффекты / screen sync.
//!
//! Общение с UI — через каналы: `Command` (из UI) и `WorkerEvent` (в UI).
//! Поток не трогает AppKit: только HID + ScreenCaptureKit + чистая логика.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use hidapi::HidApi;
use quiklight_core::hid::device::{Device, DeviceError};
use quiklight_core::hid::device_info::DeviceInfo;
use quiklight_core::protocol::actions::{
    read_device_info, set_brightness, set_led_effect, set_section_led, LED_END,
};
use quiklight_core::sync::color_extractor::ColorExtractor;
use quiklight_core::sync::screen_capturer::{CaptureError, ScreenCapturer};

use crate::settings::Settings;
use crate::updater::UpdateEvent;

/// Режим работы подсветки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Подсветка выключена.
    Off,
    /// Статичный цвет (последний выбранный).
    Static,
    /// Встроенный эффект (tipo, index).
    Effects(u8, u8),
    /// Screen sync: цвета краёв экрана.
    Sync,
    /// Свой эффект «Камин»: резко мерцающее пламя по нижней части ленты.
    Fireplace,
}

/// Команда из UI в поток.
#[derive(Debug, Clone, Copy)]
pub enum Command {
    /// Включить/выключить подсветку.
    SetPower(bool),
    /// Переключить режим.
    SetMode(Mode),
    /// Задать статичный цвет.
    SetColor { r: u8, g: u8, b: u8 },
    /// Задать яркость 0..100.
    SetBrightness(u8),
    /// Обновить настройки.
    SetSettings(Settings),
    /// Завершить работу (при выходе погасить LED, если настроено).
    Quit,
}

/// Событие из потока в UI.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// Устройство подключено/отключено (+ информация, если есть).
    DeviceChanged {
        connected: bool,
        info: Option<DeviceInfo>,
    },
    /// Режим изменился (подтверждение).
    ModeChanged(Mode),
    /// Ошибка (например, нет разрешения «Запись экрана»).
    Error(String),
    /// Событие проверки/установки обновления (см. `updater`).
    Update(UpdateEvent),
    /// Поток завершился (после погашения LED при выходе).
    Exited,
}

/// Запустить фоновый поток.
pub fn spawn(
    cmd_rx: Receiver<Command>,
    events_tx: Sender<WorkerEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("quiklight-worker".into())
        .spawn(move || run(cmd_rx, events_tx))
        .expect("не удалось запустить поток")
}

/// Состояние потока.
struct State {
    settings: Settings,
    mode: Mode,
    power: bool,
    brightness: u8,
    color: (u8, u8, u8),
    device: Option<Device>,
    /// Число LED, прочитанное с устройства (для авто-режима настроек).
    device_led: Option<u16>,
    /// Момент, раньше которого не пытаемся переподключиться (анти-спам 1 р/с).
    reconnect_at: Instant,
    /// Момент включения режима «Камин» — от него идёт время анимации пламени.
    fireplace_start: Instant,
    capturer: Option<ScreenCapturer>,
    extractor: ColorExtractor,
    /// Ошибка screen sync уже показана пользователю (не спамить).
    sync_error_reported: bool,
}

fn run(cmd_rx: Receiver<Command>, events_tx: Sender<WorkerEvent>) {
    let mut state = State {
        settings: Settings::default(),
        mode: Mode::Off,
        power: false,
        brightness: 100,
        color: (255, 255, 255),
        device: None,
        device_led: None,
        reconnect_at: Instant::now(),
        fireplace_start: Instant::now(),
        capturer: None,
        extractor: ColorExtractor::new(Settings::default().postprocess()),
        sync_error_reported: false,
    };

    // Первичная проверка устройства.
    report_device(&mut state, &events_tx);

    loop {
        let msg = match frame_interval(&state) {
            Some(interval) => cmd_rx.recv_timeout(interval),
            None => match cmd_rx.recv() {
                Ok(cmd) => Ok(cmd),
                Err(_) => Err(RecvTimeoutError::Disconnected),
            },
        };

        match msg {
            Ok(cmd) => {
                if !handle_command(&mut state, cmd, &events_tx) {
                    break; // Quit
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Таймаут в непрерывных режимах = пора слать следующий кадр,
                // но только при включённой подсветке: иначе после «Подсветка:
                // Выкл» (turn_off_all) очередной кадр снова зажжёт LED —
                // выключение в режимах Screen sync / Камин не работало бы.
                if state.power {
                    match state.mode {
                        Mode::Sync => sync_step(&mut state, &events_tx),
                        Mode::Fireplace => fireplace_step(&mut state, &events_tx),
                        _ => {}
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    // Выход: гасим подсветку, если так настроено.
    if state.settings.turn_off_on_exit {
        if let Some(device) = state.device.as_mut() {
            let _ = device.turn_off_all();
        }
    }
    // Сообщаем UI, что поток завершился (UI ждёт это перед выходом).
    let _ = events_tx.send(WorkerEvent::Exited);
}

/// Интервал кадра для непрерывных режимов (Sync / Fireplace), `None` — блокирующий приём.
fn frame_interval(state: &State) -> Option<Duration> {
    match state.mode {
        Mode::Sync => Some(Duration::from_millis(state.settings.frame_interval_ms())),
        // Камин рисуем ~30 fps — достаточно для плавного мерцания без нагрузки.
        Mode::Fireplace => Some(Duration::from_millis(33)),
        _ => None,
    }
}

/// Обработать команду. Возвращает `false`, если поток должен завершиться.
fn handle_command(state: &mut State, cmd: Command, events_tx: &Sender<WorkerEvent>) -> bool {
    match cmd {
        Command::Quit => return false,
        Command::SetPower(on) => {
            state.power = on;
            if on {
                apply_mode(state, events_tx);
            } else {
                turn_off_and_report(state, events_tx);
            }
        }
        Command::SetMode(mode) => {
            state.mode = mode;
            if state.power {
                apply_mode(state, events_tx);
            }
            let _ = events_tx.send(WorkerEvent::ModeChanged(mode));
        }
        Command::SetColor { r, g, b } => {
            state.color = (r, g, b);
            if state.power && state.mode == Mode::Static {
                apply_static(state, events_tx);
            }
        }
        Command::SetBrightness(v) => {
            state.brightness = v.min(100);
            let brightness = state.brightness;
            let static_mode = state.power && state.mode == Mode::Static;
            if let Some(device) = device_or_reconnect(state, events_tx) {
                let id = device.next_session_id();
                // Пишем без ожидания ответа: ответ на 0x87 нам не нужен, а
                // write_with_response блокирует поток на 200–700 мс на каждый
                // тик слайдера (очередь команд растягивается, а при отсутствии
                // ответа — ложный device_lost и переподключение).
                if let Err(e) = device.write_frame(&set_brightness(id, brightness)) {
                    device_lost(state, events_tx, &e);
                }
            }
            // Панель применяет яркость только при следующем рендере кадра,
            // поэтому после изменения яркости перерисовываем статичный цвет —
            // иначе яркость «проявляется» лишь при следующей смене цвета.
            if static_mode {
                apply_static(state, events_tx);
            }
        }
        Command::SetSettings(s) => {
            state.settings = s;
            state.extractor.set_config(s.postprocess());
            // В sync режиме пересоздаём захватчик под новый fps не нужно —
            // интервал берётся из настроек каждый кадр. Применяем режим заново.
            if state.power && state.mode == Mode::Sync {
                sync_step(state, events_tx);
            }
        }
    }
    true
}

/// Применить текущий режим к устройству.
fn apply_mode(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    match state.mode {
        Mode::Off => {}
        Mode::Static => apply_static(state, events_tx),
        Mode::Effects(tipo, index) => {
            if let Some(device) = device_or_reconnect(state, events_tx) {
                let id = device.next_session_id();
                let frame = set_led_effect(id, tipo, index);
                if let Err(e) = device.write_with_response(&frame) {
                    device_lost(state, events_tx, &e);
                }
            }
        }
        Mode::Sync => {
            // Захватчик создаётся при первом кадре (нужно разрешение экрана).
        }
        Mode::Fireplace => {
            // Перезапускаем анимацию с нуля; первый кадр придёт по таймеру.
            state.fireplace_start = Instant::now();
        }
    }
}

fn apply_static(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    let (r, g, b) = state.color;
    if let Some(device) = device_or_reconnect(state, events_tx) {
        let id = device.next_session_id();
        let frame = set_section_led(id, &[(1, r, g, b, LED_END)]);
        if let Err(e) = device.write_frame(&frame) {
            device_lost(state, events_tx, &e);
        }
    }
}

fn turn_off_and_report(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    if let Some(device) = state.device.as_mut() {
        if let Err(e) = device.turn_off_all() {
            state.device = None;
            let _ = events_tx.send(WorkerEvent::DeviceChanged {
                connected: false,
                info: None,
            });
            let _ = events_tx.send(WorkerEvent::Error(format!("Устройство отключилось: {e}")));
        }
    }
}

/// Открыть устройство, если ещё не открыто; при успешном переподключении
/// сообщить в UI (анти-спам: попытки не чаще раза в секунду).
fn device_or_reconnect<'a>(
    state: &'a mut State,
    events_tx: &Sender<WorkerEvent>,
) -> Option<&'a mut Device> {
    if state.device.is_none() {
        if Instant::now() < state.reconnect_at {
            return None;
        }
        state.reconnect_at = Instant::now() + Duration::from_secs(1);
        let api = HidApi::new().ok()?;
        match Device::open_first(&api) {
            Ok(device) => {
                state.device = Some(device);
                let _ = events_tx.send(WorkerEvent::DeviceChanged {
                    connected: true,
                    info: None,
                });
            }
            Err(_) => return None,
        }
    }
    state.device.as_mut()
}

/// Устройство перестало отвечать: закрыть дескриптор и сообщить в UI.
fn device_lost(state: &mut State, events_tx: &Sender<WorkerEvent>, error: &DeviceError) {
    state.device = None;
    let _ = events_tx.send(WorkerEvent::DeviceChanged {
        connected: false,
        info: None,
    });
    let _ = events_tx.send(WorkerEvent::Error(format!(
        "Устройство отключилось: {error}"
    )));
}

/// Прочитать информацию об устройстве и сообщить о подключении.
fn report_device(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    match device_or_reconnect(state, events_tx) {
        Some(device) => {
            let id = device.next_session_id();
            let info = device
                .write_with_response(&read_device_info(id))
                .ok()
                .and_then(|raw| Device::parse_rb_response(&raw).ok())
                .and_then(|frame| quiklight_core::hid::device_info::parse_device_info(&frame));
            // Запоминаем фактическое число LED — для авто-раскладки screen sync.
            state.device_led = info.as_ref().map(|i| u16::from(i.led_count));
            let _ = events_tx.send(WorkerEvent::DeviceChanged {
                connected: true,
                info,
            });
        }
        None => {
            let _ = events_tx.send(WorkerEvent::DeviceChanged {
                connected: false,
                info: None,
            });
        }
    }
}

/// Сообщение об ошибке screen sync. К ошибкам ScreenCaptureKit (обычно это
/// разрешение «Запись экрана») добавляем конкретные шаги — иначе пользователь
/// видит только код ошибки и не понимает, что делать.
fn sync_error_message(prefix: &str, e: &CaptureError) -> String {
    let hint = match e {
        CaptureError::ScreenCapture(_) => "\n\nРазрешение «Запись экрана» не подхватилось:\n\
            • Системные настройки → Конфиденциальность и безопасность → «Запись экрана и системного звука» (macOS 14: «Запись экрана»);\n\
            • включён должен быть именно QuikLight — удалите старые записи QuikLight и включите заново;\n\
            • полностью закройте приложение (Cmd+Q) и запустите снова — разрешение применяется только к новому процессу;\n\
            • запускайте собранный QuikLight.app (open), а не бинарник из target/;\n\
            • после пересборки (cargo build / build_app.sh) разрешение нужно выдавать заново: ad-hoc подпись меняет «личность» приложения, и macOS считает его новым."
            .to_string(),
        _ => String::new(),
    };
    format!("{prefix}: {e}{hint}")
}

/// Один шаг screen sync: захват → извлечение цветов → отправка SC-кадра.
fn sync_step(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    // Захватчик создаётся лениво — ScreenCaptureKit может требовать разрешение.
    if state.capturer.is_none() {
        match ScreenCapturer::new() {
            Ok(capturer) => {
                state.capturer = Some(capturer);
                state.sync_error_reported = false;
            }
            Err(e) => {
                if !state.sync_error_reported {
                    state.sync_error_reported = true;
                    let _ = events_tx.send(WorkerEvent::Error(sync_error_message(
                        "Screen sync недоступен",
                        &e,
                    )));
                }
                return;
            }
        }
    }

    let Some(capturer) = state.capturer.as_ref() else {
        return;
    };
    let frame = match capturer.capture_frame() {
        Ok(frame) => frame,
        Err(e) => {
            if !state.sync_error_reported {
                state.sync_error_reported = true;
                let _ = events_tx.send(WorkerEvent::Error(sync_error_message(
                    "Ошибка захвата экрана",
                    &e,
                )));
            }
            return;
        }
    };
    state.sync_error_reported = false;

    let layout = state.settings.layout_effective(state.device_led);
    let colors =
        state
            .extractor
            .extract(&frame.data, frame.pitch, frame.width, frame.height, &layout);

    // SC-кадр: action 128, payload — цвета RGB подряд.
    let mut payload = Vec::with_capacity(colors.len() * 3);
    for (r, g, b) in colors {
        payload.extend_from_slice(&[r, g, b]);
    }

    if let Some(device) = device_or_reconnect(state, events_tx) {
        if let Err(e) = device.send_screen_sync(&payload) {
            device_lost(state, events_tx, &e);
        }
    }
}

/// Один шаг «камина»: сгенерировать пламя по ленте и отправить SC-кадр.
///
/// Пламя живёт у нижней части ленты (нижние углы и нижняя половина боковых
/// краёв): ядро — ярко-оранжевое, к верху переходит в красный и гаснет.
/// Мерцание — резкие скачки (несколько раз в секунду) с общей «групповой»
/// составляющей: соседние LED вспыхивают и гаснут языками, как живой огонь.
fn fireplace_step(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    let layout = state.settings.layout_effective(state.device_led);
    if layout.total() == 0 {
        return;
    }
    let t = state.fireplace_start.elapsed().as_secs_f64();
    let payload = crate::effects::fireplace::payload(&layout, t);

    if let Some(device) = device_or_reconnect(state, events_tx) {
        if let Err(e) = device.send_screen_sync(&payload) {
            device_lost(state, events_tx, &e);
        }
    }
}
