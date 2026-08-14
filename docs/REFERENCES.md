# REFERENCES.md — Референсные реализации

Три независимые open-source реализации протокола Robobloq QuikLight (USB-версия). Все подтверждают друг друга; протокол в `docs/PROTOCOL.md` собран из них + реверса Electron-бандла SyncLight 2.19.1.

> Правило отбора: проекты малозвёздные (ниша узкая), но живы и свежие — не archived, есть коммиты в 2026. Для протокольной документации важна независимая верификация, а не популярность.

## 1. SyncRGB / OpenSyncLight — главный референс (Windows, Rust)

- **SyncRGB**: https://github.com/Tonic-Jin/SyncRGB — 3⭐, MIT, Rust, обновляется (последний push авг 2026), не archived
- **OpenSyncLight**: https://github.com/namethatidesire/OpenSyncLight — зеркало/форк SyncRGB (0⭐, идентичная структура, есть `src/bin/hid_probe.rs`)

**Что брать:**
- `src/device/protocol.rs` — RB/SC-пакеты, checksum, чанкинг, action-коды (самая чистая реализация)
- `src/device/serial.rs` — HID-подключение через `hidapi`: поиск 1a86:fe07, interface 0, report ID 0x00, задержки 200/500 мс
- `src/color/extractor.rs` — извлечение цветов краёв, маппинг на LED, сглаживание, гамма, light compression
- README: анализ таймингов оригинала (capture 20 мс = 50 fps, send delay syncSpeed+20 мс), таблица конфига (fps, sample_width, edge_number 3/4, wire_map RGB/RBG/GRB…)

**Портировать на macOS нельзя напрямую** (DXGI/WASAPI/winreg — Windows), но логика протокола и цветообработки переносится 1-в-1.

## 2. quicklight-linux — самый детальный PROTOCOL.md (Linux, Python)

- https://github.com/jrcn1991/quicklight-linux — 0⭐, Python 3.10+ + PyQt6, обновляется (авг 2026)

**Что брать:**
- `docs/PROTOCOL.md` — полная таблица действий (включая 137, 139, 145, 147, 149, 150, которых нет в SyncRGB), смещения ответа `readDeviceInfo`, эффекты (tipo 2/3), подводные камни (инвертированная скорость, старые прошивки без 0x97, повтор 4× при выключении)
- CLI (`quicklight color`, `info`, и т.д.) — удобная модель команд для нашего `hidprobe`
- Утверждение «1% CPU / 14 МБ RAM при 30 fps» — ориентир производительности

**Нюанс:** использует `/dev/hidraw` (Linux), на macOS это будет IOKit HID — транспорт другой, протокол тот же.

## 3. quiklight-x11 (Linux, Rust + Python-ветка)

- https://github.com/Kevinsillo/quiklight-x11 — 1⭐, Rust (есть ветка `python-version`), июнь 2026

**Что брать:**
- Конфиг раскладки: `top=29, right=17, left=17` (63 LED) — стандарт для 27" ленты; подстройка под фактическое число LED
- Подход к захвату экрана (X11) — на macOS заменяется ScreenCaptureKit
- Упоминание `hypr-quicklight` (Wayland) — ещё одна реализация для сверки

## Что НЕ брать

- **npm `@warren-robobloq/quiklight`** — приватный (внутри Electron-бандла SyncLight), в реестре отсутствует
- **BLE/донгл-варианты** (в SyncLight — модуль `62564`) — другой протокол (MAC-адресация, 20-байтные кадры)
- **Протокол RGB-клавиатур/мышей Robobloq** (модуль `82553`) — заголовок SC + CRC16, не наш случай

## Итоговая стратегия

Пишем свой код на **Rust** для macOS (см. CLINE.md): AppKit через `objc2` для GUI, ScreenCaptureKit через `objc2-screen-capture-kit`, HID через `hidapi`. Так как SyncRGB — уже Rust, перенос почти 1-в-1:
- протокол — по `docs/PROTOCOL.md` (сверять с `protocol.rs` из SyncRGB);
- цветообработка — по `extractor.rs` из SyncRGB (переносится напрямую);
- поведение команд/тайминги — по README SyncRGB и PROTOCOL.md quicklight-linux.
- транспорт — `hidapi` (тот же крейт, что в SyncRGB), интерфейс 0, report ID 0x00
