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
    read_device_info, set_brightness, set_led_effect, set_section_led, turn_off, LED_END,
};
use quiklight_core::protocol::packet::sc_frame;
use quiklight_core::sync::color_extractor::{ColorExtractor, PostProcessConfig};
use quiklight_core::sync::led_mapper::EdgeLayout;
use quiklight_core::sync::screen_capturer::{CaptureError, ScreenCapturer};

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
    /// Поток завершился (после погашения LED при выходе).
    Exited,
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
            let _ = turn_off_all(device);
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
                let _ = events_tx.send(WorkerEvent::DeviceChanged { connected: true, info: None });
            }
            Err(_) => return None,
        }
    }
    state.device.as_mut()
}

/// Устройство перестало отвечать: закрыть дескриптор и сообщить в UI.
fn device_lost(state: &mut State, events_tx: &Sender<WorkerEvent>, error: &DeviceError) {
    state.device = None;
    let _ = events_tx.send(WorkerEvent::DeviceChanged { connected: false, info: None });
    let _ = events_tx.send(WorkerEvent::Error(format!("Устройство отключилось: {error}")));
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
            let _ = events_tx.send(WorkerEvent::DeviceChanged { connected: true, info });
        }
        None => {
            let _ = events_tx.send(WorkerEvent::DeviceChanged { connected: false, info: None });
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

    let Some(capturer) = state.capturer.as_ref() else { return };
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
    let colors = state
        .extractor
        .extract(&frame.data, frame.pitch as u32, frame.width, frame.height, &layout);

    // SC-кадр: action 0x80, payload — цвета RGB подряд.
    let mut payload = Vec::with_capacity(colors.len() * 3);
    for (r, g, b) in colors {
        payload.extend_from_slice(&[r, g, b]);
    }

    if let Some(device) = device_or_reconnect(state, events_tx) {
        let id = device.next_session_id();
        let sc = sc_frame(id, 0x80, &payload).with_checksum();
        if let Err(e) = device.write_frame(&sc) {
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
    let n = layout.total() as usize;
    if n == 0 {
        return;
    }
    let heights = led_heights(&layout);
    let t = state.fireplace_start.elapsed().as_secs_f64();

    let mut payload = Vec::with_capacity(n * 3);
    for (i, &h) in heights.iter().enumerate() {
        // Фаза каждого LED своя — огонь «дышит» по ленте, а не мигает вся сразу.
        let phase = i as f64 * 0.8;
        let (r, g, b) = fire_color(h, t, phase);
        payload.extend_from_slice(&[r, g, b]);
    }

    if let Some(device) = device_or_reconnect(state, events_tx) {
        let id = device.next_session_id();
        let sc = sc_frame(id, 0x80, &payload).with_checksum();
        if let Err(e) = device.write_frame(&sc) {
            device_lost(state, events_tx, &e);
        }
    }
}

/// Высота каждого LED над нижним краем монитора, 0..1, в порядке проводов:
/// левый край снизу вверх → верх → правый сверху вниз → низ.
fn led_heights(layout: &EdgeLayout) -> Vec<f64> {
    let mut heights = Vec::with_capacity(layout.total() as usize);

    // Левый край: LED 0 — нижний угол (0), последний — верхний (1).
    let left = layout.left as usize;
    for k in 0..left {
        heights.push(if left > 1 { k as f64 / (left - 1) as f64 } else { 0.0 });
    }
    // Верх — вся сторона на высоте 1.
    heights.extend(std::iter::repeat_n(1.0, layout.top as usize));
    // Правый край: LED 0 — верхний угол (1), последний — нижний (0).
    let right = layout.right as usize;
    for k in 0..right {
        heights.push(if right > 1 { 1.0 - k as f64 / (right - 1) as f64 } else { 0.0 });
    }
    // Низ — вся сторона на высоте 0.
    heights.extend(std::iter::repeat_n(0.0, layout.bottom as usize));

    heights
}

/// Цвет пламени для LED на высоте `h` (0 = низ монитора, 1 = верх) в момент `t`.
///
/// Огонь «рваный», как живой: мерцание скачет резко, несколько раз в секунду,
/// а не плывёт плавно; тело огня остаётся ярко-оранжевым (не уходит в тёмное).
fn fire_color(h: f64, t: f64, phase: f64) -> (u8, u8, u8) {
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
fn fire_gradient(heat: f64) -> (u8, u8, u8) {
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
    let k = if span <= f64::EPSILON { 0.0 } else { ((heat - t0) / span).clamp(0.0, 1.0) };
    (
        (f64::from(r0) + (f64::from(r1) - f64::from(r0)) * k).round() as u8,
        (f64::from(g0) + (f64::from(g1) - f64::from(g0)) * k).round() as u8,
        (f64::from(b0) + (f64::from(b1) - f64::from(b0)) * k).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quiklight_core::sync::led_mapper::EdgeLayout;

    #[test]
    fn led_count_auto_falls_back_to_device() {
        let s = Settings::default(); // led_count = 0 → авто
        assert_eq!(s.led_count_effective(Some(51)), 51);
        assert_eq!(s.led_count_effective(None), 63); // запасной
        assert_eq!(s.led_count_effective(Some(0)), 63);
    }

    #[test]
    fn led_count_manual_wins() {
        let mut s = Settings::default();
        s.led_count = 100;
        assert_eq!(s.led_count_effective(Some(51)), 100);
    }

    #[test]
    fn layout_auto_uses_device_led() {
        let s = Settings::default();
        let l = s.layout_effective(Some(63));
        assert_eq!((l.left, l.top, l.right, l.bottom), (17, 29, 17, 0));
        let l51 = s.layout_effective(Some(51));
        assert_eq!(l51.total(), 51);
    }

    #[test]
    fn layout_manual_wins() {
        let mut s = Settings::default();
        s.layout = EdgeLayout { left: 5, top: 10, right: 5, bottom: 0 };
        assert_eq!(s.layout_effective(Some(51)).total(), 20);
    }

    #[test]
    fn frame_interval_clamped() {
        let mut s = Settings::default();
        s.fps = 30;
        assert_eq!(s.frame_interval_ms(), 33);
        s.fps = 60;
        assert_eq!(s.frame_interval_ms(), 16);
        s.fps = 5; // ниже минимума → 20 fps → 50 мс
        assert_eq!(s.frame_interval_ms(), 50);
        s.fps = 200; // выше максимума → 60 fps → 16 мс
        assert_eq!(s.frame_interval_ms(), 16);
    }

    #[test]
    fn led_heights_follow_wire_order() {
        let layout = EdgeLayout { left: 3, top: 2, right: 3, bottom: 2 };
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
        let layout = EdgeLayout { left: 1, top: 1, right: 1, bottom: 0 };
        let h = led_heights(&layout);
        assert_eq!(h, vec![0.0, 1.0, 0.0]);
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
                assert!(r >= 200, "у низа пламя должно быть ярко-оранжевым: t={t} phase={phase} → ({r},{g},{b})");
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
        assert!(changed >= 6, "пламя почти не мерцает: изменений {changed}/12");
    }
}
