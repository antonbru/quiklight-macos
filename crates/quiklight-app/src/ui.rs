//! UI-хелперы AppKit: виджеты окна настроек, иконка меню-бара, статус, алерты.
//!
//! Свободные функции. Класс делегата и его состояние — в `app.rs`.

use objc2::rc::Retained;
use objc2::runtime::{Bool, Sel};
use objc2::{sel, MainThreadMarker};

use block2::RcBlock;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSBezierPath, NSButton, NSColor, NSControl,
    NSControlStateValueOn, NSImage, NSSlider, NSStepper, NSTextField, NSView,
};
use objc2_core_foundation::CGFloat;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::app::AppDelegate;
/// Template-иконка menu bar (18 pt, набор `menubar18@1x/@2x/@3x` из
/// Resources бандла). `imageNamed:` сам подбирает ретину-репрезентации;
/// template = macOS перекрашивает под светлую/тёмную тему.
pub(crate) fn status_icon() -> Option<Retained<NSImage>> {
    let image = NSImage::imageNamed(&NSString::from_str("menubar18"))?;
    image.setTemplate(true);
    image.setSize(NSSize::new(18.0, 18.0));
    Some(image)
}

/// Иконка-кружок цвета; `selected` рисует вокруг кружка контрастное кольцо.
pub(crate) fn color_circle_image(
    r: u8,
    g: u8,
    b: u8,
    size: f64,
    selected: bool,
) -> Retained<NSImage> {
    // Рисуем кружок через block-based API (lockFocus устарел).
    let drawing = RcBlock::new(move |rect: NSRect| -> Bool {
        let inset = NSRect::new(
            NSPoint::new(rect.origin.x + 1.5, rect.origin.y + 1.5),
            NSSize::new(rect.size.width - 3.0, rect.size.height - 3.0),
        );
        let path = NSBezierPath::bezierPathWithOvalInRect(inset);
        NSColor::colorWithSRGBRed_green_blue_alpha(
            r as CGFloat / 255.0,
            g as CGFloat / 255.0,
            b as CGFloat / 255.0,
            1.0,
        )
        .set();
        path.fill();
        if selected {
            // Кольцо вокруг выбранного кружка: тёмное на светлых цветах, светлое на тёмных —
            // видно и в светлой, и в тёмной теме меню.
            let luminance = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
            let ring = if luminance > 140.0 { 0.25 } else { 1.0 };
            NSColor::colorWithSRGBRed_green_blue_alpha(ring, ring, ring, 1.0).setStroke();
            path.setLineWidth(2.0);
            path.stroke();
        }
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(size, size), true, &drawing)
}

/// Повесить действие на контрол (слайдер/степпер/кнопку).
pub(crate) fn control_action(control: &NSControl, target: &AppDelegate, action: Sel) {
    unsafe {
        control.setTarget(Some(target));
        control.setAction(Some(action));
    }
}

pub(crate) fn label(
    text: &str,
    x: f64,
    y: f64,
    w: f64,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, 18.0)));
    l
}

pub(crate) fn value_field(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let f = NSTextField::new(mtm);
    f.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(50.0, 22.0)));
    f.setEditable(false);
    f.setBezeled(false);
    f.setStringValue(&NSString::from_str("0"));
    f
}

/// Строка «метка + степпер + поле значения»; степпер по тегу обновляет поле.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stepper_row(
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
    field.setFrame(NSRect::new(
        NSPoint::new(x + 80.0, y),
        NSSize::new(50.0, 22.0),
    ));
    field.setStringValue(&NSString::from_str(&format!("{value:.0}")));
    content.addSubview(&field);

    let stepper = NSStepper::new(mtm);
    stepper.setFrame(NSRect::new(
        NSPoint::new(x + 135.0, y - 2.0),
        NSSize::new(19.0, 27.0),
    ));
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn slider_row(
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
    slider.setFrame(NSRect::new(
        NSPoint::new(130.0, y + 2.0),
        NSSize::new(180.0, 24.0),
    ));
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

pub(crate) fn checkbox(
    content: &NSView,
    title: &str,
    y: f64,
    _delegate: &AppDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let cb = unsafe {
        NSButton::checkboxWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
    };
    cb.setFrame(NSRect::new(NSPoint::new(12.0, y), NSSize::new(300.0, 20.0)));
    cb.setState(NSControlStateValueOn);
    content.addSubview(&cb);
    cb
}

/// Модальный алерт с сообщением.
pub(crate) fn show_alert(title: &str, message: &str) {
    let mtm = MainThreadMarker::new().unwrap();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str("OK"));
    alert.runModal();
}

/// Алерт с двумя кнопками: `true` — нажата первая (действие), `false` — вторая.
pub(crate) fn confirm_alert(title: &str, message: &str, accept: &str, cancel: &str) -> bool {
    let mtm = MainThreadMarker::new().unwrap();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(&NSString::from_str(accept));
    alert.addButtonWithTitle(&NSString::from_str(cancel));
    alert.runModal() == NSAlertFirstButtonReturn
}
