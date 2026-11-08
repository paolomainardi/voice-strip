//! Run only through scripts/e2e/run.sh, never against the desktop graph.
#![cfg(target_os = "linux")]
mod common;

use common::*;
use engine::RunningBackend;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vdev_linux::{PipeWireBackend, VIRTUAL_MIC_NAME};

fn default_source(name: &str) {
    assert!(
        std::env::var_os("PIPEWIRE_RUNTIME_DIR").is_some(),
        "private server required"
    );
    for key in ["default.configured.audio.source", "default.audio.source"] {
        let result = std::process::Command::new("pw-metadata")
            .args([
                "-n",
                "default",
                "0",
                key,
                &format!(r#"{{"name":"{name}"}}"#),
            ])
            .output()
            .expect("pw-metadata");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

fn wait_for(stage: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("input selection did not settle: {stage}");
}

#[test]
#[ignore = "needs a private PipeWire server: scripts/e2e/run.sh"]
fn self_default_keeps_capture_and_falls_back_at_start() {
    let _nodes = NullNodes::start(480);
    default_source(TEST_SRC);
    let (_handle, processor) = engine::new_engine();
    let running = PipeWireBackend::new()
        .start_pipewire(processor, None, None)
        .unwrap();
    assert!(running.wait_linked(false, Duration::from_secs(5)));
    wait_for("initial real default", || {
        running
            .input_selection()
            .is_some_and(|s| s.device.id == TEST_SRC)
    });
    let rec = Arc::new(Mutex::new(Rec::with_capacity(10)));
    let player = Arc::new(Mutex::new(Player {
        signal: (0..480_000)
            .map(|n| 0.1 * (n as f32 * std::f32::consts::TAU * 440.0 / 48_000.0).sin())
            .collect(),
        active: true,
        ..Player::default()
    }));
    let recorder = Recorder::start(
        480,
        player,
        vec![CaptureSpec {
            name: "vs_default_reader",
            source: VIRTUAL_MIC_NAME,
            monitor: false,
            rec: rec.clone(),
        }],
    );
    default_source(VIRTUAL_MIC_NAME);
    wait_for("self default keeps capture", || {
        running
            .input_selection()
            .is_some_and(|s| s.device.id == TEST_SRC && s.fallback)
    });
    assert!(running.is_linked(), "capture dropped on self default");
    std::thread::sleep(Duration::from_millis(300));
    let start = rec.lock().unwrap().samples.len();
    std::thread::sleep(Duration::from_millis(500));
    let samples = rec.lock().unwrap();
    assert!(samples.samples.len() > start + 1000);
    let peak = samples.samples[start..]
        .iter()
        .fold(0.0f32, |a, b| a.max(b.abs()));
    assert!(peak > 0.001, "virtual microphone became silent: {peak}");
    drop(samples);
    default_source(TEST_SRC2);
    wait_for("real default returns", || {
        running
            .input_selection()
            .is_some_and(|s| s.device.id == TEST_SRC2 && !s.fallback)
    });
    assert!(running.wait_linked(false, Duration::from_secs(5)));
    default_source(VIRTUAL_MIC_NAME);
    drop(recorder);
    drop(running);
    default_source(VIRTUAL_MIC_NAME);
    let (_handle, processor) = engine::new_engine();
    let running = PipeWireBackend::new()
        .start_pipewire(processor, None, None)
        .unwrap();
    assert!(running.wait_linked(false, Duration::from_secs(5)));
    wait_for("startup fallback", || {
        running
            .input_selection()
            .is_some_and(|s| (s.device.id == TEST_SRC || s.device.id == TEST_SRC2) && s.fallback)
    });
    println!(
        "self-default capture remained linked; peak={peak:.6}; real-default return and startup fallback passed"
    );
}

#[test]
#[ignore = "needs a private PipeWire server: scripts/e2e/run.sh"]
fn saved_input_reconnects_and_cli_and_default_choices_win() {
    use engine::AudioBackend;
    let nodes = NullNodes::start(480);
    default_source(TEST_SRC2);
    let mut backend = PipeWireBackend::new();
    backend.set_saved_input(Some(TEST_SRC));
    let (_handle, processor) = engine::new_engine();
    let running = backend.start_pipewire(processor, None, None).unwrap();
    assert!(running.wait_linked(false, Duration::from_secs(5)));
    wait_for("saved input", || {
        running
            .input_selection()
            .is_some_and(|s| s.device.id == TEST_SRC)
    });
    drop(nodes);
    wait_for("saved input disconnected", || {
        running.input_selection().is_none()
    });
    let _nodes = NullNodes::start(480);
    wait_for("saved input reconnected", || {
        assert!(
            running.error().is_none(),
            "backend stopped: {:?}",
            running.error()
        );
        running
            .input_selection()
            .is_some_and(|s| s.device.id == TEST_SRC)
    });
    assert!(running.wait_linked(false, Duration::from_secs(5)));
    default_source(TEST_SRC2);
    running.set_input(None);
    wait_for("default clears saved preference", || {
        running
            .input_selection()
            .is_some_and(|s| s.device.id == TEST_SRC2)
    });
    drop(running);
    let (_handle, processor) = engine::new_engine();
    let running = backend
        .start_pipewire(processor, Some(TEST_SRC2), None)
        .unwrap();
    assert!(running.wait_linked(false, Duration::from_secs(5)));
    wait_for("CLI overrides saved preference", || {
        running
            .input_selection()
            .is_some_and(|s| s.device.id == TEST_SRC2)
    });
    println!("saved input reconnect, default reset, and CLI precedence passed");
}
