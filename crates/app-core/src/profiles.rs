//! Calibrated voice profiles saved per microphone, one small text file each,
//! so a microphone calibrated once stays calibrated across restarts.
//!
//! The file name comes from the device ID (PipeWire node name, Core Audio
//! UID), made safe for the file system, plus a hash of the exact ID so two
//! IDs that differ only in unsafe characters never share a file. The format
//! is `key = value` lines; unknown keys are ignored and a file that does not
//! parse is treated as missing.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::tuning::VoiceProfile;

/// Key used when no specific microphone is selected.
pub const DEFAULT_INPUT: &str = "default";

/// Folder for the profiles: `$XDG_CONFIG_HOME` or `~/.config` on Linux,
/// `~/Library/Application Support` on macOS, then `voice-strip/profiles`.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".config")))?
    };
    Some(base.join("voice-strip").join("profiles"))
}

/// FNV-1a, 64 bit: stable across builds and platforms.
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn file_name(input: &str) -> String {
    let safe: String = input
        .chars()
        .take(80)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{}-{:016x}.profile",
        safe.trim_start_matches('.'),
        hash(input)
    )
}

/// The profile saved for `input` in `dir`, if any.
pub fn load(dir: &Path, input: &str) -> Option<VoiceProfile> {
    let text = std::fs::read_to_string(dir.join(file_name(input))).ok()?;
    parse(&text)
}

/// Saves `profile` for `input` (labelled `label` for people reading the
/// file) in `dir`, replacing any previous one atomically.
pub fn save(dir: &Path, input: &str, label: &str, profile: &VoiceProfile) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(input));
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(format(input, label, profile).as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

fn format(input: &str, label: &str, p: &VoiceProfile) -> String {
    let one_line = |s: &str| s.replace(['\n', '\r'], " ");
    let eq = p.eq_correction_db.map(|v| v.to_string()).join(", ");
    format!(
        "# Voice Strip calibration\n\
         version = 1\n\
         input = {}\n\
         label = {}\n\
         trim_db = {}\n\
         noise_floor_db = {}\n\
         sibilance_hz = {}\n\
         eq_correction_db = {eq}\n",
        one_line(input),
        one_line(label),
        p.trim_db,
        p.noise_floor_db,
        p.sibilance_hz,
    )
}

fn parse(text: &str) -> Option<VoiceProfile> {
    let mut trim = None;
    let mut floor = None;
    let mut sib = None;
    let mut eq = None;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        let num = |v: &str| v.parse::<f32>().ok().filter(|x| x.is_finite());
        match k.trim() {
            "version" if v != "1" => return None,
            "trim_db" => trim = num(v),
            "noise_floor_db" => floor = num(v),
            "sibilance_hz" => sib = num(v),
            "eq_correction_db" => {
                let vals: Vec<f32> = v.split(',').filter_map(|x| num(x.trim())).collect();
                eq = <[f32; 4]>::try_from(vals).ok();
            }
            _ => {}
        }
    }
    Some(VoiceProfile {
        trim_db: trim?,
        noise_floor_db: floor?,
        sibilance_hz: sib?,
        eq_correction_db: eq?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "voice-strip-profiles-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn saved_profile_loads_back_for_the_same_input_only() {
        let dir = tmp_dir("roundtrip");
        let p = VoiceProfile {
            trim_db: 7.5,
            noise_floor_db: -61.25,
            sibilance_hz: 6800.0,
            eq_correction_db: [-1.0, 0.5, 2.0, -0.25],
        };
        let mic = "alsa_input.usb-Focusrite_Vocaster_One_USB-00.HiFi__source";
        save(&dir, mic, "Vocaster One", &p).unwrap();
        assert_eq!(load(&dir, mic), Some(p));
        assert_eq!(load(&dir, "alsa_input.other"), None);
        assert_eq!(load(&dir, DEFAULT_INPUT), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsafe_ids_get_distinct_safe_names() {
        let a = file_name("../a:b");
        let b = file_name("../a/b");
        assert_ne!(a, b);
        for n in [a, b] {
            assert!(!n.contains('/') && !n.starts_with('.'), "{n}");
        }
    }

    #[test]
    fn broken_files_are_ignored() {
        assert_eq!(parse("trim_db = 3\n"), None);
        assert_eq!(
            parse(
                "version = 2\ntrim_db = 1\nnoise_floor_db = -60\nsibilance_hz = 6000\neq_correction_db = 0,0,0,0\n"
            ),
            None
        );
        assert_eq!(
            parse(
                "trim_db = nan\nnoise_floor_db = -60\nsibilance_hz = 6000\neq_correction_db = 0,0,0,0\n"
            ),
            None
        );
    }
}
