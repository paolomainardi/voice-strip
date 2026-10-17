# AGENTS.md

## Project Overview

Voice Strip is a desktop application for Linux and macOS that makes a live microphone sound professional (tonal balance, dynamics, loudness, clean-up of speech artifacts) with zero configuration. It reads the physical microphone, runs a fixed nine-stage processing chain, and writes the result to a virtual microphone that any application can select.

The specification is [docs/HANDOFF.md](docs/HANDOFF.md). When in doubt, it wins. Deviations must be discussed before they are implemented. Agreed deviations so far:

- The desktop UI is one egui (eframe) app for Linux and macOS, rendered on the GPU with wgpu (Vulkan on Linux, Metal on macOS). Graphical elements are drawn by the toolkit-neutral `ui-kit` crate through a painter interface; `crates/app` implements it on epaint. GTK was dropped after the egui proof of concept (`docs/egui-poc.md`).
- All UI copy is in English, even where the design sources are in Italian.
- The macOS virtual microphone is written in Rust on the raw Core Audio plug-in interface, with libASPL as a reference only.

**Tech stack:** Rust 2024 edition, 48 kHz mono float32, PipeWire on Linux, egui/eframe with wgpu for the desktop UI.

### Workspace

| Crate | Role |
|---|---|
| `crates/dsp` | Pure processing chain. No I/O, no threads, testable on files |
| `crates/engine` | Realtime core: lock-free channels, audio-thread processor, `AudioBackend` trait, simulated backend |
| `crates/vdev-linux` | PipeWire backend: captures the microphone, exposes the virtual source |
| `crates/vdev-macos-driver` | macOS virtual microphone: AudioServerPlugIn loaded by `coreaudiod`, built and signed by `scripts/macos-driver.sh` |
| `crates/app-core` | Presets, calibration, voice profiles, `Controller`. No UI toolkit code |
| `crates/ui-kit` | Toolkit-neutral widgets, layout and drawing for both views, behind a `Painter` trait |
| `crates/vdev-macos` | macOS backend: captures the microphone, resamples to the driver's clock, feeds the driver's shared ring, headphone monitor |
| `crates/app` | Desktop app (binary `voice-strip`): eframe window hosting `ui-kit` through an epaint painter, tray icon on Linux, menu bar item on macOS |

Design sources for the two views are in `design/` (`Standard.dc.html`, `Main.dc.html`). They describe layout, colors, fonts, and states. The scripts at the bottom of each file are a simulation, not code to reuse.

## Setup

Local development. Required system packages on Arch Linux: `rust`, `clang`, `pipewire`, `just`, plus a Vulkan driver (for example `vulkan-radeon` or `vulkan-intel`).

```bash
just hooks      # Enable the pre-commit hook (runs `just check`)
just check      # Format check, clippy -D warnings, tests
just start      # Run the app in the background
```

Run `just` to list all recipes.

On macOS: the Xcode Command Line Tools, Rust and `just`; `just macos-driver` and `just macos-driver-install` build and install the virtual microphone driver (see the README). Tests that capture audio need the microphone permission and run from Terminal, or through `scripts/macos-gui-run.sh` from SSH.

## Key Conventions

### Realtime rules (non-negotiable)

- In the audio callback and every `process` function: no allocations, no locks, no I/O, no logging, no panics on valid input.
- Tests wrap processing calls with `assert_no_alloc`. A violation fails the test.
- Parameters travel from the UI to the audio thread through lock-free queues (`rtrb`) or atomics, and every parameter is smoothed inside the stage (no clicks when a control moves). Meter data travels to the UI through lock-free queues. The UI never touches the audio thread.
- Every stage declares its latency through `Stage::latency_samples`. Total latency is the sum and is shown in the UI.
- Handle denormals. Use SIMD where it measurably helps.
- EQ is minimum-phase IIR only. Limiter lookahead is 1 to 2 ms. Oversample only for saturation and true-peak detection.

### Latency budget

- Virtual microphone path: full chain including neural denoise, about 25 ms.
- Headphone monitor path: optional, off by default, reduced chain, 64-sample buffers, about 4 ms. When off it adds no processing load.

### Acceptance criteria

Every task closes with a measurable automated test: latency, loudness, true peak, spectral balance, sibilance and plosive reduction, noise floor, CPU, no allocations, and a null test with each module at neutral settings. "Sounds better" is never an automated acceptance criterion.

## Code Style

- **Rust**: `rustfmt` (edition 2024) and `clippy` with `-D warnings`, configured in the workspace `Cargo.toml`.
- Run `just check` before committing. The pre-commit hook runs it.

## Git Workflow

### Commits

Follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>(<scope>): <description>
```

**Types:** `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `ci`, `perf`, `build`.
**Scope** is the crate name when the change is limited to one crate (`dsp`, `engine`, `vdev-linux`, `vdev-macos-driver`, `app-core`, `ui-kit`, `app`, `denoise-neural`).

Keep the description lowercase, imperative, no period.

### Branching

- `main` is the default branch. Every change reaches it through a pull request.
- Branch naming: `feat/`, `fix/`, `chore/`, `test/`, `docs/` prefix plus a kebab-case description (for example `feat/dsp-limiter`).
- **Never push directly to `main`.**

### Rebasing

- Rebase onto `main` before pushing. No merge commits on feature branches.
- Use `--force-with-lease` (never `--force`) after rebasing a feature branch.

## Package Management

### Rust (cargo)

- Add: `cargo add -p <crate> <dependency>`
- Dev: `cargo add -p <crate> --dev <dependency>`
- Shared versions go in `[workspace.dependencies]` in the root `Cargo.toml`.
- `Cargo.lock` is committed.

### Dependency Safety

Before adding or upgrading any dependency, follow these rules:

1. **Never assume you know the latest version.** Your training data is outdated. Always verify against the live registry before adding or upgrading any package.

2. **Check the live registry:**

```bash
curl -s "https://crates.io/api/v1/crates/<crate>" | jq '{max_stable: .crate.max_stable_version, updated: .crate.updated_at}'
```

3. **Use the newest stable major version** compatible with the project toolchain.

4. **Avoid releases published within the last 5 days** to reduce supply chain attack risk.

5. **Always regenerate the lockfile** after changing dependency manifests.

6. Voice Strip is MIT OR Apache-2.0. Dependencies must allow that: prefer MIT, Apache-2.0, BSD, Zlib; copyleft-only licences (GPL, LGPL, MPL without a permissive alternative) need an explicit decision. Record bundled or downloaded third-party material in `THIRD-PARTY.md`.
7. Do not vendor code or models. Large assets are downloaded at runtime and verified by checksum, as the noise removal model is (`crates/denoise-neural/src/model.rs`).

## Testing

`just test` runs all unit and integration tests.

- DSP tests live in `crates/dsp/tests/` and in `#[cfg(test)]` modules. They run on synthetic signals and on licensed voice recordings.
- Engine tests verify realtime safety with `assert_no_alloc`.
- Tests that need a running PipeWire server are marked `#[ignore]` and run with `cargo test -- --ignored`.

## CI/CD

GitHub Actions, `.github/workflows/check.yml`: `just check` on Ubuntu 24.04 and macOS 15, for pushes to `main` and every pull request. Actions are pinned to full commit SHAs; update them with the Dependency Safety rules above.

## Command Safety

### Safe (run autonomously)

- `just check`, `just test`, `just fmt`, `just run`
- `cargo build`, `cargo test`, `cargo clippy`
- `git status`, `git log`, `git diff`, `pw-cli ls`, `pw-dump`
- Adding a dependency after the Dependency Safety checks above
- `git push` of feature branches

### Dangerous (ask user first)

- `git push` to `main`
- Changing PipeWire or system audio configuration outside the app

### Destructive (never run)

- `git push --force` to `main`
- `rm -rf` outside `target/`
- Stopping or restarting the user's PipeWire or WirePlumber services

## Important Rules

- `docs/HANDOFF.md` wins on any doubt. Discuss deviations before implementing them.
- No allocations, locks, I/O or logging on the audio thread. Tests enforce it with `assert_no_alloc`.
- Every stage declares its latency and ships with a measurable test and a null test.
- Verify crate versions on crates.io before adding them.
- Run `just check` before committing. Follow Conventional Commits.
- UI copy is English. The GUI must match the design in `design/` exactly.
