#!/usr/bin/env bash
# Пересборка растров menu bar иконки из векторного мастера.
#
# Мастер: assets/icons/menubar.svg (сетка 18×18 pt). Скрипт растеризует его
# в template-PNG для menu bar: 16 pt (menubar@1x/@2x/@3x) и 18 pt
# (menubar18@1x/@2x/@3x). Исходники попадают в assets/icons, откуда
# build_app.sh копирует их в QuikLight.app/Contents/Resources.
#
# Требуется rsvg-convert (brew install librsvg). В сборку приложения он
# не входит — готовые PNG лежат в репозитории.
set -euo pipefail
cd "$(dirname "$0")/.."

SVG="assets/icons/menubar.svg"
OUT="assets/icons"

if ! command -v rsvg-convert >/dev/null 2>&1; then
    echo "Нужен rsvg-convert: brew install librsvg" >&2
    exit 1
fi
if [ ! -f "$SVG" ]; then
    echo "Нет мастера $SVG" >&2
    exit 1
fi

for spec in "16 menubar@1x.png" "32 menubar@2x.png" "48 menubar@3x.png" \
            "18 menubar18@1x.png" "36 menubar18@2x.png" "54 menubar18@3x.png"; do
    set -- $spec
    rsvg-convert -w "$1" -h "$1" "$SVG" -o "$OUT/$2"
    echo "✓ $OUT/$2 ($1×$1)"
done

echo "Готово. Пересоберите бандл: ./scripts/build_app.sh --release"
