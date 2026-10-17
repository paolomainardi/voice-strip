use app_core::UiState;

use crate::input::{Action, Cursor, Event};
use crate::painter::{Painter, Point};

/// A full window content. The host drives it once per frame:
/// `tick` (ballistics), then `paint_static` whenever `static_key` changed
/// since the cached layer was rendered, then `paint_dynamic` on top.
pub trait View {
    /// Natural size in logical pixels.
    fn preferred_size(&self) -> (f32, f32);
    /// Lays the view out for the given size and state. `p` is only used
    /// to measure text; nothing is drawn. Called by the host before
    /// painting each frame; cheap when nothing changed.
    fn layout(&mut self, w: f32, h: f32, state: &UiState, p: &mut dyn Painter);
    /// Changes whenever the static layer must be re-rendered.
    fn static_key(&self) -> u64;
    /// Advances meter ballistics by `dt` seconds.
    fn tick(&mut self, state: &UiState, dt: f32);
    /// Paints everything that does not change between frames at the
    /// current layout: backgrounds, bezels, scales, static labels.
    fn paint_static(&mut self, p: &mut dyn Painter);
    /// Paints the state-dependent parts on top of the static layer.
    fn paint_dynamic(&mut self, p: &mut dyn Painter, state: &UiState);
    /// Handles one input event, pushing resulting actions to `out`.
    fn handle(&mut self, ev: Event, state: &UiState, out: &mut Vec<Action>);
    fn cursor(&self) -> Cursor;
    /// Tooltip text for the point, if any (for example the full name of a
    /// device whose label is truncated).
    fn tooltip(&self, _p: Point, _state: &UiState) -> Option<String> {
        None
    }
}
