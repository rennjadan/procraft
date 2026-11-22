//! Shapes, as Paint has them: a rectangle (its corners rounded as much as asked), an ellipse, a
//! line, a triangle, a diamond, a hexagon or a star. Dragged out on the canvas, the shape stays
//! live until it is applied: its handles resize it, a drag inside moves it, and it snaps to the
//! canvas's middle and edges, the card's cut, and the rectangles placed before (their edges, their
//! middles, and even gaps inside them). A rounded rectangle inside another (or inside the card's
//! cut) takes corners parallel to its: the outer radius less the gap between them. Applied, it
//! fills, outlines (inside its edge) or joins the selection; rectangles are kept with the artwork.

use egui::{Align2, Color32, Id, Order, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use procraft_core::{
    CanvasSession, GradientKind, GradientSpec, PlacedShape, ShapeKind, ShapeMake, ShapeSpec, shape_holding,
    shape_parallel_corner, shape_points,
};

use crate::look::{Weight, argb, pc, text};

/// Within this many points a dragged edge or middle snaps.
const SNAP: f32 = 10.0;
/// A handle's reach (points) and its dot.
const HANDLE_REACH: f32 = 22.0;
const HANDLE_DOT: f32 = 6.0;
const GUIDE: Color32 = Color32::from_rgb(255, 64, 200);
const PANEL_HEIGHT: f32 = 168.0;
/// A tap this near a lasso's first point (points) closes it.
const CLOSE_REACH: f32 = 16.0;

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

/// The panel's tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Shapes,
    /// A shape drawn freehand (or tapped point by point) fills or erases at once.
    Lasso,
    /// A colour blended along a drag (the blender).
    Gradient,
}

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
    tab: Tab,
    /// The lasso being drawn freehand, and the points tapped (canvas pixels); erasing, not filling;
    /// its path smoothed.
    lasso: Vec<Pos2>,
    taps: Vec<Pos2>,
    lasso_erase: bool,
    smooth: bool,
    /// The gradient's start and end (canvas pixels), which end a drag holds (0 a new one, 1 the
    /// start, 2 the end), its kind, whether it fades to see-through (else to `second`), its bands,
    /// and the colour it was last shown with.
    grad: Option<(Pos2, Pos2)>,
    grad_grip: Option<u8>,
    grad_kind: GradientKind,
    to_clear: bool,
    second: [f32; 3],
    bands: u32,
    shown_with: Option<[f32; 3]>,
    /// A lasso closed this frame, to fill or erase once the touches are read.
    pending_lasso: Option<(Vec<Pos2>, [f32; 3])>,
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
            tab: Tab::Shapes,
            lasso: Vec::new(),
            taps: Vec::new(),
            lasso_erase: false,
            smooth: true,
            grad: None,
            grad_grip: None,
            grad_kind: GradientKind::Linear,
            to_clear: true,
            second: [1.0, 1.0, 1.0],
            bands: 1,
            shown_with: None,
            pending_lasso: None,
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

    /// Put away: a gradient being placed is kept (as a tool changed in Procreate keeps it).
    pub fn close(&mut self, session: &CanvasSession) {
        self.on = false;
        self.rect = None;
        self.drag = None;
        self.card_open = false;
        self.lasso.clear();
        self.taps.clear();
        self.end_gradient(session, true);
    }

    /// A gradient being placed taken back (undo while it shows); false if none was.
    pub fn cancel_gradient(&mut self, session: &CanvasSession) -> bool {
        let open = self.grad.is_some() || session.gradient_open();
        self.end_gradient(session, false);
        open
    }

    fn end_gradient(&mut self, session: &CanvasSession, keep: bool) {
        if self.grad.take().is_some() || session.gradient_open() {
            session.gradient_end(keep);
        }
        self.grad_grip = None;
        self.shown_with = None;
    }

    fn gradient_spec(&self, rgb: [f32; 3]) -> Option<GradientSpec> {
        let (a, b) = self.grad?;
        let [r0, g0, b0] = rgb;
        let [r1, g1, b1] = if self.to_clear { rgb } else { self.second };
        Some(GradientSpec {
            kind: self.grad_kind,
            x0: a.x,
            y0: a.y,
            x1: b.x,
            y1: b.y,
            r0,
            g0,
            b0,
            a0: 1.0,
            r1,
            g1,
            b1,
            a1: if self.to_clear { 0.0 } else { 1.0 },
            bands: if self.bands >= 2 { self.bands } else { 0 },
        })
    }

    /// The gradient shown again as it now is.
    fn show_gradient(&mut self, session: &CanvasSession, rgb: [f32; 3]) -> Option<Said> {
        let spec = self.gradient_spec(rgb)?;
        self.shown_with = Some(rgb);
        if session.gradient_show(spec) {
            None
        } else {
            self.grad = None;
            Some(Said::Notice("Can't put a gradient on this layer", true))
        }
    }

    /// The lasso's path, smoothed if asked (corner cutting, twice), as canvas x, y pairs.
    fn lasso_points(&self, path: &[Pos2]) -> Vec<f32> {
        let mut pts: Vec<Pos2> = path.to_vec();
        if self.smooth && pts.len() >= 4 {
            for _ in 0..2 {
                let n = pts.len();
                let mut next = Vec::with_capacity(n * 2);
                for i in 0..n {
                    let a = pts.get(i).copied().unwrap_or_default();
                    let b = pts.get((i + 1) % n).copied().unwrap_or_default();
                    next.push(a.lerp(b, 0.25));
                    next.push(a.lerp(b, 0.75));
                }
                pts = next;
            }
        }
        pts.iter().flat_map(|p| [p.x, p.y]).collect()
    }

    fn apply_lasso(&mut self, session: &CanvasSession, path: &[Pos2], rgb: [f32; 3]) -> Option<Said> {
        if path.len() < 3 {
            return None;
        }
        let [r, g, b] = rgb;
        if session.lasso_fill(self.lasso_points(path), self.lasso_erase, r, g, b) {
            None
        } else {
            Some(Said::Notice("Can't change this layer", true))
        }
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
        let mut close = false;
        // a colour changed while a gradient shows: shown in it
        if self.tab == Tab::Gradient && self.grad.is_some() && self.shown_with != Some(rgb) {
            said = self.show_gradient(session, rgb);
        }
        // touches over the canvas: a new shape, a handle, or the shape moved
        egui::Area::new(Id::unique("shapes over canvas")).fixed_pos(canvas_area.min).order(Order::Middle).show(
            ctx,
            |ui| {
                let response = ui.allocate_rect(canvas_area, Sense::click_and_drag());
                let at = ui.input(|i| i.pointer.interact_pos());
                let shift = ui.input(|i| i.modifiers.shift);
                if self.tab == Tab::Lasso {
                    said = self.lasso_touch(ui, &response, at, m, ppp, per_point, &to_canvas, rgb);
                    return;
                }
                if self.tab == Tab::Gradient {
                    said = self.gradient_touch(ui, session, &response, at, m, ppp, &to_canvas, rgb, shift);
                    return;
                }
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
                for (tab, name) in [(Tab::Shapes, "Shapes"), (Tab::Lasso, "Lasso"), (Tab::Gradient, "Gradient")] {
                    let label = egui::RichText::new(name).strong().size(15.0);
                    if ui.add(egui::Button::selectable(self.tab == tab, label)).clicked() && self.tab != tab {
                        // leaving the gradient keeps it
                        if self.tab == Tab::Gradient {
                            self.end_gradient(session, true);
                        }
                        self.tab = tab;
                        self.lasso.clear();
                        self.taps.clear();
                    }
                }
                ui.separator();
                if ui.button("Done").clicked() {
                    close = true;
                }
            });
            ui.add_space(4.0);
            if self.tab == Tab::Lasso {
                self.lasso_panel(&mut ui, session, rgb, &mut said);
                return;
            }
            if self.tab == Tab::Gradient {
                self.gradient_panel(&mut ui, session, rgb, &mut said);
                return;
            }
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
        if let Some((path, rgb)) = self.pending_lasso.take() {
            said = self.apply_lasso(session, &path, rgb).or(said);
        }
        if close {
            self.close(session);
            return said;
        }
        if self.drag.is_some() || !self.lasso.is_empty() {
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

    /// The lasso's touches: a drag draws it freehand and fills (or erases) as it lets go; taps put
    /// down its points one by one, a tap on the first closing it.
    #[allow(clippy::too_many_arguments)]
    fn lasso_touch(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        at: Option<Pos2>,
        m: [f32; 6],
        ppp: f32,
        per_point: f32,
        to_canvas: &dyn Fn(Pos2) -> Option<Pos2>,
        rgb: [f32; 3],
    ) -> Option<Said> {
        if response.drag_started()
            && self.taps.is_empty()
            && let Some(p) = response.interact_pointer_pos().and_then(to_canvas)
        {
            self.lasso = vec![p];
        }
        if response.dragged()
            && !self.lasso.is_empty()
            && let Some(p) = at.and_then(to_canvas)
            && self.lasso.last().is_none_or(|l| l.distance(p) >= 1.5 * per_point)
        {
            self.lasso.push(p);
        }
        if response.drag_stopped() && !self.lasso.is_empty() {
            self.pending_lasso = Some((std::mem::take(&mut self.lasso), rgb));
        }
        if response.clicked()
            && let Some(w) = response.interact_pointer_pos()
            && let Some(p) = to_canvas(w)
        {
            let near_first = self.taps.first().is_some_and(|f| to_window(m, ppp, f.x, f.y).distance(w) <= CLOSE_REACH);
            if near_first && self.taps.len() >= 3 {
                self.pending_lasso = Some((std::mem::take(&mut self.taps), rgb));
            } else {
                self.taps.push(p);
            }
        }
        // the path as it goes, and the line on to the pointer from the last point tapped
        let painter = ui.painter();
        let colour = if self.lasso_erase { Color32::from_gray(235) } else { crate::colours::srgb(rgb) };
        let window = |p: &Pos2| to_window(m, ppp, p.x, p.y);
        let hover = at.or(ui.input(|i| i.pointer.hover_pos()));
        for (path, tapped) in [(&self.lasso, false), (&self.taps, true)] {
            let mut pts: Vec<Pos2> = path.iter().map(window).collect();
            if tapped
                && !pts.is_empty()
                && let Some(h) = hover
            {
                pts.push(h);
            }
            if pts.len() >= 2 {
                painter.add(Shape::line(pts.clone(), Stroke::new(3.0, Color32::from_black_alpha(140))));
                painter.add(Shape::line(pts, Stroke::new(1.5, colour)));
            }
        }
        if let Some(first) = self.taps.first() {
            painter.circle_stroke(window(first), CLOSE_REACH / 2.0, Stroke::new(2.0, argb(pc::ACCENT)));
        }
        None
    }

    fn lasso_panel(&mut self, ui: &mut egui::Ui, session: &CanvasSession, rgb: [f32; 3], said: &mut Option<Said>) {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::selectable(!self.lasso_erase, "Fill")).clicked() {
                self.lasso_erase = false;
            }
            if ui.add(egui::Button::selectable(self.lasso_erase, "Erase")).clicked() {
                self.lasso_erase = true;
            }
            ui.separator();
            ui.checkbox(&mut self.smooth, "Smooth curves");
            ui.separator();
            if ui.add_enabled(self.taps.len() >= 3, egui::Button::new("Close shape")).clicked() {
                let path = std::mem::take(&mut self.taps);
                *said = self.apply_lasso(session, &path, rgb);
            }
            if ui.add_enabled(!self.taps.is_empty(), egui::Button::new("Clear points")).clicked() {
                self.taps.clear();
            }
        });
        ui.add_space(4.0);
        ui.label("Drag to draw a shape freehand, or tap point by point (tap the first point to close). It fills or erases at once.");
    }

    /// The gradient's touches: a drag lays one from where it starts to where it ends; its end
    /// dots move its ends. Shift keeps it to 45° steps.
    #[allow(clippy::too_many_arguments)]
    fn gradient_touch(
        &mut self,
        ui: &egui::Ui,
        session: &CanvasSession,
        response: &egui::Response,
        at: Option<Pos2>,
        m: [f32; 6],
        ppp: f32,
        to_canvas: &dyn Fn(Pos2) -> Option<Pos2>,
        rgb: [f32; 3],
        shift: bool,
    ) -> Option<Said> {
        let mut said = None;
        let window = |p: Pos2| to_window(m, ppp, p.x, p.y);
        if response.drag_started()
            && let Some(w) = response.interact_pointer_pos()
            && let Some(p) = to_canvas(w)
        {
            let near = |q: Pos2| window(q).distance(w) <= HANDLE_REACH;
            self.grad_grip = match self.grad {
                Some((a, _)) if near(a) => Some(1),
                Some((_, b)) if near(b) => Some(2),
                _ => {
                    self.grad = Some((p, p));
                    Some(0)
                }
            };
        }
        if response.dragged()
            && let (Some(grip), Some((a, b)), Some(mut p)) = (self.grad_grip, self.grad, at.and_then(to_canvas))
        {
            // Shift: the angle in 45° steps from the other end
            let other = if grip == 1 { b } else { a };
            if shift {
                let d = p - other;
                let step = std::f32::consts::FRAC_PI_4;
                let angle = (d.y.atan2(d.x) / step).round() * step;
                p = other + vec2(angle.cos(), angle.sin()) * d.length();
            }
            self.grad = Some(if grip == 1 { (p, b) } else { (a, p) });
            if self.grad.is_some_and(|(a, b)| a.distance(b) >= 1.0) {
                said = self.show_gradient(session, rgb);
            }
        }
        if response.drag_stopped() {
            self.grad_grip = None;
            // a tap, not a drag: nothing laid
            if self.grad.is_some_and(|(a, b)| a.distance(b) < 1.0) && !session.gradient_open() {
                self.grad = None;
            }
        }
        if let Some((a, b)) = self.grad {
            let painter = ui.painter();
            let (wa, wb) = (window(a), window(b));
            painter.line_segment([wa, wb], Stroke::new(3.0, Color32::from_black_alpha(140)));
            painter.line_segment([wa, wb], Stroke::new(1.5, Color32::WHITE));
            painter.circle_filled(wa, HANDLE_DOT + 2.0, crate::colours::srgb(rgb));
            painter.circle_stroke(wa, HANDLE_DOT + 2.0, Stroke::new(2.0, Color32::WHITE));
            let end = if self.to_clear { Color32::from_gray(60) } else { crate::colours::srgb(self.second) };
            painter.circle_filled(wb, HANDLE_DOT + 2.0, end);
            painter.circle_stroke(wb, HANDLE_DOT + 2.0, Stroke::new(2.0, Color32::WHITE));
        }
        said
    }

    fn gradient_panel(&mut self, ui: &mut egui::Ui, session: &CanvasSession, rgb: [f32; 3], said: &mut Option<Said>) {
        let mut changed = false;
        ui.horizontal(|ui| {
            for (kind, name) in [
                (GradientKind::Linear, "Linear"),
                (GradientKind::Radial, "Radial"),
                (GradientKind::Reflected, "Reflected"),
            ] {
                if ui.add(egui::Button::selectable(self.grad_kind == kind, name)).clicked() {
                    self.grad_kind = kind;
                    changed = true;
                }
            }
            ui.separator();
            if ui.add(egui::Button::selectable(self.to_clear, "To see-through")).clicked() {
                self.to_clear = true;
                changed = true;
            }
            if ui.add(egui::Button::selectable(!self.to_clear, "To colour")).clicked() {
                self.to_clear = false;
                changed = true;
            }
            if !self.to_clear && egui::color_picker::color_edit_button_rgb(ui, &mut self.second).changed() {
                changed = true;
            }
            ui.separator();
            ui.label("Bands");
            if ui.add(egui::Slider::new(&mut self.bands, 1..=8).show_value(false)).changed() {
                changed = true;
            }
            ui.label(if self.bands < 2 { "smooth".to_owned() } else { format!("{}", self.bands) });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let open = self.grad.is_some();
            if ui.add_enabled(open, egui::Button::new("Cancel")).clicked() {
                self.end_gradient(session, false);
            }
            let apply = egui::Button::new(egui::RichText::new("Apply").strong()).fill(argb(pc::ACCENT));
            if ui.add_enabled(open, apply).clicked() {
                self.end_gradient(session, true);
                *said = Some(Said::Notice("Gradient applied", false));
            }
            ui.label("Drag across the canvas: your colour at the start, blending to the end. Drag the dots to adjust.");
        });
        if changed && self.grad.is_some() {
            *said = self.show_gradient(session, rgb).or(said.take());
        }
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
