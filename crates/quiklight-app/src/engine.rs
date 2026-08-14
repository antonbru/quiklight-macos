//! Фоновый поток приложения QuikLight: владеет устройством (HID) и выполняет
//! режимы — выключено / статичный цвет / эффекты / screen sync.
//!
//! Общение с UI — через каналы: `Command` (из UI) и `WorkerEvent` (в UI).
//! Поток не трогает AppKit: только HID + ScreenCaptureKit + чистая логика.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use hidapi::HidApi;
use quiklight_core::hid::device::{Device, DeviceError};
use quiklight_core::hid::device_info::DeviceInfo;
use quiklight_core::protocol::actions::{
    read_device_info, set_brightness, set_led_effect, set_section_led, turn_off, LED_END,
};
use quiklight_core::protocol::packet::sc_frame;
use quiklight_core::sync::color_extractor::{ColorExtractor, PostProcessConfig};
use quiklight_core::sync::led_mapper::EdgeLayout;
use quiklight_core::sync::screen_capturer::ScreenCapturer;

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
}

/// Настройки приложения (общие для UI и потока).
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Число LED ленты; **0 = авто** (брать из `readDeviceInfo`, запасной 63).
    pub led_count: u16,
    /// Ручная раскладка по краям; если сумма 0 — автоматическая 17/29/17
    /// под эффективное число LED (см. `EdgeLayout::default_for`).
    pub layout: EdgeLayout,
    /// Частота screen sync, кадров/с (20..=60).
    pub fps: u8,
    /// Гамма (1.0 = без коррекции).
    pub gamma: f32,
    /// Насыщенность (1.0 = без изменений).
    pub saturation: f32,
    /// Временно́е сглаживание кадров.
    pub smoothing: bool,
    /// Light compression (ужать яркость > 255).
    pub light_compression: bool,
    /// Выключать подсветку при выходе из приложения.
    pub turn_off_on_exit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            led_count: 0, // авто — из устройства
            layout: EdgeLayout { left: 0, top: 0, right: 0, bottom: 0 },
            fps: 30,
            gamma: 1.0,
            saturation: 1.0,
            smoothing: true,
            light_compression: false,
            turn_off_on_exit: true,
        }
    }
}

impl Settings {
    /// Эффективное число LED: ручное, если задано, иначе из устройства (63 — запасной).
    pub fn led_count_effective(&self, device_led: Option<u16>) -> u16 {
        if self.led_count > 0 {
            self.led_count
        } else {
            device_led.filter(|&n| n > 0).unwrap_or(63)
        }
    }

    /// Эффективная раскладка: ручная, если задана, иначе авто-пропорции
    /// под эффективное число LED.
    pub fn layout_effective(&self, device_led: Option<u16>) -> EdgeLayout {
        if self.layout.total() > 0 {
            self.layout
        } else {
            EdgeLayout::default_for(self.led_count_effective(device_led))
        }
    }

    /// Интервал кадра screen sync в миллисекундах.
    pub fn frame_interval_ms(&self) -> u64 {
        let fps = self.fps.clamp(20, 60) as u64;
        (1000 / fps).clamp(16, 50)
    }

    /// Конфиг постобработки для ColorExtractor.
    pub fn postprocess(&self) -> PostProcessConfig {
        PostProcessConfig {
            gamma: self.gamma,
            saturation: self.saturation,
            light_compression: self.light_compression,
            smoothing: self.smoothing,
        }
    }
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
    DeviceChanged { connected: bool, info: Option<DeviceInfo> },
    /// Режим изменился (подтверждение).
    ModeChanged(Mode),
    /// Ошибка (например, нет разрешения «Запись экрана»).
    Error(String),
}

/// Запустить фоновый поток.
pub fn spawn(cmd_rx: Receiver<Command>, events_tx: Sender<WorkerEvent>) -> std::thread::JoinHandle<()> {
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
        capturer: None,
        extractor: ColorExtractor::new(Settings::default().postprocess()),
        sync_error_reported: false,
    };

    // Первичная проверка устройства.
    report_device(&mut state, &events_tx);

    loop {
        let msg = if state.mode == Mode::Sync {
            cmd_rx.recv_timeout(Duration::from_millis(state.settings.frame_interval_ms()))
        } else {
            match cmd_rx.recv() {
                Ok(cmd) => Ok(cmd),
                Err(_) => Err(RecvTimeoutError::Disconnected),
            }
        };

        match msg {
            Ok(cmd) => {
                if !handle_command(&mut state, cmd, &events_tx) {
                    break; // Quit
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // В режиме sync таймаут = пора слать следующий кадр.
                if state.mode == Mode::Sync {
                    sync_step(&mut state, &events_tx);
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    // Выход: гасим подсветку, если так настроено.
    if state.settings.turn_off_on_exit {
        if let Some(device) = state.device.as_mut() {
            let _ = turn_off_all(device);
        }
    }
}

/// Обработать команду. Возвращает `false`, если поток должен завершиться.
fn handle_command(state: &mut State, cmd: Command, events_tx: &Sender<WorkerEvent>) -> bool {
    match cmd {
        Command::Quit => return false,
        Command::SetPower(on) => {
            state.power = on;
            if on {
                apply_mode(state);
            } else {
                turn_off_and_report(state, events_tx);
            }
        }
        Command::SetMode(mode) => {
            state.mode = mode;
            if state.power {
                apply_mode(state);
            }
            let _ = events_tx.send(WorkerEvent::ModeChanged(mode));
        }
        Command::SetColor { r, g, b } => {
            state.color = (r, g, b);
            if state.power && state.mode == Mode::Static {
                apply_static(state);
            }
        }
        Command::SetBrightness(v) => {
            state.brightness = v.min(100);
            let brightness = state.brightness;
            if let Some(device) = device_or_reconnect(state) {
                let id = device.next_session_id();
                let _ = device.write_with_response(&set_brightness(id, brightness));
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
fn apply_mode(state: &mut State) {
    match state.mode {
        Mode::Off => {}
        Mode::Static => apply_static(state),
        Mode::Effects(tipo, index) => {
            if let Some(device) = device_or_reconnect(state) {
                let id = device.next_session_id();
                let frame = set_led_effect(id, tipo, index);
                let _ = device.write_with_response(&frame);
            }
        }
        Mode::Sync => {
            // Захватчик создаётся при первом кадре (нужно разрешение экрана).
        }
    }
}

fn apply_static(state: &mut State) {
    let (r, g, b) = state.color;
    if let Some(device) = device_or_reconnect(state) {
        let id = device.next_session_id();
        let frame = set_section_led(id, &[(1, r, g, b, LED_END)]);
        let _ = device.write_frame(&frame);
    }
}

/// Выключить подсветку: 0x97 ×4 + чёрный (см. PROTOCOL.md, подводные камни).
fn turn_off_all(device: &mut Device) -> Result<(), DeviceError> {
    for _ in 0..4 {
        let id = device.next_session_id();
        device.write_frame(&turn_off(id))?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let id = device.next_session_id();
    device.write_frame(&set_section_led(id, &[(1, 0, 0, 0, LED_END)]))
}

fn turn_off_and_report(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    if let Some(device) = state.device.as_mut() {
        if let Err(e) = turn_off_all(device) {
            state.device = None;
            let _ = events_tx.send(WorkerEvent::DeviceChanged { connected: false, info: None });
            let _ = events_tx.send(WorkerEvent::Error(format!("Устройство отключилось: {e}")));
        }
    }
}

/// Открыть устройство, если ещё не открыто; при ошибке — сообщить в UI.
fn device_or_reconnect(state: &mut State) -> Option<&mut Device> {
    if state.device.is_none() {
        let api = HidApi::new().ok()?;
        match Device::open_first(&api) {
            Ok(device) => state.device = Some(device),
            Err(_) => return None,
        }
    }
    state.device.as_mut()
}

/// Прочитать информацию об устройстве и сообщить о подключении.
fn report_device(state: &mut State, events_tx: &Sender<WorkerEvent>) {
    match device_or_reconnect(state) {
        Some(device) => {
            let id = device.next_session_id();
            let info = device
                .write_with_response(&read_device_info(id))
                .ok()
                .and_then(|raw| Device::parse_rb_response(&raw).ok())
                .and_then(|frame| quiklight_core::hid::device_info::parse_device_info(&frame));
            // Запоминаем фактическое число LED — для авто-раскладки screen sync.
            state.device_led = info.as_ref().map(|i| u16::from(i.led_count));
            let _ = events_tx.send(WorkerEvent::DeviceChanged { connected: true, info });
        }
        None => {
            let _ = events_tx.send(WorkerEvent::DeviceChanged { connected: false, info: None });
        }
    }
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
                    let _ = events_tx.send(WorkerEvent::Error(format!(
                        "Screen sync недоступен: {e}"
                    )));
                }
                return;
            }
        }
    }

    let Some(capturer) = state.capturer.as_ref() else { return };
    let frame = match capturer.capture_frame() {
        Ok(frame) => frame,
        Err(e) => {
            if !state.sync_error_reported {
                state.sync_error_reported = true;
                let _ = events_tx.send(WorkerEvent::Error(format!("Ошибка захвата экрана: {e}")));
            }
            return;
        }
    };
    state.sync_error_reported = false;

    let layout = state.settings.layout_effective(state.device_led);
    let colors = state
        .extractor
        .extract(&frame.data, frame.pitch as u32, frame.width, frame.height, &layout);

    // SC-кадр: action 0x80, payload — цвета RGB подряд.
    let mut payload = Vec::with_capacity(colors.len() * 3);
    for (r, g, b) in colors {
        payload.extend_from_slice(&[r, g, b]);
    }

    if let Some(device) = device_or_reconnect(state) {
        let id = device.next_session_id();
        let sc = sc_frame(id, 0x80, &payload).with_checksum();
        let _ = device.write_frame(&sc);
    }
}
