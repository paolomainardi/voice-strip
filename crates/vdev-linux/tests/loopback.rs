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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use engine::{MonitorConfig, RunningBackend};
use pipewire as pw;
use pw::properties::properties;
use pw::spa::pod::Pod;
use pw::stream::StreamFlags;
use vdev_linux::graph::{Graph, roundtrip};
use vdev_linux::{PipeWireBackend, VIRTUAL_MIC_NAME, params};

const RATE: u32 = 48_000;
const TEST_SRC: &str = "voice_strip_test_src";
const TEST_SINK: &str = "voice_strip_test_sink";
const TEST_SRC2: &str = "voice_strip_test_src2";
const TEST_SINK2: &str = "voice_strip_test_sink2";

/// Recorded buffers: one cycle timestamp (ns) and sample offset per
/// buffer, samples concatenated. Preallocated so the realtime callback
/// never allocates; the lock is uncontended while recording.
#[derive(Default)]
struct Rec {
    stamps: Vec<u64>,
    offsets: Vec<usize>,
    samples: Vec<f32>,
}

impl Rec {
    fn with_capacity(seconds: usize) -> Self {
        let n = RATE as usize * seconds;
        Self {
            stamps: Vec::with_capacity(n / 32),
            offsets: Vec::with_capacity(n / 32),
            samples: Vec::with_capacity(n),
        }
    }
}

type Recording = Arc<Mutex<Rec>>;

/// Chirp playback state shared with the playback stream callback.
#[derive(Default)]
struct Player {
    signal: Vec<f32>,
    pos: usize,
    active: bool,
}

enum Cmd {
    Stop,
}

/// Client owning the two null nodes.
struct NullNodes {
    tx: pw::channel::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl NullNodes {
    fn start(quantum: u32) -> Self {
        let (tx, rx) = pw::channel::channel::<Cmd>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            pw::init();
            let mainloop = pw::main_loop::MainLoopRc::new(None).expect("main loop");
            let context = pw::context::ContextRc::new(&mainloop, None).expect("context");
            let core = context.connect_rc(None).expect("connect");
            let node = |name: &str, class: &str| {
                core.create_object::<pw::node::Node>(
                    "adapter",
                    &properties! {
                        "factory.name" => "support.null-audio-sink",
                        "node.name" => name,
                        "node.description" => name,
                        "media.class" => class,
                        "audio.channels" => "1",
                        "audio.position" => "[ MONO ]",
                        "audio.rate" => RATE.to_string(),
                        "node.latency" => format!("{quantum}/{RATE}"),
                        "monitor.channel-volumes" => "true",
                    },
                )
                .expect("null node")
            };
            let _src = node(TEST_SRC, "Audio/Source/Virtual");
            let _src2 = node(TEST_SRC2, "Audio/Source/Virtual");
            let _sink = node(TEST_SINK, "Audio/Sink");
            let _sink2 = node(TEST_SINK2, "Audio/Sink");
            roundtrip(&mainloop, &core).expect("roundtrip");
            let _rx = rx.attach(mainloop.loop_(), {
                let mainloop = mainloop.clone();
                move |cmd| match cmd {
                    Cmd::Stop => mainloop.quit(),
                }
            });
            ready_tx.send(()).ok();
            mainloop.run();
        });
        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("null nodes");
        Self {
            tx,
            thread: Some(thread),
        }
    }
}

impl Drop for NullNodes {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Client owning the playback stream and the capture streams, all linked
/// by hand.
struct Recorder {
    tx: pw::channel::Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

struct CaptureSpec {
    name: &'static str,
    /// Node whose output port to capture; monitor ports when `monitor`.
    source: &'static str,
    monitor: bool,
    rec: Recording,
}

impl Recorder {
    fn start(quantum: u32, player: Arc<Mutex<Player>>, captures: Vec<CaptureSpec>) -> Self {
        let (tx, rx) = pw::channel::channel::<Cmd>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread = std::thread::spawn(move || {
            pw::init();
            let mainloop = pw::main_loop::MainLoopRc::new(None).expect("main loop");
            let context = pw::context::ContextRc::new(&mainloop, None).expect("context");
            let core = context.connect_rc(None).expect("connect");
            let registry = core.get_registry_rc().expect("registry");
            let graph = Rc::new(RefCell::new(Graph::default()));
            let _reg_listener = {
                let g1 = graph.clone();
                let g2 = graph.clone();
                registry
                    .add_listener_local()
                    .global(move |g| {
                        g1.borrow_mut().add_global(g);
                    })
                    .global_remove(move |id| {
                        g2.borrow_mut().remove_global(id);
                    })
                    .register()
            };
            let fmt = params::mono_f32_format(RATE);
            let flags = StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS;
            let stream_props = |name: &str, category: &str| {
                properties! {
                    "media.type" => "Audio",
                    "media.category" => category,
                    "media.role" => "Production",
                    "node.name" => name,
                    "node.autoconnect" => "false",
                    "node.latency" => format!("{quantum}/{RATE}"),
                    "stream.dont-remix" => "true",
                }
            };

            // Playback stream feeding the test microphone.
            let play = pw::stream::StreamRc::new(
                core.clone(),
                "vs_test_play",
                stream_props("vs_test_play", "Playback"),
            )
            .expect("play stream");
            let _play_listener = play
                .add_local_listener_with_user_data(player)
                .process(move |stream, player| {
                    let Some(mut b) = stream.dequeue_buffer() else {
                        return;
                    };
                    let datas = b.datas_mut();
                    let Some(d) = datas.first_mut() else { return };
                    let Some(bytes) = d.data() else { return };
                    let n = (bytes.len() / 4).min(quantum as usize);
                    let mut p = player.lock().unwrap();
                    for (i, out) in bytes[..n * 4].as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let x: f32 = if p.active && p.pos + i < p.signal.len() {
                            p.signal[p.pos + i]
                        } else {
                            0.0
                        };
                        *out = x.to_le_bytes();
                    }
                    if p.active {
                        p.pos += n;
                        if p.pos >= p.signal.len() {
                            p.active = false;
                        }
                    }
                    let chunk = d.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = 4;
                    *chunk.size_mut() = (n * 4) as u32;
                })
                .register()
                .expect("play listener");
            let mut p = [Pod::from_bytes(&fmt).unwrap()];
            play.connect(pw::spa::utils::Direction::Output, None, flags, &mut p)
                .expect("connect play");

            // Capture streams.
            let mut captures_rt = Vec::new();
            for spec in &captures {
                let stream = pw::stream::StreamRc::new(
                    core.clone(),
                    spec.name,
                    stream_props(spec.name, "Capture"),
                )
                .expect("capture stream");
                let listener = stream
                    .add_local_listener_with_user_data(spec.rec.clone())
                    .process(|stream, rec| {
                        let now = stream.time().map(|t| t.now() as u64).unwrap_or(0);
                        let Some(mut b) = stream.dequeue_buffer() else {
                            return;
                        };
                        let datas = b.datas_mut();
                        let Some(d) = datas.first_mut() else { return };
                        let size = d.chunk().size() as usize;
                        let off = d.chunk().offset() as usize;
                        let Some(bytes) = d.data() else { return };
                        let Ok(mut r) = rec.try_lock() else { return };
                        if r.samples.capacity() - r.samples.len() < size / 4 {
                            return;
                        }
                        let at = r.samples.len();
                        r.stamps.push(now);
                        r.offsets.push(at);
                        r.samples.extend(
                            bytes[off..off + size]
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .map(|c| f32::from_le_bytes(*c)),
                        );
                    })
                    .register()
                    .expect("capture listener");
                let mut p = [Pod::from_bytes(&fmt).unwrap()];
                stream
                    .connect(pw::spa::utils::Direction::Input, None, flags, &mut p)
                    .expect("connect capture");
                captures_rt.push((stream, listener));
            }

            // Wait until all stream nodes and ports are in the registry,
            // then link everything by hand.
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut links = Vec::new();
            loop {
                roundtrip(&mainloop, &core).expect("roundtrip");
                let g = graph.borrow();
                let mut pending = Vec::new();
                let play_node = play.node_id();
                let play_link = g.node_by_name(TEST_SRC).and_then(|src| {
                    let sp = g.capture_port(play_node)?;
                    let src_in = g.playback_ports(src).first().copied()?;
                    Some((play_node, sp, src, src_in))
                });
                let mut all_ok = play_link.is_some();
                pending.extend(play_link);
                for ((stream, _), spec) in captures_rt.iter().zip(&captures) {
                    let node = stream.node_id();
                    let target = g.node_by_name(spec.source);
                    let in_port = g.capture_in_port(node);
                    let out_port = target.and_then(|t| {
                        if spec.monitor {
                            g.monitor_port(t)
                        } else {
                            g.capture_port(t)
                        }
                    });
                    match (target, in_port, out_port) {
                        (Some(t), Some(i), Some(o)) => pending.push((t, o, node, i)),
                        _ => all_ok = false,
                    }
                }
                drop(g);
                if all_ok {
                    for (on, op, inn, ip) in pending {
                        let link = core
                            .create_object::<pw::link::Link>(
                                "link-factory",
                                &properties! {
                                    "link.output.node" => on.to_string(),
                                    "link.output.port" => op.to_string(),
                                    "link.input.node" => inn.to_string(),
                                    "link.input.port" => ip.to_string(),
                                },
                            )
                            .expect("link");
                        links.push(link);
                    }
                    roundtrip(&mainloop, &core).expect("roundtrip");
                    break;
                }
                if Instant::now() > deadline {
                    ready_tx
                        .send(Err("test streams or targets did not appear".into()))
                        .ok();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }

            let _rx = rx.attach(mainloop.loop_(), {
                let mainloop = mainloop.clone();
                move |cmd| match cmd {
                    Cmd::Stop => mainloop.quit(),
                }
            });
            ready_tx.send(Ok(())).ok();
            mainloop.run();
            drop(links);
            drop(captures_rt);
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("recorder start")
            .expect("recorder");
        Self {
            tx,
            thread: Some(thread),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

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

fn play(player: &Arc<Mutex<Player>>) {
    {
        let mut p = player.lock().unwrap();
        p.pos = 0;
        p.active = true;
    }
    let t = Instant::now();
    while player.lock().unwrap().active {
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "chirp playback stalled"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Snapshot of a recording: timestamps, buffer offsets, samples.
fn flatten(rec: &Recording) -> (Vec<u64>, Vec<usize>, Vec<f32>) {
    let rec = rec.lock().unwrap();
    (rec.stamps.clone(), rec.offsets.clone(), rec.samples.clone())
}

/// Lag of `b` relative to `a` in samples. Every `b` buffer is placed on
/// `a`'s timeline through its cycle timestamp, so a buffer a capture stream
/// missed leaves a gap instead of shifting the rest. Returns (lag,
/// correlation peak normalized by the reference energy, buffers missing in
/// `b`).
fn measure_lag(a: &Recording, b: &Recording, max_lag: usize, label: &str) -> (usize, f64, usize) {
    let (sa, oa, xa) = flatten(a);
    let (sb, ob, xb) = flatten(b);
    let mut xb_aligned = vec![0.0f32; xa.len()];
    let mut placed = 0usize;
    for (j, t) in sb.iter().enumerate() {
        let Some(i) = sa.iter().position(|u| u == t) else {
            continue;
        };
        let end = if j + 1 < ob.len() {
            ob[j + 1]
        } else {
            xb.len()
        };
        let src = &xb[ob[j]..end];
        let dst_end = (oa[i] + src.len()).min(xa.len());
        if oa[i] < dst_end {
            xb_aligned[oa[i]..dst_end].copy_from_slice(&src[..dst_end - oa[i]]);
            placed += 1;
        }
    }
    assert!(placed > 0, "no common cycle timestamp between recordings");
    let missing = sa.len().saturating_sub(placed);
    let xa = &xa[..];
    let xb = &xb_aligned[..];
    let n = xa.len();
    assert!(n > max_lag + RATE as usize, "recordings too short: {n}");
    if let Some(dir) = std::env::var_os("VOICE_STRIP_DUMP") {
        let dir = std::path::PathBuf::from(dir);
        let raw = |x: &[f32]| x.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let _ = std::fs::write(dir.join(format!("{label}_ref.raw")), raw(xa));
        let _ = std::fs::write(dir.join(format!("{label}_out.raw")), raw(xb));
    }
    let energy_a: f64 = xa[..n - max_lag].iter().map(|&x| (x as f64).powi(2)).sum();
    assert!(energy_a > 1.0, "reference recording is silent");
    let mut best = (0usize, f64::MIN);
    for lag in 0..max_lag {
        let mut acc = 0.0f64;
        for i in 0..n - max_lag {
            acc += xa[i] as f64 * xb[i + lag] as f64;
        }
        if acc > best.1 {
            best = (lag, acc);
        }
    }
    (best.0, best.1 / energy_a, missing)
}

fn nodes_named(prefix: &str) -> Vec<String> {
    let out = std::process::Command::new("pw-cli")
        .args(["ls", "Node"])
        .output()
        .expect("pw-cli");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("node.name") && l.contains(prefix))
        .map(|l| l.trim().to_owned())
        .collect()
}

fn clear(rec: &Recording) {
    let mut r = rec.lock().unwrap();
    r.stamps.clear();
    r.offsets.clear();
    r.samples.clear();
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
