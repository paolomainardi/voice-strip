//! `ui_kit::Painter` on top of egui's epaint: paths are flattened in view
//! coordinates, transformed by the current affine matrix, and collected as
//! epaint shapes grouped by clip rectangle.
//!
//! Supported transforms are similarities (translation, rotation, uniform
//! scale), which is what the `ui-kit` views use. Under them stroke widths
//! and glyph sizes scale correctly. Non-uniform scale, skew and reflection
//! are not supported: strokes and text use the mean scale factor, text is
//! never rotated, and clips are the axis-aligned bounding box of the
//! transformed rectangle.

use eframe::egui::{
    self, Color32, FontFamily, FontId, Mesh, Pos2, Shape, epaint::PathShape, text::LayoutJob,
};
use ui_kit::{Align, Color, Family, LineCap, Painter, Point, Rect, Stroke, TextSize, TextStyle};

/// Affine transform: x' = a x + c y + e, y' = b x + d y + f.
#[derive(Clone, Copy, Debug)]
struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Affine {
    const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn apply(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Mean scale factor, for stroke widths and font sizes.
    fn scale(&self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

#[derive(Clone, Copy)]
struct State {
    xf: Affine,
    clip: egui::Rect,
}

pub struct EguiPainter<'a> {
    ctx: &'a egui::Context,
    state: State,
    stack: Vec<State>,
    /// Subpaths in screen coordinates, with their closed flag.
    path: Vec<(Vec<Pos2>, bool)>,
    /// Current point in view coordinates.
    cur: Option<(f32, f32)>,
    pub shapes: Vec<(egui::Rect, Shape)>,
}

fn color32(c: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        (c.a * 255.0).round() as u8,
    )
}

/// Font family registered for a family/weight pair (see `fonts.rs`).
pub fn family_name(family: Family, weight: u16) -> &'static str {
    match (family, weight) {
        (Family::Oswald, w) if w >= 600 => "oswald-600",
        (Family::Oswald, w) if w >= 500 => "oswald-500",
        (Family::Oswald, _) => "oswald-400",
        (Family::PlexMono, w) if w >= 500 => "plexmono-500",
        (Family::PlexMono, _) => "plexmono-400",
        (Family::PlexSans, w) if w >= 600 => "plexsans-600",
        (Family::PlexSans, w) if w >= 500 => "plexsans-500",
        (Family::PlexSans, _) => "plexsans-400",
    }
}

impl<'a> EguiPainter<'a> {
    pub fn new(ctx: &'a egui::Context, screen: egui::Rect) -> Self {
        Self {
            ctx,
            state: State {
                xf: Affine::IDENTITY,
                clip: screen,
            },
            stack: Vec::new(),
            path: Vec::new(),
            cur: None,
            shapes: Vec::new(),
        }
    }

    fn push_point(&mut self, x: f32, y: f32) {
        let p = self.state.xf.apply(x, y);
        match self.path.last_mut() {
            Some((pts, false)) => pts.push(p),
            _ => self.path.push((vec![p], false)),
        }
        self.cur = Some((x, y));
    }

    fn layout(&self, text: &str, style: &TextStyle, scale: f32) -> std::sync::Arc<egui::Galley> {
        let mut job = LayoutJob::default();
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: FontId::new(
                    style.size * scale,
                    FontFamily::Name(family_name(style.family, style.weight).into()),
                ),
                extra_letter_spacing: style.letter_spacing * scale,
                color: color32(style.color),
                ..Default::default()
            },
        );
        self.ctx.fonts_mut(|f| f.layout_job(job))
    }
}

impl Painter for EguiPainter<'_> {
    fn save(&mut self) {
        self.stack.push(self.state);
    }

    fn restore(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    fn translate(&mut self, x: f32, y: f32) {
        let m = &mut self.state.xf;
        m.e += m.a * x + m.c * y;
        m.f += m.b * x + m.d * y;
    }

    fn scale(&mut self, sx: f32, sy: f32) {
        let m = &mut self.state.xf;
        m.a *= sx;
        m.b *= sx;
        m.c *= sy;
        m.d *= sy;
    }

    fn rotate(&mut self, radians: f32) {
        let (s, c) = radians.sin_cos();
        let m = self.state.xf;
        self.state.xf = Affine {
            a: m.a * c + m.c * s,
            b: m.b * c + m.d * s,
            c: -m.a * s + m.c * c,
            d: -m.b * s + m.d * c,
            e: m.e,
            f: m.f,
        };
    }

    fn clip_rect(&mut self, r: Rect) {
        let corners = [
            self.state.xf.apply(r.x, r.y),
            self.state.xf.apply(r.right(), r.y),
            self.state.xf.apply(r.right(), r.bottom()),
            self.state.xf.apply(r.x, r.bottom()),
        ];
        let rr = egui::Rect::from_points(&corners);
        self.state.clip = self.state.clip.intersect(rr);
    }

    fn clip_rounded_rect(&mut self, r: Rect, _radius: f32) {
        // epaint clips to rectangles only; the rounded corners are not
        // clipped (the shapes drawn inside stay clear of them).
        self.clip_rect(r);
    }

    fn begin_path(&mut self) {
        self.path.clear();
        self.cur = None;
    }

    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.state.xf.apply(x, y);
        self.path.push((vec![p], false));
        self.cur = Some((x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push_point(x, y);
    }

    fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x0, y0) = self.cur.unwrap_or((x1, y1));
        const N: usize = 16;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let u = 1.0 - t;
            let px = u * u * u * x0 + 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t * x;
            let py = u * u * u * y0 + 3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t * y;
            self.push_point(px, py);
        }
    }

    fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let mut a1 = a1;
        while a1 < a0 {
            a1 += std::f32::consts::TAU;
        }
        let span = a1 - a0;
        let screen_r = r * self.state.xf.scale();
        let n = ((span * screen_r / 2.0).ceil() as usize).clamp(4, 256);
        for i in 0..=n {
            let a = a0 + span * i as f32 / n as f32;
            self.push_point(cx + r * a.cos(), cy + r * a.sin());
        }
    }

    fn close_path(&mut self) {
        if let Some((_, closed)) = self.path.last_mut() {
            *closed = true;
        }
    }

    fn rect(&mut self, r: Rect) {
        self.move_to(r.x, r.y);
        self.line_to(r.right(), r.y);
        self.line_to(r.right(), r.bottom());
        self.line_to(r.x, r.bottom());
        self.close_path();
    }

    fn rounded_rect(&mut self, r: Rect, radius: f32) {
        let rad = radius.min(r.w / 2.0).min(r.h / 2.0);
        if rad <= 0.0 {
            self.rect(r);
            return;
        }
        use std::f32::consts::{FRAC_PI_2, PI};
        self.cur = None;
        self.path.push((Vec::new(), false));
        self.arc(r.right() - rad, r.y + rad, rad, -FRAC_PI_2, 0.0);
        self.arc(r.right() - rad, r.bottom() - rad, rad, 0.0, FRAC_PI_2);
        self.arc(r.x + rad, r.bottom() - rad, rad, FRAC_PI_2, PI);
        self.arc(r.x + rad, r.y + rad, rad, PI, PI + FRAC_PI_2);
        self.close_path();
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        self.cur = None;
        self.path.push((Vec::new(), false));
        self.arc(cx, cy, r, 0.0, std::f32::consts::TAU);
        self.close_path();
    }

    fn fill(&mut self, color: Color) {
        let c = color32(color);
        for (pts, _) in std::mem::take(&mut self.path) {
            if pts.len() >= 3 {
                self.shapes.push((
                    self.state.clip,
                    Shape::Path(PathShape::convex_polygon(pts, c, egui::Stroke::NONE)),
                ));
            }
        }
        self.cur = None;
    }

    /// A triangle strip from the polyline down to the baseline with
    /// per-vertex colour: a vertical gradient that follows the curve.
    fn fill_under(&mut self, points: &[Point], baseline: f32, top: Color, bottom: Color) {
        if points.len() < 2 {
            return;
        }
        let (ct, cb) = (color32(top), color32(bottom));
        let mut mesh = Mesh::default();
        mesh.reserve_vertices(points.len() * 2);
        mesh.reserve_triangles((points.len() - 1) * 2);
        for (i, pt) in points.iter().enumerate() {
            mesh.colored_vertex(self.state.xf.apply(pt.x, pt.y.min(baseline)), ct);
            mesh.colored_vertex(self.state.xf.apply(pt.x, baseline), cb);
            if i > 0 {
                let v = (i as u32) * 2;
                mesh.add_triangle(v - 2, v - 1, v);
                mesh.add_triangle(v - 1, v + 1, v);
            }
        }
        self.shapes.push((self.state.clip, Shape::mesh(mesh)));
    }

    fn stroke(&mut self, stroke: Stroke) {
        let width = stroke.width * self.state.xf.scale();
        let es = egui::Stroke::new(width, color32(stroke.color));
        for (pts, closed) in std::mem::take(&mut self.path) {
            if pts.len() < 2 {
                continue;
            }
            if stroke.dash[0] > 0.0 {
                let s = self.state.xf.scale();
                for shape in Shape::dashed_line(&pts, es, stroke.dash[0] * s, stroke.dash[1] * s) {
                    self.shapes.push((self.state.clip, shape));
                }
                continue;
            }
            if stroke.cap == LineCap::Round && !closed {
                for p in [pts[0], pts[pts.len() - 1]] {
                    self.shapes.push((
                        self.state.clip,
                        Shape::circle_filled(p, width / 2.0, es.color),
                    ));
                }
            }
            let shape = if closed {
                Shape::closed_line(pts, es)
            } else {
                Shape::line(pts, es)
            };
            self.shapes.push((self.state.clip, shape));
        }
        self.cur = None;
    }

    fn fill_text(&mut self, text: &str, x: f32, y: f32, style: &TextStyle, align: Align) {
        let scale = self.state.xf.scale();
        let galley = self.layout(text, style, scale);
        let w = galley.size().x / scale;
        let x0 = match align {
            Align::Left => x,
            Align::Center => x - w / 2.0,
            Align::Right => x - w,
        };
        let pos = self.state.xf.apply(x0, y);
        self.shapes.push((
            self.state.clip,
            Shape::galley(pos, galley, color32(style.color)),
        ));
    }

    fn measure_text(&mut self, text: &str, style: &TextStyle) -> TextSize {
        let galley = self.layout(text, style, 1.0);
        let size = galley.size();
        let ascent = galley
            .rows
            .first()
            .and_then(|r| r.glyphs.first())
            .map_or(size.y * 0.8, |g| g.pos.y);
        TextSize {
            width: size.x,
            height: size.y,
            ascent,
        }
    }
}
