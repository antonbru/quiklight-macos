//! Самообновление приложения: проверка публичного GitHub Releases, загрузка
//! архива, проверка sha256 и подписи, подмена бандла после выхода.
//!
//! Манифест `latest.json` лежит ассетом последнего релиза и содержит только
//! строковые поля: `version`, `url`, `sha256`, `min_macos`. Разбор — через
//! `NSJSONSerialization`, сеть и распаковка — штатные утилиты macOS
//! (`curl`, `ditto`, `codesign`, `shasum`), без сетевых крейтов.
//!
//! UI получает события `UpdateEvent` в общем канале `WorkerEvent`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use objc2::runtime::AnyObject;
use objc2_foundation::{NSData, NSDictionary, NSJSONReadingOptions, NSJSONSerialization, NSString};

use crate::engine::WorkerEvent;

/// URL манифеста последнего релиза (ассет `latest.json`).
/// Переопределяется переменной окружения `QUIKLIGHT_UPDATE_FEED` — удобно для
/// локальной проверки через `file://`.
pub const FEED_URL: &str =
    "https://github.com/antonbru/quiklight-macos/releases/latest/download/latest.json";

/// Идентификатор бандла: обновление обязано прийти с этим же `CFBundleIdentifier`.
const BUNDLE_ID: &str = "ru.quiklight.macos";

/// Описание доступного обновления (поля `latest.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateManifest {
    /// Версия релиза, например `0.2.0`.
    pub version: String,
    /// Прямая ссылка на `.zip` с `QuikLight.app`.
    pub url: String,
    /// SHA-256 архива (64 hex-символа, нижний регистр).
    pub sha256: String,
    /// Минимальная версия macOS для этого релиза (`13.0`, `14.0`, …).
    pub min_macos: Option<String>,
}

/// Событие обновления для UI.
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    /// Проверка завершена: `Some` — доступна новая версия.
    CheckFinished(Result<Option<UpdateManifest>, String>),
    /// Сколько байт уже скачано.
    Progress { received: u64 },
    /// Обновление скачано, проверено и распаковано.
    Ready { version: String, staged: PathBuf },
    /// Любой этап сорвался.
    Failed(String),
}

/// Версия запущенного приложения (совпадает с `CFBundleShortVersionString`,
/// которую `build_app.sh` вписывает из `Cargo.toml`).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Разобрать версию вида `0.2.0`, `v0.2.0`, `0.10.0-rc1` в три числа.
pub fn parse_version(version: &str) -> [u32; 3] {
    let trimmed = version.trim().trim_start_matches('v');
    let mut parts = [0u32; 3];
    for (index, raw) in trimmed.split('.').take(3).enumerate() {
        let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
        parts[index] = digits.parse().unwrap_or(0);
    }
    parts
}

/// `remote` строго новее `local`.
pub fn is_newer(remote: &str, local: &str) -> bool {
    parse_version(remote) > parse_version(local)
}

/// URL манифеста: константа или переопределение из окружения.
pub fn feed_url() -> String {
    match std::env::var("QUIKLIGHT_UPDATE_FEED") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => FEED_URL.to_string(),
    }
}

/// Разобрать и провалидировать манифест.
pub fn parse_manifest(json: &[u8]) -> Option<UpdateManifest> {
    let data = NSData::with_bytes(json);
    let object =
        NSJSONSerialization::JSONObjectWithData_options_error(&data, NSJSONReadingOptions(0))
            .ok()?;
    let dict = object.downcast::<NSDictionary>().ok()?;
    // SAFETY: JSON-объект верхнего уровня — словарь, ключи в JSON всегда строки;
    // значения остаются `AnyObject` и проверяются через `downcast_ref`.
    let dict = unsafe { dict.cast_unchecked::<NSString, AnyObject>() };

    let manifest = UpdateManifest {
        version: json_string(dict, "version")?,
        url: json_string(dict, "url")?,
        sha256: json_string(dict, "sha256")?,
        min_macos: json_string(dict, "min_macos"),
    };

    if manifest.version.trim().is_empty() {
        return None;
    }
    let has_scheme = ["https://", "http://", "file://"]
        .iter()
        .any(|prefix| manifest.url.starts_with(*prefix));
    if !has_scheme {
        return None;
    }
    let sha = manifest.sha256.trim();
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(manifest)
}

/// Запустить проверку обновлений в фоновом потоке.
pub fn spawn_check(events: Sender<WorkerEvent>) {
    let _ = thread::Builder::new()
        .name("quiklight-update-check".into())
        .spawn(move || {
            let result = check();
            let _ = events.send(WorkerEvent::Update(UpdateEvent::CheckFinished(result)));
        });
}

/// Запустить скачивание и подготовку обновления в фоновом потоке.
pub fn spawn_download(manifest: UpdateManifest, events: Sender<WorkerEvent>) {
    let _ = thread::Builder::new()
        .name("quiklight-update-download".into())
        .spawn(move || {
            let event = match download_and_stage(&manifest, &events) {
                Ok(staged) => UpdateEvent::Ready {
                    version: manifest.version.clone(),
                    staged,
                },
                Err(error) => UpdateEvent::Failed(error),
            };
            let _ = events.send(WorkerEvent::Update(event));
        });
}

/// Проверить манифест и сравнить версии.
fn check() -> Result<Option<UpdateManifest>, String> {
    let manifest = fetch_manifest(&feed_url())?;
    if is_newer(&manifest.version, current_version()) {
        Ok(Some(manifest))
    } else {
        Ok(None)
    }
}

fn fetch_manifest(url: &str) -> Result<UpdateManifest, String> {
    let dir = cache_dir().ok_or_else(|| "Не удалось определить каталог кэша".to_string())?;
    fs::create_dir_all(&dir).map_err(|e| format!("Не удалось создать кэш: {e}"))?;
    let path = dir.join("latest.json");
    let _ = fs::remove_file(&path);
    curl_download(url, &path, None)?;
    let bytes = fs::read(&path).map_err(|e| format!("Не удалось прочитать манифест: {e}"))?;
    parse_manifest(&bytes).ok_or_else(|| "Манифест обновления повреждён или неполный".to_string())
}

/// Скачать, проверить и распаковать обновление. Возвращает путь к готовому бандлу.
fn download_and_stage(
    manifest: &UpdateManifest,
    events: &Sender<WorkerEvent>,
) -> Result<PathBuf, String> {
    let dir = cache_dir()
        .ok_or_else(|| "Не удалось определить каталог кэша".to_string())?
        .join("updates")
        .join(format!("v{}", manifest.version));
    fs::create_dir_all(&dir).map_err(|e| format!("Не удалось создать каталог обновления: {e}"))?;

    // Раньше архива мог быть скачан целиком — тогда sha256 сойдётся и качать
    // заново не нужно.
    let zip = dir.join("QuikLight.zip");
    let cached_ok = sha256_file(&zip).as_deref() == Some(manifest.sha256.as_str());
    if !cached_ok {
        let _ = fs::remove_file(&zip);
        curl_download(&manifest.url, &zip, Some(events))?;
    }

    let sum = sha256_file(&zip).ok_or_else(|| "Не удалось посчитать SHA-256 архива".to_string())?;
    if sum != manifest.sha256 {
        return Err("Контрольная сумма архива не совпала с манифестом".to_string());
    }

    let unpacked = dir.join("unpacked");
    if unpacked.exists() {
        fs::remove_dir_all(&unpacked)
            .map_err(|e| format!("Не удалось очистить распаковку: {e}"))?;
    }
    fs::create_dir_all(&unpacked)
        .map_err(|e| format!("Не удалось создать каталог распаковки: {e}"))?;
    run_ok(
        Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&unpacked),
    )?;

    let app = find_app(&unpacked).ok_or_else(|| "В архиве нет QuikLight.app".to_string())?;
    verify_staged(&app, manifest)?;
    Ok(app)
}

/// Проверка скачанного бандла до подмены.
fn verify_staged(app: &Path, manifest: &UpdateManifest) -> Result<(), String> {
    run_ok(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
    )?;

    let report = codesign_report(app).ok_or_else(|| "Не удалось прочитать подпись".to_string())?;
    if field(&report, "Identifier=").as_deref() != Some(BUNDLE_ID) {
        return Err("Обновление собрано не как QuikLight (другой CFBundleIdentifier)".to_string());
    }
    if let Some(staged_authority) = authority(&report) {
        // Сравниваем только когда запущены из бандла и у текущей подписи есть
        // Authority (у ad-hoc подписи его нет — сравнивать нечего).
        if let Ok(current_app) = current_app() {
            let current_authority = codesign_report(&current_app).as_deref().and_then(authority);
            if let Some(current_authority) = current_authority {
                if current_authority != staged_authority {
                    return Err(format!(
                        "Обновление подписано другим сертификатом ({staged_authority}, \
                         у вас {current_authority}): после установки macOS заново попросит \
                         разрешение «Запись экрана»"
                    ));
                }
            }
        }
    }

    let staged_version = bundle_version(app)
        .ok_or_else(|| "В обновлении нет CFBundleShortVersionString".to_string())?;
    if staged_version != manifest.version {
        return Err(format!(
            "Версия в бандле ({staged_version}) не совпадает с манифестом ({})",
            manifest.version
        ));
    }

    if let (Some(min), Some(current)) = (manifest.min_macos.as_deref(), os_version()) {
        if parse_version(&current) < parse_version(min) {
            return Err(format!("Обновление требует macOS {min}, а у вас {current}"));
        }
    }
    Ok(())
}

/// Записать скрипт установки и запустить его отдельным процессом: он дождётся
/// выхода приложения, заменит бандл (с откатом при ошибке) и перезапустит его.
pub fn install_and_relaunch(staged: &Path) -> Result<(), String> {
    let target = current_app()?;
    if target.to_string_lossy().contains("/AppTranslocation/") {
        return Err(
            "Приложение запущено из временной копии macOS. Переместите QuikLight.app \
             в «Программы» и запустите оттуда"
                .to_string(),
        );
    }
    let log = log_file()?;
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Не удалось создать каталог логов: {e}"))?;
    }
    let script = cache_dir()
        .ok_or_else(|| "Не удалось определить каталог кэша".to_string())?
        .join("updates")
        .join("apply.sh");
    if let Some(parent) = script.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Не удалось создать каталог: {e}"))?;
    }
    fs::write(&script, render_install_script())
        .map_err(|e| format!("Не удалось записать скрипт установки: {e}"))?;

    Command::new("/bin/sh")
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(staged)
        .arg(&target)
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Не удалось запустить установщик: {e}"))?;
    Ok(())
}

/// Скрипт установки: `$1` — PID приложения, `$2` — новый бандл, `$3` — целевой
/// бандл, `$4` — лог. Ждём выхода, меняем бандл, при ошибке откатываемся.
pub fn render_install_script() -> String {
    const SCRIPT: &str = r#"#!/bin/sh
# Установка обновления QuikLight (сгенерировано приложением).
set -u
PID="$1"
STAGED="$2"
TARGET="$3"
LOG="$4"
BACKUP="$TARGET.bak"

log() { echo "$(date '+%Y-%m-%d %H:%M:%S') $*" >> "$LOG"; }

log "старт установки: PID $PID, $STAGED -> $TARGET"

i=0
while kill -0 "$PID" 2>/dev/null && [ "$i" -lt 600 ]; do
    sleep 0.2
    i=$((i + 1))
done

rm -rf "$BACKUP"
if /usr/bin/ditto "$TARGET" "$BACKUP" && rm -rf "$TARGET" && /usr/bin/ditto "$STAGED" "$TARGET"; then
    /usr/bin/xattr -dr com.apple.quarantine "$TARGET" 2>/dev/null
    log "обновление установлено"
    rm -rf "$BACKUP"
    /usr/bin/open "$TARGET"
else
    log "ошибка замены — откат"
    rm -rf "$TARGET"
    /usr/bin/ditto "$BACKUP" "$TARGET"
    rm -rf "$BACKUP"
    /usr/bin/open "$TARGET"
fi
rm -rf "$STAGED"
"#;
    SCRIPT.to_string()
}

// ── путь к запущенному бандлу ──

/// `QuikLight.app` запущенного приложения или `None` для сырого бинарника.
pub fn current_app() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Не удалось определить путь к приложению: {e}"))?;
    // …/QuikLight.app/Contents/MacOS/quiklight-app
    let app = exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"));
    app.map(Path::to_path_buf).ok_or_else(|| {
        "Обновление доступно только для запуска из QuikLight.app (не из cargo run)".to_string()
    })
}

fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Caches").join(BUNDLE_ID))
}

fn log_file() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "Не задан HOME".to_string())?;
    Ok(PathBuf::from(home)
        .join("Library/Logs/QuikLight")
        .join("update.log"))
}

fn find_app(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"))
}

// ── системные утилиты ──

fn curl_download(
    url: &str,
    out: &Path,
    events: Option<&Sender<WorkerEvent>>,
) -> Result<(), String> {
    let part = PathBuf::from(format!("{}.part", out.display()));
    let _ = fs::remove_file(&part);
    let mut child = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "600",
            "--output",
        ])
        .arg(&part)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Не удалось запустить curl: {e}"))?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let _ = fs::remove_file(&part);
                    return Err(format!("Загрузка не удалась (curl: {status})"));
                }
                break;
            }
            Ok(None) => {
                if let Some(tx) = events {
                    if let Ok(meta) = fs::metadata(&part) {
                        let _ = tx.send(WorkerEvent::Update(UpdateEvent::Progress {
                            received: meta.len(),
                        }));
                    }
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = fs::remove_file(&part);
                return Err(format!("Ошибка ожидания curl: {error}"));
            }
        }
    }

    fs::rename(&part, out).map_err(|e| format!("Не удалось сохранить загруженный файл: {e}"))
}

fn run_ok(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|e| {
        format!(
            "Не удалось запустить {}: {e}",
            command.get_program().to_string_lossy()
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "{} завершился с ошибкой: {}",
            command.get_program().to_string_lossy(),
            stderr.trim()
        ))
    }
}

fn sha256_file(path: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// Вывод `codesign -dv` (печатается в stderr).
fn codesign_report(app: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(app)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    (!text.trim().is_empty()).then_some(text)
}

fn field(report: &str, prefix: &str) -> Option<String> {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .map(|value| value.trim().to_string())
}

/// Подпись, которой подписан бандл (первый `Authority=`).
fn authority(report: &str) -> Option<String> {
    field(report, "Authority=")
}

fn bundle_version(app: &Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim().to_string();
    (!version.is_empty()).then_some(version)
}

fn os_version() -> Option<String> {
    let output = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

// ── JSON ──

fn json_string(dict: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<String> {
    let key = NSString::from_str(key);
    let value = dict.objectForKey(&key)?;
    value.downcast_ref::<NSString>().map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert_eq!(parse_version("v1.2.3-rc1"), [1, 2, 3]);
        assert_eq!(parse_version("2"), [2, 0, 0]);
    }

    #[test]
    fn manifest_parsed_and_validated() {
        let sha = "a".repeat(64);
        let json = format!(
            r#"{{"version":"0.2.0","url":"https://example.com/QuikLight.zip","sha256":"{sha}","min_macos":"13.0"}}"#
        );
        let manifest = parse_manifest(json.as_bytes()).expect("манифест должен разобраться");
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.url, "https://example.com/QuikLight.zip");
        assert_eq!(manifest.min_macos.as_deref(), Some("13.0"));
    }

    #[test]
    fn manifest_rejects_bad_input() {
        assert!(parse_manifest(b"not json").is_none());
        assert!(parse_manifest(br#"{"version":"1.0.0"}"#).is_none());
        // Короткий sha256.
        assert!(
            parse_manifest(br#"{"version":"1.0.0","url":"https://x/y.zip","sha256":"abc"}"#)
                .is_none()
        );
        // Неподдерживаемая схема URL.
        let sha = "b".repeat(64);
        let json = format!(r#"{{"version":"1.0.0","url":"ftp://x/y.zip","sha256":"{sha}"}}"#);
        assert!(parse_manifest(json.as_bytes()).is_none());
    }

    #[test]
    fn install_script_is_valid_shell() {
        let dir =
            std::env::temp_dir().join(format!("quiklight-update-test-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("временный каталог");
        let script = dir.join("apply.sh");
        fs::write(&script, render_install_script()).expect("запись скрипта");
        let status = Command::new("/bin/sh")
            .arg("-n")
            .arg(&script)
            .status()
            .expect("sh -n");
        let _ = fs::remove_dir_all(&dir);
        assert!(
            status.success(),
            "сгенерированный скрипт должен быть валиден"
        );
    }

    #[test]
    fn install_script_waits_and_rolls_back() {
        let script = render_install_script();
        assert!(script.contains("kill -0 \"$PID\""));
        assert!(script.contains("\"$TARGET.bak\"") || script.contains("BACKUP=\"$TARGET.bak\""));
        assert!(script.contains("/usr/bin/ditto"));
        assert!(script.contains("/usr/bin/open"));
        assert!(script.contains("откат"));
    }

    /// Сквозная проверка на локальном фиде (сеть и GitHub не нужны):
    ///
    /// ```text
    /// QUIKLIGHT_UPDATE_FEED=file:///tmp/qkfeed/latest.json \
    ///     cargo test -p quiklight-app e2e_local_feed -- --ignored
    /// ```
    ///
    /// Фид должен указывать на `.zip` с более новой версией `QuikLight.app`;
    /// как его собрать — см. `scripts/release.sh`.
    #[test]
    #[ignore = "нужен локальный фид обновления (QUIKLIGHT_UPDATE_FEED)"]
    fn e2e_local_feed_downloads_and_stages() {
        let (events, _rx) = std::sync::mpsc::channel();
        let manifest = fetch_manifest(&feed_url()).expect("манифест из фида");
        assert!(is_newer(&manifest.version, current_version()));
        let staged = download_and_stage(&manifest, &events).expect("загрузка и проверка");
        assert_eq!(
            bundle_version(&staged).as_deref(),
            Some(manifest.version.as_str()),
            "в распакованном бандле должна быть версия из манифеста"
        );
    }
}
