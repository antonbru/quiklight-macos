//! QuikLight — menu bar приложение для LED-панели Robobloq QuikLight.
//!
//! AppKit через objc2 (без Xcode-проекта): `NSStatusItem` в menu bar,
//! `NSMenu` с режимами, слайдер яркости, палитра цвета, окно настроек.
//! Вся работа с устройством — в фоновом потоке (`engine`).

#![allow(clippy::let_unit_value)]

mod engine;

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{self, Receiver, Sender};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSColor,
    NSColorPanel, NSColorSpace, NSControl, NSControlStateValueOff, NSControlStateValueOn, NSMenu,
    NSMenuItem, NSSlider, NSStepper, NSStatusBar, NSStatusItem, NSTextField, NSVariableStatusItemLength,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::CGFloat;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer};

use crate::engine::{Command, Mode, Settings, WorkerEvent};

// ───────────────────────────── Состояние UI ─────────────────────────────

/// Содержимое окна настроек (все контролы, заполняются при первом открытии).
struct SettingsPanel {
    window: Retained<NSWindow>,
    led_stepper: Retained<NSStepper>,
    led_field: Retained<NSTextField>,
    top_stepper: Retained<NSStepper>,
    top_field: Retained<NSTextField>,
    left_stepper: Retained<NSStepper>,
    left_field: Retained<NSTextField>,
    right_stepper: Retained<NSStepper>,
    right_field: Retained<NSTextField>,
    fps_slider: Retained<NSSlider>,
    fps_field: Retained<NSTextField>,
    gamma_slider: Retained<NSSlider>,
    gamma_field: Retained<NSTextField>,
    sat_slider: Retained<NSSlider>,
    sat_field: Retained<NSTextField>,
    smooth_check: Retained<NSButton>,
    lc_check: Retained<NSButton>,
    off_check: Retained<NSButton>,
}

/// Ивар-хранилище делегата приложения.
struct AppIvars {
    /// Канал команд в фоновый поток.
    cmd: RefCell<Sender<Command>>,
    /// Канал событий из фонового потока (опрашивается таймером).
    events: RefCell<Receiver<WorkerEvent>>,

    // Текущее состояние UI.
    power: Cell<bool>,
    mode: Cell<Mode>,
    brightness: Cell<u8>,
    color: Cell<(u8, u8, u8)>,
    settings: Cell<Settings>,
    device_connected: Cell<bool>,
    device_info: RefCell<Option<quiklight_core::hid::device_info::DeviceInfo>>,

    // Хэндлы UI (заполняются в `build_ui`).
    status_item: RefCell<Option<Retained<NSStatusItem>>>,
    status_header: RefCell<Option<Retained<NSMenuItem>>>,
    power_item: RefCell<Option<Retained<NSMenuItem>>>,
    mode_items: RefCell<Vec<(Mode, Retained<NSMenuItem>)>>,
    effect_items: RefCell<Vec<(u8, u8, Retained<NSMenuItem>)>>,
    brightness_slider: RefCell<Option<Retained<NSSlider>>>,
    settings_panel: RefCell<Option<SettingsPanel>>,
    color_panel: RefCell<Option<Retained<NSColorPanel>>>,
}

/// Тэги пунктов меню «Режим».
const TAG_MODE_OFF: isize = 0;
const TAG_MODE_STATIC: isize = 1;
const TAG_MODE_SYNC: isize = 2;

// ───────────────────────────── Делегат ─────────────────────────────

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "QuikLightAppDelegate"]
    #[ivars = AppIvars]
    struct AppDelegate;

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
                TAG_MODE_OFF => Mode::Off,
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
            // tag = tipo*100 + index (200..206 — динамика, 300..306 — звук).
            let (tipo, index) = ((tag / 100) as u8, (tag % 100) as u8);
            self.ivars().mode.set(Mode::Effects(tipo as u8, index as u8));
            self.ivars().power.set(true);
            self.send(Command::SetPower(true));
            self.send(Command::SetMode(self.ivars().mode.get()));
            self.update_ui();
        }

        #[unsafe(method(changeColor:))]
        fn change_color(&self, _sender: &AnyObject) {
            let mtm = MainThreadMarker::new().unwrap();
            let color = NSColorPanel::sharedColorPanel(mtm).color();
            let rgb = ns_color_to_rgb(&color);
            self.ivars().color.set(rgb);
            self.ivars().mode.set(Mode::Static);
            self.ivars().power.set(true);
            self.send(Command::SetPower(true));
            self.send(Command::SetMode(Mode::Static));
            self.send(Command::SetColor { r: rgb.0, g: rgb.1, b: rgb.2 });
            self.update_ui();
        }

        #[unsafe(method(openColorPanel:))]
        fn open_color_panel(&self, _sender: &AnyObject) {
            let mtm = MainThreadMarker::new().unwrap();
            let panel = self.ivars().color_panel.borrow().clone();
            let panel = panel.unwrap_or_else(|| {
                let p = NSColorPanel::sharedColorPanel(mtm);
                p.setContinuous(true);
                unsafe {
                    p.setTarget(Some(self));
                    p.setAction(Some(sel!(changeColor:)));
                }
                p
            });
            let (r, g, b) = self.ivars().color.get();
            panel.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
                r as CGFloat / 255.0, g as CGFloat / 255.0, b as CGFloat / 255.0, 1.0,
            ));
            panel.orderFront(Some(self));
            NSApplication::sharedApplication(mtm).activate();
            *self.ivars().color_panel.borrow_mut() = Some(panel);
        }

        #[unsafe(method(brightnessChanged:))]
        fn brightness_changed(&self, sender: &AnyObject) {
            let Some(slider) = sender.downcast_ref::<NSSlider>() else { return };
            let v = slider.doubleValue().round().clamp(0.0, 100.0) as u8;
            self.ivars().brightness.set(v);
            self.send(Command::SetBrightness(v));
            self.update_ui();
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

            let led = panel.led_stepper.doubleValue().round().clamp(1.0, 500.0) as u16;
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

        // ── выход ──
        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            self.send(Command::Quit);
            let mtm = MainThreadMarker::new().unwrap();
            NSApplication::sharedApplication(mtm).terminate(Some(self));
        }
    }
);

impl AppDelegate {
    /// Конструктор: аллокация + инициализация ivars.
    fn new(mtm: MainThreadMarker, ivars: AppIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    /// Отправить команду в фоновый поток.
    fn send(&self, cmd: Command) {
        let _ = self.ivars().cmd.borrow().send(cmd);
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
        }
    }

    /// Обновить меню и статус-бар под текущее состояние.
    fn update_ui(&self) {
        let mtm = MainThreadMarker::new().unwrap();
        let power = self.ivars().power.get();

        // Заголовок в menu bar: ● / ○.
        if let Some(status) = self.ivars().status_item.borrow().clone() {
            if let Some(button) = status.button(mtm) {
                let title = if power { "◉" } else { "○" };
                button.setTitle(&NSString::from_str(title));
            }
        }

        // Шапка меню: статус устройства.
        if let Some(header) = self.ivars().status_header.borrow().clone() {
            let text = status_line(
                self.ivars().device_connected.get(),
                self.ivars().device_info.borrow().as_ref(),
                power,
            );
            header.setTitle(&NSString::from_str(&text));
        }

        // Пункт «Подсветка».
        if let Some(item) = self.ivars().power_item.borrow().clone() {
            let title = if power { "Подсветка: Вкл" } else { "Подсветка: Выкл" };
            item.setTitle(&NSString::from_str(title));
            item.setState(if power { NSControlStateValueOn } else { NSControlStateValueOff });
        }

        // Радио-пункты режимов.
        let mode = self.ivars().mode.get();
        for (m, item) in self.ivars().mode_items.borrow().iter() {
            item.setState(if *m == mode { NSControlStateValueOn } else { NSControlStateValueOff });
        }
        // Радио-пункты эффектов.
        if let Mode::Effects(tipo, index) = mode {
            for (t, i, item) in self.ivars().effect_items.borrow().iter() {
                item.setState(if *t == tipo && *i == index { NSControlStateValueOn } else { NSControlStateValueOff });
            }
        } else {
            for (_, _, item) in self.ivars().effect_items.borrow().iter() {
                item.setState(NSControlStateValueOff);
            }
        }

        // Слайдер яркости.
        if let Some(slider) = self.ivars().brightness_slider.borrow().clone() {
            slider.setDoubleValue(f64::from(self.ivars().brightness.get()));
        }
    }

    /// Построить menu bar: статус-пункт + меню.
    fn build_ui(&self, mtm: MainThreadMarker) {
        let status_item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        status_item.setMenu(Some(&self.build_menu(mtm)));
        *self.ivars().status_item.borrow_mut() = Some(status_item);
        self.update_ui();
    }

    fn build_menu(&self, mtm: MainThreadMarker) -> Retained<NSMenu> {
        let menu = NSMenu::new(mtm);

        // Заголовок-статус.
        let header = NSMenuItem::new(mtm);
        header.setEnabled(false);
        menu.addItem(&header);
        *self.ivars().status_header.borrow_mut() = Some(header);

        // Подсветка вкл/выкл.
        let power = action_item("Подсветка: Выкл", 0, sel!(togglePower:), self, mtm);
        menu.addItem(&power);
        *self.ivars().power_item.borrow_mut() = Some(power);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Режимы.
        let mut mode_items = Vec::new();
        mode_items.push((Mode::Off, action_item("Выключена", TAG_MODE_OFF, sel!(setMode:), self, mtm)));
        mode_items.push((Mode::Static, action_item("Статичный цвет", TAG_MODE_STATIC, sel!(setMode:), self, mtm)));
        mode_items.push((Mode::Sync, action_item("Screen sync", TAG_MODE_SYNC, sel!(setMode:), self, mtm)));
        for (_, item) in &mode_items {
            menu.addItem(item);
        }

        // Эффекты — подменю.
        let effects_item = NSMenuItem::new(mtm);
        effects_item.setTitle(&NSString::from_str("Эффекты"));
        let effects_menu = NSMenu::new(mtm);
        let mut effect_items = Vec::new();
        let effect_names = [
            "Эффект 0", "Эффект 1", "Эффект 2", "Эффект 3", "Эффект 4", "Эффект 5", "Эффект 6",
        ];
        for (tipo, label) in [(2u8, "Динамика"), (3u8, "Звук")] {
            if tipo == 3 {
                effects_menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            for (idx, name) in effect_names.iter().enumerate() {
                let tag = isize::from(tipo) * 100 + idx as isize;
                let item = action_item(&format!("{label} — {name}"), tag, sel!(setEffect:), self, mtm);
                effects_menu.addItem(&item);
                effect_items.push((tipo, idx as u8, item));
            }
        }
        effects_item.setSubmenu(Some(&effects_menu));
        menu.addItem(&effects_item);
        *self.ivars().effect_items.borrow_mut() = effect_items;
        *self.ivars().mode_items.borrow_mut() = mode_items;

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Цвет.
        let color_item = action_item("Цвет…", 0, sel!(openColorPanel:), self, mtm);
        menu.addItem(&color_item);

        // Яркость — слайдер внутри пункта меню.
        let brightness_item = NSMenuItem::new(mtm);
        let slider = NSSlider::new(mtm);
        slider.setFrame(NSRect::new(NSPoint::new(10.0, 6.0), NSSize::new(160.0, 24.0)));
        slider.setMinValue(0.0);
        slider.setMaxValue(100.0);
        slider.setDoubleValue(f64::from(self.ivars().brightness.get()));
        slider.setContinuous(true);
        control_action(&slider, self, sel!(brightnessChanged:));
        brightness_item.setView(Some(&slider));
        *self.ivars().brightness_slider.borrow_mut() = Some(slider);
        menu.addItem(&brightness_item);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Настройки и выход.
        let settings_item = action_item("Настройки…", 0, sel!(openSettings:), self, mtm);
        menu.addItem(&settings_item);

        let quit_item = action_item("Выйти", 0, sel!(quit:), self, mtm);
        quit_item.setKeyEquivalent(&NSString::from_str("q"));
        menu.addItem(&quit_item);

        menu
    }

    // ── окно настроек ──

    fn build_settings_panel(&self, mtm: MainThreadMarker) -> SettingsPanel {
        let content = NSView::new(mtm);
        content.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 470.0)));

        // Заголовок.
        content.addSubview(&label("Настройки QuikLight", 12.0, 440.0, 380.0, mtm));

        // Число LED.
        content.addSubview(&label("Число LED", 12.0, 408.0, 120.0, mtm));
        let (led_field, led_stepper) = stepper_row(&content, 1, 63.0, 1.0, 500.0, 140.0, 408.0, self, mtm);

        // Раскладка.
        content.addSubview(&label("Раскладка (0 = авто 17/29/17)", 12.0, 374.0, 300.0, mtm));
        let (top_field, top_stepper) = stepper_row(&content, 2, 29.0, 0.0, 200.0, 140.0, 340.0, self, mtm);
        let (left_field, left_stepper) = stepper_row(&content, 3, 17.0, 0.0, 200.0, 140.0, 308.0, self, mtm);
        let (right_field, right_stepper) = stepper_row(&content, 4, 17.0, 0.0, 200.0, 140.0, 276.0, self, mtm);
        // Пометки к степперам (над каждым рядом).
        for (title, y) in [("Верх:", 340.0), ("Лево:", 308.0), ("Право:", 276.0)] {
            content.addSubview(&label(title, 12.0, y + 4.0, 90.0, mtm));
        }

        // FPS / гамма / насыщенность.
        let fps_row = slider_row(&content, 5, "FPS (screen sync)", 20.0, 60.0, 30.0, 240.0, self, mtm);
        let gamma_row = slider_row(&content, 6, "Гамма", 0.5, 2.5, 1.0, 206.0, self, mtm);
        let sat_row = slider_row(&content, 7, "Насыщенность", 0.0, 2.0, 1.0, 172.0, self, mtm);

        // Чекбоксы.
        let smooth_check = checkbox(&content, "Сглаживание кадров", 132.0, self, mtm);
        let lc_check = checkbox(&content, "Light compression", 102.0, self, mtm);
        let off_check = checkbox(&content, "Выключать LED при выходе", 72.0, self, mtm);

        // Кнопки.
        let apply = NSButton::new(mtm);
        apply.setTitle(&NSString::from_str("Применить и закрыть"));
        control_action(&apply, self, sel!(applySettings:));
        apply.setFrame(NSRect::new(NSPoint::new(210.0, 20.0), NSSize::new(178.0, 30.0)));
        content.addSubview(&apply);

        let cancel = NSButton::new(mtm);
        cancel.setTitle(&NSString::from_str("Отмена"));
        control_action(&cancel, self, sel!(closeSettings:));
        cancel.setFrame(NSRect::new(NSPoint::new(12.0, 20.0), NSSize::new(178.0, 30.0)));
        content.addSubview(&cancel);

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 470.0)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("Настройки QuikLight"));
        window.setContentView(Some(&content));

        SettingsPanel {
            window,
            led_stepper,
            led_field,
            top_stepper,
            top_field,
            left_stepper,
            left_field,
            right_stepper,
            right_field,
            fps_slider: fps_row.0,
            fps_field: fps_row.1,
            gamma_slider: gamma_row.0,
            gamma_field: gamma_row.1,
            sat_slider: sat_row.0,
            sat_field: sat_row.1,
            smooth_check,
            lc_check,
            off_check,
        }
    }

    /// Скрыть окно настроек.
    fn hide_settings_window(&self) {
        if let Some(panel) = self.ivars().settings_panel.borrow().as_ref() {
            panel.window.orderOut(None);
        }
    }

    /// Заполнить контролы текущими настройками (при открытии окна).
    fn refresh_settings_panel(&self, panel: &SettingsPanel) {        let s = self.ivars().settings.get();
        panel.led_stepper.setDoubleValue(f64::from(s.led_count));
        panel.top_stepper.setDoubleValue(f64::from(s.layout.top));
        panel.left_stepper.setDoubleValue(f64::from(s.layout.left));
        panel.right_stepper.setDoubleValue(f64::from(s.layout.right));
        panel.fps_slider.setDoubleValue(f64::from(s.fps));
        panel.gamma_slider.setDoubleValue(f64::from(s.gamma));
        panel.sat_slider.setDoubleValue(f64::from(s.saturation));
        panel.smooth_check.setState(if s.smoothing { NSControlStateValueOn } else { NSControlStateValueOff });
        panel.lc_check.setState(if s.light_compression { NSControlStateValueOn } else { NSControlStateValueOff });
        panel.off_check.setState(if s.turn_off_on_exit { NSControlStateValueOn } else { NSControlStateValueOff });
        // Поля значений.
        panel.led_field.setStringValue(&NSString::from_str(&format!("{}", s.led_count)));
        panel.top_field.setStringValue(&NSString::from_str(&format!("{}", s.layout.top)));
        panel.left_field.setStringValue(&NSString::from_str(&format!("{}", s.layout.left)));
        panel.right_field.setStringValue(&NSString::from_str(&format!("{}", s.layout.right)));
        panel.fps_field.setStringValue(&NSString::from_str(&format!("{}", s.fps)));
        panel.gamma_field.setStringValue(&NSString::from_str(&format!("{:.2}", s.gamma)));
        panel.sat_field.setStringValue(&NSString::from_str(&format!("{:.2}", s.saturation)));
    }
}

// ───────────────────────────── Хелперы UI ─────────────────────────────

/// Пункт меню с действием (title + tag + action, target — делегат).
fn action_item(
    title: &str,
    tag: isize,
    action: Sel,
    target: &AppDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    item.setTag(tag);
    unsafe {
        item.setTarget(Some(target));
        item.setAction(Some(action));
    }
    item
}

/// Повесить действие на контрол (слайдер/степпер/кнопку).
fn control_action(control: &NSControl, target: &AppDelegate, action: Sel) {
    unsafe {
        control.setTarget(Some(target));
        control.setAction(Some(action));
    }
}

fn label(text: &str, x: f64, y: f64, w: f64, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, 18.0)));
    l
}

fn value_field(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let f = NSTextField::new(mtm);
    f.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(50.0, 22.0)));
    f.setEditable(false);
    f.setBezeled(false);
    f.setStringValue(&NSString::from_str("0"));
    f
}

/// Строка «метка + степпер + поле значения»; степпер по тегу обновляет поле.
fn stepper_row(
    content: &NSView,
    tag: isize,
    value: f64,
    min: f64,
    max: f64,
    x: f64,
    y: f64,
    delegate: &AppDelegate,
    mtm: MainThreadMarker,
) -> (Retained<NSTextField>, Retained<NSStepper>) {
    let field = value_field(mtm);
    field.setFrame(NSRect::new(NSPoint::new(x + 80.0, y), NSSize::new(50.0, 22.0)));
    field.setStringValue(&NSString::from_str(&format!("{value:.0}")));
    content.addSubview(&field);

    let stepper = NSStepper::new(mtm);
    stepper.setFrame(NSRect::new(NSPoint::new(x + 135.0, y - 2.0), NSSize::new(19.0, 27.0)));
    stepper.setMinValue(min);
    stepper.setMaxValue(max);
    stepper.setIncrement(1.0);
    stepper.setValueWraps(false);
    stepper.setDoubleValue(value);
    stepper.setTag(tag);
    control_action(&stepper, delegate, sel!(stepperChanged:));
    content.addSubview(&stepper);
    (field, stepper)
}

/// Строка «метка + слайдер + поле значения»; слайдер по тегу обновляет поле.
fn slider_row(
    content: &NSView,
    tag: isize,
    title: &str,
    min: f64,
    max: f64,
    value: f64,
    y: f64,
    delegate: &AppDelegate,
    mtm: MainThreadMarker,
) -> (Retained<NSSlider>, Retained<NSTextField>) {
    let l = label(title, 12.0, y, 110.0, mtm);
    content.addSubview(&l);

    let slider = NSSlider::new(mtm);
    slider.setFrame(NSRect::new(NSPoint::new(130.0, y + 2.0), NSSize::new(180.0, 24.0)));
    slider.setMinValue(min);
    slider.setMaxValue(max);
    slider.setDoubleValue(value);
    slider.setContinuous(true);
    slider.setTag(tag);
    control_action(&slider, delegate, sel!(sliderChanged:));
    content.addSubview(&slider);

    let field = value_field(mtm);
    field.setFrame(NSRect::new(NSPoint::new(320.0, y), NSSize::new(60.0, 22.0)));
    field.setStringValue(&NSString::from_str(&format!("{value:.0}")));
    content.addSubview(&field);
    (slider, field)
}

fn checkbox(
    content: &NSView,
    title: &str,
    y: f64,
    _delegate: &AppDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let cb = unsafe { NSButton::checkboxWithTitle_target_action(
        &NSString::from_str(title),
        None,
        None,
        mtm,
    ) };
    cb.setFrame(NSRect::new(NSPoint::new(12.0, y), NSSize::new(300.0, 20.0)));
    cb.setState(NSControlStateValueOn);
    content.addSubview(&cb);
    cb
}

/// Строка статуса для шапки меню.
fn status_line(connected: bool, info: Option<&quiklight_core::hid::device_info::DeviceInfo>, power: bool) -> String {
    let state = if power { "вкл" } else { "выкл" };
    match info {
        Some(i) => format!(
            "QuikLight — {} · LED {} · прошивка {}",
            state,
            i.led_count,
            i.firmware_string()
        ),
        None if connected => format!("QuikLight — {state} · панель подключена"),
        None => format!("QuikLight — {state} · панель не найдена"),
    }
}

/// NSColor → (r, g, b) 0..255.
fn ns_color_to_rgb(color: &NSColor) -> (u8, u8, u8) {
    let rgb = color.colorUsingColorSpace(&NSColorSpace::deviceRGBColorSpace());
    let c = rgb.as_deref().unwrap_or(color);
    let mut r: CGFloat = 0.0;
    let mut g: CGFloat = 0.0;
    let mut b: CGFloat = 0.0;
    unsafe {
        c.getRed_green_blue_alpha(&mut r, &mut g, &mut b, std::ptr::null_mut());
    }
    (
        (r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (b.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Модальный алерт с сообщением.
fn show_alert(title: &str, message: &str) {
    let mtm = MainThreadMarker::new().unwrap();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    alert.runModal();
}

// ───────────────────────────── main ─────────────────────────────

fn main() {
    let mtm = MainThreadMarker::new().expect("QuikLight должен запускаться на главном потоке");

    let app = NSApplication::sharedApplication(mtm);
    // Menu bar app: без иконки в Dock.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // Каналы UI ↔ фоновый поток.
    let (cmd_tx, cmd_rx): (Sender<Command>, _) = mpsc::channel();
    let (evt_tx, evt_rx): (Sender<WorkerEvent>, _) = mpsc::channel();
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
            power_item: RefCell::new(None),
            mode_items: RefCell::new(Vec::new()),
            effect_items: RefCell::new(Vec::new()),
            brightness_slider: RefCell::new(None),
            settings_panel: RefCell::new(None),
            color_panel: RefCell::new(None),
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
