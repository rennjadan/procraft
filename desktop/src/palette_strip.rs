//! Five colours in the top bar, as Adobe Color's five-colour themes: tapped one is painted with,
//! held it takes the colour painted with, dragged out it fills where it lands (ColorDrop). The
//! button beside them suggests palettes by the harmony rules (round the colour painted with or
//! at random, shuffled again at will): one picked fills the strip; it can be kept as a palette or
//! put down as swatches on a layer of their own. The strip can be hidden; the button stays.

use egui::{Color32, Id, Order, Rect, Sense, Stroke, pos2, vec2};
use procraft_core::{CanvasSession, HarmonyPalette, harmony_palettes};

use crate::colours::of_hex;
use crate::look::{argb, pc};

/// A swatch's size and the gap between (points).
const SWATCH: f32 = 28.0;
const GAP: f32 = 6.0;
const BUTTON: f32 = 30.0;

pub struct Strip {
    pub colors: [u32; 5],
    pub shown: bool,
    picker: bool,
    random_base: bool,
    seed: u64,
    list: Vec<HarmonyPalette>,
    /// The picker's place, from the button.
    button: Rect,
}

impl Default for Strip {
    fn default() -> Strip {
        Strip {
            colors: [0x2B2D42, 0x8D99AE, 0xEDF2F4, 0xEF233C, 0xD90429],
            shown: true,
            picker: false,
            random_base: false,
            seed: 1,
            list: Vec::new(),
            button: Rect::NOTHING,
        }
    }
}

/// What the strip asks of the screen.
pub enum Said {
    /// The strip changed (to keep).
    Changed,
    Notice(&'static str),
    /// Keep these five as a palette.
    Save([u32; 5]),
}

/// Where the swatches and the button go in `gap` (the top bar between its action buttons and its
/// tools): the button, then the swatches (none hidden or without room).
pub fn layout(gap: Rect, shown: bool) -> (Option<Rect>, Vec<Rect>) {
    let swatches = if shown { 5.0 * SWATCH + 4.0 * GAP + 10.0 } else { 0.0 };
    let need = BUTTON + swatches;
    if gap.width() < need + 16.0 {
        // not even the button fits
        if gap.width() < BUTTON + 16.0 {
            return (None, Vec::new());
        }
        let button = Rect::from_center_size(gap.center(), vec2(BUTTON, BUTTON));
        return (Some(button), Vec::new());
    }
    let left = gap.center().x - need / 2.0;
    let cy = gap.center().y;
    let button = Rect::from_min_size(pos2(left, cy - BUTTON / 2.0), vec2(BUTTON, BUTTON));
    let rects = if shown {
        (0..5)
            .map(|i| {
                let x = button.right() + 10.0 + i as f32 * (SWATCH + GAP);
                Rect::from_min_size(pos2(x, cy - SWATCH / 2.0), vec2(SWATCH, SWATCH))
            })
            .collect()
    } else {
        Vec::new()
    };
    (Some(button), rects)
}

impl Strip {
    /// The kept strip: its colours (hex, comma between) and whether it shows.
    pub fn kept(colors: &str, shown: bool) -> Strip {
        let mut s = Strip { shown, ..Strip::default() };
        let read: Vec<u32> = colors.split(',').filter_map(|c| u32::from_str_radix(c.trim(), 16).ok()).collect();
        if let Ok(five) = <[u32; 5]>::try_from(read) {
            s.colors = five;
        }
        s
    }

    pub fn colors_text(&self) -> String {
        self.colors.iter().map(|c| format!("{c:06X}")).collect::<Vec<_>>().join(",")
    }

    /// Draws the button and the swatches (the screen handles the swatches' touches); a tap on
    /// the button opens or closes the picker.
    pub fn paint(&mut self, ui: &egui::Ui, button: Option<Rect>, swatches: &[Rect]) {
        let painter = ui.painter();
        for (rect, c) in swatches.iter().zip(self.colors) {
            painter.rect_filled(*rect, 6.0, crate::colours::srgb(of_hex(c)));
            painter.rect_stroke(*rect, 6.0, Stroke::new(1.0, Color32::from_white_alpha(70)), egui::StrokeKind::Inside);
        }
        let Some(b) = button else { return };
        self.button = b;
        let lit = self.picker;
        painter.circle_filled(b.center(), BUTTON / 2.0, if lit { argb(pc::ACCENT) } else { argb(pc::ACTION_DISC) });
        // five dots round a centre, as a palette
        for i in 0..5 {
            let a = (i as f32 * 72.0 - 90.0).to_radians();
            let p = b.center() + vec2(a.cos(), a.sin()) * 7.5;
            let c = self.colors.get(i).copied().unwrap_or(0xFFFFFF);
            painter.circle_filled(p, 3.4, crate::colours::srgb(of_hex(c)));
        }
        if ui.interact(b, ui.scope_id().with("palette strip button"), Sense::click()).clicked() {
            self.picker = !self.picker;
            if self.picker && self.list.is_empty() {
                self.refresh(0);
            }
        }
    }

    fn refresh(&mut self, base: u32) {
        self.list = harmony_palettes(base, self.random_base, self.seed);
    }

    /// The picker hanging from the button, while open; `current`: the colour painted with.
    pub fn picker(&mut self, ctx: &egui::Context, session: &CanvasSession, current: u32) -> Option<Said> {
        if !self.picker {
            return None;
        }
        if self.list.is_empty() {
            self.refresh(current);
        }
        let mut said = None;
        let top = self.button.bottom() + 10.0;
        let width = 380.0;
        let left = (self.button.center().x - width / 2.0).max(8.0);
        let area = egui::Area::new(Id::unique("palette picker")).fixed_pos(pos2(left, top)).order(Order::Foreground);
        let shown = area.show(ctx, |ui| {
            egui::Frame::NONE
                .fill(Color32::from_rgba_unmultiplied(34, 34, 34, 245))
                .corner_radius(14.0)
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    ui.visuals_mut().override_text_color = Some(Color32::from_gray(225));
                    ui.horizontal(|ui| {
                        ui.strong("Colour palettes");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() {
                                self.picker = false;
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::selectable(!self.random_base, "From my colour")).clicked() {
                            self.random_base = false;
                            self.refresh(current);
                        }
                        if ui.add(egui::Button::selectable(self.random_base, "Random")).clicked() {
                            self.random_base = true;
                            self.refresh(current);
                        }
                        if ui.button("🔀 Shuffle").clicked() {
                            self.seed = self
                                .seed
                                .wrapping_mul(6_364_136_223_846_793_005)
                                .wrapping_add(1_442_695_040_888_963_407);
                            self.refresh(current);
                        }
                    });
                    ui.add_space(6.0);
                    let mut picked = None;
                    for (i, p) in self.list.iter().enumerate() {
                        let (rect, response) = ui.allocate_exact_size(vec2(width - 24.0, 34.0), Sense::click());
                        let painter = ui.painter();
                        if response.hovered() {
                            painter.rect_filled(rect, 8.0, Color32::from_white_alpha(14));
                        }
                        crate::look::text(
                            painter,
                            pos2(rect.left() + 6.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &p.name,
                            13.5,
                            crate::look::Weight::Regular,
                            Color32::from_gray(210),
                        );
                        let cell = 36.0;
                        for (k, c) in p.colors.iter().enumerate() {
                            let x = rect.right() - 5.0 * cell + k as f32 * cell;
                            let r =
                                Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(cell - 2.0, rect.height() - 6.0));
                            painter.rect_filled(r, 4.0, crate::colours::srgb(of_hex(*c)));
                        }
                        if response.clicked() {
                            picked = Some(i);
                        }
                    }
                    if let Some(five) =
                        picked.and_then(|i| self.list.get(i)).and_then(|p| <[u32; 5]>::try_from(p.colors.clone()).ok())
                    {
                        self.colors = five;
                        self.shown = true;
                        said = Some(Said::Changed);
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        if ui.checkbox(&mut self.shown, "Show in top bar").changed() {
                            said = Some(Said::Changed);
                        }
                        if ui.button("Save to Palettes").clicked() {
                            said = Some(Said::Save(self.colors));
                        }
                        if ui.button("Put on a layer").clicked() {
                            said = Some(self.put_on_layer(session));
                        }
                    });
                    ui.label(
                        egui::RichText::new(
                            "Tap a swatch to paint with it, hold it to set it to your colour, drag it to fill.",
                        )
                        .size(11.5)
                        .color(Color32::from_gray(160)),
                    );
                });
        });
        // a press elsewhere closes it (not on the button, which toggles it)
        let pressed = ctx.input(|i| i.pointer.primary_pressed().then(|| i.pointer.interact_pos()).flatten());
        if let Some(p) = pressed
            && !shown.response.rect.contains(p)
            && !self.button.contains(p)
        {
            self.picker = false;
        }
        said
    }

    /// The five colours as squares along the top of a new layer named "Palette".
    fn put_on_layer(&self, session: &CanvasSession) -> Said {
        let info = session.info();
        let (w, h) = (info.width as f32, info.height as f32);
        let side = (w.min(h) / 10.0).max(8.0);
        let margin = side / 3.0;
        let id = session.add_layer();
        session.rename_layer(id, "Palette".into());
        let mut all = true;
        for (i, c) in self.colors.iter().enumerate() {
            let x = margin + i as f32 * (side + margin / 2.0);
            let [r, g, b] = of_hex(*c);
            let square = vec![x, margin, x + side, margin, x + side, margin + side, x, margin + side];
            all &= session.lasso_fill(square, false, r, g, b);
        }
        if all { Said::Notice("Palette put on its own layer") } else { Said::Notice("Couldn't put the palette down") }
    }
}

/// The gap in the top bar between the Shapes button and the brush button, if both are there.
pub fn bar_gap(bar: &crate::chrome::Bar) -> Option<Rect> {
    use crate::chrome::BarButton;
    let left = bar.rect(BarButton::Shapes)?;
    let right = bar.rect(BarButton::Paint)?;
    (right.left() > left.right())
        .then(|| Rect::from_min_max(pos2(left.right(), left.top()), pos2(right.left(), left.bottom())))
}
