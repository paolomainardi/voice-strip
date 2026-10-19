//! `voice-strip driver status | install [--if-needed] | uninstall`: the
//! macOS virtual microphone driver, managed from the app's own command.
//!
//! The driver to install is the bundle shipped with this binary: Homebrew's
//! `libexec/VoiceStrip.driver`, or `target/VoiceStrip.driver` in a clone
//! (`just macos-driver`). Its `Info.plist` carries the release version and
//! `VoiceStripDriverRevision`, a hash of the driver's sources: an installed
//! driver needs replacing only when the revisions differ.
//!
//! Installing copies the bundle into `/Library/Audio/Plug-Ins/HAL` and
//! restarts Core Audio, as root: through `sudo` in a terminal, otherwise
//! through the macOS administrator prompt. A running Voice Strip is stopped
//! first, so it is not attached to the device while Core Audio restarts.
//! With `VOICE_STRIP_TEAM` set, only a bundle signed by that team installs.
//! Linux needs no driver.

const USAGE: &str = "usage: voice-strip driver status | install [--if-needed] | uninstall";

#[cfg(not(target_os = "macos"))]
pub fn cli(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("status" | "install" | "uninstall") => {
            println!(
                "Linux needs no driver: Voice Strip creates its virtual microphone through PipeWire."
            );
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

/// Notice for the app's footer when the driver is missing or outdated.
#[cfg(not(target_os = "macos"))]
pub fn notice() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
pub use mac::{cli, notice};

#[cfg(target_os = "macos")]
mod mac {
    use std::io::IsTerminal;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::USAGE;

    const HAL: &str = "/Library/Audio/Plug-Ins/HAL";
    const NAME: &str = "VoiceStrip.driver";
    /// `pgrep -f` pattern (an extended regex) for the driver's helper process.
    const HELPER: &str = r"Core Audio Driver \(VoiceStrip\.driver\)";

    fn installed() -> PathBuf {
        Path::new(HAL).join(NAME)
    }

    /// The driver shipped with this binary, if any.
    fn shipped() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("VOICE_STRIP_DRIVER") {
            return Some(PathBuf::from(p));
        }
        let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
        let dir = exe.parent()?;
        [
            dir.join("../libexec").join(NAME),
            dir.join("..").join(NAME),
            dir.join(NAME),
        ]
        .into_iter()
        .find(|p| p.join("Contents/Info.plist").is_file())
    }

    fn plist(bundle: &Path, key: &str) -> Option<String> {
        let out = Command::new("/usr/libexec/PlistBuddy")
            .arg("-c")
            .arg(format!("Print :{key}"))
            .arg(bundle.join("Contents/Info.plist"))
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    struct Info {
        version: String,
        revision: Option<String>,
    }

    fn info(bundle: &Path) -> Option<Info> {
        bundle.join("Contents/Info.plist").is_file().then(|| Info {
            version: plist(bundle, "CFBundleShortVersionString").unwrap_or_default(),
            revision: plist(bundle, "VoiceStripDriverRevision"),
        })
    }

    fn helper_pid() -> Option<String> {
        let out = Command::new("pgrep").args(["-f", HELPER]).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines().next().map(str::to_owned)
    }

    /// Whether the installed driver differs from the shipped one; `None`
    /// when either is missing.
    fn outdated(installed: &Info, shipped: &Info) -> bool {
        match (&installed.revision, &shipped.revision) {
            (Some(a), Some(b)) => a != b,
            // An older driver had no revision: replace it.
            (None, Some(_)) => true,
            _ => installed.version != shipped.version,
        }
    }

    pub fn notice() -> Option<String> {
        let shipped = shipped().and_then(|p| info(&p))?;
        match info(&installed()) {
            None => Some("Install the driver: voice-strip driver install".into()),
            Some(i) if outdated(&i, &shipped) => {
                Some("Driver update needed: voice-strip driver install".into())
            }
            Some(_) => None,
        }
    }

    pub fn cli(args: &[String]) -> i32 {
        match (
            args.first().map(String::as_str),
            args.get(1).map(String::as_str),
        ) {
            (Some("status"), None) => status(),
            (Some("install"), None) => install(false),
            (Some("install"), Some("--if-needed")) => install(true),
            (Some("uninstall"), None) => uninstall(),
            _ => {
                eprintln!("{USAGE}");
                2
            }
        }
    }

    fn describe(i: &Info) -> String {
        match &i.revision {
            Some(r) => format!("{} (revision {r})", i.version),
            None => i.version.clone(),
        }
    }

    fn status() -> i32 {
        let inst = info(&installed());
        let ship = shipped().and_then(|p| info(&p));
        match &inst {
            Some(i) => println!("installed: {} in {HAL}", describe(i)),
            None => println!("installed: no"),
        }
        match &ship {
            Some(s) => println!("shipped:   {}", describe(s)),
            None => println!("shipped:   none next to this binary (just macos-driver builds one)"),
        }
        println!(
            "loaded:    {}",
            if helper_pid().is_some() { "yes" } else { "no" }
        );
        match (&inst, &ship) {
            (None, Some(_)) => {
                println!("run: voice-strip driver install");
                1
            }
            (Some(i), Some(s)) if outdated(i, s) => {
                println!("update needed: voice-strip driver install");
                1
            }
            (None, None) => 1,
            _ => 0,
        }
    }

    /// Quotes for `sh -c`.
    fn quote(p: &Path) -> String {
        format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"))
    }

    /// Runs `script` as root: `sudo` in a terminal, the macOS administrator
    /// prompt otherwise.
    fn as_root(script: &str) -> bool {
        let status = if std::io::stdin().is_terminal() {
            Command::new("sudo")
                .args(["/bin/sh", "-c", script])
                .status()
        } else {
            let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
            Command::new("osascript")
                .arg("-e")
                .arg(format!(
                    "do shell script \"{escaped}\" with administrator privileges"
                ))
                .status()
        };
        status.is_ok_and(|s| s.success())
    }

    /// Asks for the password before anything stops: `sudo` caches it, so a
    /// refused password leaves the running app alone. The macOS prompt asks
    /// when the script runs.
    fn authorize() -> bool {
        !std::io::stdin().is_terminal()
            || Command::new("sudo")
                .arg("-v")
                .status()
                .is_ok_and(|s| s.success())
    }

    /// Other running Voice Strip processes, not this one.
    fn others() -> Vec<String> {
        let me = std::process::id().to_string();
        Command::new("pgrep")
            .args(["-x", "voice-strip"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|p| p.trim().to_owned())
                    .filter(|p| *p != me)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn wait_gone(secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !others().is_empty() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// Stops a running Voice Strip and waits for it: an app attached to the
    /// device while coreaudiod restarts can get stuck reconnecting and keep
    /// coreaudiod busy. `Some(true)` when one was running, `None` when it
    /// did not stop.
    fn stop_app() -> Option<bool> {
        if others().is_empty() {
            return Some(false);
        }
        println!("stopping Voice Strip while the driver changes");
        let _ = app_core::remote::client::request("quit");
        if wait_gone(5) {
            return Some(true);
        }
        let _ = Command::new("kill").args(others()).status();
        if wait_gone(5) {
            return Some(true);
        }
        eprintln!("voice-strip driver: Voice Strip did not stop; quit it and retry");
        None
    }

    /// Checks the bundle's signature, and its team when `VOICE_STRIP_TEAM`
    /// is set.
    fn verify(bundle: &Path) -> bool {
        let valid = Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(bundle)
            .status()
            .is_ok_and(|s| s.success());
        if !valid {
            eprintln!(
                "voice-strip driver: {} has no valid signature",
                bundle.display()
            );
            return false;
        }
        let Some(want) = std::env::var("VOICE_STRIP_TEAM")
            .ok()
            .filter(|t| !t.is_empty())
        else {
            return true;
        };
        let team = Command::new("codesign")
            .args(["-dvv"])
            .arg(bundle)
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .find_map(|l| l.strip_prefix("TeamIdentifier=").map(str::to_owned))
            });
        if team.as_deref() != Some(want.as_str()) {
            eprintln!(
                "voice-strip driver: {} is not signed by team {want} (found: {})",
                bundle.display(),
                team.as_deref().unwrap_or("none")
            );
            return false;
        }
        true
    }

    /// Waits for a new driver helper and for Core Audio to list Voice Strip,
    /// so nothing starts while every client reconnects.
    fn settle(old_helper: Option<String>) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut loaded = false;
        while Instant::now() < deadline {
            if !loaded {
                loaded = helper_pid().is_some_and(|p| Some(&p) != old_helper.as_ref());
            } else if vdev_macos::hal::find(vdev_macos_driver::model::DEVICE_UID)
                .ok()
                .flatten()
                .is_some()
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        false
    }

    fn install(if_needed: bool) -> i32 {
        let Some(src) = shipped() else {
            eprintln!(
                "voice-strip driver: no driver next to this binary (just macos-driver builds one)"
            );
            return 1;
        };
        let Some(ship) = info(&src) else {
            eprintln!(
                "voice-strip driver: {} is not a driver bundle",
                src.display()
            );
            return 1;
        };
        if if_needed && info(&installed()).is_some_and(|i| !outdated(&i, &ship)) {
            println!("driver {} already installed", describe(&ship));
            return 0;
        }
        if !verify(&src) {
            return 1;
        }
        println!(
            "installing driver {} (asks for your password; audio stops for a moment)",
            describe(&ship)
        );
        if !authorize() {
            eprintln!("voice-strip driver: not installed");
            return 1;
        }
        let Some(was_running) = stop_app() else {
            return 1;
        };
        let restart_hint = || {
            if was_running {
                println!("start Voice Strip again: voice-strip");
            }
        };
        let old = helper_pid();
        // As root: copy the bundle into a private directory (in sticky
        // /var/tmp, not the caller's TMPDIR), check that copy
        // (the user's tree could change after `verify`), then swap it in, so
        // a failure leaves the installed driver as it was.
        let team = std::env::var("VOICE_STRIP_TEAM").unwrap_or_default();
        let team_check = if team.is_empty() {
            String::new()
        } else {
            format!(
                " && /usr/bin/codesign -dvv \"$t/{NAME}\" 2>&1 | grep -qx {}",
                quote(Path::new(&format!("TeamIdentifier={team}")))
            )
        };
        let script = format!(
            "t=$(mktemp -d /var/tmp/voice-strip.XXXXXX) && trap 'rm -rf \"$t\"' EXIT && cp -R {src} \"$t/\" \\
             && /usr/bin/codesign --verify --strict \"$t/{NAME}\"{team_check} \\
             && rm -rf {dst} && mv \"$t/{NAME}\" {hal}/ && killall coreaudiod",
            src = quote(&src),
            dst = quote(&installed()),
            hal = quote(Path::new(HAL)),
        );
        if !as_root(&script) {
            eprintln!("voice-strip driver: not installed");
            restart_hint();
            return 1;
        }
        if !settle(old) {
            eprintln!("voice-strip driver: installed, but Core Audio has not loaded it yet");
            restart_hint();
            return 1;
        }
        println!("driver installed and loaded");
        restart_hint();
        0
    }

    fn uninstall() -> i32 {
        if info(&installed()).is_none() {
            println!("driver not installed");
            return 0;
        }
        println!("removing the driver (asks for your password; audio stops for a moment)");
        if !authorize() || stop_app().is_none() {
            eprintln!("voice-strip driver: not removed");
            return 1;
        }
        let script = format!("rm -rf {} && killall coreaudiod", quote(&installed()));
        if as_root(&script) {
            println!("driver removed");
            0
        } else {
            eprintln!("voice-strip driver: not removed");
            1
        }
    }
}
