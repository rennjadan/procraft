//! Shapes, as Paint has them: a rectangle (its corners rounded as much as asked), an ellipse, a
//! line, a triangle, a diamond, a hexagon or a star. Dragged out on the canvas, the shape stays
//! live until it is applied: its handles resize it, a drag inside moves it, and it snaps to the
//! canvas's middle and edges, the card's cut, and the rectangles placed before (their edges, their
//! middles, and even gaps inside them). A rounded rectangle inside another (or inside the card's
//! cut) takes corners parallel to its: the outer radius less the gap between them. Applied, it
//! fills, outlines (inside its edge) or joins the selection; rectangles are kept with the artwork.

use egui::{Align2, Color32, Id, Order, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use procraft_core::{
    CanvasSession, PlacedShape, ShapeKind, ShapeMake, ShapeSpec, shape_holding, shape_parallel_corner, shape_points,
};

use crate::look::{Weight, argb, pc, text};

/// Within this many points a dragged edge or middle snaps.
const SNAP: f32 = 10.0;
/// A handle's reach (points) and its dot.
const HANDLE_REACH: f32 = 22.0;
const HANDLE_DOT: f32 = 6.0;
const GUIDE: Color32 = Color32::from_rgb(255, 64, 200);
const PANEL_HEIGHT: f32 = 132.0;

const KINDS: [(ShapeKind, &str); 7] = [
    (ShapeKind::Rectangle, "Rectangle"),
    (ShapeKind::Ellipse, "Ellipse"),
    (ShapeKind::Line, "Line"),
    (ShapeKind::Triangle, "Triangle"),
    (ShapeKind::Diamond, "Diamond"),
    (ShapeKind::Hexagon, "Hexagon"),
    (ShapeKind::Star, "Star"),
];

/// Card corner presets: name and radius (mm).
const CORNERS: [(&str, f32); 3] = [("1/8\" (poker, MTG, Pokémon)", 3.175), ("3 mm", 3.0), ("5.5 mm (US game)", 5.5)];

/// What a drag on the canvas is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grip {
    /// Dragging a new shape out from its first corner.
    New,
    Move,
    /// A corner (0 top left, clockwise), or an edge's middle (4 top, 5 right, 6 bottom, 7 left);
    /// a line's ends are corners 0 and 2.
    Handle(u8),
}

#[derive(Clone, Copy)]
struct Drag {
    grip: Grip,
    from: Pos2,
    was: [f32; 4],
}

/// The card the artwork is: its corner radius and bleed, for its cut to snap to and match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    pub corner_mm: f32,
    pub bleed_in: f32,
    /// The card's trimmed width (inches): the canvas is this plus the bleed on each side.
    pub width_in: f32,
}

pub struct Shapes {
    pub on: bool,
    kind: ShapeKind,
    make: ShapeMake,
    /// An outline's or a line's width, and the corner radius (canvas pixels).
    width: f32,
    corner: f32,
    /// A rounded rectangle inside another takes corners parallel to its.
    matched: bool,
    /// Keeps a rectangle or ellipse square while it is dragged.
    square: bool,
    /// The live shape (canvas pixels): left, top, right, bottom (a line's two ends).
    rect: Option<[f32; 4]>,
    drag: Option<Drag>,
    /// The guides it snapped to this frame (canvas x, canvas y).
    guides: (Option<f32>, Option<f32>),
    card_open: bool,
}

impl Default for Shapes {
    fn default() -> Shapes {
        Shapes {
            on: false,
            kind: ShapeKind::Rectangle,
            make: ShapeMake::Fill,
            width: 20.0,
            corner: 0.0,
            matched: true,
            square: false,
            rect: None,
            drag: None,
            guides: (None, None),
            card_open: false,
        }
    }
}

/// What the tool asks of the screen.
pub enum Said {
    Notice(&'static str, bool),
    /// The card's settings changed (to keep).
    Card(Card),
}

/// The view as [`CanvasSession::view_matrix`] gives it (canvas to window pixels).
fn matrix(session: &CanvasSession) -> [f32; 6] {
    let v = session.view_matrix();
    v.get(..6).and_then(|m| <[f32; 6]>::try_from(m).ok()).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
}

/// Canvas pixel (x, y) in the window (points).
fn to_window([a, b, c, d, e, f]: [f32; 6], ppp: f32, x: f32, y: f32) -> Pos2 {
    pos2(a * x + c * y + e, b * x + d * y + f) / ppp
}

fn normal(r: [f32; 4]) -> [f32; 4] {
    let [x0, y0, x1, y1] = r;
    [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
}

fn placed(r: [f32; 4], corner: f32) -> PlacedShape {
    let [x0, y0, x1, y1] = normal(r);
    PlacedShape { x0, y0, x1, y1, corner }
}

impl Shapes {
    pub fn open(&mut self) {
        self.on = true;
        self.rect = None;
        self.drag = None;
    }

    pub fn close(&mut self) {
        self.on = false;
        self.rect = None;
        self.drag = None;
        self.card_open = false;
    }

    /// The card's cut on a `w` × `h` canvas (canvas pixels), its corner radius in pixels.
    fn card_cut(card: Card, w: f32, h: f32) -> PlacedShape {
        let per_inch = w / (card.width_in + 2.0 * card.bleed_in).max(0.1);
        let bleed = card.bleed_in * per_inch;
        PlacedShape { x0: bleed, y0: bleed, x1: w - bleed, y1: h - bleed, corner: card.corner_mm / 25.4 * per_inch }
    }

    fn spec(&self, rgb: [f32; 3]) -> Option<ShapeSpec> {
        let [x0, y0, x1, y1] = self.rect?;
        let [r, g, b] = rgb;
        Some(ShapeSpec {
            kind: self.kind,
            make: self.make,
            x0,
            y0,
            x1,
            y1,
            corner: self.corner,
            width: self.width,
            r,
            g,
            b,
        })
    }

    /// Shows the tool over the canvas (below `top`) and its panel: touches there make and change
    /// the shape. `size`: the canvas's size (pixels).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        session: &CanvasSession,
        top: f32,
        rgb: [f32; 3],
        card: &mut Card,
    ) -> Option<Said> {
        if !self.on {
            return None;
        }
        let ppp = ctx.pixels_per_point();
        let m = matrix(session);
        let [a, b, ..] = m;
        // canvas pixels a point spans
        let per_point = ppp / a.hypot(b).max(1e-6);
        let size = session.info();
        let (cw, ch) = (size.width as f32, size.height as f32);
        let cut = Self::card_cut(*card, cw, ch);
        let kept = session.placed_shapes();
        let window = ctx.content_rect();
        let canvas_area =
            Rect::from_min_max(pos2(window.left(), top), pos2(window.right(), window.bottom() - PANEL_HEIGHT));
        let to_canvas = |p: Pos2| -> Option<Pos2> {
            let v = session.window_to_canvas(p.x * ppp, p.y * ppp)?;
            Some(pos2(*v.first()?, *v.get(1)?))
        };
        let mut said = None;
        // touches over the canvas: a new shape, a handle, or the shape moved
        egui::Area::new(Id::unique("shapes over canvas")).fixed_pos(canvas_area.min).order(Order::Middle).show(
            ctx,
            |ui| {
                let response = ui.allocate_rect(canvas_area, Sense::click_and_drag());
                let at = ui.input(|i| i.pointer.interact_pos());
                let shift = ui.input(|i| i.modifiers.shift);
                if response.drag_started()
                    && let Some(p) = response.interact_pointer_pos().and_then(to_canvas)
                {
                    let grip = self.grip_at(m, ppp, response.interact_pointer_pos().unwrap_or_default());
                    let was = match grip {
                        Grip::New => [p.x, p.y, p.x, p.y],
                        _ => self.rect.unwrap_or([p.x, p.y, p.x, p.y]),
                    };
                    self.drag = Some(Drag { grip, from: p, was });
                }
                if let (Some(drag), Some(now)) = (self.drag, at.and_then(to_canvas))
                    && response.dragged()
                {
                    let r = self.dragged(drag, now, shift || self.square);
                    let r = self.snapped(drag.grip, r, cw, ch, &cut, &kept, SNAP * per_point);
                    self.rect = Some(r);
                    self.match_corner(&cut, &kept);
                }
                if response.drag_stopped() {
                    self.drag = None;
                    self.guides = (None, None);
                    // a click with no drag leaves nothing behind
                    if self.rect.is_some_and(|r| {
                        let [x0, y0, x1, y1] = normal(r);
                        self.kind != ShapeKind::Line && (x1 - x0 < 1.0 || y1 - y0 < 1.0)
                    }) {
                        self.rect = None;
                    }
                }
                // what it will make, the card's cut and what was placed, faintly
                let painter = ui.painter();
                let faint = Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 200, 255, 110));
                let outline = |spec: &ShapeSpec| -> Vec<Pos2> {
                    shape_points(*spec).as_chunks::<2>().0.iter().map(|[x, y]| to_window(m, ppp, *x, *y)).collect()
                };
                let rect_spec = |p: &PlacedShape| ShapeSpec {
                    kind: ShapeKind::Rectangle,
                    make: ShapeMake::Fill,
                    x0: p.x0,
                    y0: p.y0,
                    x1: p.x1,
                    y1: p.y1,
                    corner: p.corner,
                    width: 0.0,
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                };
                for p in kept.iter().chain(std::iter::once(&cut)) {
                    painter.add(Shape::closed_line(outline(&rect_spec(p)), faint));
                }
                if let Some(x) = self.guides.0 {
                    painter
                        .line_segment([to_window(m, ppp, x, 0.0), to_window(m, ppp, x, ch)], Stroke::new(1.0, GUIDE));
                }
                if let Some(y) = self.guides.1 {
                    painter
                        .line_segment([to_window(m, ppp, 0.0, y), to_window(m, ppp, cw, y)], Stroke::new(1.0, GUIDE));
                }
                if let Some(spec) = self.spec(rgb) {
                    let colour = crate::colours::srgb(rgb);
                    let points = outline(&spec);
                    let line = Stroke::new(2.0, colour);
                    if self.kind == ShapeKind::Line {
                        let w = (self.width / per_point).max(1.0);
                        painter.add(Shape::line(points.clone(), Stroke::new(w, colour)));
                    } else {
                        painter.add(Shape::closed_line(points.clone(), line));
                        painter.add(Shape::closed_line(points, Stroke::new(1.0, Color32::from_black_alpha(120))));
                    }
                    for h in self.handles(m, ppp) {
                        painter.circle_filled(h, HANDLE_DOT, Color32::WHITE);
                        painter.circle_stroke(h, HANDLE_DOT, Stroke::new(1.5, argb(pc::ACCENT)));
                    }
                }
            },
        );
        // the panel along the bottom
        let panel = Rect::from_min_max(pos2(window.left(), window.bottom() - PANEL_HEIGHT), window.max);
        egui::Area::new(Id::unique("shapes panel")).fixed_pos(panel.min).order(Order::Foreground).show(ctx, |ui| {
            ui.painter().rect_filled(panel, 0, Color32::from_rgba_unmultiplied(30, 30, 30, 240));
            ui.allocate_rect(panel, Sense::click());
            let inner = panel.shrink2(vec2(16.0, 10.0));
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
            ui.visuals_mut().override_text_color = Some(Color32::from_gray(225));
            ui.spacing_mut().button_padding = vec2(10.0, 6.0);
            ui.horizontal(|ui| {
                for (kind, name) in KINDS {
                    if ui.add(egui::Button::selectable(self.kind == kind, name)).clicked() {
                        self.kind = kind;
                        self.match_corner(&cut, &kept);
                    }
                }
                ui.separator();
                for (make, name) in
                    [(ShapeMake::Fill, "Fill"), (ShapeMake::Outline, "Outline"), (ShapeMake::Select, "Selection")]
                {
                    if ui.add(egui::Button::selectable(self.make == make, name)).clicked() {
                        self.make = make;
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.square, "Square");
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let per_inch = cw / (card.width_in + 2.0 * card.bleed_in).max(0.1);
                if self.kind == ShapeKind::Rectangle {
                    let most = self.rect.map_or(cw.min(ch) / 2.0, |r| {
                        let [x0, y0, x1, y1] = normal(r);
                        ((x1 - x0).min(y1 - y0) / 2.0).max(1.0)
                    });
                    let before = self.corner;
                    ui.label("Corners");
                    ui.add(egui::Slider::new(&mut self.corner, 0.0..=most.max(1.0)).suffix(" px"));
                    if self.corner != before {
                        self.matched = false;
                    }
                    ui.label(format!("{:.3}\"", self.corner / per_inch));
                    if ui.checkbox(&mut self.matched, "Match corners").changed() {
                        self.match_corner(&cut, &kept);
                    }
                    ui.separator();
                }
                if self.kind == ShapeKind::Line || self.make == ShapeMake::Outline {
                    ui.label("Width");
                    ui.add(egui::Slider::new(&mut self.width, 1.0..=400.0).logarithmic(true).suffix(" px"));
                    ui.separator();
                }
                if ui.button("Card edge").on_hover_text("A rectangle on the card's cut, its corners matching").clicked()
                {
                    self.rect = Some([cut.x0, cut.y0, cut.x1, cut.y1]);
                    self.kind = ShapeKind::Rectangle;
                    self.corner = cut.corner;
                    self.matched = true;
                }
                if ui.button("Rotate 90°").clicked() {
                    self.rotate();
                    self.match_corner(&cut, &kept);
                }
                if ui.button("Card…").clicked() {
                    self.card_open = !self.card_open;
                }
                ui.separator();
                if ui.button("Cancel").clicked() {
                    self.rect = None;
                }
                let apply = egui::Button::new(egui::RichText::new("Apply").strong()).fill(argb(pc::ACCENT));
                if ui.add_enabled(self.rect.is_some(), apply).clicked() {
                    said = Some(self.apply(session, rgb));
                }
                if ui.button("Done").clicked() {
                    self.close();
                }
            });
            if self.card_open {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let was = *card;
                    ui.label("Card corner");
                    for (name, mm) in CORNERS {
                        if ui.add(egui::Button::selectable((card.corner_mm - mm).abs() < 0.01, name)).clicked() {
                            card.corner_mm = mm;
                        }
                    }
                    ui.add(egui::DragValue::new(&mut card.corner_mm).range(0.0..=20.0).speed(0.05).suffix(" mm"));
                    ui.separator();
                    ui.label("Bleed");
                    if ui.add(egui::Button::selectable(card.bleed_in == 0.0, "None")).clicked() {
                        card.bleed_in = 0.0;
                    }
                    if ui.add(egui::Button::selectable(card.bleed_in == 0.125, "1/8\"")).clicked() {
                        card.bleed_in = 0.125;
                    }
                    ui.separator();
                    ui.label("Card width");
                    ui.add(egui::DragValue::new(&mut card.width_in).range(0.5..=20.0).speed(0.01).suffix(" in"));
                    if *card != was {
                        said = Some(Said::Card(*card));
                    }
                });
            }
        });
        if self.drag.is_some() {
            ctx.request_repaint();
        }
        // the shape's size, by its box
        if let Some(r) = self.rect.filter(|_| self.drag.is_some()) {
            let [x0, y0, x1, y1] = normal(r);
            let p = to_window(m, ppp, x1, y1) + vec2(12.0, 12.0);
            let painter = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::unique("shape size")));
            let label = format!("{:.0} × {:.0}", x1 - x0, y1 - y0);
            text(&painter, p, Align2::LEFT_TOP, &label, 14.0, Weight::Medium, Color32::WHITE);
        }
        said
    }

    /// Where the handles are (points): the box's corners and edge middles; a line's ends.
    fn handles(&self, m: [f32; 6], ppp: f32) -> Vec<Pos2> {
        let Some(r) = self.rect else { return Vec::new() };
        if self.kind == ShapeKind::Line {
            let [x0, y0, x1, y1] = r;
            return vec![to_window(m, ppp, x0, y0), to_window(m, ppp, x1, y1)];
        }
        let [x0, y0, x1, y1] = normal(r);
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (mx, y0), (x1, my), (mx, y1), (x0, my)]
            .into_iter()
            .map(|(x, y)| to_window(m, ppp, x, y))
            .collect()
    }

    /// What a press at window point `p` (points) takes hold of.
    fn grip_at(&self, m: [f32; 6], ppp: f32, p: Pos2) -> Grip {
        let handles = self.handles(m, ppp);
        let nearest = handles
            .iter()
            .enumerate()
            .map(|(i, h)| (i, h.distance(p)))
            .filter(|(_, d)| *d <= HANDLE_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = nearest {
            // a line's second end is its corner 2
            let i = if self.kind == ShapeKind::Line && i == 1 { 2 } else { i };
            return Grip::Handle(i as u8);
        }
        let inside = self.rect.is_some_and(|r| {
            let [x0, y0, x1, y1] = normal(r);
            let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| to_window(m, ppp, x, y));
            // inside the box as the window shows it (turned with the view)
            let mut inside = false;
            let [c0, c1, c2, c3] = corners;
            for (a, b) in [(c0, c3), (c1, c0), (c2, c1), (c3, c2)] {
                if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                    inside = !inside;
                }
            }
            inside
        });
        if inside { Grip::Move } else { Grip::New }
    }

    /// The shape as the drag from `drag.from` to `now` (canvas pixels) makes it.
    fn dragged(&self, drag: Drag, now: Pos2, square: bool) -> [f32; 4] {
        let (dx, dy) = (now.x - drag.from.x, now.y - drag.from.y);
        let line = self.kind == ShapeKind::Line;
        let [x0, y0, x1, y1] = if line { drag.was } else { normal(drag.was) };
        let mut r = match drag.grip {
            Grip::New => [drag.from.x, drag.from.y, now.x, now.y],
            Grip::Move => [x0 + dx, y0 + dy, x1 + dx, y1 + dy],
            Grip::Handle(0) if line => [x0 + dx, y0 + dy, x1, y1],
            Grip::Handle(_) if line => [x0, y0, x1 + dx, y1 + dy],
            Grip::Handle(0) => [x0 + dx, y0 + dy, x1, y1],
            Grip::Handle(1) => [x0, y0 + dy, x1 + dx, y1],
            Grip::Handle(2) => [x0, y0, x1 + dx, y1 + dy],
            Grip::Handle(3) => [x0 + dx, y0, x1, y1 + dy],
            Grip::Handle(4) => [x0, y0 + dy, x1, y1],
            Grip::Handle(5) => [x0, y0, x1 + dx, y1],
            Grip::Handle(6) => [x0, y0, x1, y1 + dy],
            Grip::Handle(_) => [x0 + dx, y0, x1, y1],
        };
        // square: the side dragged most sets both
        if square && !line && matches!(drag.grip, Grip::New | Grip::Handle(0..=3)) {
            let [ax, ay, bx, by] = r;
            let side = (bx - ax).abs().max((by - ay).abs());
            let (sx, sy) = (if bx >= ax { 1.0 } else { -1.0 }, if by >= ay { 1.0 } else { -1.0 });
            r = match drag.grip {
                Grip::Handle(0) => [bx - side * sx, by - side * sy, bx, by],
                Grip::Handle(1) => [ax, by - side * sy, ax + side * sx, by],
                Grip::Handle(3) => [bx - side * sx, ay, bx, ay + side * sy],
                _ => [ax, ay, ax + side * sx, ay + side * sy],
            };
        }
        r
    }

    /// `r` snapped: its moving edges (or, moved, its edges and middle) to the canvas's middle and
    /// edges, the card's cut, the rectangles placed, and gaps even with the others inside the one
    /// holding it, within `reach` canvas pixels. The guides it snapped to are kept to show.
    fn snapped(
        &mut self,
        grip: Grip,
        r: [f32; 4],
        cw: f32,
        ch: f32,
        cut: &PlacedShape,
        kept: &[PlacedShape],
        reach: f32,
    ) -> [f32; 4] {
        self.guides = (None, None);
        if self.kind == ShapeKind::Line {
            return r;
        }
        let mut xs = vec![0.0, cw / 2.0, cw, cut.x0, cut.x1];
        let mut ys = vec![0.0, ch / 2.0, ch, cut.y0, cut.y1];
        for p in kept {
            xs.extend([p.x0, p.x1, (p.x0 + p.x1) / 2.0]);
            ys.extend([p.y0, p.y1, (p.y0 + p.y1) / 2.0]);
        }
        let [x0, y0, x1, y1] = normal(r);
        // even gaps: inside the one holding it, an edge as far in as another already is
        if let Some(o) = shape_holding(kept.to_vec(), Some(*cut), placed(r, 0.0)) {
            let gaps = [x0 - o.x0, o.x1 - x1, y0 - o.y0, o.y1 - y1].map(|g| g.max(0.0));
            for g in gaps {
                xs.extend([o.x0 + g, o.x1 - g]);
                ys.extend([o.y0 + g, o.y1 - g]);
            }
        }
        // the nearest target to any of `edges` (moving edge value, which), within reach
        let nearest = |edges: &[f32], targets: &[f32]| -> Option<(f32, f32)> {
            edges
                .iter()
                .flat_map(|&e| targets.iter().map(move |&t| (t - e, t)))
                .filter(|(d, _)| d.abs() <= reach)
                .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
        };
        let (mut x0, mut y0, mut x1, mut y1) = (x0, y0, x1, y1);
        match grip {
            Grip::Move => {
                if let Some((d, t)) = nearest(&[x0, x1, (x0 + x1) / 2.0], &xs) {
                    (x0, x1) = (x0 + d, x1 + d);
                    self.guides.0 = Some(t);
                }
                if let Some((d, t)) = nearest(&[y0, y1, (y0 + y1) / 2.0], &ys) {
                    (y0, y1) = (y0 + d, y1 + d);
                    self.guides.1 = Some(t);
                }
            }
            _ => {
                // the edges being dragged: by which handle, or for a new shape the far corner
                let [rx0, ry0, rx1, ry1] = r;
                let (left, right, top, bottom) = match grip {
                    Grip::Handle(0) => (true, false, true, false),
                    Grip::Handle(1) => (false, true, true, false),
                    Grip::Handle(2) => (false, true, false, true),
                    Grip::Handle(3) => (true, false, false, true),
                    Grip::Handle(4) => (false, false, true, false),
                    Grip::Handle(5) => (false, true, false, false),
                    Grip::Handle(6) => (false, false, false, true),
                    Grip::Handle(_) => (true, false, false, false),
                    // the corner following the pointer
                    _ => (rx1 < rx0, rx1 >= rx0, ry1 < ry0, ry1 >= ry0),
                };
                for (on, x) in [(left, &mut x0), (right, &mut x1)] {
                    if on && let Some((d, t)) = nearest(&[*x], &xs) {
                        *x += d;
                        self.guides.0 = Some(t);
                    }
                }
                for (on, y) in [(top, &mut y0), (bottom, &mut y1)] {
                    if on && let Some((d, t)) = nearest(&[*y], &ys) {
                        *y += d;
                        self.guides.1 = Some(t);
                    }
                }
            }
        }
        [x0, y0, x1, y1]
    }

    /// A rounded rectangle inside another (or the card's cut) takes corners parallel to its,
    /// while Match corners is on.
    fn match_corner(&mut self, cut: &PlacedShape, kept: &[PlacedShape]) {
        if !self.matched || self.kind != ShapeKind::Rectangle {
            return;
        }
        let Some(r) = self.rect else { return };
        let inner = placed(r, 0.0);
        if let Some(outer) = shape_holding(kept.to_vec(), Some(*cut), inner) {
            self.corner = shape_parallel_corner(outer, inner);
        } else if (inner.x0 - cut.x0).abs() < 0.5
            && (inner.y0 - cut.y0).abs() < 0.5
            && (inner.x1 - cut.x1).abs() < 0.5
            && (inner.y1 - cut.y1).abs() < 0.5
        {
            // on the card's cut itself: its own corners
            self.corner = cut.corner;
        }
    }

    /// The shape turned a quarter about its middle.
    fn rotate(&mut self) {
        let Some([x0, y0, x1, y1]) = self.rect else { return };
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let turn = |x: f32, y: f32| (cx - (y - cy), cy + (x - cx));
        let (a, b) = (turn(x0, y0), turn(x1, y1));
        self.rect =
            Some(if self.kind == ShapeKind::Line { [a.0, a.1, b.0, b.1] } else { normal([a.0, a.1, b.0, b.1]) });
    }

    /// Makes the live shape; the tool stays open for the next.
    fn apply(&mut self, session: &CanvasSession, rgb: [f32; 3]) -> Said {
        let Some(spec) = self.spec(rgb) else { return Said::Notice("Drag on the canvas to draw a shape", false) };
        if session.make_shape(spec) {
            self.rect = None;
            Said::Notice(if spec.make == ShapeMake::Select { "Shape selected" } else { "Shape made" }, false)
        } else {
            Said::Notice("Can't draw a shape on this layer", true)
        }
    }
}
