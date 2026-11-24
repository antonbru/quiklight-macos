#!/usr/bin/env bash
# Создание самоподписанного сертификата Code Signing для QuikLight.
#
#   ./scripts/create_dev_cert.sh ["Имя сертификата"]
#
# Нужен один раз: сертификат живёт в связке ключей login и переиспользуется
# всеми релизами. Стабильная подпись — залог того, что разрешение
# «Запись экрана» (TCC) переживёт обновление приложения.
#
# После импорта macOS может один раз спросить доступ к ключу со стороны
# codesign — выберите «Всегда разрешать».

set -euo pipefail

NAME="${1:-QuikLight Dev}"
WORK="$(mktemp -d)"

cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

if security find-identity -p codesigning 2>/dev/null | grep -qF "\"$NAME\""; then
    echo "✓ Сертификат «${NAME}» уже есть в связке ключей:"
    security find-identity -p codesigning | grep -F "\"$NAME\"" || true
    exit 0
fi

echo "▶ Генерирую ключ и самоподписанный сертификат «${NAME}»…"
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$WORK/key.pem" \
    -out "$WORK/cert.pem" \
    -subj "/CN=$NAME" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning"

/usr/bin/openssl pkcs12 -export \
    -inkey "$WORK/key.pem" \
    -in "$WORK/cert.pem" \
    -out "$WORK/cert.p12" \
    -passout pass:

echo "▶ Импорт в связку ключей login (может потребоваться разрешение macOS)…"
/usr/bin/security import "$WORK/cert.p12" \
    -k "$HOME/Library/Keychains/login.keychain-db" \
    -P "" \
    -T /usr/bin/codesign \
    -T /usr/bin/security

echo "▶ Проверка:"
# Важно: codesign подписывает и «недоверенным» самоподписанным сертификатом
# (trust влияет только на Gatekeeper, а не на подпись), поэтому проверяем
# наличие удостоверения, а не флаг -v («valid identities»).
if security find-identity -p codesigning 2>/dev/null | grep -qF "\"${NAME}\""; then
    security find-identity -p codesigning | grep -F "\"${NAME}\"" || true
    echo "✓ Готово. Сборка: ./scripts/build_app.sh --release --sign \"${NAME}\""
    echo "  Строка (CSSMERR_TP_NOT_TRUSTED) — нормально: сертификат самоподписанный,"
    echo "  для локальной установки доверие macOS не требуется."
    exit 0
fi

echo "⚠ Сертификат «${NAME}» не появился в связке ключей login." >&2
echo "  Проверьте: security find-identity -p codesigning" >&2
exit 1
