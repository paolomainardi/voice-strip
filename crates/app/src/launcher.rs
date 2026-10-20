//! `voice-strip app status | install | uninstall`: per-user desktop integration.
//! On macOS, the app lives in the user's Applications folder, so Spotlight,
//! Launchpad and Finder can open it.
//!
//! The app is the `Voice Strip.app` bundle that holds this binary: Homebrew's
//! `Voice Strip.app`, or `target/macos.noindex/Voice Strip.app` in a clone
//! (`just macos-app`). Installing copies it to `~/Applications`. The copy is
//! not updated by `brew upgrade`, so the command is run again after one.
//! Linux installs its desktop launcher and icons from embedded assets.

use std::path::{Path, PathBuf};

const USAGE: &str = "usage: voice-strip app status | install | uninstall";

/// The `.app` bundle that holds `exe`, laid out as `X.app/Contents/MacOS/<exe>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn enclosing_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::cli;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn cli(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("status" | "install" | "uninstall") => {
            println!("App installation is supported on Linux and macOS.");
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::cli;

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{USAGE, enclosing_bundle};

    const NAME: &str = "Voice Strip.app";

    pub fn cli(args: &[String]) -> i32 {
        match args {
            [a] if a == "status" => status(),
            [a] if a == "install" => install(),
            [a] if a == "uninstall" => uninstall(),
            _ => {
                eprintln!("{USAGE}");
                2
            }
        }
    }

    /// Where the app is installed: `~/Applications/Voice Strip.app`.
    fn installed() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(Path::new(&home).join("Applications").join(NAME))
    }

    /// The bundle this binary runs from, if any.
    fn current() -> Option<PathBuf> {
        enclosing_bundle(&std::env::current_exe().ok()?.canonicalize().ok()?)
    }

    fn version(bundle: &Path) -> Option<String> {
        let out = Command::new("/usr/libexec/PlistBuddy")
            .arg("-c")
            .arg("Print :CFBundleShortVersionString")
            .arg(bundle.join("Contents/Info.plist"))
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn status() -> i32 {
        let Some(dst) = installed() else {
            eprintln!("voice-strip: HOME is not set");
            return 1;
        };
        let shipped = current().and_then(|b| version(&b));
        match version(&dst) {
            None => println!("not installed ({})", dst.display()),
            Some(v) => {
                println!("installed: {} (version {v})", dst.display());
                if shipped.as_ref().is_some_and(|s| *s != v) {
                    println!("outdated: this build is {}", shipped.unwrap_or_default());
                    println!("run: voice-strip app install");
                }
            }
        }
        0
    }

    fn install() -> i32 {
        let Some(dst) = installed() else {
            eprintln!("voice-strip: HOME is not set");
            return 1;
        };
        let Some(src) = current() else {
            eprintln!(
                "voice-strip: not running from a {NAME} bundle. In a clone, run `just macos-app`, \
then `\"target/macos.noindex/{NAME}/Contents/MacOS/voice-strip\" app install`."
            );
            return 1;
        };
        if src.canonicalize().ok() == dst.canonicalize().ok() {
            println!("already running from {}", dst.display());
            return 0;
        }
        if let Some(dir) = dst.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("voice-strip: cannot create {}: {e}", dir.display());
            return 1;
        }
        if dst.exists()
            && let Err(e) = std::fs::remove_dir_all(&dst)
        {
            eprintln!("voice-strip: cannot replace {}: {e}", dst.display());
            return 1;
        }
        // `ditto` keeps the bundle's code signature and extended attributes.
        let copied = Command::new("/usr/bin/ditto")
            .arg(&src)
            .arg(&dst)
            .status()
            .is_ok_and(|s| s.success());
        let valid = copied
            && Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict"])
                .arg(&dst)
                .status()
                .is_ok_and(|s| s.success());
        if !valid {
            eprintln!("voice-strip: could not install {}", dst.display());
            return 1;
        }
        // Lets Spotlight list the app right away instead of at its next scan.
        let _ = Command::new("/usr/bin/mdimport").arg(&dst).status();
        println!("installed {}", dst.display());
        println!("Open Voice Strip from Spotlight or Launchpad. Run this again after an upgrade.");
        0
    }

    fn uninstall() -> i32 {
        let Some(dst) = installed() else {
            eprintln!("voice-strip: HOME is not set");
            return 1;
        };
        if !dst.exists() {
            println!("not installed");
            return 0;
        }
        match std::fs::remove_dir_all(&dst) {
            Ok(()) => {
                println!("removed {}", dst.display());
                0
            }
            Err(e) => {
                eprintln!("voice-strip: cannot remove {}: {e}", dst.display());
                1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::enclosing_bundle;
    use std::path::Path;

    #[test]
    fn finds_the_bundle_around_the_executable() {
        let exe = Path::new(
            "/opt/homebrew/Cellar/voice-strip/0.6.0/Voice Strip.app/Contents/MacOS/voice-strip",
        );
        assert_eq!(
            enclosing_bundle(exe).as_deref(),
            Some(Path::new(
                "/opt/homebrew/Cellar/voice-strip/0.6.0/Voice Strip.app"
            ))
        );
    }

    #[test]
    fn a_bare_binary_is_not_in_a_bundle() {
        assert_eq!(
            enclosing_bundle(Path::new("/clone/target/release/voice-strip")),
            None
        );
        assert_eq!(
            enclosing_bundle(Path::new("/x/Voice Strip.app/Contents/Other/voice-strip")),
            None
        );
        assert_eq!(
            enclosing_bundle(Path::new("/x/folder/Contents/MacOS/voice-strip")),
            None
        );
    }
}
