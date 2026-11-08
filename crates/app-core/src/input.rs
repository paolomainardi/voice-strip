//! Remembered microphone selection. Called by the UI, never the audio thread.
use std::path::Path;

pub fn load(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("input")).ok()?;
    let id = text.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

pub fn save(dir: &Path, input: Option<&str>) -> std::io::Result<()> {
    let file = dir.join("input");
    if let Some(id) = input {
        std::fs::create_dir_all(dir)?;
        std::fs::write(file, format!("{id}\n"))
    } else {
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_choice_and_default_removes_it() {
        let dir = std::env::temp_dir().join(format!("vs-input-{}", std::process::id()));
        assert_eq!(load(&dir), None);
        save(&dir, Some("alsa_input.usb-mic")).unwrap();
        assert_eq!(load(&dir).as_deref(), Some("alsa_input.usb-mic"));
        save(&dir, Some("macOS-device-UID")).unwrap();
        assert_eq!(load(&dir).as_deref(), Some("macOS-device-UID"));
        save(&dir, None).unwrap();
        save(&dir, None).unwrap();
        assert!(!dir.join("input").exists());
        assert_eq!(load(&dir), None);
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn missing_and_empty_files_mean_default() {
        let dir = std::env::temp_dir().join(format!("vs-input-empty-{}", std::process::id()));
        assert_eq!(load(&dir), None);
        save(&dir, Some(" \n")).unwrap();
        assert_eq!(load(&dir), None);
        save(&dir, Some("")).unwrap();
        assert_eq!(load(&dir), None);
        save(&dir, None).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
