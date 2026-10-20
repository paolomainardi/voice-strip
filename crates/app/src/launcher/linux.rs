//! Per-user desktop integration, using assets embedded in the binary.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const ID: &str = "io.github.paolomainardi.voicestrip";
const TEMPLATE: &str =
    include_str!("../../../../extras/linux/io.github.paolomainardi.voicestrip.desktop");
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

const ICONS: &[(&str, &[u8])] = &[
    (
        "16x16/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-16.png"),
    ),
    (
        "32x32/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-32.png"),
    ),
    (
        "48x48/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-48.png"),
    ),
    (
        "64x64/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-64.png"),
    ),
    (
        "128x128/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-128.png"),
    ),
    (
        "256x256/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-256.png"),
    ),
    (
        "512x512/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-512.png"),
    ),
    (
        "1024x1024/apps/io.github.paolomainardi.voicestrip.png",
        include_bytes!("../../../../assets/icon/png/voice-strip-1024.png"),
    ),
    (
        "scalable/apps/io.github.paolomainardi.voicestrip.svg",
        include_bytes!("../../../../assets/icon/voice-strip.svg"),
    ),
];

pub fn cli(args: &[String]) -> i32 {
    let action = match args {
        [action] if matches!(action.as_str(), "install" | "status" | "uninstall") => {
            action.as_str()
        }
        _ => {
            eprintln!("{}", super::USAGE);
            return 2;
        }
    };
    let result = data_dir().and_then(|data| match action {
        "install" => install(&data),
        "uninstall" => uninstall(&data),
        _ => status(&data),
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("voice-strip app: {error}");
            1
        }
    }
}

fn data_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Keep a matching PATH symlink rather than its versioned Cellar target.
fn preferred_executable(own: &Path, path: Option<&OsStr>) -> io::Result<PathBuf> {
    if let Some(path) = path {
        for dir in std::env::split_paths(path) {
            let candidate = dir.join("voice-strip");
            if is_executable(&candidate) && candidate.canonicalize().ok().as_deref() == Some(own) {
                return std::path::absolute(candidate);
            }
        }
    }
    Ok(own.to_path_buf())
}

fn quote_exec(path: &Path) -> io::Result<String> {
    let path = path.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "executable path is not UTF-8")
    })?;
    if path.contains('=') || path.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "desktop executable paths cannot contain = or control characters",
        ));
    }
    // Exec quoting is decoded after the desktop entry's string escapes.
    let mut quoted = String::new();
    for c in path.chars() {
        match c {
            '\\' | '"' | '$' | '`' => quoted.push('\\'),
            '%' => quoted.push('%'),
            _ => {}
        }
        quoted.push(c);
    }
    Ok(format!("\"{}\"", quoted.replace('\\', "\\\\")))
}

/// Read the single executable in our launcher, also accepting the old bare name.
fn exec_path(value: &str) -> Option<PathBuf> {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        decoded.push(if c == '\\' {
            match chars.next()? {
                '\\' => '\\',
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                _ => return None,
            }
        } else {
            c
        });
    }
    let body = if decoded.starts_with('"') {
        decoded.strip_prefix('"')?.strip_suffix('"')?
    } else if decoded.contains(char::is_whitespace) || decoded.contains('"') {
        return None;
    } else {
        &decoded
    };
    let mut path = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let escaped = chars.next()?;
                if !matches!(escaped, '\\' | '"' | '$' | '`') {
                    return None;
                }
                path.push(escaped);
            }
            '%' => {
                if chars.next()? != '%' {
                    return None;
                }
                path.push('%');
            }
            '"' => return None,
            _ => path.push(c),
        }
    }
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn points_to_current(exec: &str, own: &Path, path: Option<&OsStr>) -> bool {
    let Some(target) = exec_path(exec) else {
        return false;
    };
    let target = if target.is_absolute() {
        target
    } else if target == Path::new("voice-strip") {
        let Some(target) = path.and_then(|path| {
            std::env::split_paths(path)
                .map(|dir| dir.join("voice-strip"))
                .find(|p| is_executable(p))
        }) else {
            return false;
        };
        target
    } else {
        return false;
    };
    is_executable(&target) && target.canonicalize().ok().as_deref() == Some(own)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(path.parent().expect("asset has a parent"))?;
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temp)?;
    let result = file.write_all(bytes).and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn refresh_caches(data: &Path) {
    let _ = Command::new("gtk-update-icon-cache")
        .args(["--force", "--ignore-theme-index"])
        .arg(data.join("icons/hicolor"))
        .output();
    let _ = Command::new("update-desktop-database")
        .arg(data.join("applications"))
        .output();
}

fn install(data: &Path) -> io::Result<()> {
    let own = std::env::current_exe()?.canonicalize()?;
    let executable = preferred_executable(&own, std::env::var_os("PATH").as_deref())?;
    let exec = quote_exec(&executable)?;
    let desktop = TEMPLATE.replace("Exec=voice-strip", &format!("Exec={exec}"));
    for (relative, bytes) in ICONS {
        atomic_write(&data.join("icons/hicolor").join(relative), bytes)?;
    }
    let launcher = data.join("applications").join(format!("{ID}.desktop"));
    atomic_write(&launcher, desktop.as_bytes())?;
    refresh_caches(data);
    println!("installed {}", launcher.display());
    Ok(())
}

fn status(data: &Path) -> io::Result<()> {
    let launcher = data.join("applications").join(format!("{ID}.desktop"));
    let desktop = match fs::read_to_string(&launcher) {
        Ok(desktop) => desktop,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("not installed ({})", launcher.display());
            println!("run: voice-strip app install");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let exec = desktop.lines().find_map(|line| line.strip_prefix("Exec="));
    let own = std::env::current_exe()?.canonicalize()?;
    let current =
        exec.is_some_and(|exec| points_to_current(exec, &own, std::env::var_os("PATH").as_deref()));
    println!("installed: {}", launcher.display());
    println!("Exec: {}", exec.unwrap_or("<missing>"));
    if current {
        println!("current: yes");
    } else {
        println!("stale: launcher does not point to this binary");
        println!("run: voice-strip app install");
    }
    Ok(())
}

fn uninstall(data: &Path) -> io::Result<()> {
    let launcher = data.join("applications").join(format!("{ID}.desktop"));
    for path in std::iter::once(launcher).chain(
        ICONS
            .iter()
            .map(|(relative, _)| data.join("icons/hicolor").join(relative)),
    ) {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    refresh_caches(data);
    println!("removed Voice Strip launcher and icons");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "voice-strip-launcher-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn executable(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"fixture").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn exec_escaping_round_trips_reserved_characters() {
        let path = Path::new(r#"/bin with spaces/"quote"/$cash/`tick`/\back/%U"#);
        let quoted = quote_exec(path).unwrap();
        assert_eq!(
            quoted,
            r#""/bin with spaces/\\"quote\\"/\\$cash/\\`tick\\`/\\\\back/%%U""#
        );
        assert_eq!(exec_path(&quoted).as_deref(), Some(path));
        for path in [
            "/bin/plain",
            "/bin/apostrophe'and;punctuation&()",
            "/bin/音声",
        ] {
            assert_eq!(
                exec_path(&quote_exec(Path::new(path)).unwrap()).as_deref(),
                Some(Path::new(path))
            );
        }
    }

    #[test]
    fn exec_rejects_invalid_paths_and_commands() {
        for path in [
            "/bin/a=b",
            "/bin/a\nb",
            "/bin/a\tb",
            "/bin/a\rb",
            "/bin/a\0b",
        ] {
            assert!(quote_exec(Path::new(path)).is_err());
        }
        for value in [
            "",
            "\"/bin/unclosed",
            "\"/bin/voice-strip\" --extra",
            "%U",
            "bad\\escape",
        ] {
            assert!(exec_path(value).is_none(), "accepted {value:?}");
        }
    }

    #[test]
    fn prefers_matching_path_symlink_and_falls_back_to_own_path() {
        let fixture = Fixture::new();
        let own = fixture.executable("Cellar/1.0/bin/voice-strip");
        let wrong = fixture.executable("other/voice-strip");
        let stable = fixture.0.join("bin/voice-strip");
        fs::create_dir(stable.parent().unwrap()).unwrap();
        symlink(&own, &stable).unwrap();
        let path =
            std::env::join_paths([wrong.parent().unwrap(), stable.parent().unwrap()]).unwrap();
        assert_eq!(preferred_executable(&own, Some(&path)).unwrap(), stable);
        let path = std::env::join_paths([wrong.parent().unwrap()]).unwrap();
        assert_eq!(preferred_executable(&own, Some(&path)).unwrap(), own);
        assert_eq!(preferred_executable(&own, None).unwrap(), own);
    }

    #[test]
    fn status_follows_upgraded_symlink_and_detects_stale_targets() {
        let fixture = Fixture::new();
        let old = fixture.executable("Cellar/1.0/voice-strip");
        let new = fixture.executable("Cellar/2.0/voice-strip");
        let stable = fixture.0.join("voice-strip");
        symlink(&old, &stable).unwrap();
        let exec = quote_exec(&stable).unwrap();
        assert!(points_to_current(&exec, &old, None));
        assert!(!points_to_current(&exec, &new, None));
        fs::remove_file(&stable).unwrap();
        symlink(&new, &stable).unwrap();
        assert!(points_to_current(&exec, &new, None));
        assert!(!points_to_current(&quote_exec(&old).unwrap(), &new, None));
        fs::remove_file(&stable).unwrap();
        assert!(!points_to_current(&exec, &new, None));
    }

    #[test]
    fn legacy_bare_name_status_uses_the_first_executable_on_path() {
        let fixture = Fixture::new();
        let own = fixture.executable("own/voice-strip");
        let wrong = fixture.executable("other/voice-strip");
        let path = std::env::join_paths([own.parent().unwrap()]).unwrap();
        assert!(points_to_current("voice-strip", &own, Some(&path)));
        let path = std::env::join_paths([wrong.parent().unwrap(), own.parent().unwrap()]).unwrap();
        assert!(!points_to_current("voice-strip", &own, Some(&path)));
    }

    #[test]
    fn atomic_write_replaces_a_symlink_without_touching_its_target() {
        let fixture = Fixture::new();
        let target = fixture.0.join("unrelated");
        fs::write(&target, b"keep").unwrap();
        let asset = fixture.0.join("asset");
        symlink(&target, &asset).unwrap();
        atomic_write(&asset, b"new").unwrap();
        assert_eq!(fs::read(&asset).unwrap(), b"new");
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);
    }
}
