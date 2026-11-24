#!/usr/bin/env bash
# Сборка QuikLight.app — нативного menu bar приложения для LED-панели
# Robobloq QuikLight (без Xcode, только Command Line Tools).
#
# Результат: target/release/QuikLight.app
# Использование: ./scripts/build_app.sh [--release|--debug|--skip-sign] [--sign "ИМЯ"]
#   --sign "ИМЯ"  — подписать постоянным сертификатом (имя из связки ключей):
#                   разрешение «Запись экрана» переживает пересборки.
#                   Без --sign используется ad-hoc подпись (-s -), которая при
#                   каждой пересборке меняет «личность» приложения → macOS снова
#                   запрашивает разрешение (см. README, «Запись экрана»).

set -euo pipefail

cd "$(dirname "$0")/.."

MODE="${1:---release}"
SKIP_SIGN=0
if [ "$MODE" = "--debug" ]; then
    PROFILE="debug"
    CARGO_ARGS=""
elif [ "$MODE" = "--skip-sign" ]; then
    PROFILE="release"
    CARGO_ARGS="--release"
    SKIP_SIGN=1
elif [ "$MODE" = "--release" ]; then
    PROFILE="release"
    CARGO_ARGS="--release"
else
    echo "Использование: $0 [--release|--debug|--skip-sign] [--sign \"ИМЯ\"]" >&2
    exit 2
fi

# Постоянный сертификат для подписи (второй аргумент --sign "ИМЯ").
IDENTITY=""
if [ "${2:-}" = "--sign" ] && [ -n "${3:-}" ]; then
    IDENTITY="$3"
fi

# Версия для Info.plist берётся из [workspace.package] Cargo.toml —
# единый источник истины для приложения и для манифеста обновления.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
if [ -z "$VERSION" ]; then
    echo "Не удалось прочитать версию из Cargo.toml" >&2
    exit 1
fi
echo "▶ Версия: $VERSION"

echo "▶ Сборка ($PROFILE)..."
cargo build --workspace $CARGO_ARGS

APP="target/$PROFILE/QuikLight.app"
BIN="target/$PROFILE/quiklight-app"
ASSETS="assets/icons"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/quiklight-app"

# Иконка меню-бара (template: чёрный + альфа, macOS перекрашивает под тему).
cp "$ASSETS"/menubar@1x.png "$ASSETS"/menubar@2x.png "$ASSETS"/menubar@3x.png \
   "$ASSETS"/menubar18@1x.png "$ASSETS"/menubar18@2x.png "$ASSETS"/menubar18@3x.png \
   "$APP/Contents/Resources/"

# App icon: из мастера 1024×1024 собираем .icns (sips → iconset → iconutil).
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
for spec in \
    "16 icon_16x16.png" "32 icon_16x16@2x.png" \
    "32 icon_32x32.png" "64 icon_32x32@2x.png" \
    "128 icon_128x128.png" "256 icon_128x128@2x.png" \
    "256 icon_256x256.png" "512 icon_256x256@2x.png" \
    "512 icon_512x512.png" "1024 icon_512x512@2x.png"; do
    set -- $spec
    sips -z "$1" "$1" -s format png "$ASSETS/AppIcon-1024.png" --out "$ICONSET/$2" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>quiklight-app</string>
    <key>CFBundleIdentifier</key>
    <string>ru.quiklight.macos</string>
    <key>CFBundleName</key>
    <string>QuikLight</string>
    <key>CFBundleDisplayName</key>
    <string>QuikLight</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>__QUIKLIGHT_VERSION__</string>
    <key>CFBundleVersion</key>
    <string>__QUIKLIGHT_VERSION__</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSScreenCaptureUsageDescription</key>
    <string>QuikLight использует запись экрана для режима Screen sync (подсветка краёв монитора).</string>
</dict>
</plist>
PLIST

# Heredoc выше в кавычках (без подстановки), поэтому версию вписываем после.
sed -i '' "s/__QUIKLIGHT_VERSION__/$VERSION/g" "$APP/Contents/Info.plist"

if [ "$SKIP_SIGN" = "0" ]; then
    if [ -n "$IDENTITY" ]; then
        echo "▶ Подпись сертификатом «${IDENTITY}»..."
        codesign --force --deep --sign "$IDENTITY" "$APP"
    else
        echo "▶ Ad-hoc подпись (-s -)..."
        codesign --force --deep -s - "$APP"
        echo
        echo "⚠ Ad-hoc подпись при каждой пересборке меняет «личность» приложения:"
        echo "  macOS снова запросит разрешение «Запись экрана» (старая запись в"
        echo "  настройках останется, но к новому бинарнику не применится)."
        echo "  Чтобы разрешение переживало пересборки, подпишите постоянным"
        echo "  сертификатом:"
        echo "    ./scripts/build_app.sh --release --sign \"Имя сертификата\""
        echo "  (создать: Keychain Access → Certificate Assistant → Create a"
        echo "  Certificate → тип Code Signing; подробнее — README, «Запись экрана»)."
    fi
fi

echo "✓ Готово: $APP"
echo "  Запуск: open $APP"
