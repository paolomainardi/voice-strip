//! Standby primitives against a private PipeWire server: the backend sees
//! applications reading the virtual microphone come and go, and pausing
//! capture removes its link to the microphone and stops processing until
//! it resumes.
//!
//! Run with `scripts/e2e/run.sh cargo test -p vdev-linux --test standby -- --ignored`
//! (never against the desktop's PipeWire: a running Voice Strip owns the
//! same node names).

#![cfg(target_os = "linux")]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use vdev_linux::{PipeWireBackend, VIRTUAL_MIC_NAME};

fn wait_for(what: &str, timeout: Duration, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

#[test]
#[ignore = "needs a private PipeWire server: scripts/e2e/run.sh"]
fn readers_and_capture_pause() {
    let _nodes = NullNodes::start(480);
    let (handle, processor) = engine::new_engine();
    let running = PipeWireBackend::new()
        .with_quantum(480)
        .start_pipewire(processor, Some(TEST_SRC), None)
        .expect("backend start");
    assert!(
        running.wait_linked(false, Duration::from_secs(5)),
        "capture link not established: {:?}",
        running.error()
    );

    wait_for("no reader at start", Duration::from_secs(2), || {
        running.has_readers() == Some(false)
    });

    let player = Arc::new(Mutex::new(Player {
        signal: vec![0.0; 4_800],
        ..Player::default()
    }));
    let rec: Recording = Arc::new(Mutex::new(Rec::with_capacity(5)));
    let recorder = Recorder::start(
        480,
        player,
        vec![CaptureSpec {
            name: "vs_standby_reader",
            source: VIRTUAL_MIC_NAME,
            monitor: false,
            rec,
        }],
    );
    wait_for("a reader", Duration::from_secs(3), || {
        running.has_readers() == Some(true)
    });
    drop(recorder);
    wait_for("the reader to leave", Duration::from_secs(3), || {
        running.has_readers() == Some(false)
    });

    // Nobody reads: after the grace period the backend pauses by itself.
    wait_for(
        "standby",
        engine::standby::STANDBY_AFTER + Duration::from_secs(3),
        || handle.is_standby(),
    );
    assert!(
        !running.is_linked(),
        "the microphone is still linked in standby"
    );
    std::thread::sleep(Duration::from_millis(300));
    let paused = running.cycles();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(running.cycles(), paused, "processing ran in standby");

    // A reader brings capture back.
    let player = Arc::new(Mutex::new(Player {
        signal: vec![0.0; 4_800],
        ..Player::default()
    }));
    let rec: Recording = Arc::new(Mutex::new(Rec::with_capacity(5)));
    let _reader = Recorder::start(
        480,
        player,
        vec![CaptureSpec {
            name: "vs_standby_reader2",
            source: VIRTUAL_MIC_NAME,
            monitor: false,
            rec,
        }],
    );
    wait_for("capture back", Duration::from_secs(3), || {
        !handle.is_standby()
    });
    assert!(
        running.wait_linked(false, Duration::from_secs(3)),
        "capture did not resume: {:?}",
        running.error()
    );
    let resumed = running.cycles();
    std::thread::sleep(Duration::from_millis(300));
    assert!(running.cycles() > resumed, "processing did not resume");
}
