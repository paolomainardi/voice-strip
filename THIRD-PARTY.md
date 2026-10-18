# Third-party material

Voice Strip is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). It uses or downloads the following material under its own licence.

| Material | Where | Licence |
|---|---|---|
| IBM Plex Sans and IBM Plex Mono fonts | embedded, `crates/app/assets/fonts/` | SIL Open Font License 1.1 (`OFL-IBM-Plex.txt`) |
| Oswald font | embedded, `crates/app/assets/fonts/` | SIL Open Font License 1.1 (`OFL-Oswald.txt`) |
| DeepFilterNet crate (`deep_filter`) | Cargo git dependency, fork [paolomainardi/DeepFilterNet](https://github.com/paolomainardi/DeepFilterNet) (upstream [Rikorose/DeepFilterNet](https://github.com/Rikorose/DeepFilterNet) v0.5.6 plus the tract upgrade) | MIT OR Apache-2.0 |
| DeepFilterNet3 low-latency model | downloaded on first use from the same commit, not distributed with Voice Strip | MIT OR Apache-2.0 |
| LibriSpeech speech recordings | downloaded by `just fetch-testdata` for measurements only, not distributed | CC BY 4.0 |
| Rust crates | Cargo dependencies, see `Cargo.lock` | permissive licences (MIT, Apache-2.0, BSD, Zlib, ISC, Unicode-3.0 and similar); none requires copyleft |
