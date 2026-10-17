//! Toolkit-neutral drawing interface. Frontends implement it with Cairo,
//! Core Graphics, or anything else that can fill paths and draw text.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// `0xRRGGBB`.
    pub const fn hex(v: u32) -> Self {
        Self {
            r: ((v >> 16) & 0xff) as f32 / 255.0,
            g: ((v >> 8) & 0xff) as f32 / 255.0,
            b: (v & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }

    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.x + self.w && p.y >= self.y && p.y < self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Shrinks the rectangle by `d` on every side.
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Oswald,
    PlexMono,
    PlexSans,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub family: Family,
    /// CSS weight (400, 500, 600).
    pub weight: u16,
    /// Font size in logical pixels.
    pub size: f32,
    /// Extra advance per character, logical pixels (CSS `letter-spacing`).
    pub letter_spacing: f32,
    pub color: Color,
}

impl TextStyle {
    pub const fn new(family: Family, weight: u16, size: f32, color: Color) -> Self {
        Self {
            family,
            weight,
            size,
            letter_spacing: 0.0,
            color,
        }
    }

    pub const fn spacing(self, letter_spacing: f32) -> Self {
        Self {
            letter_spacing,
            ..self
        }
    }

    pub const fn color(self, color: Color) -> Self {
        Self { color, ..self }
    }
}

/// Size of a laid-out single line of text. `height` is the line box height
/// (ascent + descent), which is what CSS uses for `line-height: normal`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextSize {
    pub width: f32,
    pub height: f32,
    /// Distance from the top of the line box to the baseline.
    pub ascent: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub color: Color,
    pub width: f32,
    pub cap: LineCap,
    /// Dash pattern, empty for solid.
    pub dash: [f32; 2],
}

impl Stroke {
    pub const fn new(color: Color, width: f32) -> Self {
        Self {
            color,
            width,
            cap: LineCap::Butt,
            dash: [0.0; 2],
        }
    }

    pub const fn cap(self, cap: LineCap) -> Self {
        Self { cap, ..self }
    }

    pub const fn dash(self, on: f32, off: f32) -> Self {
        Self {
            dash: [on, off],
            ..self
        }
    }
}

/// All coordinates are logical pixels. Text is drawn with `(x, y)` at the
/// top-left of its line box (CSS semantics) so that layout can be expressed
/// as line boxes; `align` moves the anchor horizontally.
pub trait Painter {
    fn save(&mut self);
    fn restore(&mut self);
    fn translate(&mut self, x: f32, y: f32);
    fn scale(&mut self, sx: f32, sy: f32);
    /// Rotates the current transform by `radians` around the origin.
    fn rotate(&mut self, radians: f32);
    fn clip_rect(&mut self, r: Rect);
    fn clip_rounded_rect(&mut self, r: Rect, radius: f32);

    fn begin_path(&mut self);
    fn move_to(&mut self, x: f32, y: f32);
    fn line_to(&mut self, x: f32, y: f32);
    fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32);
    /// Arc around `(cx, cy)` from `a0` to `a1` radians, clockwise in screen
    /// space (y down), connecting from the current point when there is one.
    fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32);
    fn close_path(&mut self);
    fn rect(&mut self, r: Rect);
    fn rounded_rect(&mut self, r: Rect, radius: f32);
    fn circle(&mut self, cx: f32, cy: f32, r: f32);

    /// Fills and consumes the current path.
    fn fill(&mut self, color: Color);
    /// Strokes and consumes the current path.
    fn stroke(&mut self, stroke: Stroke);

    fn fill_text(&mut self, text: &str, x: f32, y: f32, style: &TextStyle, align: Align);
    fn measure_text(&mut self, text: &str, style: &TextStyle) -> TextSize;

    // Convenience helpers built on the primitives.

    fn fill_rect(&mut self, r: Rect, color: Color) {
        self.begin_path();
        self.rect(r);
        self.fill(color);
    }

    fn fill_rounded_rect(&mut self, r: Rect, radius: f32, color: Color) {
        self.begin_path();
        self.rounded_rect(r, radius);
        self.fill(color);
    }

    /// Strokes a rounded rectangle whose outer edge is `r`, the way CSS
    /// draws a `border` of `width` inside the box.
    fn border_rounded_rect(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        self.begin_path();
        self.rounded_rect(r.inset(width / 2.0), (radius - width / 2.0).max(0.0));
        self.stroke(Stroke::new(color, width));
    }

    /// Fills the area between the polyline `points` and the horizontal
    /// line `y = baseline`, fading from `top` at the polyline to `bottom`
    /// at the baseline. This default draws each segment as a flat quad in
    /// `top`; painters with per-vertex colour override it.
    fn fill_under(&mut self, points: &[Point], baseline: f32, top: Color, _bottom: Color) {
        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            self.begin_path();
            self.move_to(a.x, a.y.min(baseline));
            self.line_to(b.x, b.y.min(baseline));
            self.line_to(b.x, baseline);
            self.line_to(a.x, baseline);
            self.close_path();
            self.fill(top);
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: Color) {
        self.begin_path();
        self.circle(cx, cy, r);
        self.fill(color);
    }

    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, stroke: Stroke) {
        self.begin_path();
        self.move_to(x0, y0);
        self.line_to(x1, y1);
        self.stroke(stroke);
    }

    /// Draws `text` with its baseline at `y` (SVG `<text>` semantics).
    fn text_baseline(&mut self, text: &str, x: f32, y: f32, style: &TextStyle, align: Align) {
        let size = self.measure_text(text, style);
        self.fill_text(text, x, y - size.ascent, style, align);
    }

    /// Draws `text` vertically centered in `r` (CSS flex `align-items: center`).
    fn text_in_rect(&mut self, text: &str, r: Rect, style: &TextStyle, align: Align) {
        let size = self.measure_text(text, style);
        let y = r.y + (r.h - size.height) / 2.0;
        let x = match align {
            Align::Left => r.x,
            Align::Center => r.x + r.w / 2.0,
            Align::Right => r.right(),
        };
        self.fill_text(text, x, y, style, align);
    }
}
