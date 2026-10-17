//! Bundled fonts (OFL), one family per weight: egui selects faces by family
//! name only. Oswald's weights are static instances of the variable font.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Egui family name, file name, contents.
const FONTS: [(&str, &str, &[u8]); 8] = [
    (
        "oswald-400",
        "Oswald-Regular.ttf",
        include_bytes!("../assets/fonts/Oswald-Regular.ttf"),
    ),
    (
        "oswald-500",
        "Oswald-Medium.ttf",
        include_bytes!("../assets/fonts/Oswald-Medium.ttf"),
    ),
    (
        "oswald-600",
        "Oswald-SemiBold.ttf",
        include_bytes!("../assets/fonts/Oswald-SemiBold.ttf"),
    ),
    (
        "plexmono-400",
        "IBMPlexMono-Regular.ttf",
        include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    ),
    (
        "plexmono-500",
        "IBMPlexMono-Medium.ttf",
        include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
    ),
    (
        "plexsans-400",
        "IBMPlexSans-Regular.ttf",
        include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    ),
    (
        "plexsans-500",
        "IBMPlexSans-Medium.ttf",
        include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    ),
    (
        "plexsans-600",
        "IBMPlexSans-SemiBold.ttf",
        include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    ),
];

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    for (name, _, bytes) in FONTS {
        defs.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
        defs.families
            .insert(FontFamily::Name(name.into()), vec![name.into()]);
    }
    // egui's own widgets (tooltips, device menu) use IBM Plex Sans too.
    defs.families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "plexsans-400".into());
    ctx.set_fonts(defs);
}

/// Folder with the bundled font files, for clients that draw with the
/// app's type (the bar widget). The app embeds its fonts, so they are
/// written to the user's cache (`$XDG_CACHE_HOME/voice-strip/fonts`, else
/// `~/.cache/voice-strip/fonts`) and rewritten when they differ.
/// `VOICE_STRIP_FONTS` overrides the folder.
pub fn files_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("VOICE_STRIP_FONTS") {
        return PathBuf::from(d);
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = cache.join("voice-strip").join("fonts");
    if let Err(e) = export(&dir) {
        eprintln!("voice-strip: cannot write fonts to {}: {e}", dir.display());
    }
    dir
}

/// Writes the embedded fonts into `dir`, replacing files that differ.
fn export(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (_, file, bytes) in FONTS {
        let path = dir.join(file);
        if std::fs::read(&path).is_ok_and(|b| b == bytes) {
            continue;
        }
        // Write then rename, so a reader never sees half a font.
        let tmp = dir.join(format!(".{file}.{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_the_embedded_fonts() {
        let dir = std::env::temp_dir().join(format!("voice-strip-fonts-{}", std::process::id()));
        export(&dir).unwrap();
        for (_, file, bytes) in FONTS {
            assert_eq!(std::fs::read(dir.join(file)).unwrap(), bytes);
        }
        // A damaged file is replaced.
        std::fs::write(dir.join(FONTS[0].1), b"x").unwrap();
        export(&dir).unwrap();
        assert_eq!(std::fs::read(dir.join(FONTS[0].1)).unwrap(), FONTS[0].2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
