//! Update check: once a day, asks GitHub for the latest release and tells
//! whether it is newer than the running version. It sends nothing but the
//! request itself (your IP address and a `curl` user agent), and never
//! downloads or installs anything.
//!
//! It is on by default and off when `VOICE_STRIP_NO_UPDATE_CHECK` is set or
//! the `no-update-check` file exists in the settings folder
//! (`voice-strip ctl updates off` creates it).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// Where the latest release is announced.
pub const RELEASES_API: &str =
    "https://api.github.com/repos/paolomainardi/voice-strip/releases/latest";
/// The page a person reads to upgrade.
pub const RELEASES_PAGE: &str = "https://github.com/paolomainardi/voice-strip/releases/latest";
/// How often the running app checks.
pub const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const ENV_OFF: &str = "VOICE_STRIP_NO_UPDATE_CHECK";
const OFF_FILE: &str = "no-update-check";

/// The running version.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Whether checks are allowed with settings in `dir`.
pub fn enabled(dir: &Path) -> bool {
    std::env::var_os(ENV_OFF).is_none() && !dir.join(OFF_FILE).exists()
}

/// Turns checks on or off for settings in `dir`, persistently.
pub fn set_enabled(dir: &Path, on: bool) -> std::io::Result<()> {
    let file = dir.join(OFF_FILE);
    if on {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            file,
            "Voice Strip does not check for updates while this file exists.\n",
        )
    }
}

/// How this copy is upgraded, from where its binary lives.
pub fn how_to_upgrade() -> &'static str {
    let exe = std::env::current_exe().unwrap_or_default();
    let path = exe.to_string_lossy();
    if path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("/linuxbrew/") {
        "run `brew upgrade voice-strip`"
    } else if path.contains("/target/release/") || path.contains("/target/debug/") {
        "run `just upgrade` in your clone"
    } else {
        "download it from the release page"
    }
}

/// Settings folder next to the saved calibrations.
pub fn settings_dir() -> Option<PathBuf> {
    crate::profiles::default_dir()?
        .parent()
        .map(Path::to_path_buf)
}

/// `major.minor.patch` from a version or a `v`-prefixed tag.
fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let n = |s: Option<&str>| s?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok();
    Some((n(it.next())?, n(it.next())?, n(it.next())?))
}

/// The latest version when it is newer than `current`.
pub fn newer(latest_tag: &str, current: &str) -> Option<String> {
    let (l, c) = (parse(latest_tag)?, parse(current)?);
    (l > c).then(|| format!("{}.{}.{}", l.0, l.1, l.2))
}

/// `tag_name` from the release JSON, without a JSON parser.
fn tag_name(json: &str) -> Option<&str> {
    let rest = &json[json.find("\"tag_name\"")? + "\"tag_name\"".len()..];
    let rest = &rest[rest.find('"')? + 1..];
    Some(&rest[..rest.find('"')?])
}

/// Checks on a background thread: the newer version, `None` when up to
/// date, or an error message.
pub fn spawn_check() -> Receiver<Result<Option<String>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let out = Command::new("curl")
                .args([
                    "-fsSL",
                    "--max-time",
                    "15",
                    "-H",
                    "Accept: application/vnd.github+json",
                ])
                .arg(RELEASES_API)
                .output();
            let result = match out {
                Ok(o) if o.status.success() => {
                    let body = String::from_utf8_lossy(&o.stdout);
                    tag_name(&body)
                        .map(|t| newer(t, current()))
                        .ok_or_else(|| "no release found".to_string())
                }
                Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
    if let Err(e) = spawned {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(Err(e.to_string()));
        return rx;
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_newer_releases_count() {
        assert_eq!(newer("v0.3.2", "0.3.1").as_deref(), Some("0.3.2"));
        assert_eq!(newer("v0.10.0", "0.9.9").as_deref(), Some("0.10.0"));
        assert_eq!(newer("v0.3.1", "0.3.1"), None);
        assert_eq!(newer("v0.3.0", "0.3.1"), None);
        assert_eq!(newer("nightly", "0.3.1"), None);
    }

    #[test]
    fn reads_the_tag_from_the_release() {
        let json = r#"{"url":"x","tag_name": "v0.4.0","name":"Voice Strip 0.4.0"}"#;
        assert_eq!(tag_name(json), Some("v0.4.0"));
        assert_eq!(tag_name("{}"), None);
    }

    #[test]
    fn the_off_switch_is_saved() {
        let dir = std::env::temp_dir().join(format!("voice-strip-updates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if std::env::var_os(ENV_OFF).is_none() {
            assert!(enabled(&dir));
        }
        set_enabled(&dir, false).unwrap();
        assert!(!enabled(&dir));
        set_enabled(&dir, true).unwrap();
        set_enabled(&dir, true).unwrap();
        if std::env::var_os(ENV_OFF).is_none() {
            assert!(enabled(&dir));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
