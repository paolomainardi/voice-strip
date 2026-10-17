//! Checks the installed `VoiceStrip.driver` end to end: this process writes
//! a position-coded signal into the shared ring on the device timeline,
//! captures `Voice Strip` like any client, and checks that every frame comes
//! back intact, once, exactly the reported latency later.
//!
//! Needs the driver installed and the microphone permission, so it runs in
//! the desktop session: `scripts/macos-gui-run.sh just test-macos-driver`.

#![cfg(target_os = "macos")]

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::{Duration, Instant};

use vdev_macos::{hal, probe};
use vdev_macos_driver::{clock, model, shm};

/// Writes the coded signal up to the current timeline position, as an app
/// would after each capture cycle, until `stop`.
fn write_until(m: &shm::Mapping, stop: &AtomicBool) {
    let tpf = clock::ticks_per_frame(hal::host_ticks_per_second());
    let mut next = clock::position(hal::host_now(), tpf);
    let mut buf = [0.0f32; 1024];
    let mut beat = Instant::now();
    while !stop.load(Relaxed) {
        let now = clock::position(hal::host_now(), tpf);
        while next < now {
            let n = ((now - next) as usize).min(buf.len());
            for (i, s) in buf[..n].iter_mut().enumerate() {
                *s = probe::code(next + i as i64);
            }
            m.segment().write(next, &buf[..n]);
            next += n as i64;
        }
        if beat.elapsed() > Duration::from_millis(100) {
            m.segment().header.heartbeat.fetch_add(1, Relaxed);
            beat = Instant::now();
        }
        std::thread::sleep(Duration::from_micros(500));
    }
}

#[test]
#[ignore = "needs VoiceStrip.driver installed and microphone access"]
fn voice_strip_plays_the_ring_with_its_reported_latency() {
    let dev = hal::find(model::DEVICE_UID)
        .expect("device list")
        .expect("VoiceStrip.driver is not loaded");
    assert_eq!(hal::name(dev).unwrap(), model::DEVICE_NAME);
    let latency = hal::latency(dev).unwrap();
    println!(
        "device {dev}, IO buffer {} frames, reported latency {latency} frames",
        hal::buffer_frame_size(dev).unwrap()
    );
    assert_eq!(latency, model::TRANSPORT_LATENCY);

    let generation = RandomState::new().build_hasher().finish() | 1;
    let m = shm::create(generation).expect("create segment");
    let stop = AtomicBool::new(false);
    let r = std::thread::scope(|s| {
        s.spawn(|| write_until(&m, &stop));
        // The driver maps a new segment within 250 ms.
        std::thread::sleep(Duration::from_millis(600));
        let (samples, blocks) =
            hal::capture(dev, Duration::from_secs(3), model::SAMPLE_RATE).expect("capture");
        stop.store(true, Relaxed);
        probe::analyze(&samples, &blocks, i64::from(latency))
    });
    m.unlink();
    println!("{r:#?}");

    assert_eq!(r.frames, 3 * 48_000);
    assert_eq!(
        r.timeline_gaps, 0,
        "IO cycles skipped on the device timeline"
    );
    assert_eq!(r.wrong, 0, "frames torn, stale, repeated or misplaced");
    assert_eq!(r.silent, 0, "frames missing from the ring");
    assert_eq!(r.measured_latency, Some(i64::from(latency)));
}
