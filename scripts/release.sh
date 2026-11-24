#!/usr/bin/env bash
# Публикация релиза QuikLight в GitHub Releases.
#
#   ./scripts/release.sh [--sign "ИМЯ"] [--notes FILE] [--draft]
#
# Что делает:
#   1) читает версию из [workspace.package] Cargo.toml (единый источник истины);
#   2) собирает и подписывает QuikLight.app (build_app.sh --release --sign);
#   3) упаковывает .app в QuikLight-<версия>.zip (ditto, с сохранением ресурсов);
#   4) считает SHA-256 и размер;
#   5) пишет latest.json — манифест, который читает само приложение;
#   6) создаёт тег v<версия> и GitHub Release с ассетами .zip и latest.json.
#
# Секреты не нужны: используется локальный сертификат и уже авторизованный gh.
# Переменные: CODESIGN_IDENTITY (по умолчанию «QuikLight Dev»),
#             QUIKLIGHT_REPO (по умолчанию antonbru/quiklight-macos).

set -euo pipefail

cd "$(dirname "$0")/.."

IDENTITY="${CODESIGN_IDENTITY:-QuikLight Dev}"
NOTES_FILE=""
DRAFT=""
REPO="${QUIKLIGHT_REPO:-antonbru/quiklight-macos}"
MIN_MACOS="13.0"

while [ $# -gt 0 ]; do
    case "$1" in
        --sign)
            IDENTITY="${2:-}"
            shift 2
            ;;
        --notes)
            NOTES_FILE="${2:-}"
            shift 2
            ;;
        --draft)
            DRAFT="--draft"
            shift
            ;;
        *)
            echo "Использование: $0 [--sign \"ИМЯ\"] [--notes FILE] [--draft]" >&2
            exit 2
            ;;
    esac
done

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
if [ -z "$VERSION" ]; then
    echo "Не удалось прочитать версию из Cargo.toml" >&2
    exit 1
fi
TAG="v$VERSION"
ZIP="target/release/QuikLight-$VERSION.zip"
MANIFEST="target/release/latest.json"

echo "▶ Релиз QuikLight $VERSION (тег $TAG, репозиторий $REPO)"

# Предпосылки: gh авторизован, сертификат на месте, рабочее дерево чистое,
# тег ещё не занят.
if ! command -v gh >/dev/null 2>&1; then
    echo "Нужен GitHub CLI: brew install gh" >&2
    exit 1
fi
if ! gh auth status >/dev/null 2>&1; then
    echo "Нужна авторизация: gh auth login" >&2
    exit 1
fi
if ! security find-identity -p codesigning 2>/dev/null | grep -qF "\"$IDENTITY\""; then
    echo "В связке ключей нет сертификата «${IDENTITY}»." >&2
    echo "Создайте его: ./scripts/create_dev_cert.sh \"$IDENTITY\"" >&2
    echo "или передайте существующий: --sign \"ИМЯ\"" >&2
    exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
    echo "В рабочем дереве есть незакоммиченные изменения — закоммитьте их." >&2
    exit 1
fi
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    echo "Тег $TAG уже существует — поднимите версию в Cargo.toml." >&2
    exit 1
fi

echo "▶ Сборка и подпись…"
./scripts/build_app.sh --release --sign "$IDENTITY"

echo "▶ Упаковка…"
rm -f "$ZIP"
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "target/release/QuikLight.app" "$ZIP"

SHA="$(/usr/bin/shasum -a 256 "$ZIP" | cut -d' ' -f1)"
SIZE="$(/usr/bin/stat -f%z "$ZIP")"
ASSET="$(basename "$ZIP")"
URL="https://github.com/$REPO/releases/download/$TAG/$ASSET"

# Манифест читает приложение (см. crates/quiklight-app/src/updater.rs).
# Только строковые поля в ASCII — никакого экранирования не требуется.
cat > "$MANIFEST" <<JSON
{
  "version": "$VERSION",
  "url": "$URL",
  "sha256": "$SHA",
  "min_macos": "$MIN_MACOS"
}
JSON

echo "▶ Манифест:"
cat "$MANIFEST"

echo "▶ Публикация…"
if [ -n "$NOTES_FILE" ]; then
    gh release create "$TAG" "$ZIP" "$MANIFEST" \
        --repo "$REPO" \
        --title "QuikLight $VERSION" \
        --notes-file "$NOTES_FILE" \
        $DRAFT
else
    gh release create "$TAG" "$ZIP" "$MANIFEST" \
        --repo "$REPO" \
        --title "QuikLight $VERSION" \
        --generate-notes \
        $DRAFT
fi

echo "✓ Готово: https://github.com/$REPO/releases/tag/$TAG"
echo "  Проверка обновления в приложении: меню → «Проверить обновление…»"
