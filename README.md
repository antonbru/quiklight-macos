# QuikLight — нативное macOS-приложение для LED-панели Robobloq QuikLight

<p align="center">
  <img src="assets/icons/preview-appicon.png" alt="QuikLight" width="128">
</p>

Лёгкий нативный menu bar app (Rust + AppKit через `objc2`) вместо тяжёлого
официального **SyncLight** (Electron, ~430 МБ RAM, 5 процессов).

Возможности:

- **статичный цвет** (палитра macOS), яркость 0–100, выключение;
- **встроенные эффекты** панели: 2 типа × 7 эффектов (динамика и звук);
- **свой эффект «Камин»** — резко мерцающее ярко-оранжевое пламя по нижней
  части ленты, будто за монитором горит огонь (рисует само приложение, ~30 fps);
- **screen sync** — ambilight: цвета краёв экрана → LED в реальном времени
  (ScreenCaptureKit, macOS 14+, 20–60 fps, гамма/насыщенность/сглаживание);
- настройки: число LED, раскладка по краям, fps, гамма, насыщенность,
  световое сжатие, «выключать LED при выходе»;
- автопереподключение: при отключении/подключении панели приложение
  продолжает работать.

## Железо

| | |
|---|---|
| Устройство | Robobloq QuikLight / Feelin Light Q1 (USB) |
| VID:PID | `0x1A86:0xFE07` |
| Interface 0 | канал управления (64-байтные HID-отчёты) |

> ⚠️ У панели **два** HID-интерфейса. Interface 1 (keyboard) игнорирует
> команды цвета — приложение всегда открывает interface 0.
> Протокол верифицирован: см. `docs/PROTOCOL.md` и `docs/REFERENCES.md`.

## Сборка

Требуется только Rust (stable) и Command Line Tools (не Xcode):

```bash
cargo build --release
./scripts/build_app.sh --release        # собирает target/release/QuikLight.app
open target/release/QuikLight.app
```

## Использование

1. Подключите панель по USB (драйверы не нужны — стандартный HID).
2. Запустите приложение — в menu bar появится иконка QuikLight
   (template-иконка, macOS перекрашивает её под светлую/тёмную тему;
   при выключенной подсветке иконка полупрозрачная).
3. Меню:
   - **Подсветка** — переключатель: включить/погасить;
   - **Режимы** (выбрать можно только один):
     - **Статичный цвет** — под ним сразу кружки готовых оттенков;
     - **Screen sync** — цвет по краям экрана;
     - **Эффекты** → Динамика и Звук (встроенные) + **Камин** (свой эффект);
   - **Яркость** — слайдер прямо в меню;
   - **Настройки…** — LED, раскладка, fps, гамма, насыщенность;
   - **Выйти** — погасит подсветку, если включена опция.

Для **Screen sync** при первом включении macOS спросит разрешение
«Запись экрана»: System Settings → Privacy & Security → Screen Recording →
включите **QuikLight** и перезапустите приложение.

## CLI-проверка железа (hidprobe)

```bash
cargo run -p hidprobe -- info          # MAC/LED/прошивка (readDeviceInfo)
cargo run -p hidprobe -- color 255 0 0 # вся лента красным
cargo run -p hidprobe -- brightness 50
cargo run -p hidprobe -- off
cargo run -p hidprobe -- effect 2 0    # динамический эффект 0
```

## Проверка SC-каналов (screen sync) на железе

```bash
cargo run -p quiklight-core --example sync_probe
```

Показывает на ленте два градиента (многочанковые SC-кадры: 51 LED × 3 байта =
153 байта payload → 3 HID-отчёта), затем гасит. Полезно, если screen sync
в приложении не работает, а статичный цвет — работает.

## Тесты

```bash
cargo test --workspace
```

Юнит-тесты протокола (checksum, чанкинг, эталонные hex-пакеты, парсер
`readDeviceInfo` — включая реальный ответ панели) и screen sync (маппинг
краёв, постобработка) не требуют железа.

## Структура

```
crates/
├── quiklight-core/   # протокол RB/SC, HID-транспорт, захват экрана, цвет
├── quiklight-app/    # menu bar приложение (AppKit через objc2)
└── hidprobe/         # CLI-проверка железа (фаза 0)
assets/icons/         # иконки: AppIcon-1024.png (мастер) + template-наборы меню-бара + preview-appicon.png
scripts/build_app.sh  # сборка .app-бандла (+ ad-hoc подпись)
docs/                 # PROTOCOL.md, REFERENCES.md, ICON_TZ.md
```

## Ограничения

- **Screen sync требует macOS 14+** (используется `SCScreenshotManager`);
  статичные режимы и эффекты работают на macOS 13+.
- BLE/донгл-вариант панели не поддерживается (другой протокол).
- При одновременной работе с официальным SyncLight возможны конфликты записи
  в HID — закрывайте SyncLight.
