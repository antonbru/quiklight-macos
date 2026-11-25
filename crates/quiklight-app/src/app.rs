//! Делегат приложения: состояние UI, действия меню, применение событий потока.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};

use objc2_app_kit::{
    NSApplication, NSButton, NSControlStateValueOff, NSControlStateValueOn, NSMenuItem, NSSlider,
    NSStatusBar, NSStatusItem, NSStepper, NSSwitch, NSTextField, NSVariableStatusItemLength,
};
use objc2_foundation::{NSString, NSTimer};

use crate::engine::{Command, Mode, WorkerEvent};
use crate::menu::{PRESET_COLORS, TAG_EFFECT_FIREPLACE, TAG_MODE_STATIC, TAG_MODE_SYNC};
use crate::settings::Settings;
use crate::settings_window::SettingsPanel;
use crate::ui::{color_circle_image, confirm_alert, show_alert, status_icon};
use crate::updater::{self, UpdateEvent};
/// Кнопка-кружок пресета палитры: цвет (R, G, B) + кнопка.
pub(crate) type PresetButton = (u8, u8, u8, Retained<NSButton>);

/// Ивар-хранилище делегата приложения.
pub(crate) struct AppIvars {
    /// Канал команд в фоновый поток.
    pub(crate) cmd: RefCell<Sender<Command>>,
    /// Канал событий из фонового потока (опрашивается таймером).
    pub(crate) events: RefCell<Receiver<WorkerEvent>>,

    // Текущее состояние UI.
    pub(crate) power: Cell<bool>,
    pub(crate) mode: Cell<Mode>,
    pub(crate) brightness: Cell<u8>,
    pub(crate) color: Cell<(u8, u8, u8)>,
    pub(crate) settings: Cell<Settings>,
    pub(crate) device_connected: Cell<bool>,
    pub(crate) device_info: RefCell<Option<quiklight_core::hid::device_info::DeviceInfo>>,

    // Хэндлы UI (заполняются в `build_ui`).
    pub(crate) status_item: RefCell<Option<Retained<NSStatusItem>>>,
    pub(crate) power_switch: RefCell<Option<Retained<NSSwitch>>>,
    pub(crate) mode_items: RefCell<Vec<(Mode, Retained<NSMenuItem>)>>,
    pub(crate) effects_menu_item: RefCell<Option<Retained<NSMenuItem>>>,
    pub(crate) effect_items: RefCell<Vec<(u8, u8, Retained<NSMenuItem>)>>,
    pub(crate) fireplace_item: RefCell<Option<Retained<NSMenuItem>>>,
    pub(crate) preset_items: RefCell<Vec<PresetButton>>,
    /// Пресет, вокруг которого сейчас нарисовано кольцо (`None` — без кольца).
    pub(crate) preset_ring: Cell<Option<(u8, u8, u8)>>,
    pub(crate) settings_panel: RefCell<Option<SettingsPanel>>,

    // Обновление приложения.
    /// Клон канала событий для потоков проверки/загрузки обновления.
    pub(crate) updater_events: RefCell<Sender<WorkerEvent>>,
    /// Пункт меню «Проверить обновление…».
    pub(crate) check_update_item: RefCell<Option<Retained<NSMenuItem>>>,
    /// Скачанный и проверенный бандл, готовый к подмене.
    pub(crate) staged_update: RefCell<Option<PathBuf>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "QuikLightAppDelegate"]
    #[ivars = AppIvars]
    pub(crate) struct AppDelegate;

    impl AppDelegate {
        // ── таймер: опрос событий потока ──
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            let events = self.ivars().events.borrow_mut();
            for event in events.try_iter() {
                self.apply_event(event);
            }
        }

        // ── действия меню ──
        #[unsafe(method(togglePower:))]
        fn toggle_power(&self, _sender: &AnyObject) {
            let on = !self.ivars().power.get();
            self.ivars().power.set(on);
            if on && self.ivars().mode.get() == Mode::Off {
                self.ivars().mode.set(Mode::Static);
            }
            self.send(Command::SetPower(on));
            self.send(Command::SetMode(self.ivars().mode.get()));
            self.update_ui();
        }

        #[unsafe(method(setMode:))]
        fn set_mode(&self, sender: &AnyObject) {
            let Some(item) = sender.downcast_ref::<NSMenuItem>() else { return };
            let mode = match item.tag() {
                TAG_MODE_STATIC => Mode::Static,
                TAG_MODE_SYNC => Mode::Sync,
                _ => return,
            };
            self.ivars().mode.set(mode);
            self.ivars().power.set(true);
            self.send(Command::SetPower(true));
            self.send(Command::SetMode(mode));
            self.update_ui();
        }

        #[unsafe(method(setEffect:))]
        fn set_effect(&self, sender: &AnyObject) {
            let Some(item) = sender.downcast_ref::<NSMenuItem>() else { return };
            let tag = item.tag();
            // tag = tipo*100 + index (200..206 — динамика, 300..306 — звук);
            // 900 — свой эффект «Камин».
            let mode = if tag == TAG_EFFECT_FIREPLACE {
                Mode::Fireplace
            } else {
                let (tipo, index) = ((tag / 100) as u8, (tag % 100) as u8);
                Mode::Effects(tipo, index)
            };
            self.ivars().mode.set(mode);
            self.ivars().power.set(true);
            self.send(Command::SetPower(true));
            self.send(Command::SetMode(mode));
            self.update_ui();
        }

        #[unsafe(method(setPresetColor:))]
        fn set_preset_color(&self, sender: &AnyObject) {
            // Сендер — кнопка-кружок в строке палитры (тег = индекс пресета).
            let idx = if let Some(btn) = sender.downcast_ref::<NSButton>() {
                btn.tag()
            } else if let Some(item) = sender.downcast_ref::<NSMenuItem>() {
                item.tag()
            } else {
                return;
            };
            let Some(&(_, r, g, b)) = PRESET_COLORS.get(idx as usize) else { return };
            self.ivars().color.set((r, g, b));
            self.ivars().mode.set(Mode::Static);
            self.ivars().power.set(true);
            self.send(Command::SetPower(true));
            self.send(Command::SetMode(Mode::Static));
            self.send(Command::SetColor { r, g, b });
            self.update_ui();
        }

        #[unsafe(method(brightnessChanged:))]
        fn brightness_changed(&self, sender: &AnyObject) {
            let Some(slider) = sender.downcast_ref::<NSSlider>() else {
                return;
            };
            let v = slider.doubleValue().round().clamp(0.0, 100.0) as u8;
            self.ivars().brightness.set(v);
            self.send(Command::SetBrightness(v));
            // Меню не обновляем: ползунок уже показывает значение, а перерисовка
            // пунктов меню прямо во время перетаскивания сбивает tracking слайдера.
        }

        // ── окно настроек ──
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: &AnyObject) {
            let mtm = MainThreadMarker::new().unwrap();
            if self.ivars().settings_panel.borrow().is_none() {
                let panel = self.build_settings_panel(mtm);
                *self.ivars().settings_panel.borrow_mut() = Some(panel);
            }
            let panel_ref = self.ivars().settings_panel.borrow();
            if let Some(panel) = panel_ref.as_ref() {
                self.refresh_settings_panel(panel);
                panel.window.makeKeyAndOrderFront(Some(self));
                panel.window.center();
                NSApplication::sharedApplication(mtm).activate();
            }
        }

        #[unsafe(method(applySettings:))]
        fn apply_settings(&self, _sender: &AnyObject) {
            let panel_ref = self.ivars().settings_panel.borrow();
            let Some(panel) = panel_ref.as_ref() else { return };

            let led = panel.led_stepper.doubleValue().round().clamp(0.0, 500.0) as u16;
            let top = panel.top_stepper.doubleValue().round().clamp(0.0, 200.0) as u16;
            let left = panel.left_stepper.doubleValue().round().clamp(0.0, 200.0) as u16;
            let right = panel.right_stepper.doubleValue().round().clamp(0.0, 200.0) as u16;
            let fps = panel.fps_slider.doubleValue().round().clamp(20.0, 60.0) as u8;
            let gamma = panel.gamma_slider.doubleValue().clamp(0.5, 2.5) as f32;
            let saturation = panel.sat_slider.doubleValue().clamp(0.0, 2.0) as f32;
            let smoothing = panel.smooth_check.state() == NSControlStateValueOn;
            let light_compression = panel.lc_check.state() == NSControlStateValueOn;
            let turn_off_on_exit = panel.off_check.state() == NSControlStateValueOn;

            let settings = Settings {
                led_count: led,
                layout: quiklight_core::sync::led_mapper::EdgeLayout { top, left, right, bottom: 0 },
                fps,
                gamma,
                saturation,
                smoothing,
                light_compression,
                turn_off_on_exit,
            };
            drop(panel_ref);
            self.ivars().settings.set(settings);
            self.send(Command::SetSettings(settings));
            self.hide_settings_window();
            self.update_ui();
        }

        #[unsafe(method(closeSettings:))]
        fn close_settings(&self, _sender: &AnyObject) {
            self.hide_settings_window();
        }

        // ── контролы настроек: степперы и слайдеры обновляют поля ──
        #[unsafe(method(stepperChanged:))]
        fn stepper_changed(&self, sender: &AnyObject) {
            let Some(stepper) = sender.downcast_ref::<NSStepper>() else { return };
            let panel_ref = self.ivars().settings_panel.borrow();
            let Some(panel) = panel_ref.as_ref() else { return };
            let value = stepper.doubleValue().round();
            let field: &NSTextField = match stepper.tag() {
                1 => &panel.led_field,
                2 => &panel.top_field,
                3 => &panel.left_field,
                4 => &panel.right_field,
                _ => return,
            };
            field.setStringValue(&NSString::from_str(&format!("{value:.0}")));
        }

        #[unsafe(method(sliderChanged:))]
        fn slider_changed(&self, sender: &AnyObject) {
            let Some(slider) = sender.downcast_ref::<NSSlider>() else { return };
            let panel_ref = self.ivars().settings_panel.borrow();
            let Some(panel) = panel_ref.as_ref() else { return };
            let value = slider.doubleValue();
            let (field, text): (&NSTextField, String) = match slider.tag() {
                5 => (&panel.fps_field, format!("{:.0}", value)),
                6 => (&panel.gamma_field, format!("{value:.2}")),
                7 => (&panel.sat_field, format!("{value:.2}")),
                _ => return,
            };
            field.setStringValue(&NSString::from_str(&text));
        }

        // ── обновление приложения ──
        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: &AnyObject) {
            if self.update_busy() {
                return;
            }
            // Обновляемся только из бандла: подменять запущенный бинарник
            // из target/ бессмысленно, а helper-скрипт ждёт именно .app.
            if let Err(message) = updater::current_app() {
                show_alert("QuikLight", &message);
                return;
            }
            self.set_update_item("Проверка обновлений…", false);
            updater::spawn_check(self.ivars().updater_events.borrow().clone());
        }

        // ── выход ──
        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            self.shutdown_and_terminate();
        }
    }
);

impl AppDelegate {
    /// Конструктор: аллокация + инициализация ivars.
    pub(crate) fn new(mtm: MainThreadMarker, ivars: AppIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    /// Отправить команду в фоновый поток.
    fn send(&self, cmd: Command) {
        let _ = self.ivars().cmd.borrow().send(cmd);
    }

    // ── обновление приложения ──

    /// Титул и доступность пункта «Проверить обновление…».
    fn set_update_item(&self, title: &str, enabled: bool) {
        if let Some(item) = self.ivars().check_update_item.borrow().as_ref() {
            item.setTitle(&NSString::from_str(title));
            item.setEnabled(enabled);
        }
    }

    /// Идёт проверка или загрузка: пункт меню заблокирован.
    fn update_busy(&self) -> bool {
        self.ivars()
            .check_update_item
            .borrow()
            .as_ref()
            .map(|item| !item.isEnabled())
            .unwrap_or(false)
    }

    /// Обработать событие потока обновления.
    fn handle_update(&self, event: UpdateEvent) {
        match event {
            UpdateEvent::CheckFinished(Ok(Some(manifest))) => {
                self.set_update_item("Проверить обновление…", true);
                let message = format!(
                    "Доступна версия {} (у вас {}).\n\n\
                     Установить сейчас? Приложение перезапустится.",
                    manifest.version,
                    updater::current_version()
                );
                if confirm_alert("QuikLight", &message, "Скачать и установить", "Позже")
                {
                    self.set_update_item("Скачивание обновления…", false);
                    updater::spawn_download(manifest, self.ivars().updater_events.borrow().clone());
                }
            }
            UpdateEvent::CheckFinished(Ok(None)) => {
                self.set_update_item("Проверить обновление…", true);
                show_alert(
                    "QuikLight",
                    &format!("У вас последняя версия: {}.", updater::current_version()),
                );
            }
            UpdateEvent::CheckFinished(Err(message)) => {
                self.set_update_item("Проверить обновление…", true);
                show_alert(
                    "QuikLight",
                    &format!("Не удалось проверить обновления.\n\n{message}"),
                );
            }
            UpdateEvent::Progress { received } => {
                self.set_update_item(
                    &format!(
                        "Скачивание обновления… {:.1} МБ",
                        received as f64 / (1024.0 * 1024.0)
                    ),
                    false,
                );
            }
            UpdateEvent::Ready { version, staged } => {
                self.set_update_item("Проверить обновление…", true);
                *self.ivars().staged_update.borrow_mut() = Some(staged);
                let message = format!(
                    "Обновление {version} скачано и проверено.\n\n\
                     Перезапустить QuikLight сейчас?"
                );
                if confirm_alert("QuikLight", &message, "Перезапустить", "Позже")
                {
                    self.install_staged_update();
                }
            }
            UpdateEvent::Failed(message) => {
                self.set_update_item("Проверить обновление…", true);
                show_alert(
                    "QuikLight",
                    &format!("Не удалось подготовить обновление.\n\n{message}"),
                );
            }
        }
    }

    /// Подменить бандл скачанным и завершить приложение.
    fn install_staged_update(&self) {
        let Some(staged) = self.ivars().staged_update.borrow_mut().take() else {
            return;
        };
        match updater::install_and_relaunch(&staged) {
            Ok(()) => self.shutdown_and_terminate(),
            Err(message) => show_alert(
                "QuikLight",
                &format!("Не удалось установить обновление.\n\n{message}"),
            ),
        }
    }

    /// Погасить LED (если настроено), дождаться движка и завершить приложение.
    fn shutdown_and_terminate(&self) {
        self.send(Command::Quit);
        // Ждём ack «поток завершился»: он гасит LED (если включена опция).
        // Ограничено 2 секундами, чтобы не зависнуть при сбое захвата экрана.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            {
                let events = self.ivars().events.borrow_mut();
                if events.try_iter().any(|e| matches!(e, WorkerEvent::Exited)) {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let mtm = MainThreadMarker::new().unwrap();
        NSApplication::sharedApplication(mtm).terminate(Some(self));
    }

    /// Применить событие от потока.
    fn apply_event(&self, event: WorkerEvent) {
        match event {
            WorkerEvent::DeviceChanged { connected, info } => {
                self.ivars().device_connected.set(connected);
                *self.ivars().device_info.borrow_mut() = info;
                self.update_ui();
            }
            WorkerEvent::ModeChanged(mode) => {
                self.ivars().mode.set(mode);
                self.update_ui();
            }
            WorkerEvent::Error(message) => {
                show_alert("QuikLight", &message);
            }
            WorkerEvent::Update(event) => {
                self.handle_update(event);
            }
            WorkerEvent::Exited => {
                // Поток завершился (при выходе) — UI больше ничего не делает.
            }
        }
    }

    /// Обновить меню и статус-бар под текущее состояние.
    fn update_ui(&self) {
        let mtm = MainThreadMarker::new().unwrap();
        let power = self.ivars().power.get();

        // Иконка в menu bar: template-изображение (меняется под тему macOS),
        // при выключенной подсветке — полупрозрачная.
        if let Some(status) = self.ivars().status_item.borrow().clone() {
            if let Some(button) = status.button(mtm) {
                if let Some(icon) = status_icon() {
                    button.setImage(Some(&icon));
                    button.setTitle(&NSString::from_str(""));
                } else {
                    // Фолбэк без .app-бандла (cargo run): старый глиф.
                    let title = if power { "◉" } else { "○" };
                    button.setTitle(&NSString::from_str(title));
                }
                button.setAlphaValue(if power { 1.0 } else { 0.35 });
            }
        }

        // Переключатель подсветки.
        if let Some(sw) = self.ivars().power_switch.borrow().clone() {
            sw.setState(if power {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }

        // Радио-пункты режимов.
        let mode = self.ivars().mode.get();
        for (m, item) in self.ivars().mode_items.borrow().iter() {
            item.setState(if *m == mode {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        // Пункт «Эффекты» (родитель подменю) — радио-пункт той же группы.
        if let Some(item) = self.ivars().effects_menu_item.borrow().clone() {
            let on = matches!(mode, Mode::Effects(..) | Mode::Fireplace);
            item.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        // Радио-пункты эффектов.
        if let Mode::Effects(tipo, index) = mode {
            for (t, i, item) in self.ivars().effect_items.borrow().iter() {
                item.setState(if *t == tipo && *i == index {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
            }
        } else {
            for (_, _, item) in self.ivars().effect_items.borrow().iter() {
                item.setState(NSControlStateValueOff);
            }
        }
        // Пункт «Камин» — своё место в той же группе радио-пунктов.
        if let Some(item) = self.ivars().fireplace_item.borrow().clone() {
            item.setState(if mode == Mode::Fireplace {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }

        // Палитра: кольцо вокруг текущего пресета (только в режиме статичного цвета).
        // Картинки перерисовываем только при смене выбранного пресета, чтобы не
        // трогать пункты открытого меню без нужды.
        let cur = self.ivars().color.get();
        let ring = (power && mode == Mode::Static).then_some(cur);
        if self.ivars().preset_ring.get() != ring {
            self.ivars().preset_ring.set(ring);
            for (r, g, b, btn) in self.ivars().preset_items.borrow().iter() {
                let on = ring == Some((*r, *g, *b));
                btn.setImage(Some(&color_circle_image(*r, *g, *b, 16.0, on)));
            }
        }
    }

    /// Построить menu bar: статус-пункт + меню.
    pub(crate) fn build_ui(&self, mtm: MainThreadMarker) {
        let status_item =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        status_item.setMenu(Some(&self.build_menu(mtm)));
        *self.ivars().status_item.borrow_mut() = Some(status_item);
        self.update_ui();
    }
}
