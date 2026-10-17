//! Loopback latency measurement against the live PipeWire server.
//!
//! Run with `cargo test -p vdev-linux -- --ignored --nocapture`.
//!
//! Two private null nodes (`support.null-audio-sink`) stand in for the
//! microphone (`Audio/Source/Virtual`) and the headphones (`Audio/Sink`).
//! The backend captures the test microphone into `voice_strip_mic` and, when
//! monitoring is on, feeds the test sink. One test client plays a chirp into
//! the test microphone and records, in the same graph cycle, the test
//! microphone (reference), the virtual microphone and the test sink; every
//! buffer is tagged with the cycle timestamp. Cross-correlating a recording
//! against the reference gives the latency the backend adds, which must equal
//! the reported engine latency plus I/O latency within one quantum.
//!
//! All streams are linked by hand with `link-factory` and never autoconnect,
//! so nothing reaches real devices. Every node belongs to a client
//! connection and disappears with it, even when an assertion fails.

#![cfg(target_os = "linux")]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use engine::{MonitorConfig, RunningBackend};
use vdev_linux::{PipeWireBackend, VIRTUAL_MIC_NAME};

fn chirp() -> Vec<f32> {
    let n = RATE as usize;
    let (f0, f1) = (100.0f64, 6000.0f64);
    (0..n)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            let phase = std::f64::consts::TAU * (f0 * t + (f1 - f0) * t * t / 2.0);
            0.5 * phase.sin() as f32
        })
        .collect()
}

/// `quantum` is requested through `node.latency`; the graph may settle on
/// another value, which the measurement reports.
fn run_at_quantum(quantum: u32) {
    let nodes = NullNodes::start(quantum);

    let (mut handle, processor) = engine::new_engine();
    let backend = PipeWireBackend::new().with_quantum(quantum);
    let running = backend
        .start_pipewire(processor, Some(TEST_SRC), Some(TEST_SINK))
        .expect("backend start");
    assert!(
        running.wait_linked(false, Duration::from_secs(5)),
        "capture link not established: {:?}",
        running.error()
    );

    let reference: Recording = Arc::new(Mutex::new(Rec::with_capacity(20)));
    let virtual_mic: Recording = Arc::new(Mutex::new(Rec::with_capacity(20)));
    let monitor: Recording = Arc::new(Mutex::new(Rec::with_capacity(20)));
    let player = Arc::new(Mutex::new(Player {
        signal: chirp(),
        ..Player::default()
    }));
    let recorder = Recorder::start(
        quantum,
        player.clone(),
        vec![
            CaptureSpec {
                name: "vs_test_ref",
                source: TEST_SRC,
                monitor: false,
                rec: reference.clone(),
            },
            CaptureSpec {
                name: "vs_test_vmic",
                source: VIRTUAL_MIC_NAME,
                monitor: false,
                rec: virtual_mic.clone(),
            },
            CaptureSpec {
                name: "vs_test_mon",
                source: TEST_SINK,
                monitor: true,
                rec: monitor.clone(),
            },
        ],
    );
    std::thread::sleep(Duration::from_millis(500));

    // Main path.
    clear(&reference);
    clear(&virtual_mic);
    play(&player);
    std::thread::sleep(Duration::from_millis(300));
    let q_main = running.quantum();
    let engine_main = handle.main_latency_samples();
    let (io_main, io_mon) = running.io_latency_samples();
    let (lag_main, corr_main, miss_main) =
        measure_lag(&reference, &virtual_mic, 4 * dsp::BLOCK_MAIN, "main");
    let expected_main = engine_main + io_main;
    println!(
        "quantum {q_main}: main path measured {lag_main} samples ({:.2} ms), reported {expected_main} (engine {engine_main} + io {io_main}), peak corr {corr_main:.3}, buffers missed by the test recorder {miss_main}",
        lag_main as f64 * 1000.0 / RATE as f64
    );

    // Monitor path.
    handle.set_monitor(MonitorConfig {
        enabled: true,
        ..MonitorConfig::default()
    });
    assert!(
        running.wait_linked(true, Duration::from_secs(5)),
        "monitor link not established: {:?}",
        running.error()
    );
    std::thread::sleep(Duration::from_millis(500));
    clear(&reference);
    clear(&monitor);
    play(&player);
    std::thread::sleep(Duration::from_millis(300));
    let q_mon = running.quantum();
    let engine_mon = handle.monitor_latency_samples();
    let (lag_mon, corr_mon, miss_mon) =
        measure_lag(&reference, &monitor, 4 * dsp::BLOCK_MAIN, "monitor");
    let expected_mon = engine_mon + io_mon;
    println!(
        "quantum {q_mon}: monitor path measured {lag_mon} samples ({:.2} ms), reported {expected_mon} (engine {engine_mon} + io {io_mon}), peak corr {corr_mon:.3}, buffers missed by the test recorder {miss_mon}",
        lag_mon as f64 * 1000.0 / RATE as f64
    );

    handle.set_monitor(MonitorConfig::default());
    let t = Instant::now();
    while running.is_monitor_linked() && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let monitor_removed = !running.is_monitor_linked();

    drop(recorder);
    drop(running);
    drop(nodes);
    let leftovers = nodes_named("voice_strip");
    assert!(
        leftovers.is_empty(),
        "test nodes left behind: {leftovers:?}"
    );

    assert!(monitor_removed, "monitor links not removed");
    assert!(corr_main > 0.9, "main path: no clear correlation peak");
    assert!(
        lag_main.abs_diff(expected_main) <= q_main,
        "main path: measured {lag_main} vs reported {expected_main} (quantum {q_main})"
    );
    assert!(corr_mon > 0.9, "monitor path: no clear correlation peak");
    assert!(
        lag_mon.abs_diff(expected_mon) <= q_mon,
        "monitor path: measured {lag_mon} vs reported {expected_mon} (quantum {q_mon})"
    );
}

#[test]
#[ignore = "needs a running PipeWire server"]
fn loopback_latency_request_480() {
    // PipeWire rounds the quantum down to a power of two by default
    // (`clock.power-of-two-quantum`), so this usually runs at 256.
    run_at_quantum(480);
}

#[test]
#[ignore = "needs a running PipeWire server"]
fn loopback_latency_request_64() {
    run_at_quantum(64);
}

/// Names of the source nodes currently linked into the filter input.
fn capture_sources() -> Vec<String> {
    let out = std::process::Command::new("pw-link")
        .arg("-l")
        .output()
        .expect("pw-link");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("voice_strip_dsp:input") {
            while let Some(next) = lines.peek() {
                if let Some(src) = next.trim_start().strip_prefix("|<- ") {
                    found.push(src.split(':').next().unwrap_or("").to_string());
                    lines.next();
                } else {
                    break;
                }
            }
        }
    }
    found
}

/// Switching the microphone at runtime relinks the capture port to the new
/// source, with no link left to the old one.
#[test]
#[ignore = "needs a running PipeWire server"]
fn switch_input_at_runtime() {
    let _nodes = NullNodes::start(480);
    let (_handle, processor) = engine::new_engine();
    let running = PipeWireBackend::new()
        .with_quantum(480)
        .start_pipewire(processor, Some(TEST_SRC), None)
        .expect("backend start");
    assert!(running.wait_linked(false, Duration::from_secs(5)));
    assert_eq!(capture_sources(), vec![TEST_SRC.to_string()]);
    running.set_input(Some(TEST_SRC2));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut now = capture_sources();
    while now != vec![TEST_SRC2.to_string()] && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        now = capture_sources();
    }
    eprintln!("capture sources after switch: {now:?}");
    assert_eq!(now, vec![TEST_SRC2.to_string()]);
}

/// Names of the sink nodes the monitor port currently feeds.
fn monitor_sinks() -> Vec<String> {
    let out = std::process::Command::new("pw-link")
        .arg("-l")
        .output()
        .expect("pw-link");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("voice_strip_dsp:monitor") {
            while let Some(next) = lines.peek() {
                if let Some(dst) = next.trim_start().strip_prefix("|-> ") {
                    found.push(dst.split(':').next().unwrap_or("").to_string());
                    lines.next();
                } else {
                    break;
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Switching the headphones at runtime relinks the monitor port to the new
/// sink, with no link left to the old one.
#[test]
#[ignore = "needs a running PipeWire server"]
fn switch_monitor_output_at_runtime() {
    let _nodes = NullNodes::start(64);
    let (mut handle, processor) = engine::new_engine();
    let running = PipeWireBackend::new()
        .with_quantum(480)
        .start_pipewire(processor, Some(TEST_SRC), Some(TEST_SINK))
        .expect("backend start");
    handle.set_monitor(MonitorConfig {
        enabled: true,
        with_denoise: false,
        level: 1.0,
    });
    assert!(running.wait_linked(true, Duration::from_secs(5)));
    assert_eq!(monitor_sinks(), vec![TEST_SINK.to_string()]);
    running.set_monitor_output(Some(TEST_SINK2));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut now = monitor_sinks();
    while now != vec![TEST_SINK2.to_string()] && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        now = monitor_sinks();
    }
    eprintln!("monitor sinks after switch: {now:?}");
    assert_eq!(now, vec![TEST_SINK2.to_string()]);
}
