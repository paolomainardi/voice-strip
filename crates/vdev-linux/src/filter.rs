//! The `pw_filter` node and the main loop thread that owns it.

use std::cell::RefCell;
use std::ffi::{CStr, c_void};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Duration;

use engine::{AudioProcessor, BackendError, MAX_HOST_BLOCK, ProcessorWatch};
use pipewire as pw;
use pw::properties::{PropertiesBox, properties};
use pw::spa;
use pw::spa::sys as spa_sys;
use pw::sys as pw_sys;

use crate::graph::{Graph, err};
use crate::{
    Command, Config, MONITOR_QUANTUM, Status, VIRTUAL_MIC_DESCRIPTION, VIRTUAL_MIC_NAME, params,
};

const ID_INVALID: u32 = u32::MAX;

/// Diagnostics on stderr when `VOICE_STRIP_DEBUG` is set. Main loop thread
/// only, never from the realtime callback. Arguments are formatted only
/// when enabled.
macro_rules! debug {
    ($($arg:tt)*) => {
        if std::env::var_os("VOICE_STRIP_DEBUG").is_some() {
            eprintln!("[vdev-linux] {}", format_args!($($arg)*));
        }
    };
}
/// Name of the processing (filter) node.
pub const DSP_NODE_NAME: &str = "voice_strip_dsp";
const DSP_FORMAT: &str = "32 bit float mono audio";

/// Data owned by the realtime callback. Allocated once, freed after the
/// filter is destroyed.
struct RtData {
    processor: AudioProcessor,
    in_port: *mut c_void,
    out_port: *mut c_void,
    /// Monitor port while monitoring is on, null otherwise. Written by the
    /// main loop thread, read by the realtime callback.
    mon_port: AtomicPtr<c_void>,
    last_block: usize,
    silence: Vec<f32>,
    scratch: Vec<f32>,
    status: Arc<Status>,
}

unsafe extern "C" fn on_process(data: *mut c_void, position: *mut spa_sys::spa_io_position) {
    // SAFETY: `data` is the `RtData` registered with the listener and
    // outlives the filter; `position` is valid for the duration of the call.
    let d = unsafe { &mut *data.cast::<RtData>() };
    let clock = unsafe { &(*position).clock };
    let n = clock.duration as usize;
    d.status.driver_delay.store(clock.delay, Ordering::Relaxed);
    if n == 0 || n > MAX_HOST_BLOCK {
        return;
    }
    let RtData {
        processor,
        in_port,
        out_port,
        mon_port,
        last_block,
        silence,
        scratch,
        status,
    } = d;
    // SAFETY: port handles come from `pw_filter_add_port`; the returned
    // buffers hold `n` f32 samples for this cycle.
    let out = unsafe { pw_sys::pw_filter_get_dsp_buffer(*out_port, n as u32) }.cast::<f32>();
    // Without a consumer the output port has no buffer: keep processing
    // (meters, state) into scratch space.
    let main_out = if out.is_null() {
        &mut scratch[..n]
    } else {
        unsafe { std::slice::from_raw_parts_mut(out, n) }
    };
    let inp = unsafe { pw_sys::pw_filter_get_dsp_buffer(*in_port, n as u32) }.cast::<f32>();
    let input: &[f32] = if inp.is_null() {
        &silence[..n]
    } else {
        unsafe { std::slice::from_raw_parts(inp, n) }
    };
    let mon = mon_port.load(Ordering::Acquire);
    let monitor_out = if mon.is_null() {
        None
    } else {
        let p = unsafe { pw_sys::pw_filter_get_dsp_buffer(mon, n as u32) }.cast::<f32>();
        if p.is_null() {
            None
        } else {
            Some(unsafe { std::slice::from_raw_parts_mut(p, n) })
        }
    };
    if n != *last_block {
        processor.set_host_block_size(Some(n));
        *last_block = n;
        status.quantum.store(n, Ordering::Release);
    }
    processor.process(input, main_out, monitor_out);
    status.cycles.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn on_state_changed(
    data: *mut c_void,
    _old: pw_sys::pw_filter_state,
    state: pw_sys::pw_filter_state,
    error: *const std::os::raw::c_char,
) {
    if state != pw_sys::pw_filter_state_PW_FILTER_STATE_ERROR {
        return;
    }
    // SAFETY: see `on_process`; `error` is a C string or null.
    let d = unsafe { &*data.cast::<RtData>() };
    let msg = if error.is_null() {
        "filter error".to_owned()
    } else {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    };
    d.status.set_error(msg);
}

static EVENTS: pw_sys::pw_filter_events = pw_sys::pw_filter_events {
    version: pw_sys::PW_VERSION_FILTER_EVENTS,
    destroy: None,
    state_changed: Some(on_state_changed),
    io_changed: None,
    param_changed: None,
    add_buffer: None,
    remove_buffer: None,
    process: Some(on_process),
    drained: None,
    command: None,
};

/// Main-loop-thread state: registry view, links, filter handle.
struct State {
    cfg: Config,
    core: pw::core::CoreRc,
    filter: *mut pw_sys::pw_filter,
    rt: *mut RtData,
    graph: Graph,
    status: Arc<Status>,
    watch: ProcessorWatch,
    capture_link: Option<pw::link::Link>,
    capture_target: Option<(u32, u32)>,
    /// The virtual microphone node (null adapter) and its feeding link.
    virtual_node: Option<pw::node::Node>,
    output_link: Option<pw::link::Link>,
    output_target: Option<u32>,
    monitor_links: Vec<pw::link::Link>,
    monitor_sink: Option<u32>,
    metadata: Option<(pw::metadata::Metadata, pw::metadata::MetadataListener)>,
    reported_latency: usize,
    /// Capture wanted: off, the microphone link is removed so the source
    /// can suspend and nothing runs.
    capture_on: bool,
}

impl State {
    fn node_id(&self) -> Option<u32> {
        // SAFETY: `filter` is a live filter until `run` tears it down.
        let id = unsafe { pw_sys::pw_filter_get_node_id(self.filter) };
        (id != ID_INVALID && id != 0).then_some(id)
    }

    fn make_link(
        &self,
        out_node: u32,
        out_port: u32,
        in_node: u32,
        in_port: u32,
    ) -> Option<pw::link::Link> {
        self.core
            .create_object::<pw::link::Link>(
                "link-factory",
                &properties! {
                    "link.output.node" => out_node.to_string(),
                    "link.output.port" => out_port.to_string(),
                    "link.input.node" => in_node.to_string(),
                    "link.input.port" => in_port.to_string(),
                    "object.linger" => "false",
                },
            )
            .ok()
    }

    /// Establishes the capture link when source, ports and our node exist.
    /// Switches the capture source; the next sync links the new one.
    fn set_input(&mut self, input: Option<String>) {
        if self.cfg.input == input {
            return;
        }
        debug!("capture: input changed to {input:?}, relinking");
        self.cfg.input = input;
        self.capture_link = None;
        self.capture_target = None;
        self.status.linked.store(false, Ordering::Release);
        self.sync_capture();
    }

    /// Switches the headphones; the next sync links the monitor port to it.
    fn set_monitor_output(&mut self, output: Option<String>) {
        if self.cfg.monitor_output == output {
            return;
        }
        debug!("monitor: output changed to {output:?}, relinking");
        self.cfg.monitor_output = output;
        self.monitor_links.clear();
        self.monitor_sink = None;
        self.status.monitor_linked.store(false, Ordering::Release);
        self.sync_monitor();
    }

    /// Pauses or resumes capture (see [`RunningBackend::set_capture`]).
    fn set_capture(&mut self, on: bool) {
        if self.capture_on == on {
            return;
        }
        debug!("capture: {}", if on { "resumed" } else { "paused" });
        self.capture_on = on;
        // Inactive, the node is not scheduled even while its output stays
        // linked to the virtual microphone.
        // SAFETY: `filter` is a live filter until `run` tears it down.
        unsafe { pw_sys::pw_filter_set_active(self.filter, on) };
        if on {
            self.sync_capture();
        } else {
            self.capture_link = None;
            self.capture_target = None;
            self.status.linked.store(false, Ordering::Release);
        }
    }

    fn sync_capture(&mut self) {
        if self.capture_link.is_some() || !self.capture_on {
            return;
        }
        let Some(me) = self.node_id() else {
            debug!("capture: node id not yet known");
            return;
        };
        let Some(in_port) = self.graph.port_named(me, "input", false) else {
            debug!("capture: input port not yet registered");
            return;
        };
        let Some(name) = self
            .cfg
            .input
            .clone()
            .or_else(|| self.graph.default_source.clone())
        else {
            debug!("capture: no source selected and no default source");
            return;
        };
        if name == VIRTUAL_MIC_NAME {
            return;
        }
        let Some(target) = self.graph.node_by_name(&name) else {
            debug!("capture: source node {name} not found");
            return;
        };
        let Some(out_port) = self.graph.capture_port(target) else {
            debug!("capture: source {name} has no output port yet");
            return;
        };
        match self.make_link(target, out_port, me, in_port) {
            Some(link) => {
                debug!("capture: linked {name}:{out_port} -> {me}:{in_port}");
                self.capture_link = Some(link);
                self.capture_target = Some((target, out_port));
                self.status.linked.store(true, Ordering::Release);
            }
            None => debug!("capture: link-factory failed"),
        }
    }

    /// Links the filter output to the virtual microphone node.
    fn sync_output(&mut self) {
        if self.output_link.is_some() {
            return;
        }
        let Some(me) = self.node_id() else { return };
        let Some(out_port) = self.graph.port_named(me, "output", true) else {
            return;
        };
        let Some(vmic) = self.graph.node_by_name(VIRTUAL_MIC_NAME) else {
            debug!("output: virtual microphone node not yet registered");
            return;
        };
        let Some(in_port) = self.graph.playback_ports(vmic).first().copied() else {
            debug!("output: virtual microphone has no input port yet");
            return;
        };
        match self.make_link(me, out_port, vmic, in_port) {
            Some(link) => {
                debug!("output: linked {me}:{out_port} -> {vmic}:{in_port}");
                self.output_link = Some(link);
                self.output_target = Some(vmic);
                self.status.output_linked.store(true, Ordering::Release);
            }
            None => debug!("output: link-factory failed"),
        }
    }

    fn sync_monitor(&mut self) {
        // SAFETY: `rt` outlives the filter.
        let mon = unsafe { &*self.rt }.mon_port.load(Ordering::Acquire);
        if mon.is_null() || !self.monitor_links.is_empty() {
            return;
        }
        let Some(me) = self.node_id() else { return };
        let Some(mon_port) = self.graph.port_named(me, "monitor", true) else {
            return;
        };
        let Some(name) = self
            .cfg
            .monitor_output
            .clone()
            .or_else(|| self.graph.default_sink.clone())
        else {
            return;
        };
        let Some(sink) = self.graph.node_by_name(&name) else {
            return;
        };
        let links: Vec<pw::link::Link> = self
            .graph
            .playback_ports(sink)
            .into_iter()
            .filter_map(|p| self.make_link(me, mon_port, sink, p))
            .collect();
        if !links.is_empty() {
            self.monitor_links = links;
            self.monitor_sink = Some(sink);
            self.status.monitor_linked.store(true, Ordering::Release);
        }
    }

    /// A default device changed: drop the link that followed the old
    /// default so the next sync picks the new one.
    fn on_default_changed(&mut self, key: &str) {
        match key {
            "default.audio.source" if self.cfg.input.is_none() => {
                let current = self
                    .capture_target
                    .and_then(|(node, _)| self.graph.nodes.get(&node))
                    .map(|n| n.name.clone());
                if current.is_some() && current != self.graph.default_source {
                    debug!("capture: default source changed, relinking");
                    self.capture_link = None;
                    self.capture_target = None;
                    self.status.linked.store(false, Ordering::Release);
                }
            }
            "default.audio.sink" if self.cfg.monitor_output.is_none() => {
                let current = self
                    .monitor_sink
                    .and_then(|node| self.graph.nodes.get(&node))
                    .map(|n| n.name.clone());
                if current.is_some() && current != self.graph.default_sink {
                    debug!("monitor: default sink changed, relinking");
                    self.monitor_links.clear();
                    self.monitor_sink = None;
                    self.status.monitor_linked.store(false, Ordering::Release);
                }
            }
            _ => {}
        }
    }

    fn on_global_remove(&mut self, id: u32) {
        if self.graph.remove_global(id) {
            if let Some((node, port)) = self.capture_target
                && (id == node || id == port)
            {
                self.capture_link = None;
                self.capture_target = None;
                self.status.linked.store(false, Ordering::Release);
            }
            if self.monitor_sink == Some(id) {
                self.monitor_links.clear();
                self.monitor_sink = None;
                self.status.monitor_linked.store(false, Ordering::Release);
            }
            if self.output_target == Some(id) {
                self.output_link = None;
                self.output_target = None;
                self.status.output_linked.store(false, Ordering::Release);
            }
            self.sync_capture();
            self.sync_output();
            self.sync_monitor();
        }
    }

    fn set_node_latency(&self, quantum: u32) {
        let props = properties! {
            "node.latency" => format!("{quantum}/{}", dsp::SAMPLE_RATE),
        };
        // SAFETY: live filter; the dict is valid for the call.
        unsafe {
            pw_sys::pw_filter_update_properties(
                self.filter,
                std::ptr::null_mut(),
                props.dict().as_raw_ptr(),
            );
        }
    }

    /// Periodic: links, monitor port lifecycle and latency declaration.
    fn poll(&mut self) {
        self.status.readers.store(
            self.graph.readers().unwrap_or(usize::MAX),
            Ordering::Release,
        );
        self.sync_capture();
        self.sync_output();
        let wanted = self.watch.monitor_wanted();
        // SAFETY: `rt` outlives the filter.
        let rt = unsafe { &*self.rt };
        let mon = rt.mon_port.load(Ordering::Acquire);
        if wanted && mon.is_null() {
            let port = add_port(self.filter, false, "monitor");
            if !port.is_null() {
                rt.mon_port.store(port, Ordering::Release);
                self.set_node_latency(MONITOR_QUANTUM);
            }
        } else if !wanted && !mon.is_null() {
            self.monitor_links.clear();
            self.monitor_sink = None;
            self.status.monitor_linked.store(false, Ordering::Release);
            rt.mon_port.store(std::ptr::null_mut(), Ordering::Release);
            // SAFETY: `mon` came from `pw_filter_add_port`; the realtime
            // thread no longer dereferences it after the store above and
            // removal is synchronized with the data loop by PipeWire.
            unsafe { pw_sys::pw_filter_remove_port(mon) };
            self.set_node_latency(self.cfg.quantum);
        }
        self.sync_monitor();

        let latency = self.watch.main_latency_samples();
        if latency != self.reported_latency {
            self.reported_latency = latency;
            let pod = params::process_latency(latency);
            let mut p = [pod.as_ptr().cast::<spa_sys::spa_pod>()];
            // SAFETY: live filter; `pod` outlives the call.
            unsafe {
                pw_sys::pw_filter_update_params(
                    self.filter,
                    std::ptr::null_mut(),
                    p.as_mut_ptr(),
                    1,
                );
            }
        }
    }
}

fn add_port(filter: *mut pw_sys::pw_filter, input: bool, name: &str) -> *mut c_void {
    let props: PropertiesBox = properties! {
        "format.dsp" => DSP_FORMAT,
        "port.name" => name,
        "audio.channel" => "MONO",
    };
    let direction = if input {
        spa_sys::SPA_DIRECTION_INPUT
    } else {
        spa_sys::SPA_DIRECTION_OUTPUT
    };
    // SAFETY: valid filter; PipeWire takes ownership of the properties.
    unsafe {
        pw_sys::pw_filter_add_port(
            filter,
            direction,
            pw_sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
            std::mem::size_of::<usize>(),
            props.into_raw(),
            std::ptr::null_mut(),
            0,
        )
    }
}

pub(crate) fn run(
    cfg: Config,
    processor: AudioProcessor,
    rx: pw::channel::Receiver<Command>,
    status: Arc<Status>,
) -> Result<(), BackendError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(err)?;
    let context = pw::context::ContextRc::new(
        &mainloop,
        Some(properties! { "application.name" => "Voice Strip" }),
    )
    .map_err(err)?;
    let core = context.connect_rc(None).map_err(err)?;
    let registry = core.get_registry_rc().map_err(err)?;
    let watch = processor.watch();

    // The virtual microphone other applications see: a null adapter with
    // full node params (format, props), which pipewire-pulse requires. The
    // filter feeds its input port in the same cycle.
    let virtual_node = core
        .create_object::<pw::node::Node>(
            "adapter",
            &properties! {
                "factory.name" => "support.null-audio-sink",
                "node.name" => VIRTUAL_MIC_NAME,
                "node.description" => VIRTUAL_MIC_DESCRIPTION,
                "node.nick" => "Voice Strip",
                "media.class" => "Audio/Source/Virtual",
                "audio.channels" => "1",
                "audio.position" => "[ MONO ]",
                "audio.rate" => dsp::SAMPLE_RATE.to_string(),
                "node.virtual" => "true",
                "node.latency" => format!("{}/{}", cfg.quantum, dsp::SAMPLE_RATE),
                // Never drive the graph: the physical source's driver wins.
                "priority.driver" => "0",
                "monitor.channel-volumes" => "true",
            },
        )
        .map_err(err)?;

    let fprops: PropertiesBox = properties! {
        "media.type" => "Audio",
        "media.category" => "Filter",
        "media.role" => "DSP",
        "node.name" => DSP_NODE_NAME,
        "node.description" => "Voice Strip DSP",
        "node.latency" => format!("{}/{}", cfg.quantum, dsp::SAMPLE_RATE),
        "node.autoconnect" => "false",
        "node.always-process" => "true",
    };
    // SAFETY: core is connected; PipeWire takes ownership of the properties.
    let filter = unsafe {
        pw_sys::pw_filter_new(
            core.as_raw_ptr(),
            c"voice_strip_dsp".as_ptr(),
            fprops.into_raw(),
        )
    };
    if filter.is_null() {
        return Err(BackendError("pw_filter_new failed".into()));
    }
    let in_port = add_port(filter, true, "input");
    let out_port = add_port(filter, false, "output");
    if in_port.is_null() || out_port.is_null() {
        // SAFETY: filter is live and unconnected.
        unsafe { pw_sys::pw_filter_destroy(filter) };
        return Err(BackendError("pw_filter_add_port failed".into()));
    }
    let rt = Box::into_raw(Box::new(RtData {
        processor,
        in_port,
        out_port,
        mon_port: AtomicPtr::new(std::ptr::null_mut()),
        last_block: 0,
        silence: vec![0.0; MAX_HOST_BLOCK],
        scratch: vec![0.0; MAX_HOST_BLOCK],
        status: status.clone(),
    }));
    // SAFETY: zeroed hook storage is valid; the hook is removed before the
    // filter is destroyed.
    let mut hook: Box<spa_sys::spa_hook> = Box::new(unsafe { std::mem::zeroed() });
    unsafe {
        pw_sys::pw_filter_add_listener(filter, &mut *hook, &EVENTS, rt.cast::<c_void>());
    }
    let reported_latency = watch.main_latency_samples();
    let lat_pod = params::process_latency(reported_latency);
    let mut params = [lat_pod.as_ptr().cast::<spa_sys::spa_pod>()];
    // SAFETY: filter and params are valid.
    let res = unsafe {
        pw_sys::pw_filter_connect(
            filter,
            pw_sys::pw_filter_flags_PW_FILTER_FLAG_RT_PROCESS,
            params.as_mut_ptr(),
            1,
        )
    };
    if res < 0 {
        unsafe {
            spa::utils::hook::remove(*hook);
            pw_sys::pw_filter_destroy(filter);
            drop(Box::from_raw(rt));
        }
        return Err(BackendError(format!("pw_filter_connect failed: {res}")));
    }

    let state = Rc::new(RefCell::new(State {
        cfg,
        core: core.clone(),
        filter,
        rt,
        graph: Graph::default(),
        status: status.clone(),
        watch,
        capture_link: None,
        capture_target: None,
        virtual_node: Some(virtual_node),
        output_link: None,
        output_target: None,
        monitor_links: Vec::new(),
        monitor_sink: None,
        metadata: None,
        reported_latency,
        capture_on: true,
    }));

    let _core_listener = {
        let status = status.clone();
        let mainloop = mainloop.clone();
        core.add_listener_local()
            .error(move |_id, _seq, res, msg| {
                status.set_error(format!("core error {res}: {msg}"));
                mainloop.quit();
            })
            .register()
    };

    let _registry_listener = {
        let state_g = state.clone();
        let state_r = state.clone();
        let registry_b = registry.clone();
        registry
            .add_listener_local()
            .global(move |g| {
                if g.type_ == pw::types::ObjectType::Metadata
                    && g.props.and_then(|p| p.get("metadata.name")) == Some("default")
                    && state_g.borrow().metadata.is_none()
                {
                    match registry_b.bind::<pw::metadata::Metadata, _>(g) {
                        Ok(meta) => {
                            // Properties arrive right after the bind; the
                            // listener must exist before the loop iterates.
                            let listener = {
                                let state = state_g.clone();
                                meta.add_listener_local()
                                    .property(move |_subject, key, _type, value| {
                                        let mut s = state.borrow_mut();
                                        if let Some(key) = key
                                            && s.graph.set_default(key, value)
                                        {
                                            debug!("metadata {key} = {value:?}");
                                            s.on_default_changed(key);
                                            s.sync_capture();
                                            s.sync_output();
                                            s.sync_monitor();
                                        }
                                        0
                                    })
                                    .register()
                            };
                            state_g.borrow_mut().metadata = Some((meta, listener));
                        }
                        Err(e) => debug!("metadata bind failed: {e}"),
                    }
                    return;
                }
                let mut s = state_g.borrow_mut();
                if s.graph.add_global(g) {
                    s.sync_capture();
                    s.sync_output();
                    s.sync_monitor();
                }
            })
            .global_remove(move |id| state_r.borrow_mut().on_global_remove(id))
            .register()
    };

    let _timer = {
        let state = state.clone();
        let timer = mainloop
            .loop_()
            .add_timer(move |_| state.borrow_mut().poll());
        timer
            .update_timer(
                Some(Duration::from_millis(20)),
                Some(Duration::from_millis(100)),
            )
            .into_result()
            .map_err(err)?;
        timer
    };

    let _rx = rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let state = state.clone();
        move |cmd| match cmd {
            Command::Stop => mainloop.quit(),
            Command::SetInput(input) => state.borrow_mut().set_input(input),
            Command::SetMonitorOutput(out) => state.borrow_mut().set_monitor_output(out),
            Command::SetCapture(on) => state.borrow_mut().set_capture(on),
        }
    });

    status.connected.store(true, Ordering::Release);
    mainloop.run();

    // Teardown, in dependency order.
    {
        let mut s = state.borrow_mut();
        s.monitor_links.clear();
        s.capture_link = None;
        s.output_link = None;
        s.virtual_node = None;
        s.metadata = None;
    }
    drop(_timer);
    drop(_registry_listener);
    drop(_rx);
    drop(state);
    // SAFETY: nothing references the filter after this point; `rt` was
    // allocated by `Box::into_raw` above and the listener is removed first.
    unsafe {
        pw_sys::pw_filter_disconnect(filter);
        spa::utils::hook::remove(*hook);
        pw_sys::pw_filter_destroy(filter);
        drop(Box::from_raw(rt));
    }
    drop(hook);
    drop(registry);
    drop(core);
    drop(context);
    Ok(())
}
