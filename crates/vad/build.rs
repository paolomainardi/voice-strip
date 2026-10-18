//! Fetches the Silero VAD model at a pinned commit and checks its SHA-256,
//! so `src/lib.rs` can embed it. The model is not stored in this
//! repository. Offline builds can point `VOICE_STRIP_VAD_MODEL` at a copy
//! of the file; it is checked the same way.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const URL: &str = "https://raw.githubusercontent.com/snakers4/silero-vad/5cd7945676eb32225748052e2e6a0580e4686a08/src/silero_vad/data/silero_vad_op18_ifless.onnx";
const SHA256: &str = "7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28";
const ENV: &str = "VOICE_STRIP_VAD_MODEL";

fn sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={ENV}");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("silero_vad.onnx");
    if sha256(&out).as_deref() == Some(SHA256) {
        return;
    }
    let part = out.with_extension("part");
    match std::env::var_os(ENV) {
        Some(src) => {
            std::fs::copy(&src, &part)
                .unwrap_or_else(|e| panic!("{ENV}={}: {e}", Path::new(&src).display()));
        }
        None => {
            let ok = Command::new("curl")
                .args(["-fsSL", "--retry", "3", "-o"])
                .arg(&part)
                .arg(URL)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                panic!(
                    "could not download the Silero VAD model from {URL}. \
                     Download it yourself and set {ENV} to its path."
                );
            }
        }
    }
    let got = sha256(&part).unwrap_or_default();
    if got != SHA256 {
        let _ = std::fs::remove_file(&part);
        panic!("Silero VAD model checksum mismatch: expected {SHA256}, got {got}");
    }
    std::fs::rename(&part, &out).expect("store the Silero VAD model");
}
