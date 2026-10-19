//! Input events coming from the host and actions going back to it.

use app_core::{DenoiseLevel, PresetId};
use dsp::Module;
use engine::MonitorConfig;

use crate::painter::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Space,
    Enter,
    Escape,
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    PointerMove(Point),
    PointerDown(Point),
    PointerUp(Point),
    PointerLeave,
    /// Vertical scroll in notches (positive = towards the user).
    Scroll(Point, f32),
    Key(Key),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Standard,
    Advanced,
}

/// What the user asked for. The host applies it to `app_core::Controller`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    SetOn(bool),
    /// Mute: others hear silence.
    SetMuted(bool),
    SetAbOriginal(bool),
    SetPreset(PresetId),
    SetIntensity(u8),
    /// Neural noise removal level, `None` for off.
    SetNeuralDenoise(Option<DenoiseLevel>),
    SetMonitor(MonitorConfig),
    SetModuleEnabled(Module, bool),
    SwitchView(ViewKind),
    Recalibrate,
    /// Open the microphone chooser, anchored to this rectangle (view
    /// coordinates).
    ChooseInput(Rect),
    /// Open the headphone chooser, anchored to this rectangle.
    ChooseOutput(Rect),
    /// Download the noise removal model (the user agreed in the dialog).
    DownloadModel,
    /// Close the model dialog, cancelling a download in progress.
    DismissModelPrompt,
    /// Open the release page of the newer version.
    OpenUpdate,
}

/// Cursor shape the host should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Default,
    Pointer,
    EwResize,
}

/// Press/release tracking shared by all clickable widgets: a click fires
/// when the pointer is released over the widget it was pressed on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clicker<Id: Copy + PartialEq> {
    pub hover: Option<Id>,
    pub pressed: Option<Id>,
}

impl<Id: Copy + PartialEq> Clicker<Id> {
    pub fn new() -> Self {
        Self {
            hover: None,
            pressed: None,
        }
    }

    pub fn moved(&mut self, hit: Option<Id>) {
        self.hover = hit;
    }

    pub fn down(&mut self, hit: Option<Id>) {
        self.pressed = hit;
    }

    /// Returns the clicked id, if any.
    pub fn up(&mut self, hit: Option<Id>) -> Option<Id> {
        let p = self.pressed.take();
        match (p, hit) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        }
    }

    pub fn leave(&mut self) {
        self.hover = None;
        self.pressed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_requires_release_on_same_widget() {
        let mut c: Clicker<u8> = Clicker::new();
        c.down(Some(1));
        assert_eq!(c.up(Some(2)), None);
        c.down(Some(1));
        assert_eq!(c.up(Some(1)), Some(1));
        c.down(None);
        assert_eq!(c.up(Some(1)), None);
    }
}
