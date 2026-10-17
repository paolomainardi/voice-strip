//! The macOS backend end to end with the real microphone's callbacks: its
//! samples are replaced by a known signal right before conversion, then
//! `Voice Strip` is captured like any client. Needs the driver installed
//! and the microphone permission, so it runs in the desktop session:
//! `scripts/macos-gui-run.sh just test-macos-backend`.

#![cfg(target_os = "macos")]

use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use vdev_macos::backend::{MacBackend, MacRunning, TestSignal};
use vdev_macos::{hal, probe};
use vdev_macos_driver::model;

/// Starts the backend with every module off, so only transport is tested.
fn start(signal: TestSignal) -> (engine::EngineHandle, MacRunning, u32) {
    let (mut handle, processor) = engine::new_engine();
    assert!(handle.set_settings(dsp::ChainSettings {
        enabled: [false; 9],
        ..Default::default()
    }));
    let running = MacBackend::with_test_signal(signal)
        .start_mac(processor, None, None)
        .expect("start backend");
    let info = running.info();
    println!(
        "capture {} frames at {} Hz, io latency {} frames",
        info.capture_frames, info.source_rate, info.io_latency
    );
    let dev = hal::find(model::DEVICE_UID)
        .unwrap()
        .expect("VoiceStrip.driver is not loaded");
    // The driver maps a new segment within 250 ms; let the clock loop settle.
    std::thread::sleep(Duration::from_secs(2));
    (handle, running, dev)
}

fn report(r: &MacRunning) {
    let s = r.status();
    println!(
        "cycles {}, skipped {}, reanchors {}, phase error {:.2} frames, correction {:.1} ppm",
        s.cycles.load(Relaxed),
        s.skipped.load(Relaxed),
        s.reanchors.load(Relaxed),
        f64::from_bits(s.error.load(Relaxed)),
        f64::from_bits(s.correction.load(Relaxed)) * 1e6
    );
}

#[test]
#[ignore = "needs VoiceStrip.driver installed and microphone access"]
fn a_sine_through_the_backend_stays_continuous() {
    let (_handle, running, dev) = start(TestSignal::Sine);
    let (samples, blocks) = hal::capture(dev, Duration::from_secs(5), model::SAMPLE_RATE).unwrap();
    report(&running);
    let r = probe::sine(&samples, &blocks, 440.0);
    println!("{r:#?}");
    assert_eq!(
        running.status().reanchors.load(Relaxed),
        0,
        "converter re-anchored"
    );
    assert_eq!(r.timeline_gaps, 0);
    assert!(r.silent < 4, "{} silent frames", r.silent);
    assert!(
        r.step_ratio < 1.1,
        "discontinuity, step ratio {}",
        r.step_ratio
    );
    assert!(
        (r.frequency_hz - 440.0).abs() < 0.5,
        "{} Hz",
        r.frequency_hz
    );
    assert!((r.peak - 0.25).abs() < 0.01, "peak {}", r.peak);
}

#[test]
#[ignore = "needs VoiceStrip.driver installed and microphone access"]
fn clicks_arrive_with_the_reported_latency() {
    let (handle, running, dev) = start(TestSignal::Clicks);
    let (samples, blocks) = hal::capture(dev, Duration::from_secs(5), model::SAMPLE_RATE).unwrap();
    report(&running);
    let s = running.status();
    let clicks: Vec<f64> = (0..8)
        .map(|i| f64::from_bits(s.clicks[i].load(Relaxed)))
        .filter(|p| *p > 0.0)
        .collect();
    // Device position of every captured peak.
    let mut peaks = Vec::new();
    let mut at = 0usize;
    for b in &blocks {
        for i in 0..b.frames as usize {
            if samples.get(at + i).is_some_and(|v| v.abs() > 0.3) {
                peaks.push(b.sample_time + i as f64);
            }
        }
        at += b.frames as usize;
    }
    // Expected: converter delay + engine latency + transport margin, on
    // top of the click's own timeline position.
    let expected = running.info().converter_delay
        + f64::from(model::TRANSPORT_LATENCY)
        + handle.main_latency_samples() as f64;
    let lags: Vec<f64> = peaks
        .iter()
        .filter_map(|p| {
            clicks
                .iter()
                .map(|c| p - c)
                .filter(|d| *d >= 0.0 && *d < 4_800.0)
                .reduce(f64::min)
        })
        .collect();
    let loudest = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    println!(
        "loudest captured sample {loudest}, nonzero {}",
        samples.iter().filter(|v| **v != 0.0).count()
    );
    println!("clicks {clicks:?}\npeaks {peaks:?}\nlags {lags:?}, expected {expected}");
    assert!(lags.len() >= 3, "too few clicks captured");
    assert!(
        lags.iter().all(|l| (l - expected).abs() <= 3.0),
        "measured {lags:?} vs expected {expected}"
    );
}

/// Clicks from the microphone's callbacks to the headphones: the host time
/// each leaves the headphones' converters (as the device reports it) minus
/// the host time it was captured. Plays quiet clicks on the default output.
#[test]
#[ignore = "needs microphone access; plays quiet clicks on the default output"]
fn monitor_clicks_reach_the_headphones_within_budget() {
    let (mut handle, running, _dev) = start(TestSignal::Clicks);
    handle.set_monitor(engine::MonitorConfig {
        enabled: true,
        with_denoise: false,
        level: 0.05,
    });
    std::thread::sleep(Duration::from_secs(5));
    let s = running.status();
    let tpf = hal::host_ticks_per_second() / 48_000.0;
    let captured: Vec<f64> = (0..8)
        .map(|i| f64::from_bits(s.clicks[i].load(Relaxed)) * tpf)
        .filter(|h| *h > 0.0)
        .collect();
    let played: Vec<f64> = (0..8)
        .map(|i| f64::from_bits(s.monitor_clicks[i].load(Relaxed)))
        .filter(|h| *h > 0.0)
        .collect();
    let ms_per_tick = 1000.0 / hal::host_ticks_per_second();
    let lags: Vec<f64> = played
        .iter()
        .filter_map(|p| {
            captured
                .iter()
                .map(|c| (p - c) * ms_per_tick)
                .filter(|d| *d >= 0.0 && *d < 500.0)
                .reduce(f64::min)
        })
        .collect();
    let reported = engine::RunningBackend::io_latency_samples(&running).1 as f64
        + handle.monitor_latency_samples() as f64;
    println!(
        "monitor: lags {lags:.2?} ms, reported {:.2} ms, cycles {}, fill {:.1}, clicks out {}, underruns {}, overflows {}",
        reported / 48.0,
        s.monitor_cycles.load(Relaxed),
        f64::from_bits(s.monitor_fill.load(Relaxed)),
        s.monitor_click_count.load(Relaxed),
        s.monitor_underruns.load(Relaxed),
        s.monitor_overflows.load(Relaxed)
    );
    assert!(lags.len() >= 3, "too few clicks reached the headphones");
    assert_eq!(s.monitor_underruns.load(Relaxed), 0);
    assert!(
        lags.iter().all(|l| *l < 15.0),
        "monitor latency {lags:?} ms"
    );
}
