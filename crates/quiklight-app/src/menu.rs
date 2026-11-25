//! Меню menu bar: режимы, палитра пресетов, яркость, настройки, выход.

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{sel, DefinedClass, MainThreadMarker};

use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSButton, NSButtonType, NSCellImagePosition, NSControlStateValueOff,
    NSImageScaling, NSMenu, NSMenuItem, NSSlider, NSSwitch, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::app::{AppDelegate, PresetButton};
use crate::engine::Mode;
use crate::ui::{color_circle_image, control_action, label};
/// Тэги пунктов меню «Режимы».
pub(crate) const TAG_MODE_STATIC: isize = 1;
pub(crate) const TAG_MODE_SYNC: isize = 2;

/// Тэг пункта «Камин» в подменю эффектов (свой программный эффект, не с панели).
pub(crate) const TAG_EFFECT_FIREPLACE: isize = 900;

/// Готовые оттенки для статичного цвета (имя — подсказка, R, G, B).
/// В меню показываются одной строкой цветных кружков без названий.
pub(crate) const PRESET_COLORS: &[(&str, u8, u8, u8)] = &[
    ("Белый", 255, 255, 255),
    ("Тёплый белый", 255, 235, 205),
    ("Красный", 255, 50, 50),
    ("Оранжевый", 255, 80, 0),
    ("Жёлтый", 255, 220, 40),
    ("Зелёный", 60, 255, 60),
    ("Бирюзовый", 0, 230, 200),
    ("Голубой", 60, 200, 255),
    ("Синий", 60, 110, 255),
    ("Фиолетовый", 170, 60, 255),
    ("Розовый", 255, 90, 180),
    ("Пурпурный", 255, 60, 255),
];

impl AppDelegate {
    pub(crate) fn build_menu(&self, mtm: MainThreadMarker) -> Retained<NSMenu> {
        let menu = NSMenu::new(mtm);

        // Подсветка — переключатель (не пункт меню).
        let switch_item = NSMenuItem::new(mtm);
        let switch_view = NSView::new(mtm);
        switch_view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(136.0, 24.0),
        ));
        switch_view.addSubview(&label("Подсветка", 8.0, 3.0, 80.0, mtm));
        let sw = NSSwitch::new(mtm);
        sw.setFrame(NSRect::new(
            NSPoint::new(94.0, 1.0),
            NSSize::new(38.0, 22.0),
        ));
        sw.setState(NSControlStateValueOff);
        control_action(&sw, self, sel!(togglePower:));
        switch_view.addSubview(&sw);
        switch_item.setView(Some(&switch_view));
        menu.addItem(&switch_item);
        *self.ivars().power_switch.borrow_mut() = Some(sw);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Режимы: заголовок + радио-пункты (выбрать можно только один).
        let modes_header = NSMenuItem::new(mtm);
        modes_header.setTitle(&NSString::from_str("Режимы"));
        modes_header.setEnabled(false);
        menu.addItem(&modes_header);

        let mut mode_items = Vec::new();

        let static_item = action_item("Статичный цвет", TAG_MODE_STATIC, sel!(setMode:), self, mtm);
        menu.addItem(&static_item);
        mode_items.push((Mode::Static, static_item));

        // Кружки цветов — сразу под режимом «Статичный цвет».
        let (palette_item, preset_items) = preset_palette_row(mtm, self);
        palette_item.setIndentationLevel(1);
        menu.addItem(&palette_item);
        *self.ivars().preset_items.borrow_mut() = preset_items;

        let sync_item = action_item("Screen sync", TAG_MODE_SYNC, sel!(setMode:), self, mtm);
        menu.addItem(&sync_item);
        mode_items.push((Mode::Sync, sync_item));

        // Эффекты — подменю; сам пункт — радио-пункт той же группы «Режимы».
        let effects_item = NSMenuItem::new(mtm);
        effects_item.setTitle(&NSString::from_str("Эффекты"));
        let effects_menu = NSMenu::new(mtm);
        let mut effect_items = Vec::new();
        let effect_names = [
            "Эффект 0",
            "Эффект 1",
            "Эффект 2",
            "Эффект 3",
            "Эффект 4",
            "Эффект 5",
            "Эффект 6",
        ];
        for (tipo, label) in [(2u8, "Динамика"), (3u8, "Звук")] {
            if tipo == 3 {
                effects_menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            for (idx, name) in effect_names.iter().enumerate() {
                let tag = isize::from(tipo) * 100 + idx as isize;
                let item = action_item(
                    &format!("{label} — {name}"),
                    tag,
                    sel!(setEffect:),
                    self,
                    mtm,
                );
                effects_menu.addItem(&item);
                effect_items.push((tipo, idx as u8, item));
            }
        }

        // Свой эффект «Камин» — плавно мерцающее пламя (рисует сам движок).
        effects_menu.addItem(&NSMenuItem::separatorItem(mtm));
        let fireplace_item =
            action_item("Камин", TAG_EFFECT_FIREPLACE, sel!(setEffect:), self, mtm);
        effects_menu.addItem(&fireplace_item);
        *self.ivars().fireplace_item.borrow_mut() = Some(fireplace_item);

        effects_item.setSubmenu(Some(&effects_menu));
        menu.addItem(&effects_item);
        *self.ivars().effects_menu_item.borrow_mut() = Some(effects_item);
        *self.ivars().effect_items.borrow_mut() = effect_items;
        *self.ivars().mode_items.borrow_mut() = mode_items;

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Яркость — слайдер внутри пункта меню во всю ширину.
        // Слайдер должен быть самим view пункта: в контейнере меню перехватывает
        // перетаскивание, и ползунок не двигается.
        const SLIDER_WIDTH: f64 = 300.0;
        let brightness_item = NSMenuItem::new(mtm);
        let slider = NSSlider::new(mtm);
        slider.setFrame(NSRect::new(
            NSPoint::new(0.0, 2.0),
            NSSize::new(SLIDER_WIDTH, 24.0),
        ));
        // Если AppKit растянет view пункта до ширины меню — тянемся вместе с ним.
        slider.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        slider.setMinValue(0.0);
        slider.setMaxValue(100.0);
        slider.setDoubleValue(f64::from(self.ivars().brightness.get()));
        slider.setContinuous(true);
        control_action(&slider, self, sel!(brightnessChanged:));
        brightness_item.setView(Some(&slider));
        menu.addItem(&brightness_item);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Настройки, обновление и выход.
        let settings_item = action_item("Настройки…", 0, sel!(openSettings:), self, mtm);
        menu.addItem(&settings_item);

        let update_item = action_item(
            "Проверить обновление…",
            0,
            sel!(checkForUpdates:),
            self,
            mtm,
        );
        menu.addItem(&update_item);
        *self.ivars().check_update_item.borrow_mut() = Some(update_item);

        let quit_item = action_item("Выйти", 0, sel!(quit:), self, mtm);
        quit_item.setKeyEquivalent(&NSString::from_str("q"));
        menu.addItem(&quit_item);

        menu
    }
}

/// Пункт меню с действием (title + tag + action, target — делегат).
pub(crate) fn action_item(
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

/// Пункт меню «палитра»: одна строка цветных кружков без названий.
/// Возвращает пункт меню и кнопки (R, G, B, кнопка) для подсветки выбора.
pub(crate) fn preset_palette_row(
    mtm: MainThreadMarker,
    target: &AppDelegate,
) -> (Retained<NSMenuItem>, Vec<PresetButton>) {
    const CIRCLE: f64 = 18.0; // сторона кнопки-кружка
    const GAP: f64 = 4.0; // промежуток между кружками
    const PAD: f64 = 6.0; // отступ слева/справа

    let n = PRESET_COLORS.len() as f64;
    let width = PAD * 2.0 + n * CIRCLE + (n - 1.0) * GAP;
    let height = CIRCLE + 6.0;

    let view = NSView::new(mtm);
    view.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(width, height),
    ));

    let mut buttons = Vec::new();
    for (i, (name, r, g, b)) in PRESET_COLORS.iter().enumerate() {
        let x = PAD + i as f64 * (CIRCLE + GAP);
        let btn = NSButton::new(mtm);
        btn.setFrame(NSRect::new(
            NSPoint::new(x, 3.0),
            NSSize::new(CIRCLE, CIRCLE),
        ));
        btn.setButtonType(NSButtonType::MomentaryChange);
        btn.setBordered(false);
        btn.setImagePosition(NSCellImagePosition::ImageOnly);
        btn.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        btn.setImage(Some(&color_circle_image(*r, *g, *b, 16.0, false)));
        btn.setTag(i as isize);
        btn.setToolTip(Some(&NSString::from_str(name)));
        control_action(&btn, target, sel!(setPresetColor:));
        view.addSubview(&btn);
        buttons.push((*r, *g, *b, btn));
    }

    let item = NSMenuItem::new(mtm);
    item.setView(Some(&view));
    (item, buttons)
}
