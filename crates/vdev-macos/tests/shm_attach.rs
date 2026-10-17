//! Checks that the installed driver, running as `_coreaudiod` in the HAL's
//! sandbox, maps the segment this process creates, follows a restart of the
//! writer and notices when it stops. Reads the driver's log; needs no
//! microphone permission, so it runs over SSH.

#![cfg(target_os = "macos")]

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::process::Command;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use vdev_macos_driver::shm;

fn random_generation() -> u64 {
    RandomState::new().build_hasher().finish() | 1
}

/// Waits until the driver logs a line containing `needle`.
fn wait_for_log(needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let out = Command::new("/usr/bin/log")
            .args(["show", "--last", "30s", "--style", "compact", "--predicate"])
            .arg("eventMessage CONTAINS \"VoiceStrip: shm\"")
            .output()
            .expect("log show");
        if String::from_utf8_lossy(&out.stdout).contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

fn beat_for(m: &shm::Mapping, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        m.segment().header.heartbeat.fetch_add(1, Relaxed);
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "needs VoiceStrip.driver installed"]
fn driver_attaches_follows_restarts_and_detects_a_stale_writer() {
    let first = random_generation();
    let a = shm::create(first).expect("create segment");
    beat_for(&a, Duration::from_millis(500));
    assert!(
        wait_for_log(
            &format!("attached generation {first:016x}"),
            Duration::from_secs(5)
        ),
        "driver did not attach the first segment"
    );

    // Writer restart: a new segment under the same name.
    let second = random_generation();
    let b = shm::create(second).expect("recreate segment");
    drop(a);
    beat_for(&b, Duration::from_millis(500));
    assert!(
        wait_for_log(
            &format!("attached generation {second:016x}"),
            Duration::from_secs(5)
        ),
        "driver did not follow the restarted writer"
    );

    // Writer stops beating.
    assert!(
        wait_for_log(
            &format!("writer stale, generation {second:016x}"),
            Duration::from_secs(5)
        ),
        "driver did not notice the stale writer"
    );
    b.unlink();
}
