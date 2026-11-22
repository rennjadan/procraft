//! Shapes (as Paint's): a rectangle (its corners rounded as much as asked), an ellipse, a line, a
//! triangle, a diamond, a hexagon or a star, in a box on the canvas, filled, outlined (inside its
//! edge, so its outer edge is the box's) or made the selection. Rectangles placed are kept with
//! the artwork: a shape drawn later snaps to them, and a rounded one inside another takes corners
//! parallel to its (the outer radius less the gap between them).

use procraft_engine::fill;
use procraft_engine::geom::IRect;
use procraft_engine::selection::Mask;

use crate::CanvasSession;
use crate::layer_ops::mask_part;
use crate::selection::SelectionJoin;

/// A shape's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
    Triangle,
    Diamond,
    Hexagon,
    Star,
}

/// What a shape makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ShapeMake {
    Fill,
    /// Its edge, `width` wide, inside the box.
    Outline,
    /// It joins the selection.
    Select,
}

/// A shape: its kind, what it makes, its box (canvas pixels; a line runs from the first corner to
/// the second), a rectangle's corner radius, an outline's or a line's width, and its colour
/// (0..1).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct ShapeSpec {
    pub kind: ShapeKind,
    pub make: ShapeMake,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub corner: f32,
    pub width: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

/// A rectangle placed (canvas pixels) and its corner radius.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct PlacedShape {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub corner: f32,
}

/// The most rectangles kept with an artwork (the oldest go first).
const MOST_KEPT: usize = 64;

/// The corner radius that runs parallel to `outer`'s for a rounded rectangle at `inner` inside
/// it: the outer radius less the narrowest gap between them (none past it). Its corners then
/// share their centres with the outer one's where the gaps are even.
pub fn parallel_corner(outer: &PlacedShape, inner: &PlacedShape) -> f32 {
    let gap = (inner.x0 - outer.x0).min(outer.x1 - inner.x1).min(inner.y0 - outer.y0).min(outer.y1 - inner.y1);
    (outer.corner - gap.max(0.0)).max(0.0)
}

/// The smallest placed rectangle (or the card's cut, `card`) holding `inner` whole, if any.
pub fn holding(placed: &[PlacedShape], card: Option<PlacedShape>, inner: &PlacedShape) -> Option<PlacedShape> {
    let inside = |o: &PlacedShape| {
        o.x0 <= inner.x0 + 0.5
            && o.y0 <= inner.y0 + 0.5
            && o.x1 >= inner.x1 - 0.5
            && o.y1 >= inner.y1 - 0.5
            && (o.x1 - o.x0) * (o.y1 - o.y0) > (inner.x1 - inner.x0) * (inner.y1 - inner.y0) + 1.0
    };
    placed
        .iter()
        .copied()
        .chain(card)
        .filter(inside)
        .min_by(|a, b| ((a.x1 - a.x0) * (a.y1 - a.y0)).total_cmp(&((b.x1 - b.x0) * (b.y1 - b.y0))))
}

/// The shape's outline points in canvas pixels (closed, for showing it while it is placed).
pub fn shape_outline(spec: &ShapeSpec) -> Vec<f32> {
    let (x0, y0, x1, y1) = boxed(spec);
    let (w, h) = (x1 - x0, y1 - y0);
    let mut out = Vec::new();
    match spec.kind {
        ShapeKind::Line => out.extend([spec.x0, spec.y0, spec.x1, spec.y1]),
        ShapeKind::Rectangle => {
            let r = spec.corner.clamp(0.0, w.min(h) / 2.0);
            let corners =
                [(x1 - r, y0 + r, -90.0), (x1 - r, y1 - r, 0.0), (x0 + r, y1 - r, 90.0), (x0 + r, y0 + r, 180.0)];
            for (cx, cy, from) in corners {
                for i in 0..=12 {
                    let a = (from + 90.0 * i as f32 / 12.0_f32).to_radians();
                    out.extend([cx + r * a.cos(), cy + r * a.sin()]);
                }
            }
        }
        ShapeKind::Ellipse => {
            for i in 0..72 {
                let a = i as f32 / 72.0 * std::f32::consts::TAU;
                out.extend([x0 + w / 2.0 * (1.0 + a.cos()), y0 + h / 2.0 * (1.0 + a.sin())]);
            }
        }
        _ => {
            for (u, v) in unit_polygon(spec.kind) {
                out.extend([x0 + u * w, y0 + v * h]);
            }
        }
    }
    out
}

/// The box's left, top, right and bottom.
fn boxed(spec: &ShapeSpec) -> (f32, f32, f32, f32) {
    (spec.x0.min(spec.x1), spec.y0.min(spec.y1), spec.x0.max(spec.x1), spec.y0.max(spec.y1))
}

/// A polygon's corners in a unit box.
fn unit_polygon(kind: ShapeKind) -> Vec<(f32, f32)> {
    match kind {
        ShapeKind::Triangle => vec![(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)],
        ShapeKind::Diamond => vec![(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)],
        ShapeKind::Hexagon => vec![(0.25, 0.0), (0.75, 0.0), (1.0, 0.5), (0.75, 1.0), (0.25, 1.0), (0.0, 0.5)],
        ShapeKind::Star => {
            // five points, the inner ones as a regular star has them, stretched to the box
            let raw: Vec<(f32, f32)> = (0..10)
                .map(|i| {
                    let a = (i as f32 * 36.0 - 90.0).to_radians();
                    let r = if i % 2 == 0 { 1.0 } else { 0.382 };
                    (r * a.cos(), r * a.sin())
                })
                .collect();
            let (lo_y, hi_y) = raw.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
            let (lo_x, hi_x) = raw.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
            raw.into_iter().map(|(x, y)| ((x - lo_x) / (hi_x - lo_x), (y - lo_y) / (hi_y - lo_y))).collect()
        }
        _ => Vec::new(),
    }
}

/// The signed distance (pixels; inside below zero) from (`x`, `y`) to the shape's edge.
fn distance(spec: &ShapeSpec, poly: &[(f32, f32)], x: f32, y: f32) -> f32 {
    let (x0, y0, x1, y1) = boxed(spec);
    let (hw, hh) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let (px, py) = (x - (x0 + hw), y - (y0 + hh));
    match spec.kind {
        ShapeKind::Rectangle => {
            let r = spec.corner.clamp(0.0, hw.min(hh));
            let (qx, qy) = (px.abs() - (hw - r), py.abs() - (hh - r));
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
        }
        ShapeKind::Ellipse => {
            let (a, b) = (hw.max(0.5), hh.max(0.5));
            let f = (px / a).powi(2) + (py / b).powi(2) - 1.0;
            let g = (2.0 * px / (a * a)).hypot(2.0 * py / (b * b));
            if g > 1e-6 { f / g } else { -a.min(b) }
        }
        ShapeKind::Line => {
            let (ax, ay, bx, by) = (spec.x0, spec.y0, spec.x1, spec.y1);
            let (ex, ey) = (bx - ax, by - ay);
            let (wx, wy) = (x - ax, y - ay);
            let len = ex * ex + ey * ey;
            let t = if len > 0.0 { ((wx * ex + wy * ey) / len).clamp(0.0, 1.0) } else { 0.0 };
            (wx - ex * t).hypot(wy - ey * t) - spec.width.max(1.0) / 2.0
        }
        _ => polygon_distance(poly, x, y),
    }
}

/// The signed distance from (`x`, `y`) to the polygon through `poly`.
fn polygon_distance(poly: &[(f32, f32)], x: f32, y: f32) -> f32 {
    let Some(&first) = poly.first() else { return f32::MAX };
    let mut d = (x - first.0).powi(2) + (y - first.1).powi(2);
    let mut inside = false;
    let mut j = poly.len() - 1;
    for (i, &(vx, vy)) in poly.iter().enumerate() {
        let (ux, uy) = poly.get(j).copied().unwrap_or(first);
        let (ex, ey) = (ux - vx, uy - vy);
        let (wx, wy) = (x - vx, y - vy);
        let len = ex * ex + ey * ey;
        let t = if len > 0.0 { ((wx * ex + wy * ey) / len).clamp(0.0, 1.0) } else { 0.0 };
        d = d.min((wx - ex * t).powi(2) + (wy - ey * t).powi(2));
        if (vy > y) != (uy > y) && x < (ux - vx) * (y - vy) / (uy - vy) + vx {
            inside = !inside;
        }
        j = i;
    }
    if inside { -d.sqrt() } else { d.sqrt() }
}

/// How much of the pixel centred at (`x`, `y`) the shape covers (0..1), smoothed a pixel wide.
fn coverage(spec: &ShapeSpec, poly: &[(f32, f32)], x: f32, y: f32) -> f32 {
    let d = distance(spec, poly, x, y);
    let inside = (0.5 - d).clamp(0.0, 1.0);
    match spec.make {
        // a line is its own outline
        ShapeMake::Outline if spec.kind != ShapeKind::Line => inside * (0.5 + d + spec.width.max(1.0)).clamp(0.0, 1.0),
        _ => inside,
    }
}

/// The shape's coverage over `rect` (canvas pixels), a byte a pixel.
fn shape_mask(spec: &ShapeSpec, rect: IRect) -> Vec<u8> {
    let (x0, y0, x1, y1) = boxed(spec);
    let poly: Vec<(f32, f32)> =
        unit_polygon(spec.kind).into_iter().map(|(u, v)| (x0 + u * (x1 - x0), y0 + v * (y1 - y0))).collect();
    let mut out = Vec::with_capacity(rect.width() as usize * rect.height() as usize);
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let c = coverage(spec, &poly, x as f32 + 0.5, y as f32 + 0.5);
            out.push((c * 255.0).round() as u8);
        }
    }
    out
}

/// The canvas pixels a shape can touch, within a `w` × `h` canvas; none if it is off it or has no
/// size.
fn reach(spec: &ShapeSpec, w: u32, h: u32) -> Option<IRect> {
    let (x0, y0, x1, y1) = boxed(spec);
    let pad = if spec.kind == ShapeKind::Line { spec.width.max(1.0) / 2.0 + 2.0 } else { 2.0 };
    let n = |v: f32| v.clamp(-1.0e6, 1.0e6) as i32;
    let r = IRect::new(n((x0 - pad).floor()), n((y0 - pad).floor()), n((x1 + pad).ceil()), n((y1 + pad).ceil()))
        .intersect(IRect::from_size(w, h));
    let sized = spec.kind == ShapeKind::Line || (x1 - x0 >= 1.0 && y1 - y0 >= 1.0);
    (!r.is_empty() && sized && [x0, y0, x1, y1].iter().all(|v| v.is_finite())).then_some(r)
}

#[uniffi::export]
impl CanvasSession {
    /// Makes `spec` on the current layer (filled or outlined in its colour, within the selection
    /// if there is one, only where it has paint when alpha-locked) as one undo step, or joins it
    /// to the selection. A rectangle is kept with the artwork to snap to. False if nothing was
    /// made (off the canvas, no size, or the layer locked, text or a plugin's).
    pub fn make_shape(&self, spec: ShapeSpec) -> bool {
        let mut guard = self.shared.lock();
        let st = &mut *guard;
        let (w, h) = (st.doc.width(), st.doc.height());
        let Some(rect) = reach(&spec, w, h) else { return false };
        let data = shape_mask(&spec, rect);
        if spec.make == ShapeMake::Select {
            // the whole canvas's worth, the shape in its place
            let mut full = vec![0u8; w as usize * h as usize];
            let rw = rect.width() as usize;
            for (row, line) in data.chunks(rw).enumerate() {
                let start = (rect.y0 as usize + row) * w as usize + rect.x0 as usize;
                if let Some(to) = full.get_mut(start..start + line.len()) {
                    to.copy_from_slice(line);
                }
            }
            st.join(Mask { width: w, height: h, data: full }, SelectionJoin::Add, true);
        } else {
            let current = st.doc.current();
            let Some(layer) = st.doc.layer(current).cloned() else { return false };
            if st.doc.is_locked(current) || layer.drawn_from_data() {
                return false;
            }
            let mut data = data;
            if let Some(sel) = st.reach_selection() {
                for (m, s) in data.iter_mut().zip(mask_part(&sel, rect)) {
                    *m = ((u16::from(*m) * u16::from(s) + 127) / 255) as u8;
                }
            }
            let Some(mut pixels) = st.layer_part(current, rect) else { return false };
            let mask = Mask { width: rect.width(), height: rect.height(), data };
            fill::pour_over(&mut pixels, &mask, [spec.r, spec.g, spec.b], layer.alpha_lock);
            if let Some(canvas) = st.canvas.as_mut() {
                canvas.name_next_step("Shape");
            }
            st.rewrite_layer(current, rect, pixels);
        }
        if spec.kind == ShapeKind::Rectangle {
            let (x0, y0, x1, y1) = boxed(&spec);
            let kept = [x0, y0, x1, y1, spec.corner.clamp(0.0, (x1 - x0).min(y1 - y0) / 2.0)];
            let shapes = &mut st.doc.shapes;
            shapes.retain(|s| s.iter().zip(&kept).any(|(a, b)| (a - b).abs() > 0.5));
            shapes.push(kept);
            if shapes.len() > MOST_KEPT {
                shapes.remove(0);
            }
        }
        st.revision += 1;
        self.shared.frame(st);
        true
    }

    /// The rectangles placed in this artwork, oldest first.
    pub fn placed_shapes(&self) -> Vec<PlacedShape> {
        let st = self.shared.lock();
        st.doc.shapes.iter().map(|&[x0, y0, x1, y1, corner]| PlacedShape { x0, y0, x1, y1, corner }).collect()
    }

    /// Forgets the rectangles placed (the pixels they made stay).
    pub fn forget_placed_shapes(&self) {
        let mut st = self.shared.lock();
        if !st.doc.shapes.is_empty() {
            st.doc.shapes.clear();
            st.revision += 1;
        }
    }

    /// Canvas pixel (x, y) under window point (`x`, `y`), wherever it is (off the canvas too);
    /// none with no view yet.
    pub fn window_to_canvas(&self, x: f32, y: f32) -> Option<Vec<f32>> {
        let st = self.shared.lock();
        let view = st.view?;
        let p = view.screen_to_canvas(procraft_engine::geom::Vec2::new(x, y))?;
        Some(vec![p.x, p.y])
    }
}

/// The free functions of the shape tool, for the apps.
#[uniffi::export]
pub fn shape_points(spec: ShapeSpec) -> Vec<f32> {
    shape_outline(&spec)
}

/// The corner radius parallel to `outer`'s for a rounded rectangle at `inner` inside it.
#[uniffi::export]
pub fn shape_parallel_corner(outer: PlacedShape, inner: PlacedShape) -> f32 {
    parallel_corner(&outer, &inner)
}

/// The smallest placed rectangle (or the card's cut) holding `inner` whole, if any.
#[uniffi::export]
pub fn shape_holding(placed: Vec<PlacedShape>, card: Option<PlacedShape>, inner: PlacedShape) -> Option<PlacedShape> {
    holding(&placed, card, &inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: ShapeKind, make: ShapeMake, b: [f32; 4], corner: f32, width: f32) -> ShapeSpec {
        let [x0, y0, x1, y1] = b;
        ShapeSpec { kind, make, x0, y0, x1, y1, corner, width, r: 1.0, g: 0.0, b: 0.0 }
    }

    #[test]
    fn a_rounded_rectangle_inside_another_takes_parallel_corners() {
        let card = PlacedShape { x0: 0.0, y0: 0.0, x1: 1500.0, y1: 2100.0, corner: 75.0 };
        let border = PlacedShape { x0: 30.0, y0: 30.0, x1: 1470.0, y1: 2070.0, corner: 0.0 };
        assert_eq!(parallel_corner(&card, &border), 45.0);
        // a text box low on the card: the nearest gap (its sides and bottom) sets it
        let text = PlacedShape { x0: 70.0, y0: 1500.0, x1: 1430.0, y1: 2030.0, corner: 0.0 };
        let border = PlacedShape { corner: 45.0, ..border };
        assert_eq!(parallel_corner(&border, &text), 5.0);
        // past the outer corner's reach, square
        let deep = PlacedShape { x0: 200.0, y0: 200.0, x1: 400.0, y1: 400.0, corner: 0.0 };
        assert_eq!(parallel_corner(&card, &deep), 0.0);
        // the smallest one holding it is the one it matches
        let kept = [card, border];
        assert_eq!(holding(&kept, None, &text), Some(border));
        assert_eq!(holding(&[], Some(card), &text), Some(card));
    }

    #[test]
    fn shapes_cover_inside_and_outlines_keep_to_the_edge() {
        let poly = Vec::new();
        let square = spec(ShapeKind::Rectangle, ShapeMake::Fill, [10.0, 10.0, 50.0, 50.0], 0.0, 0.0);
        assert_eq!(coverage(&square, &poly, 30.5, 30.5), 1.0);
        assert_eq!(coverage(&square, &poly, 5.5, 30.5), 0.0);
        // a round corner leaves its very corner empty
        let round = ShapeSpec { corner: 15.0, ..square };
        assert_eq!(coverage(&round, &poly, 10.5, 10.5), 0.0);
        assert_eq!(coverage(&round, &poly, 25.5, 10.5), 1.0);
        // an outline 4 wide: its edge, not its middle
        let ring = ShapeSpec { make: ShapeMake::Outline, width: 4.0, ..round };
        assert_eq!(coverage(&ring, &poly, 30.5, 11.5), 1.0);
        assert_eq!(coverage(&ring, &poly, 30.5, 30.5), 0.0);
        let ellipse = ShapeSpec { kind: ShapeKind::Ellipse, ..square };
        assert_eq!(coverage(&ellipse, &poly, 30.5, 30.5), 1.0);
        assert_eq!(coverage(&ellipse, &poly, 11.5, 11.5), 0.0);
        let tri = ShapeSpec { kind: ShapeKind::Triangle, ..square };
        let pts: Vec<(f32, f32)> =
            unit_polygon(ShapeKind::Triangle).iter().map(|(u, v)| (10.0 + u * 40.0, 10.0 + v * 40.0)).collect();
        assert_eq!(coverage(&tri, &pts, 30.5, 45.5), 1.0);
        assert_eq!(coverage(&tri, &pts, 12.5, 12.5), 0.0);
        let line = spec(ShapeKind::Line, ShapeMake::Fill, [0.0, 20.0, 100.0, 20.0], 0.0, 6.0);
        assert_eq!(coverage(&line, &poly, 50.5, 20.5), 1.0);
        assert_eq!(coverage(&line, &poly, 50.5, 30.5), 0.0);
        assert_eq!(shape_outline(&ShapeSpec { kind: ShapeKind::Star, ..square }).len(), 20);
    }
}
