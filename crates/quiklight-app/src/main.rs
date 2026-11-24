//! QuikLight — menu bar приложение для LED-панели Robobloq QuikLight.
//!
//! AppKit через objc2 (без Xcode-проекта). Точка входа: каналы UI <-> поток,
//! запуск фонового движка (`engine`) и делегата приложения (`app`).
//!
//! Модули: `app` (делегат), `menu` (меню), `settings_window` (окно настроек),
//! `ui` (хелперы), `settings` (конфиг), `effects` (программные эффекты).

#![allow(clippy::let_unit_value)]

mod app;
mod effects;
mod engine;
mod menu;
mod settings;
mod settings_window;
mod ui;
mod updater;

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{self, Sender};

use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::NSTimer;

use crate::app::{AppDelegate, AppIvars};
use crate::engine::{Command, Mode, WorkerEvent};
use crate::settings::Settings;

fn main() {
    let mtm = MainThreadMarker::new().expect("QuikLight должен запускаться на главном потоке");

    let app = NSApplication::sharedApplication(mtm);
    // Menu bar app: без иконки в Dock.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // Каналы UI ↔ фоновый поток.
    let (cmd_tx, cmd_rx): (Sender<Command>, _) = mpsc::channel();
    let (evt_tx, evt_rx): (Sender<WorkerEvent>, _) = mpsc::channel();
    // Клон для потока обновлений: события идут в тот же канал, который
    // опрашивает таймер UI.
    let evt_tx_updater = evt_tx.clone();
    let _worker = engine::spawn(cmd_rx, evt_tx);

    let delegate = AppDelegate::new(
        mtm,
        AppIvars {
            cmd: RefCell::new(cmd_tx),
            events: RefCell::new(evt_rx),
            power: Cell::new(false),
            mode: Cell::new(Mode::Off),
            brightness: Cell::new(100),
            color: Cell::new((255, 255, 255)),
            settings: Cell::new(Settings::default()),
            device_connected: Cell::new(false),
            device_info: RefCell::new(None),
            status_item: RefCell::new(None),
            status_header: RefCell::new(None),
            power_switch: RefCell::new(None),
            mode_items: RefCell::new(Vec::new()),
            effects_menu_item: RefCell::new(None),
            effect_items: RefCell::new(Vec::new()),
            fireplace_item: RefCell::new(None),
            preset_items: RefCell::new(Vec::new()),
            brightness_slider: RefCell::new(None),
            settings_panel: RefCell::new(None),
            updater_events: RefCell::new(evt_tx_updater),
            check_update_item: RefCell::new(None),
            staged_update: RefCell::new(None),
        },
    );

    delegate.build_ui(mtm);

    // Таймер опроса событий фонового потока (0.25 с).
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            0.25,
            &delegate,
            sel!(tick:),
            None,
            true,
        );
    }

    // `delegate` живёт до выхода из main() — меню, таймер и панели удерживают
    // его и через свои ссылки, так что app.run() работает без дополнительных
    // хитростей.
    let _ = delegate;

    app.run();
}
