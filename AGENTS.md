# AGENTS.md — QuikLight (macOS)

Нативное menu bar приложение для USB LED-панели Robobloq QuikLight / Feelin Light Q1
(`1A86:FE07`). Rust workspace: `quiklight-core` (протокол, HID, захват экрана),
`quiklight-app` (AppKit через `objc2`), `hidprobe` (CLI-диагностика). Edition 2021;
статичные режимы — macOS 13+, screen sync — macOS 14+ (`SCScreenshotManager`).

## Карта документов

| Документ | Что там | Когда читать |
|---|---|---|
| `docs/PROTOCOL.md` | Канон протокола: RB/SC-кадры, action-коды, HID-передача, hex-векторы, подводные камни | Перед любой правкой `quiklight-core/src/{protocol,hid}` — целиком |
| `docs/ARCHITECTURE.md` | Крейты, модули, поток данных UI ↔ engine | Перед правкой структуры, добавлением режима/эффекта/настройки |
| `README.md` | Сборка, запуск, разрешение «Запись экрана», CLI | Пользовательские инструкции и troubleshooting |
| `docs/DECISIONS.md` | Журнал архитектурных решений (ADR) и открытые вопросы | Перед сменой стека или подхода |
| `docs/REFERENCES.md` | Внешние open-source реализации (SyncRGB и др.) | Справочник по логике, не источник истины |
| `docs/ICON_TZ.md` | ТЗ на иконки — **архив, выполнено** | Не обновлять |
| `.clinerules/CLINE.md` | Правила для Cline (фазы, история проекта) | Только при работе как Cline |

## Команды

```bash
./scripts/build_app.sh --release [--sign "Имя"]   # → target/release/QuikLight.app
open target/release/QuikLight.app                 # запускать бандл, не сырой бинарник
cargo fmt --all
cargo clippy --workspace --all-targets            # должен быть чист
cargo test --workspace                            # юнит-тесты, железо не требуется
cargo test -p quiklight-core packet::             # один модуль
```

В `build_app.sh` режим — **первый** аргумент, `--sign` — вторым.

Железо (панель подключена, официальный SyncLight закрыт):

```bash
cargo run -p hidprobe -- info|color R G B|brightness N|off|effect T I|speed N
cargo run -p quiklight-core --example sync_probe   # проверка SC-канала
```

## Жёсткие инварианты

- HID — только **interface 0**; interface 1 (keyboard) молча игнорирует команды.
- Кадр пишется целиком: чанки одного кадра не перемежаются (сериализация через `&mut Device`).
- Выключение: `0x97` ×4 с интервалом 20 мс, затем чёрный `[1,0,0,0,254]`.
- LED нумеруются **с 1**, `254` = «до конца ленты».
- Скорость эффектов инвертируется: на устройство уходит `100 - slider`.
- Число LED берётся из `readDeviceInfo`; `DEFAULT_LED_COUNT = 63` — фолбэк, у панели
  владельца 51 LED (раскладка 17/29/17).
- Правка протокола = правка `docs/PROTOCOL.md` + hex-векторов в тестах.

## Куда добавлять код

- **Команда протокола** → `protocol/actions.rs` + типизированный билдер + тест с hex.
- **Программный эффект** → `quiklight-app/src/effects/` + вариант `engine::Mode` +
  пункт меню в `menu.rs`.
- **Настройка** → поле в `settings.rs` + контрол в `settings_window.rs` +
  проброс через `Command::SetSettings`.
- Подробности — в `docs/ARCHITECTURE.md`, раздел «Как расширять».

## Правила

- Никаких тяжёлых зависимостей (Electron/Tauri/Flutter) и BLE/клавиатурных протоколов.
- Тесты inline (`#[cfg(test)] mod tests`), без железа; `cargo fmt` перед коммитом.
- Документация, комментарии и UI-строки — на русском.
- Не коммитить `target/`, `*.app`, `.reasonix/`.
- Если код расходится с `docs/PROTOCOL.md` — сначала проверь код (протокол верифицирован
  тремя реализациями), затем фиксируй расхождение в документации.

## Подводные камни

- **ad-hoc подпись ломает TCC**: разрешение «Запись экрана» привязано к подписи
  (см. README и `scripts/build_app.sh`).
- Официальный SyncLight пишет в то же HID-устройство — закрывать его.
- Настройки не персистятся: живут в памяти процесса (`Settings` — `Copy`), файла нет.
- `hidprobe info` печатает `device_id`, `UUID`, число LED и прошивку — не «MAC».
