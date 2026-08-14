#!/usr/bin/env bash
# Сборка QuikLight.app — нативного menu bar приложения для LED-панели
# Robobloq QuikLight (без Xcode, только Command Line Tools).
#
# Результат: target/release/QuikLight.app
# Использование: ./scripts/build_app.sh [--release|--debug] [--skip-sign]

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
    echo "Использование: $0 [--release|--debug] [--skip-sign]" >&2
    exit 2
fi

echo "▶ Сборка ($PROFILE)..."
cargo build --workspace $CARGO_ARGS

APP="target/$PROFILE/QuikLight.app"
BIN="target/$PROFILE/quiklight-app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/quiklight-app"

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
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
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

if [ "$SKIP_SIGN" = "0" ]; then
    echo "▶ Ad-hoc подпись..."
    codesign --force --deep -s - "$APP"
fi

echo "✓ Готово: $APP"
echo "  Запуск: open $APP"
