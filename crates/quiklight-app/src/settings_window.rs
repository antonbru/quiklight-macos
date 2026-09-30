//! Окно настроек: контролы LED/раскладки/fps/гаммы/постобработки.
//!
//! Все контролы создаются один раз при первом открытии окна и затем
//! заполняются из текущих `Settings` (см. `app::open_settings`).

use objc2::rc::Retained;
use objc2::{sel, DefinedClass, MainThreadMarker, MainThreadOnly};

use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn, NSSlider,
    NSStepper, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::app::AppDelegate;
use crate::ui::{checkbox, control_action, label, slider_row, stepper_row};
/// Содержимое окна настроек (все контролы, заполняются при первом открытии).
pub(crate) struct SettingsPanel {
    pub(crate) window: Retained<NSWindow>,
    pub(crate) led_stepper: Retained<NSStepper>,
    pub(crate) led_field: Retained<NSTextField>,
    pub(crate) top_stepper: Retained<NSStepper>,
    pub(crate) top_field: Retained<NSTextField>,
    pub(crate) left_stepper: Retained<NSStepper>,
    pub(crate) left_field: Retained<NSTextField>,
    pub(crate) right_stepper: Retained<NSStepper>,
    pub(crate) right_field: Retained<NSTextField>,
    pub(crate) fps_slider: Retained<NSSlider>,
    pub(crate) fps_field: Retained<NSTextField>,
    pub(crate) gamma_slider: Retained<NSSlider>,
    pub(crate) gamma_field: Retained<NSTextField>,
    pub(crate) sat_slider: Retained<NSSlider>,
    pub(crate) sat_field: Retained<NSTextField>,
    pub(crate) smooth_check: Retained<NSButton>,
    pub(crate) lc_check: Retained<NSButton>,
    pub(crate) off_check: Retained<NSButton>,
}

impl AppDelegate {
    pub(crate) fn build_settings_panel(&self, mtm: MainThreadMarker) -> SettingsPanel {
        let content = NSView::new(mtm);
        content.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(400.0, 470.0),
        ));

        // Заголовок.
        content.addSubview(&label("Настройки QuikLight", 12.0, 440.0, 380.0, mtm));

        // Число LED (0 = авто — из устройства).
        content.addSubview(&label("Число LED (0 = авто)", 12.0, 408.0, 160.0, mtm));
        let (led_field, led_stepper) =
            stepper_row(&content, 1, 0.0, 0.0, 500.0, 160.0, 408.0, self, mtm);

        // Раскладка.
        content.addSubview(&label(
            "Раскладка (0 = авто 17/29/17)",
            12.0,
            374.0,
            300.0,
            mtm,
        ));
        let (top_field, top_stepper) =
            stepper_row(&content, 2, 29.0, 0.0, 200.0, 140.0, 340.0, self, mtm);
        let (left_field, left_stepper) =
            stepper_row(&content, 3, 17.0, 0.0, 200.0, 140.0, 308.0, self, mtm);
        let (right_field, right_stepper) =
            stepper_row(&content, 4, 17.0, 0.0, 200.0, 140.0, 276.0, self, mtm);
        // Пометки к степперам (над каждым рядом).
        for (title, y) in [("Верх:", 340.0), ("Лево:", 308.0), ("Право:", 276.0)] {
            content.addSubview(&label(title, 12.0, y + 4.0, 90.0, mtm));
        }

        // FPS / гамма / насыщенность.
        let fps_row = slider_row(
            &content,
            5,
            "FPS (screen sync)",
            20.0,
            60.0,
            30.0,
            240.0,
            self,
            mtm,
        );
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
        apply.setFrame(NSRect::new(
            NSPoint::new(210.0, 20.0),
            NSSize::new(178.0, 30.0),
        ));
        content.addSubview(&apply);

        let cancel = NSButton::new(mtm);
        cancel.setTitle(&NSString::from_str("Отмена"));
        control_action(&cancel, self, sel!(closeSettings:));
        cancel.setFrame(NSRect::new(
            NSPoint::new(12.0, 20.0),
            NSSize::new(178.0, 30.0),
        ));
        content.addSubview(&cancel);

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 470.0)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable,
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
    pub(crate) fn hide_settings_window(&self) {
        if let Some(panel) = self.ivars().settings_panel.borrow().as_ref() {
            panel.window.orderOut(None);
        }
    }

    /// Заполнить контролы текущими настройками (при открытии окна).
    pub(crate) fn refresh_settings_panel(&self, panel: &SettingsPanel) {
        let s = self.ivars().settings.get();
        panel.led_stepper.setDoubleValue(f64::from(s.led_count));
        panel.top_stepper.setDoubleValue(f64::from(s.layout.top));
        panel.left_stepper.setDoubleValue(f64::from(s.layout.left));
        panel
            .right_stepper
            .setDoubleValue(f64::from(s.layout.right));
        panel.fps_slider.setDoubleValue(f64::from(s.fps));
        panel.gamma_slider.setDoubleValue(f64::from(s.gamma));
        panel.sat_slider.setDoubleValue(f64::from(s.saturation));
        panel.smooth_check.setState(if s.smoothing {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        panel.lc_check.setState(if s.light_compression {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        panel.off_check.setState(if s.turn_off_on_exit {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        // Поля значений.
        panel
            .led_field
            .setStringValue(&NSString::from_str(&format!("{}", s.led_count)));
        panel
            .top_field
            .setStringValue(&NSString::from_str(&format!("{}", s.layout.top)));
        panel
            .left_field
            .setStringValue(&NSString::from_str(&format!("{}", s.layout.left)));
        panel
            .right_field
            .setStringValue(&NSString::from_str(&format!("{}", s.layout.right)));
        panel
            .fps_field
            .setStringValue(&NSString::from_str(&format!("{}", s.fps)));
        panel
            .gamma_field
            .setStringValue(&NSString::from_str(&format!("{:.2}", s.gamma)));
        panel
            .sat_field
            .setStringValue(&NSString::from_str(&format!("{:.2}", s.saturation)));
    }
}
