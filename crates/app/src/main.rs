//! Voice Strip desktop app (Linux and macOS): the `ui-kit` views drawn
//! through an epaint `Painter` and rendered on the GPU with wgpu (Vulkan on
//! Linux, Metal on macOS), in one borderless, transparent window.
//!
//! voice-strip [--backend pipewire|sim] [--advanced] [--input NODE]
//!             [--headphones NODE] [--preset NAME] [--noise LEVEL]
//!             [--list-devices]
//! voice-strip --screenshot standard|advanced <out.png> [--scale N] [--monitor]
//! voice-strip ctl <command>   (controls the running app, see `voice-strip ctl --help`)
//! voice-strip --fonts-dir     (prints the folder of the bundled fonts)

mod driver;
mod fonts;
#[cfg(target_os = "macos")]
mod menubar;
mod painter;
mod remote;
mod tray;

use std::path::PathBuf;
use std::time::Instant;

use std::cell::RefCell;
use std::rc::Rc;

use app_core::remote::Effect;
use app_core::{Controller, DenoiseLevel, PresetId};
use eframe::egui::{self, ViewportCommand};
use engine::{AudioBackend, DeviceInfo, RunningBackend};
use ui_kit::{
    Action, AdvancedView, Cursor, Event, Key, ModelDialog, Painter, Point, StandardView, View,
    ViewKind, advanced, standard,
};

use crate::painter::EguiPainter;

const APP_ID: &str = "io.github.paolomainardi.voicestrip";

const USAGE: &str =
    "usage: voice-strip [--backend pipewire|sim] [--advanced] [--background] [--input NODE] \
[--headphones NODE] [--preset broadcast|podcast|call|stream|narration] \
[--noise off|light|medium|strong] [--list-devices]
       voice-strip --screenshot standard|advanced <out.png> [--scale N] [--monitor] [--model-prompt]
       voice-strip ctl <command>   (see voice-strip ctl --help)
       voice-strip driver status | install [--if-needed] | uninstall   (macOS virtual microphone)";

#[derive(Default)]
struct Args {
    sim: bool,
    advanced: bool,
    input: Option<String>,
    headphones: Option<String>,
    preset: Option<PresetId>,
    noise: Option<Option<DenoiseLevel>>,
    list_devices: bool,
    screenshot: Option<(ViewKind, PathBuf)>,
    scale: f32,
    monitor: bool,
    /// Screenshot with the model download dialog open.
    model_prompt: bool,
    /// Start without the window: processing, tray and remote control only.
    background: bool,
}

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args {
        scale: 1.0,
        ..Default::default()
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--backend" => {
                a.sim = match it.next().as_deref() {
                    Some("sim") => true,
                    Some("pipewire") => false,
                    _ => usage(),
                }
            }
            "--advanced" => a.advanced = true,
            "--input" => a.input = Some(it.next().unwrap_or_else(|| usage())),
            "--headphones" => a.headphones = Some(it.next().unwrap_or_else(|| usage())),
            "--preset" => {
                use PresetId as P;
                a.preset = Some(match it.next().as_deref() {
                    Some("broadcast") => P::Broadcast,
                    Some("podcast") => P::Podcast,
                    Some("call") => P::Call,
                    Some("stream") => P::Stream,
                    Some("narration") => P::Narration,
                    _ => usage(),
                });
            }
            "--noise" => {
                use DenoiseLevel as D;
                a.noise = Some(match it.next().as_deref() {
                    Some("off") => None,
                    Some("light") => Some(D::Light),
                    Some("medium") => Some(D::Medium),
                    Some("strong") => Some(D::Strong),
                    _ => usage(),
                });
            }
            "--list-devices" => a.list_devices = true,
            "--screenshot" => {
                let kind = match it.next().as_deref() {
                    Some("standard") => ViewKind::Standard,
                    Some("advanced") => ViewKind::Advanced,
                    _ => usage(),
                };
                let out = it.next().unwrap_or_else(|| usage());
                a.screenshot = Some((kind, PathBuf::from(out)));
            }
            "--scale" => {
                a.scale = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            "--monitor" => a.monitor = true,
            "--model-prompt" => a.model_prompt = true,
            "--background" => a.background = true,
            _ => usage(),
        }
    }
    a
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Input,
    Output,
}

/// Screenshot mode: render the demo state, capture a frame, save, quit.
struct Shot {
    out: PathBuf,
    frames: u32,
    requested: bool,
}

#[cfg(target_os = "linux")]
type Tray = (
    ksni::blocking::Handle<tray::VoiceStripTray>,
    std::sync::mpsc::Receiver<tray::TrayCmd>,
    tray::TrayState,
);

struct VoiceStrip {
    controller: Controller,
    standard: StandardView,
    advanced: AdvancedView,
    /// Drawn over the active view while the controller asks for the model.
    dialog: ModelDialog,
    /// Whether the dialog was asked for at the last service pass.
    prompted: bool,
    active: ViewKind,
    backend: Option<Box<dyn RunningBackend>>,
    /// The active view's static layer, tessellated once and reused every
    /// frame until its key changes.
    static_layer: Option<StaticLayer>,
    /// The platform backend, kept to list devices again.
    devices: Box<dyn AudioBackend>,
    inputs: Vec<DeviceInfo>,
    outputs: Vec<DeviceInfo>,
    input_index: Option<usize>,
    output_index: Option<usize>,
    menu: Option<(Menu, egui::Pos2)>,
    last: Instant,
    /// Window size last requested, logical pixels.
    size: (f32, f32),
    /// Scale from view coordinates to window points: below 1 when the
    /// view is larger than the screen.
    scale: f32,
    /// Monitor size at zoom 1, points; taken once.
    screen: Option<egui::Vec2>,
    /// Pointer press on bare canvas: the window is being dragged.
    pressed_on_canvas: bool,
    hover_since: Option<(Point, Instant)>,
    /// Fractional wheel notches not yet dispatched.
    scroll_acc: f32,
    shot: Option<Shot>,
    #[cfg(target_os = "linux")]
    tray: Option<Tray>,
    #[cfg(target_os = "macos")]
    menubar: Option<menubar::MenuBar>,
    remote: Option<remote::Remote>,
    /// Last attempt to take the control socket while another instance held
    /// it: one that is still exiting (a restart) frees it a moment later.
    /// `None` never retries (screenshots).
    remote_retry: Option<Instant>,
    /// Last run of [`VoiceStrip::service`].
    last_service: Instant,
    /// Quit was asked for (tray, remote control): closing the window ends
    /// the app instead of leaving it in the background.
    quit: bool,
    /// Whether the window is shown, for the tray and menu bar window item.
    window_open: bool,
}

impl VoiceStrip {
    fn view(&mut self) -> &mut dyn View {
        match self.active {
            ViewKind::Standard => &mut self.standard,
            ViewKind::Advanced => &mut self.advanced,
        }
    }

    fn view_size(&self) -> (f32, f32) {
        match self.active {
            ViewKind::Standard => (standard::WIDTH, standard::HEIGHT),
            ViewKind::Advanced => {
                let h = self.advanced.natural_height();
                (
                    advanced::DEFAULT_WIDTH,
                    if h > 0.0 { h } else { advanced::DEFAULT_HEIGHT },
                )
            }
        }
    }

    /// Lists the devices again, so ones connected since start (AirPods, a
    /// USB interface) show up, keeping the current selections.
    fn refresh_devices(&mut self) {
        let id_of = |list: &[DeviceInfo], i: Option<usize>| {
            i.and_then(|i| list.get(i)).map(|d| d.id.clone())
        };
        let input = id_of(&self.inputs, self.input_index);
        let output = id_of(&self.outputs, self.output_index);
        if let Ok(list) = self.devices.list_inputs() {
            self.inputs = list;
        }
        if let Ok(list) = self.devices.list_outputs() {
            self.outputs = list;
        }
        self.input_index = input.and_then(|id| self.inputs.iter().position(|d| d.id == id));
        self.output_index = output.and_then(|id| self.outputs.iter().position(|d| d.id == id));
    }

    fn dispatch(&mut self, ev: Event) {
        let mut actions = Vec::new();
        let state = self.controller.state().clone();
        if ModelDialog::is_open(&state) {
            self.dialog.handle(ev, &state, &mut actions);
        } else {
            self.view().handle(ev, &state, &mut actions);
        }
        for a in actions {
            match a {
                Action::SetOn(on) => self.controller.set_on(on),
                Action::SetMuted(m) => self.controller.set_muted(m),
                Action::SetAbOriginal(o) => self.controller.set_ab_original(o),
                Action::SetPreset(p) => self.controller.set_preset(p),
                Action::SetIntensity(i) => self.controller.set_intensity(i),
                Action::SetNeuralDenoise(l) => self.controller.set_neural_denoise(l),
                Action::SetMonitor(m) => self.controller.set_monitor(m),
                Action::SetModuleEnabled(m, e) => self.controller.set_module_enabled(m, e),
                Action::SwitchView(k) => self.active = k,
                Action::Recalibrate => {
                    if !self.controller.is_calibrating() {
                        self.controller.start_calibration();
                    }
                }
                Action::ChooseInput(r) => {
                    self.refresh_devices();
                    self.menu = Some((Menu::Input, egui::pos2(r.x, r.bottom()) * self.scale));
                }
                Action::ChooseOutput(r) => {
                    self.refresh_devices();
                    self.menu = Some((Menu::Output, egui::pos2(r.x, r.bottom()) * self.scale));
                }
                Action::DownloadModel => self.controller.download_model(),
                Action::DismissModelPrompt => self.controller.dismiss_model_prompt(),
                Action::OpenUpdate => open_url(app_core::updates::RELEASES_PAGE),
            }
        }
    }

    /// Drains the scope queues into the analyzers (only the advanced view
    /// shows them), straight from the queue slices: no copy, no allocation.
    fn feed_scopes(&mut self) {
        let feed = self.active == ViewKind::Advanced;
        let engine = self.controller.engine_mut();
        let advanced = &mut self.advanced;
        for (rx, is_input) in [
            (&mut engine.scope_in_rx, true),
            (&mut engine.scope_out_rx, false),
        ] {
            let n = rx.slots();
            if let Ok(chunk) = rx.read_chunk(n) {
                if feed {
                    let (a, b) = chunk.as_slices();
                    for part in [a, b] {
                        if is_input {
                            advanced.feed(part, &[]);
                        } else {
                            advanced.feed(&[], part);
                        }
                    }
                }
                chunk.commit_all();
            }
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            // An open device menu takes the keyboard: Escape closes it and no
            // shortcut reaches the view underneath.
            if self.menu.is_some() && matches!(ev, egui::Event::Key { .. } | egui::Event::Text(_)) {
                if let egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } = ev
                {
                    self.menu = None;
                }
                continue;
            }
            // Cmd+W on macOS, Ctrl+W elsewhere: hide the window, keep running.
            if let egui::Event::Key {
                key: egui::Key::W,
                pressed: true,
                modifiers,
                ..
            } = ev
                && modifiers.command
            {
                self.hide_window(ctx);
                continue;
            }
            match ev {
                egui::Event::PointerMoved(p) => {
                    let pt = Point::new(p.x / self.scale, p.y / self.scale);
                    self.dispatch(Event::PointerMove(pt));
                    self.hover_since = Some((pt, Instant::now()));
                }
                egui::Event::PointerGone => {
                    self.hover_since = None;
                    self.dispatch(Event::PointerLeave);
                }
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    ..
                } => {
                    let pt = Point::new(pos.x / self.scale, pos.y / self.scale);
                    if pressed {
                        if self.menu.is_some() {
                            continue;
                        }
                        self.dispatch(Event::PointerMove(pt));
                        if self.cursor() == Cursor::Default {
                            // Bare canvas: move the window (no title bar).
                            self.pressed_on_canvas = true;
                            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                        } else {
                            self.dispatch(Event::PointerDown(pt));
                        }
                    } else if !std::mem::take(&mut self.pressed_on_canvas) {
                        self.dispatch(Event::PointerUp(pt));
                    }
                }
                egui::Event::MouseWheel { unit, delta, .. } => {
                    if self.menu.is_some() || delta.y == 0.0 {
                        continue;
                    }
                    // Convert to notches: lines are notches, points (touchpads)
                    // count 40 px per notch, pages 3 notches.
                    let notches = match unit {
                        egui::MouseWheelUnit::Line => delta.y,
                        egui::MouseWheelUnit::Point => delta.y / 40.0,
                        egui::MouseWheelUnit::Page => delta.y * 3.0,
                    };
                    self.scroll_acc += notches;
                    let whole = self.scroll_acc.trunc();
                    if whole != 0.0 {
                        self.scroll_acc -= whole;
                        if let Some(p) = ctx.pointer_latest_pos() {
                            // egui: positive y scrolls up; ui-kit: positive is
                            // towards the user.
                            let pt = Point::new(p.x / self.scale, p.y / self.scale);
                            self.dispatch(Event::Scroll(pt, -whole));
                        }
                    }
                }
                egui::Event::Key {
                    key, pressed: true, ..
                } => {
                    let k = match key {
                        egui::Key::ArrowUp => Some(Key::Up),
                        egui::Key::ArrowDown => Some(Key::Down),
                        egui::Key::ArrowLeft => Some(Key::Left),
                        egui::Key::ArrowRight => Some(Key::Right),
                        egui::Key::Space => Some(Key::Space),
                        egui::Key::Enter => Some(Key::Enter),
                        egui::Key::Escape => Some(Key::Escape),
                        _ => None,
                    };
                    if let Some(k) = k {
                        self.dispatch(Event::Key(k));
                    }
                }
                egui::Event::Text(t) => {
                    for c in t.chars() {
                        self.dispatch(Event::Key(Key::Char(c)));
                    }
                }
                _ => {}
            }
        }
    }

    fn device_menu(&mut self, ctx: &egui::Context) {
        let Some((menu, pos)) = self.menu else {
            return;
        };
        let (list, current, title, default) = match menu {
            Menu::Input => (
                &self.inputs,
                self.input_index,
                "Microphone",
                "Default microphone",
            ),
            Menu::Output => (
                &self.outputs,
                self.output_index,
                "Headphones",
                "Default output",
            ),
        };
        let mut pick: Option<Option<usize>> = None;
        let resp = egui::Area::new(egui::Id::new("device-menu"))
            .fixed_pos(pos + egui::vec2(0.0, 4.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.strong(title);
                    if ui.radio(current.is_none(), default).clicked() {
                        pick = Some(None);
                    }
                    for (i, d) in list.iter().enumerate() {
                        if ui.radio(current == Some(i), &d.label).clicked() {
                            pick = Some(Some(i));
                        }
                    }
                });
            });
        let clicked_outside =
            ctx.input(|i| i.pointer.any_pressed()) && !resp.response.contains_pointer();
        if let Some(choice) = pick {
            let dev = choice.and_then(|i| list.get(i)).cloned();
            if let Some(b) = &self.backend {
                match menu {
                    Menu::Input => b.set_input(dev.as_ref().map(|d| d.id.as_str())),
                    Menu::Output => b.set_monitor_output(dev.as_ref().map(|d| d.id.as_str())),
                }
            }
            let st = self.controller.state_mut();
            match menu {
                Menu::Input => {
                    self.input_index = choice;
                    let id = dev.as_ref().map(|d| d.id.clone());
                    let label = dev.map_or_else(|| default.into(), |d| d.label);
                    self.controller.select_input(id.as_deref(), label);
                }
                Menu::Output => {
                    self.output_index = choice;
                    st.output_device = dev.map_or_else(String::new, |d| d.label);
                }
            }
            self.menu = None;
        } else if clicked_outside {
            self.menu = None;
        }
    }

    fn tooltip(&mut self, ctx: &egui::Context) {
        let Some((pt, since)) = self.hover_since else {
            return;
        };
        if since.elapsed().as_millis() < 600
            || self.menu.is_some()
            || ModelDialog::is_open(self.controller.state())
        {
            return;
        }
        let state = self.controller.state().clone();
        if let Some(text) = self.view().tooltip(pt, &state) {
            egui::Area::new(egui::Id::new("tip"))
                .fixed_pos(egui::pos2(
                    pt.x * self.scale + 12.0,
                    pt.y * self.scale + 18.0,
                ))
                .order(egui::Order::Tooltip)
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.label(text);
                    });
                });
        }
    }

    /// Hides the window and keeps running. macOS hides it and keeps the
    /// event loop, which AppKit needs to open the menu bar item's menu;
    /// elsewhere the window closes (Wayland cannot hide one) and
    /// [`run_background`] takes over.
    fn hide_window(&mut self, ctx: &egui::Context) {
        if cfg!(target_os = "macos") {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.window_open = false;
        } else {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        if cfg!(target_os = "macos") {
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            self.window_open = true;
        }
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    /// Applies one tray or menu bar command.
    fn apply(&mut self, cmd: tray::TrayCmd, effect: &mut Effect) {
        match cmd {
            tray::TrayCmd::SetOn(on) => self.controller.set_on(on),
            tray::TrayCmd::SetMuted(m) => self.controller.set_muted(m),
            tray::TrayCmd::SetPreset(p) => self.controller.set_preset(p),
            tray::TrayCmd::SetNoise(n) => self.controller.set_neural_denoise(n),
            tray::TrayCmd::OpenUpdate => open_url(app_core::updates::RELEASES_PAGE),
            tray::TrayCmd::ToggleWindow if *effect == Effect::Quit => {}
            tray::TrayCmd::ToggleWindow => {
                *effect = if self.window_open {
                    Effect::Hide
                } else {
                    Effect::Show
                }
            }
            tray::TrayCmd::Quit => *effect = Effect::Quit,
        }
    }

    /// Applies the tray's commands and mirrors the state into it.
    #[cfg(target_os = "linux")]
    fn tray(&mut self) -> Effect {
        let mut effect = Effect::None;
        let Some((_, rx, _)) = &self.tray else {
            return effect;
        };
        let cmds: Vec<_> = rx.try_iter().collect();
        for cmd in cmds {
            self.apply(cmd, &mut effect);
        }
        let now = tray::TrayState::of(self.controller.state(), self.window_open);
        if let Some((handle, _, last)) = &mut self.tray
            && now != *last
        {
            *last = now;
            handle.update(|t| t.state = now);
        }
        effect
    }

    /// Applies the menu bar's commands and mirrors the state into it.
    #[cfg(target_os = "macos")]
    fn tray(&mut self) -> Effect {
        let mut effect = Effect::None;
        let Some(bar) = &self.menubar else {
            return effect;
        };
        for cmd in bar.commands() {
            self.apply(cmd, &mut effect);
        }
        let now = tray::TrayState::of(self.controller.state(), self.window_open);
        if let Some(bar) = &mut self.menubar {
            bar.update(now);
        }
        effect
    }

    /// The work that goes on with or without the window: engine meters and
    /// scopes, the tray and remote control. Returns what is left to do with
    /// the window.
    fn service(&mut self) -> Effect {
        let dt = self.last_service.elapsed().as_secs_f32().min(0.1);
        self.last_service = Instant::now();
        if self.shot.is_none() {
            // Backend latency changes while running (headphones started,
            // another microphone), so it is read every pass.
            if let Some(b) = &self.backend {
                let (main, monitor) = b.io_latency_samples();
                let ms = |n: usize| n as f32 * 1000.0 / dsp::SAMPLE_RATE as f32;
                self.controller.set_io_latency_ms(ms(main));
                self.controller.set_io_monitor_latency_ms(ms(monitor));
            }
            self.controller.tick();
            self.feed_scopes();
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let mut effect = self.tray();
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let mut effect = Effect::None;
        if self.remote.is_none()
            && let Some(t) = self.remote_retry
            && t.elapsed() >= std::time::Duration::from_secs(1)
        {
            self.remote_retry = Some(Instant::now());
            self.remote = remote::Remote::start(true);
        }
        if let Some(r) = &mut self.remote {
            let e = r.frame(&mut self.controller, dt);
            // A quit from the tray or remote control always wins.
            if e != Effect::None && effect != Effect::Quit {
                effect = e;
            }
        }
        // A noise removal choice from the tray, the menu bar or remote
        // control that needs the model brings the window up with the dialog.
        let prompted = ModelDialog::is_open(self.controller.state());
        if prompted && !self.prompted && effect == Effect::None {
            effect = Effect::Show;
        }
        self.prompted = prompted;
        if effect == Effect::Quit {
            self.quit = true;
        }
        effect
    }

    /// 30 frames a second for the meters; 4 in standby once the needles
    /// have fallen to rest, since nothing moves (input still repaints at
    /// once).
    fn repaint_period(&self) -> std::time::Duration {
        let resting = std::time::Duration::from_secs(2);
        if self.controller.standby_for().is_some_and(|t| t >= resting) {
            std::time::Duration::from_millis(250)
        } else {
            std::time::Duration::from_millis(33)
        }
    }

    fn cursor(&mut self) -> Cursor {
        if ModelDialog::is_open(self.controller.state()) {
            self.dialog.cursor()
        } else {
            self.view().cursor()
        }
    }

    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else {
            return;
        };
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            if let Err(e) = save_png(&shot.out, &image) {
                eprintln!("voice-strip: screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        shot.frames += 1;
        // A few frames for the window to reach its size before capturing.
        if shot.frames == 8 && !shot.requested {
            shot.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }
}

fn save_png(path: &std::path::Path, image: &egui::ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(file),
        image.size[0] as u32,
        image.size[1] as u32,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|p| p.to_srgba_unmultiplied())
        .collect();
    w.write_image_data(&bytes).map_err(|e| e.to_string())
}

impl eframe::App for VoiceStrip {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let dt = self.last.elapsed().as_secs_f32().min(0.1);
        self.last = Instant::now();
        self.handle_input(ctx);
        let state = self.controller.state().clone();
        self.view().tick(&state, dt);

        // The window is the canvas: follow the active view's size, scaled
        // down to fit the screen (the advanced view is taller than a 14"
        // MacBook's). A fixed-size window is resized through its min/max
        // bounds, which is what Wayland honours.
        let size = self.view_size();
        if size != self.size {
            self.size = size;
            self.scale = if self.shot.is_none() {
                fit_scale(ctx, size, &mut self.screen)
            } else {
                1.0
            };
            let v = egui::vec2(size.0, size.1) * self.scale;
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(v));
            ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(v));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(v));
        }

        let screen = ctx.content_rect();
        let shapes = {
            let mut p = EguiPainter::new(ctx, screen);
            p.scale(self.scale, self.scale);
            let view: &mut dyn View = match self.active {
                ViewKind::Standard => &mut self.standard,
                ViewKind::Advanced => &mut self.advanced,
            };
            view.layout(size.0, size.1, &state, &mut p);
            let key = StaticKey {
                view: self.active as u8,
                view_key: view.static_key(),
                size: (size.0.to_bits(), size.1.to_bits()),
                scale: self.scale.to_bits(),
                pixels_per_point: ctx.pixels_per_point().to_bits(),
            };
            if self.static_layer.as_ref().is_none_or(|l| l.key != key) {
                let mut sp = EguiPainter::new(ctx, screen);
                sp.scale(self.scale, self.scale);
                view.paint_static(&mut sp);
                self.static_layer = Some(StaticLayer::tessellate(ctx, key, sp.shapes));
            }
            if let Some(layer) = &self.static_layer {
                for (clip, mesh) in &layer.meshes {
                    p.shapes.push((*clip, egui::Shape::Mesh(mesh.clone())));
                }
                p.shapes.extend(layer.text.iter().cloned());
            }
            view.paint_dynamic(&mut p, &state);
            if ModelDialog::is_open(&state) {
                let dark = self.active == ViewKind::Advanced;
                self.dialog.paint(&mut p, size.0, size.1, &state, dark);
            }
            p.shapes
        };
        let layer = ctx.layer_painter(egui::LayerId::background());
        for (clip, shape) in shapes {
            layer.with_clip_rect(clip).add(shape);
        }

        ctx.set_cursor_icon(match self.cursor() {
            Cursor::Default => egui::CursorIcon::Default,
            Cursor::Pointer => egui::CursorIcon::PointingHand,
            Cursor::EwResize => egui::CursorIcon::ResizeHorizontal,
        });
        self.device_menu(ctx);
        self.tooltip(ctx);
        self.screenshot(ctx);
        // Frames are paced by `repaint_period` (in `logic`); a screenshot
        // needs a few frames in a row.
        if self.shot.is_some() {
            ctx.request_repaint();
        }
    }
}

/// Room kept free around the window inside the usable screen, points.
const SCREEN_EDGE: f32 = 16.0;
/// Where the usable area is unknown: room for panels and docks, points.
const SCREEN_MARGIN: (f32, f32) = (40.0, 140.0);

/// Scale that makes a view of `size` points fit the usable screen, at most
/// 1. The usable size is taken once into `screen`.
fn fit_scale(ctx: &egui::Context, size: (f32, f32), screen: &mut Option<egui::Vec2>) -> f32 {
    if screen.is_none() {
        *screen = usable_screen().or_else(|| {
            ctx.input(|i| i.viewport().monitor_size)
                .map(|m| m - egui::vec2(SCREEN_MARGIN.0, SCREEN_MARGIN.1))
        });
    }
    let Some(usable) = *screen else {
        return 1.0;
    };
    ((usable.x - SCREEN_EDGE) / size.0)
        .min((usable.y - SCREEN_EDGE) / size.1)
        .clamp(0.5, 1.0)
}

/// The screen area free of the menu bar and the Dock (NSScreen
/// visibleFrame), in points.
#[cfg(target_os = "macos")]
fn usable_screen() -> Option<egui::Vec2> {
    let mtm = objc2::MainThreadMarker::new()?;
    let frame = objc2_app_kit::NSScreen::mainScreen(mtm)?.visibleFrame();
    Some(egui::vec2(
        frame.size.width as f32,
        frame.size.height as f32,
    ))
}

#[cfg(not(target_os = "macos"))]
fn usable_screen() -> Option<egui::Vec2> {
    None
}

#[cfg(target_os = "linux")]
fn platform_backend(sim: bool) -> Box<dyn AudioBackend> {
    if sim {
        Box::new(engine::sim::SimBackend)
    } else {
        Box::new(vdev_linux::PipeWireBackend::new())
    }
}

#[cfg(target_os = "macos")]
fn platform_backend(sim: bool) -> Box<dyn AudioBackend> {
    if sim {
        Box::new(engine::sim::SimBackend)
    } else {
        Box::new(vdev_macos::MacBackend::new())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_backend(sim: bool) -> Box<dyn AudioBackend> {
    if !sim {
        eprintln!("voice-strip: no audio backend on this platform, using --backend sim");
    }
    Box::new(engine::sim::SimBackend)
}

fn main() -> eframe::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("ctl") => std::process::exit(app_core::remote::client::cli(&argv[1..])),
        Some("driver") => std::process::exit(driver::cli(&argv[1..])),
        // For clients that draw with the app's type while it is closed.
        Some("--fonts-dir") => {
            println!("{}", fonts::files_dir().display());
            return Ok(());
        }
        _ => {}
    }
    let args = parse_args();
    let (handle, processor) = engine::new_engine();
    let mut controller = Controller::new(handle);
    let mut standard_view = StandardView::new();
    let mut advanced_view = AdvancedView::new();

    let screenshot = args.screenshot.is_some();
    let mut active = if args.advanced {
        ViewKind::Advanced
    } else {
        ViewKind::Standard
    };
    let mut backend = platform_backend(args.sim || screenshot);
    let inputs = backend.list_inputs().unwrap_or_default();
    let outputs = backend.list_outputs().unwrap_or_default();
    if args.list_devices {
        println!("inputs (use with --input):");
        for d in &inputs {
            println!("  {}  ({})", d.id, d.label);
        }
        println!("outputs (use with --headphones):");
        for d in &outputs {
            println!("  {}  ({})", d.id, d.label);
        }
        return Ok(());
    }

    let mut running = None;
    let mut shot = None;
    if let Some((kind, out)) = &args.screenshot {
        // Deterministic demo state, no audio.
        active = *kind;
        let mut demo = ui_kit::demo::demo_state();
        if args.monitor {
            demo.monitor.enabled = true;
            demo.monitor.with_denoise = true;
            demo.monitor_latency_ms = 24.2;
        }
        if args.model_prompt {
            demo.model = app_core::ModelState::Missing;
            demo.neural_denoise = None;
            demo.model_prompt = Some(app_core::ModelUse::Noise(DenoiseLevel::Medium));
        }
        *controller.state_mut() = demo;
        advanced_view.set_demo_spectrum();
        let st = controller.state().clone();
        for _ in 0..300 {
            standard_view.tick(&st, 1.0 / 60.0);
            advanced_view.tick(&st, 1.0 / 60.0);
        }
        shot = Some(Shot {
            out: out.clone(),
            frames: 0,
            requested: false,
        });
    } else {
        let r = backend
            .start(processor, args.input.as_deref(), args.headphones.as_deref())
            .map_err(|e| eframe::Error::AppCreation(Box::new(e)))?;
        let (io_main, io_mon) = r.io_latency_samples();
        controller.set_io_latency_ms(io_main as f32 * 1000.0 / dsp::SAMPLE_RATE as f32);
        controller.set_io_monitor_latency_ms(io_mon as f32 * 1000.0 / dsp::SAMPLE_RATE as f32);
        let label_of = |list: &[DeviceInfo], id: &str| {
            list.iter()
                .find(|d| d.id == id)
                .map_or_else(|| id.to_string(), |d| d.label.clone())
        };
        let st = controller.state_mut();
        st.virtual_device = r.virtual_device_name().into();
        if let Some(id) = &args.headphones {
            st.output_device = label_of(&outputs, id);
        }
        if let Some(dir) = app_core::profiles::default_dir() {
            controller.set_profiles_dir(dir);
        }
        if let Some(dir) = app_core::updates::settings_dir() {
            controller.enable_update_check(dir);
        }
        let label = args
            .input
            .as_deref()
            .map_or_else(|| "Default microphone".into(), |id| label_of(&inputs, id));
        controller.select_input(args.input.as_deref(), label);
        // macOS: the virtual microphone driver missing or older than the
        // one shipped with this binary.
        if let Some(n) = driver::notice() {
            controller.state_mut().notice = Some(n);
        }
        if let Some(p) = args.preset {
            controller.set_preset(p);
        }
        if let Some(n) = args.noise {
            controller.set_neural_denoise(n);
        }
        let intensity = controller.state().intensity;
        controller.set_intensity(intensity);
        running = Some(r);
    }
    let input_index = args
        .input
        .as_deref()
        .and_then(|id| inputs.iter().position(|d| d.id == id));
    let output_index = args
        .headphones
        .as_deref()
        .and_then(|id| outputs.iter().position(|d| d.id == id));

    #[cfg(target_os = "linux")]
    let tray = if screenshot {
        None
    } else {
        start_tray(controller.state())
    };
    #[cfg(target_os = "macos")]
    let menubar = if screenshot {
        None
    } else {
        menubar::MenuBar::new(tray::TrayState::of(controller.state(), false))
            .map_err(|e| eprintln!("voice-strip: no menu bar item ({e})"))
            .ok()
    };
    let remote = if screenshot {
        None
    } else {
        remote::Remote::start(false)
    };
    let remote_retry = (!screenshot).then(Instant::now);
    // The app lives outside the window: closing the window leaves the
    // processing, the tray and remote control running, and `show` (tray,
    // `voice-strip ctl show`) opens it again. Quit ends everything.
    let app = Rc::new(RefCell::new(VoiceStrip {
        controller,
        standard: standard_view,
        advanced: advanced_view,
        dialog: ModelDialog::new(),
        prompted: false,
        active,
        backend: running,
        static_layer: None,
        devices: backend,
        inputs,
        outputs,
        input_index,
        output_index,
        menu: None,
        last: Instant::now(),
        size: (0.0, 0.0),
        scale: 1.0,
        screen: None,
        pressed_on_canvas: false,
        hover_since: None,
        scroll_acc: 0.0,
        shot,
        #[cfg(target_os = "linux")]
        tray,
        #[cfg(target_os = "macos")]
        menubar,
        remote,
        remote_retry,
        last_service: Instant::now(),
        quit: false,
        window_open: false,
    }));
    // macOS: one window for the whole run, hidden for --background and
    // whenever the user hides it.
    // The `return` is the last statement only on macOS.
    #[cfg(target_os = "macos")]
    #[allow(clippy::needless_return)]
    {
        let visible = screenshot || !args.background;
        app.borrow_mut().window_open = visible;
        run_window(&app, args.scale, screenshot, visible)?;
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    let mut window = screenshot || !args.background;
    #[cfg(not(target_os = "macos"))]
    loop {
        if window {
            app.borrow_mut().window_open = true;
            run_window(&app, args.scale, screenshot, true)?;
            app.borrow_mut().window_open = false;
            if screenshot || app.borrow().quit {
                return Ok(());
            }
        }
        match run_background(&app) {
            Effect::Show => window = true,
            _ => return Ok(()),
        }
    }
}

/// Shows the window until it is closed.
fn run_window(
    app: &Rc<RefCell<VoiceStrip>>,
    scale: f32,
    screenshot: bool,
    visible: bool,
) -> eframe::Result<()> {
    let initial = {
        let mut a = app.borrow_mut();
        // A new window: request its size again and restart the frame clock.
        a.size = (0.0, 0.0);
        a.last = Instant::now();
        a.view_size()
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_title("Voice Strip")
            .with_inner_size(initial)
            .with_visible(visible)
            .with_resizable(false)
            .with_decorations(false)
            .with_transparent(true),
        // GPU rendering through wgpu: Vulkan on Linux, Metal on macOS.
        renderer: eframe::Renderer::Wgpu,
        // Return when the window closes, so the app can carry on without it.
        run_and_return: true,
        ..Default::default()
    };
    let app = app.clone();
    eframe::run_native(
        "Voice Strip",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            if let Some(rs) = &cc.wgpu_render_state {
                let info = rs.adapter.get_info();
                eprintln!(
                    "voice-strip: GPU {} ({:?}, {:?})",
                    info.name, info.backend, info.device_type
                );
            }
            if screenshot {
                cc.egui_ctx.set_pixels_per_point(scale);
            }
            Ok(Box::new(Window(app)))
        }),
    )
}

/// Runs without a window, about 30 times a second, until something asks
/// for the window or for quitting. Not on macOS, where the window is hidden
/// instead.
#[cfg(not(target_os = "macos"))]
fn run_background(app: &Rc<RefCell<VoiceStrip>>) -> Effect {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(33));
        // Already hidden: nothing to do for a hide.
        let effect = app.borrow_mut().service();
        if !matches!(effect, Effect::None | Effect::Hide) {
            return effect;
        }
    }
}

/// The window's view of the app, which outlives it.
struct Window(Rc<RefCell<VoiceStrip>>);

impl eframe::App for Window {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.0.borrow().clear_color(visuals)
    }

    /// The service runs here, before every frame and also while the window
    /// is covered or minimized, when eframe paints nothing: macOS stops
    /// redrawing an occluded window, and remote control, the menu bar and
    /// the meters must go on.
    /// On macOS a hidden window stays, so this goes on while it is hidden
    /// too (eframe runs it 10 times a second for an invisible window).
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut app = self.0.borrow_mut();
        // macOS: a close that is not a quit hides the window instead.
        if cfg!(target_os = "macos")
            && ctx.input(|i| i.viewport().close_requested())
            && !app.quit
            && app.shot.is_none()
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            app.hide_window(ctx);
        }
        match app.service() {
            Effect::None => {}
            Effect::Show => app.show_window(ctx),
            Effect::Hide => app.hide_window(ctx),
            Effect::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
        drop(app);
        let period = self.0.borrow().repaint_period();
        ctx.request_repaint_after(period);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.borrow_mut().ui(ui, frame);
    }
}

/// What the static layer depends on.
#[derive(Clone, Copy, PartialEq)]
struct StaticKey {
    view: u8,
    view_key: u64,
    size: (u32, u32),
    scale: u32,
    pixels_per_point: u32,
}

/// The static layer's paths as triangles, so drawing them costs a copy,
/// not a tessellation. Text stays text: its triangles point into the font
/// atlas, which egui repacks as glyphs come and go, and egui caches laid
/// out text anyway. Text is drawn after the paths, as static labels sit on
/// the faceplate.
struct StaticLayer {
    key: StaticKey,
    meshes: Vec<(egui::Rect, std::sync::Arc<egui::Mesh>)>,
    text: Vec<(egui::Rect, egui::Shape)>,
}

impl StaticLayer {
    fn tessellate(
        ctx: &egui::Context,
        key: StaticKey,
        shapes: Vec<(egui::Rect, egui::Shape)>,
    ) -> Self {
        let (text, shapes): (Vec<_>, Vec<_>) = shapes
            .into_iter()
            .partition(|(_, s)| matches!(s, egui::Shape::Text(_)));
        let shapes = shapes
            .into_iter()
            .map(|(clip_rect, shape)| egui::epaint::ClippedShape { clip_rect, shape })
            .collect();
        let meshes = ctx
            .tessellate(shapes, ctx.pixels_per_point())
            .into_iter()
            .filter_map(|p| match p.primitive {
                egui::epaint::Primitive::Mesh(m) => Some((p.clip_rect, std::sync::Arc::new(m))),
                egui::epaint::Primitive::Callback(_) => None,
            })
            .collect();
        Self { key, meshes, text }
    }
}

/// Opens `url` in the default browser.
fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener).arg(url).spawn() {
        // Reaped off the UI thread, so it does not linger as a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("voice-strip: cannot open {url} ({e})"),
    }
}

/// Starts the tray icon; without a tray host the app runs without it.
#[cfg(target_os = "linux")]
fn start_tray(st: &app_core::UiState) -> Option<Tray> {
    use ksni::blocking::TrayMethods;
    let state = tray::TrayState::of(st, false);
    let (tx, rx) = std::sync::mpsc::channel();
    match tray::VoiceStripTray::new(tx, state).spawn() {
        Ok(h) => Some((h, rx, state)),
        Err(e) => {
            eprintln!("voice-strip: no tray icon ({e})");
            None
        }
    }
}
