//! macOS menu bar item: the same menu as the Linux tray, built with
//! `tray-icon` (an NSStatusItem) on the main thread. Menu clicks arrive as
//! events that [`MenuBar::commands`] turns into [`TrayCmd`]s; the item
//! mirrors the controller state through [`MenuBar::update`].
//!
//! AppKit delivers the clicks only while its event loop runs, so on macOS
//! the window is hidden, never closed, and eframe's loop keeps running.

use app_core::{DenoiseLevel, PresetId};
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::tray::{TrayCmd, TrayState};

pub struct MenuBar {
    icon: TrayIcon,
    on: CheckMenuItem,
    mute: CheckMenuItem,
    presets: Vec<(PresetId, CheckMenuItem)>,
    noise: Vec<(Option<DenoiseLevel>, CheckMenuItem)>,
    show: MenuItem,
    /// "Update to x.y.z…", in the menu only while a newer release exists.
    update: MenuItem,
    menu: Menu,
    quit: MenuId,
    state: TrayState,
}

/// The menu bar glyph: the app's three level bars between two ticks, as a
/// template image (macOS tints it for light and dark menu bars).
fn glyph(state: TrayState) -> Icon {
    Icon::from_rgba(
        crate::icons::glyph(36, crate::icons::Status::of(state), true),
        36,
        36,
    )
    .expect("valid glyph")
}

impl MenuBar {
    /// Builds the item. Call on the main thread.
    pub fn new(state: TrayState) -> Result<Self, String> {
        // The status bar needs the application object, which no window has
        // created yet at start.
        let mtm = objc2::MainThreadMarker::new().ok_or("not on the main thread")?;
        objc2_app_kit::NSApplication::sharedApplication(mtm);
        let on = CheckMenuItem::new("Processing on", true, state.on, None);
        let mute = CheckMenuItem::new("Mute", true, state.muted, None);
        let presets: Vec<_> = PresetId::ALL
            .iter()
            .map(|&p| {
                let label = app_core::presets::builtin(p).label;
                (p, CheckMenuItem::new(label, true, p == state.preset, None))
            })
            .collect();
        let noise: Vec<_> = std::iter::once(None)
            .chain(DenoiseLevel::ALL.iter().map(|&l| Some(l)))
            .map(|l| {
                let label = l.map_or("Off", |l| l.label());
                (l, CheckMenuItem::new(label, true, l == state.noise, None))
            })
            .collect();
        let show = MenuItem::new(state.window_label(), true, None);
        let quit = MenuItem::new("Quit", true, None);
        let update = MenuItem::new(state.update_label().unwrap_or_default(), true, None);

        let preset_menu = Submenu::new("Preset", true);
        for (_, item) in &presets {
            preset_menu.append(item).map_err(|e| e.to_string())?;
        }
        let noise_menu = Submenu::new("Noise removal", true);
        for (_, item) in &noise {
            noise_menu.append(item).map_err(|e| e.to_string())?;
        }
        let menu = Menu::new();
        menu.append_items(&[
            &on,
            &mute,
            &PredefinedMenuItem::separator(),
            &preset_menu,
            &noise_menu,
            &PredefinedMenuItem::separator(),
            &show,
            &quit,
        ])
        .map_err(|e| e.to_string())?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_icon_templated(glyph(state))
            .with_tooltip("Voice Strip")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            icon,
            on,
            mute,
            presets,
            noise,
            show,
            update,
            menu,
            quit: quit.id().clone(),
            state,
        })
    }

    /// Commands picked in the menu since the last call.
    pub fn commands(&self) -> Vec<TrayCmd> {
        let mut out = Vec::new();
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id;
            if id == *self.on.id() {
                out.push(TrayCmd::SetOn(!self.state.on));
            } else if id == *self.mute.id() {
                out.push(TrayCmd::SetMuted(!self.state.muted));
            } else if id == *self.update.id() {
                out.push(TrayCmd::OpenUpdate);
            } else if id == *self.show.id() {
                out.push(TrayCmd::ToggleWindow);
            } else if id == self.quit {
                out.push(TrayCmd::Quit);
            } else if let Some((p, _)) = self.presets.iter().find(|(_, i)| *i.id() == id) {
                out.push(TrayCmd::SetPreset(*p));
            } else if let Some((l, _)) = self.noise.iter().find(|(_, i)| *i.id() == id) {
                out.push(TrayCmd::SetNoise(*l));
            }
        }
        out
    }

    /// Mirrors `state` into the check marks. AppKit toggles a check item on
    /// click by itself, so every mark is set, not only the changed ones.
    pub fn update(&mut self, state: TrayState) {
        if state.update != self.state.update {
            let _ = self.menu.remove(&self.update);
            if let Some(label) = state.update_label() {
                self.update.set_text(label);
                // Just before Show window.
                let show = self.show.id().clone();
                let at = self
                    .menu
                    .items()
                    .iter()
                    .position(|i| *i.id() == show)
                    .unwrap_or(0);
                let _ = self.menu.insert(&self.update, at);
            }
        }
        if crate::icons::Status::of(state) != crate::icons::Status::of(self.state)
            && let Err(error) = self.icon.set_icon_templated(Some(glyph(state)))
        {
            eprintln!("voice-strip: menu bar icon: {error}");
        }
        self.state = state;
        self.on.set_checked(state.on);
        self.mute.set_checked(state.muted);
        self.show.set_text(state.window_label());
        for (p, item) in &self.presets {
            item.set_checked(*p == state.preset);
        }
        for (l, item) in &self.noise {
            item.set_checked(*l == state.noise);
        }
    }
}
