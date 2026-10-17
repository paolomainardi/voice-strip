//! Where the DeepFilterNet3 low-latency model comes from. It is not part of
//! the repository or the binary:
//!
//! 1. `VOICE_STRIP_MODEL` names a model file the user supplies, used as is;
//! 2. otherwise the copy in the user's cache directory, once its SHA-256
//!    matches [`SHA256`].
//!
//! Nothing is downloaded without being asked: a [`Download`] fetches the
//! model from the upstream repository at the pinned commit ([`URL`]) with
//! `curl`, checks it, and keeps it in the cache. The app starts one when the
//! user agrees to it; `just fetch-model` does it from the command line.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The model at DeepFilterNet tag v0.5.6, the commit the `deep_filter`
/// dependency is pinned to.
pub const URL: &str = "https://raw.githubusercontent.com/Rikorose/DeepFilterNet/978576aa8400552a4ce9730838c635aa30db5e61/models/DeepFilterNet3_ll_onnx.tar.gz";
pub const SHA256: &str = "5998e58e8ba0e09bb76986ef97b84afa065a571ef282d4a1222f341e3251cf3a";
/// Size of the model file, bytes.
pub const SIZE: u64 = 36_359_660;
/// Where the model comes from, as shown to the user.
pub const SOURCE: &str = "github.com/Rikorose/DeepFilterNet";
pub const LICENCE: &str = "MIT or Apache-2.0";
const FILE: &str = "DeepFilterNet3_ll_onnx.tar.gz";
/// Environment variable naming a model file to use instead.
pub const ENV: &str = "VOICE_STRIP_MODEL";

/// The cached model's path: `$XDG_CACHE_HOME` or `~/.cache` on Linux,
/// `~/Library/Caches` on macOS, then `voice-strip/`.
pub fn cache_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".cache")))
    }?;
    Some(base.join("voice-strip").join(FILE))
}

/// Whether a model is there to load, without reading it: the user's file
/// or the cached copy. [`locate`] still checks the copy when loading.
pub fn present() -> bool {
    match std::env::var_os(ENV) {
        Some(p) => Path::new(&p).is_file(),
        None => cache_path().is_some_and(|p| p.is_file()),
    }
}

/// The model file to load. A cached copy whose SHA-256 does not match is
/// removed, so the next download replaces it.
pub fn locate() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os(ENV) {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("{ENV}={} is not a file", p.display()))
        };
    }
    let path = cache_path().ok_or("no cache directory (HOME is not set)")?;
    if !path.is_file() {
        return Err("the noise removal model is not downloaded".into());
    }
    if sha256(&path)? != SHA256 {
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "{} was damaged and has been removed; download it again",
            path.display()
        ));
    }
    Ok(path)
}

/// A model download running on its own thread. Dropping it cancels it.
pub struct Download {
    bytes: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<PathBuf, String>>,
}

impl Download {
    /// Starts downloading the model into the cache.
    pub fn start() -> Result<Self, String> {
        let path = cache_path().ok_or("no cache directory (HOME is not set)")?;
        let bytes = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, done) = std::sync::mpsc::channel();
        let (b, c) = (bytes.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("model-download".into())
            .spawn(move || {
                let _ = tx.send(fetch(&path, &b, &c).map(|()| path));
            })
            .map_err(|e| format!("cannot start the download: {e}"))?;
        Ok(Self {
            bytes,
            cancel,
            done,
        })
    }

    /// Bytes received so far.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// The outcome once the download has ended: the model's path, or why
    /// it failed.
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        match self.done.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the download stopped".into())),
        }
    }

    /// Waits for the outcome.
    pub fn wait(self) -> Result<PathBuf, String> {
        self.done
            .recv()
            .unwrap_or_else(|_| Err("the download stopped".into()))
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Downloads the model to `path`, keeping it only if its SHA-256 matches.
/// Reports the bytes received in `bytes`; stops when `cancel` is set.
fn fetch(path: &Path, bytes: &AtomicU64, cancel: &AtomicBool) -> Result<(), String> {
    let dir = path.parent().ok_or("model path has no directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = path.with_extension("part");
    let mut child = std::process::Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "2",
            "--output",
        ])
        .arg(&part)
        .arg(URL)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run curl to download the model: {e}"))?;
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&part);
            return Err("download cancelled".into());
        }
        if let Some(status) = child.try_wait().map_err(|e| format!("curl: {e}"))? {
            break status;
        }
        if let Ok(m) = std::fs::metadata(&part) {
            bytes.store(m.len(), Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let _ = std::fs::remove_file(&part);
        let err = err.trim().trim_start_matches("curl: ");
        return Err(if err.is_empty() {
            format!("model download failed ({status})")
        } else {
            format!("model download failed: {err}")
        });
    }
    let got = sha256(&part)?;
    if got != SHA256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "downloaded model has SHA-256 {got}, expected {SHA256}"
        ));
    }
    // A cancel that came while curl was finishing or the file was checked.
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&part);
        return Err("download cancelled".into());
    }
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_a_known_digest() {
        let dir = std::env::temp_dir().join(format!("vs-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("abc");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_cache_path_ends_in_the_model_file() {
        if let Some(p) = cache_path() {
            assert!(p.ends_with("voice-strip/DeepFilterNet3_ll_onnx.tar.gz"));
        }
    }
}
