//! PipeWire test clients shared by the loopback and end-to-end tests:
//! private null nodes standing in for the microphone and headphones, and a
//! client that plays into the test microphone and records any node, every
//! stream linked by hand.

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::properties::properties;
use pw::spa::pod::Pod;
use pw::stream::StreamFlags;
use vdev_linux::graph::{Graph, roundtrip};
use vdev_linux::params;

pub const RATE: u32 = 48_000;
pub const TEST_SRC: &str = "voice_strip_test_src";
pub const TEST_SINK: &str = "voice_strip_test_sink";
pub const TEST_SRC2: &str = "voice_strip_test_src2";
pub const TEST_SINK2: &str = "voice_strip_test_sink2";

/// Recorded buffers: one cycle timestamp (ns) and sample offset per
/// buffer, samples concatenated. Preallocated so the realtime callback
/// never allocates; the lock is uncontended while recording.
#[derive(Default)]
pub struct Rec {
    pub stamps: Vec<u64>,
    pub offsets: Vec<usize>,
    pub samples: Vec<f32>,
}

impl Rec {
    pub fn with_capacity(seconds: usize) -> Self {
        let n = RATE as usize * seconds;
        Self {
            stamps: Vec::with_capacity(n / 32),
            offsets: Vec::with_capacity(n / 32),
            samples: Vec::with_capacity(n),
        }
    }
}

pub type Recording = Arc<Mutex<Rec>>;

/// Playback state shared with the playback stream callback.
#[derive(Default)]
pub struct Player {
    pub signal: Vec<f32>,
    pub pos: usize,
    pub active: bool,
}

pub enum Cmd {
    Stop,
}

/// Client owning the two null nodes.
pub struct NullNodes {
    pub tx: pw::channel::Sender<Cmd>,
    pub thread: Option<JoinHandle<()>>,
}

impl NullNodes {
    pub fn start(quantum: u32) -> Self {
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
pub struct Recorder {
    pub tx: pw::channel::Sender<Cmd>,
    pub thread: Option<JoinHandle<()>>,
}

pub struct CaptureSpec {
    pub name: &'static str,
    /// Node whose output port to capture; monitor ports when `monitor`.
    pub source: &'static str,
    pub monitor: bool,
    pub rec: Recording,
}

impl Recorder {
    pub fn start(quantum: u32, player: Arc<Mutex<Player>>, captures: Vec<CaptureSpec>) -> Self {
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

/// Starts playing the player's signal from the beginning.
pub fn start_play(player: &Arc<Mutex<Player>>) {
    let mut p = player.lock().unwrap();
    p.pos = 0;
    p.active = true;
}

pub fn is_playing(player: &Arc<Mutex<Player>>) -> bool {
    player.lock().unwrap().active
}

/// Plays the player's signal once and waits for the end.
pub fn play(player: &Arc<Mutex<Player>>) {
    start_play(player);
    let len = player.lock().unwrap().signal.len();
    let limit = Duration::from_secs_f64(len as f64 / RATE as f64 + 5.0);
    let t = Instant::now();
    while is_playing(player) {
        assert!(t.elapsed() < limit, "playback stalled");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Snapshot of a recording: timestamps, buffer offsets, samples.
pub fn flatten(rec: &Recording) -> (Vec<u64>, Vec<usize>, Vec<f32>) {
    let rec = rec.lock().unwrap();
    (rec.stamps.clone(), rec.offsets.clone(), rec.samples.clone())
}

/// `a`'s samples and `b`'s samples placed on `a`'s timeline through their
/// cycle timestamps, so a buffer a capture stream missed leaves a gap of
/// silence instead of shifting the rest; plus the number of such buffers.
pub fn align(a: &Recording, b: &Recording) -> (Vec<f32>, Vec<f32>, usize) {
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
    (xa, xb_aligned, missing)
}

/// Lag of `b` relative to `a` in samples, on the timeline of [`align`].
/// Returns (lag,
/// correlation peak normalized by the reference energy, buffers missing in
/// `b`).
pub fn measure_lag(
    a: &Recording,
    b: &Recording,
    max_lag: usize,
    label: &str,
) -> (usize, f64, usize) {
    let (xa, xb_aligned, missing) = align(a, b);
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

pub fn nodes_named(prefix: &str) -> Vec<String> {
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

pub fn clear(rec: &Recording) {
    let mut r = rec.lock().unwrap();
    r.stamps.clear();
    r.offsets.clear();
    r.samples.clear();
}
