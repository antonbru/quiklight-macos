# ARCHITECTURE.md — карта модулей QuikLight

> Краткая карта проекта: кто за что отвечает и как ходят данные.
> Протокол устройства — в `docs/PROTOCOL.md`, команды и правила работы — в
> `AGENTS.md`, история решений — в `docs/DECISIONS.md`.

## Крейты

| Крейт | Тип | Ответственность |
|---|---|---|
| `quiklight-core` | lib (+ пример) | Протокол, HID-транспорт, захват экрана, цветообработка. Без UI |
| `quiklight-app` | bin | Menu bar приложение: AppKit-UI + фоновый движок режимов |
| `hidprobe` | bin | CLI-проверка железа (info/color/brightness/off/effect/speed) |

## quiklight-core

| Модуль | Ответственность |
|---|---|
| `color.rs` | `Rgb = (u8,u8,u8)` и HSV→RGB (общие утилиты) |
| `protocol/packet.rs` | `Frame`: RB (1-байт size) и SC (2-байт BE size), checksum, чанкинг в 64-байтные HID-отчёты |
| `protocol/actions.rs` | Коды действий + типизированные команды (brightness, effect, section, turn_off, set_all_black) |
| `hid/device.rs` | `Device`: поиск `1a86:fe07`, interface 0, `write_frame`, `write_with_response`, `turn_off_all` (0x97×4 + чёрный), `send_screen_sync`. Запись сериализуется через `&mut self` — отдельной очереди как объекта нет |
| `hid/device_info.rs` | `DeviceInfo` + `parse_device_info`: смещения полей ответа `readDeviceInfo`; `DEFAULT_LED_COUNT = 63` — фолбэк, если панель ещё не ответила |
| `sync/screen_capturer.rs` | `ScreenCapturer`: `SCScreenshotManager` → `CapturedFrame` (BGRA + pitch). Захват синхронный (локальный канал + `RcBlock`) и блокирует engine-поток до готовности кадра |
| `sync/color_extractor.rs` | кадр → сетка → края → постобработка (гамма/насыщенность/light compression/сглаживание) |
| `sync/led_mapper.rs` | края экрана → LED в порядке проводов; `EdgeLayout::default_for` |

`mod.rs` в каждом модуле только реэкспортирует подмодули
(`color`, `protocol`, `hid`, `sync`, `effects`).

Инварианты ядра:

- запись в HID — только через `Device` (`&mut self`), поэтому чанки одного кадра не перемежаются чужими;
- id сессии растёт монотонно (после 255 → 1) и передаётся в каждый кадр;
- в ядре нет зависимостей от AppKit — всё тестируется без GUI.

## quiklight-app

| Модуль | Ответственность |
|---|---|
| `main.rs` | Точка входа: каналы, `engine::spawn`, сборка делегата, `app.run()` |
| `app.rs` | `AppDelegate` + `AppIvars`: состояние UI, действия меню (селекторы), `update_ui`, обработка событий потока |
| `menu.rs` | Меню menu bar: переключатель «Подсветка», режимы, палитра пресетов, слайдер яркости |
| `settings_window.rs` | `SettingsPanel` + окно настроек: создание контролов, применение, заполнение |
| `ui.rs` | AppKit-хелперы: label/слайдер/степпер/чекбокс, кружок цвета, иконка menu bar, алерт |
| `settings.rs` | `Settings` (Copy) + производные: эффективные LED/раскладка, интервал кадров, конфиг постобработки |
| `engine.rs` | Фоновый поток: состояние, обработка `Command`, режимы (Off/Static/Effects/Sync/Fireplace), HID-запись, автопереподключение |
| `effects/fireplace.rs` | Чистая математика «Камина»: высоты LED → цвет пламени → payload |

## Поток данных

```
        UI (main thread, AppKit)                  Фоновый поток (engine)
  ┌────────────────────────────────┐   Command   ┌───────────────────────────────┐
  │ AppDelegate (app.rs)           │ ──────────► │ recv_timeout(интервал режима) │
  │  • меню/настройки → Command    │             │  • apply_mode / sync_step     │
  │  • таймер 0.25 c ← WorkerEvent │ ◄────────── │  • fireplace_step             │
  └────────────────────────────────┘ WorkerEvent └───────────────────────────────┘
                                                  │ Device (core, &mut, очередь)
                                                  ▼
                                         1a86:fe07 HID interface 0 (64-байтные отчёты)
```

- `Command`: `SetPower`, `SetMode`, `SetColor`, `SetBrightness`, `SetSettings`, `Quit`.
- `WorkerEvent`: `DeviceChanged { connected, info }`, `ModeChanged`, `Error(String)`, `Exited`.
- Непрерывные режимы тикают по таймауту `recv_timeout`; кадр screen sync —
  `ScreenCapturer` → `ColorExtractor` → `EdgeLayout` → `Device::send_screen_sync`.
- Выход: по `Command::Quit` движок гасит LED (если включено) и шлёт `Exited`;
  UI ждёт это (максимум 2 с) и завершает приложение.

## Потоки и производительность

- Один фоновый поток на всё устройство: UI никогда не блокируется на HID.
- Screen sync: интервал 16–50 мс (20–60 fps из настроек); захват ограничен
  480 px по большой стороне — для ambilight этого достаточно.
- «Камин» рисуется программно с фиксированным интервалом 33 мс (~30 fps);
  настройка fps влияет только на screen sync. Кадры уходят теми же SC-кадрами
  (action 128).
- `readDeviceInfo` запрашивается один раз при старте. Если панель подключили
  позже, число LED не перечитывается — авто-раскладка остаётся на
  `DEFAULT_LED_COUNT = 63`.

## Как расширять

- **Новая команда протокола** — константа в `protocol/actions.rs` + типизированный
  конструктор кадра + юнит-тест с эталонным hex.
- **Новый программный эффект** — модуль в `quiklight-app/src/effects/`, значение
  `engine::Mode`, пункт меню в `menu.rs`.
- **Новая настройка** — поле в `settings.rs`, контрол в `settings_window.rs`,
  проброс через `Command::SetSettings`.

## Тесты

Юнит-тесты inline (`#[cfg(test)] mod tests`) рядом с кодом: протокол (`packet`,
`actions`), парсер `device_info`, маппинг LED, постобработка цвета, настройки,
математика «Камина» и HSV. Железо не требуется: `cargo test --workspace`;
один модуль — `cargo test -p quiklight-core packet::`.

