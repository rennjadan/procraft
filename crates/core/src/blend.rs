//! The gradient (blend) tool: a colour blended along a drag into see-through or a second colour,
//! straight across (linear), out from a point (radial) or both ways from a line (reflected),
//! smooth or in flat bands (as cel shading steps it). It shows on the current layer as it is
//! dragged and changed, the layer as it was kept underneath, within the selection if there is one
//! and only where it has paint when alpha-locked; applied it is one undo step, cancelled the layer
//! goes back.

use procraft_engine::document::LayerId;
use procraft_engine::selection::Mask;

use crate::CanvasSession;

/// How a gradient runs from its start to its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum GradientKind {
    Linear,
    Radial,
    Reflected,
}

/// A gradient: its kind, start and end (canvas pixels), its colours (0..1, the second's opacity
/// 0 for see-through), and how many flat bands it steps in (below 2: smooth).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct GradientSpec {
    pub kind: GradientKind,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub r0: f32,
    pub g0: f32,
    pub b0: f32,
    pub a0: f32,
    pub r1: f32,
    pub g1: f32,
    pub b1: f32,
    pub a1: f32,
    pub bands: u32,
}

/// A gradient or shape being placed: its layer as it was, what of it can change, where it last
/// showed (a shape's), and the undo step's name.
pub(crate) struct Draft {
    pub(crate) layer: LayerId,
    pub(crate) before: Vec<u8>,
    pub(crate) selection: Option<Mask>,
    pub(crate) alpha_lock: bool,
    pub(crate) shown: Option<procraft_engine::geom::IRect>,
    pub(crate) name: &'static str,
}

/// Begins a draft on the current layer (its pixels kept, the layer held), unless one is open;
/// false if it can't be (the layer locked, text or a plugin's).
pub(crate) fn begin_draft(st: &mut crate::State, name: &'static str) -> bool {
    if st.gradient.is_some() {
        return true;
    }
    let current = st.doc.current();
    let Some(layer) = st.doc.layer(current).cloned() else { return false };
    if st.doc.is_locked(current) || layer.drawn_from_data() {
        return false;
    }
    let Some(before) = st.current_pixels() else { return false };
    let selection = st.reach_selection();
    if let Some(canvas) = st.canvas.as_mut() {
        canvas.hold_layer(current);
    }
    st.gradient = Some(Draft { layer: current, before, selection, alpha_lock: layer.alpha_lock, shown: None, name });
    true
}

/// Ends the open draft: `keep` makes it one undo step (its name), else the layer goes back.
pub(crate) fn end_draft(shared: &crate::Shared, st: &mut crate::State, keep: bool) -> bool {
    let Some(draft) = st.gradient.take() else { return false };
    if let Some(canvas) = st.canvas.as_mut() {
        if keep {
            canvas.name_next_step(draft.name);
        }
        canvas.release_layer(keep, None);
    }
    st.revision += 1;
    shared.frame(st);
    true
}

/// How far along the gradient (0..1) the pixel centred at (`x`, `y`) is.
fn along(spec: &GradientSpec, x: f32, y: f32) -> f32 {
    let (dx, dy) = (spec.x1 - spec.x0, spec.y1 - spec.y0);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let (px, py) = (x - spec.x0, y - spec.y0);
    let t = match spec.kind {
        GradientKind::Linear => (px * dx + py * dy) / len2,
        GradientKind::Reflected => ((px * dx + py * dy) / len2).abs(),
        GradientKind::Radial => (px * px + py * py).sqrt() / len2.sqrt(),
    };
    let t = t.clamp(0.0, 1.0);
    if spec.bands >= 2 {
        let n = spec.bands as f32;
        (t * n).floor().min(n - 1.0) / (n - 1.0)
    } else {
        t
    }
}

/// The gradient over `before` (premultiplied RGBA8, `w` wide), as much as `selection` takes it;
/// with `alpha_lock`, each pixel's coverage kept.
fn blended(spec: &GradientSpec, before: &[u8], w: u32, selection: Option<&Mask>, alpha_lock: bool) -> Vec<u8> {
    let w = w.max(1) as usize;
    let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let c0 = [spec.r0, spec.g0, spec.b0, spec.a0].map(unit);
    let c1 = [spec.r1, spec.g1, spec.b1, spec.a1].map(unit);
    let mut out = before.to_vec();
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let k = selection.map_or(1.0, |m| f32::from(m.data.get(i).copied().unwrap_or(0)) / 255.0);
        if k <= 0.0 {
            continue;
        }
        let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
        let t = along(spec, x, y);
        // the colour there, straight then premultiplied
        let a = c0[3] + (c1[3] - c0[3]) * t;
        let rgb = [0, 1, 2].map(|c| {
            let (s, e) = (c0.get(c).copied().unwrap_or(0.0), c1.get(c).copied().unwrap_or(0.0));
            // toward see-through, the colour stays the first one's
            let e = if c1[3] <= 0.0 { s } else { e };
            (s + (e - s) * t) * a
        });
        let d = px.map(|v| f32::from(v) / 255.0);
        // over what is there
        let mut o =
            [rgb[0] + d[0] * (1.0 - a), rgb[1] + d[1] * (1.0 - a), rgb[2] + d[2] * (1.0 - a), a + d[3] * (1.0 - a)];
        if alpha_lock {
            let scale = if o[3] > 0.0 { d[3] / o[3] } else { 0.0 };
            o = [o[0] * scale, o[1] * scale, o[2] * scale, d[3]];
        }
        for (v, (n, was)) in px.iter_mut().zip(o.iter().zip(d)) {
            let mixed = was + (n - was) * k;
            *v = (mixed.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        // premultiplied: no channel above the alpha
        let alpha = px[3];
        for c in px.iter_mut().take(3) {
            *c = (*c).min(alpha);
        }
    }
    out
}

#[uniffi::export]
impl CanvasSession {
    /// Shows `spec` on the current layer (begun the first time, the layer as it was kept). False
    /// if it can't be (the layer locked, text or a plugin's).
    pub fn gradient_show(&self, spec: GradientSpec) -> bool {
        let mut guard = self.shared.lock();
        let st = &mut *guard;
        if !begin_draft(st, "Gradient") {
            return false;
        }
        let (w, h) = (st.doc.width(), st.doc.height());
        let Some(draft) = st.gradient.as_ref() else { return false };
        let pixels = blended(&spec, &draft.before, w, draft.selection.as_ref(), draft.alpha_lock);
        let layer = draft.layer;
        if let Some(canvas) = st.canvas.as_mut() {
            canvas.put_layer_region(layer, 0, 0, w, h, pixels);
        }
        self.shared.frame(st);
        true
    }

    /// Ends the gradient being placed: `keep` makes it one undo step ("Gradient"), otherwise the
    /// layer goes back to how it was.
    pub fn gradient_end(&self, keep: bool) {
        let mut guard = self.shared.lock();
        let st = &mut *guard;
        end_draft(&self.shared, st, keep);
    }

    /// Whether a gradient is being placed.
    pub fn gradient_open(&self) -> bool {
        self.shared.lock().gradient.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: GradientKind, bands: u32) -> GradientSpec {
        GradientSpec {
            kind,
            x0: 0.0,
            y0: 0.0,
            x1: 100.0,
            y1: 0.0,
            r0: 1.0,
            g0: 0.0,
            b0: 0.0,
            a0: 1.0,
            r1: 0.0,
            g1: 0.0,
            b1: 1.0,
            a1: 0.0,
            bands,
        }
    }

    #[test]
    fn a_gradient_runs_from_its_start_to_its_end_smooth_or_in_bands() {
        let linear = spec(GradientKind::Linear, 0);
        assert!(along(&linear, 0.0, 5.0) < 0.01);
        assert!((along(&linear, 50.0, 30.0) - 0.5).abs() < 0.01);
        assert_eq!(along(&linear, 150.0, 0.0), 1.0);
        let reflected = spec(GradientKind::Reflected, 0);
        assert!((along(&reflected, -50.0, 0.0) - 0.5).abs() < 0.01);
        let radial = spec(GradientKind::Radial, 0);
        assert!((along(&radial, 0.0, 50.0) - 0.5).abs() < 0.01);
        // three bands: flat steps at 0, a half and all
        let banded = spec(GradientKind::Linear, 3);
        assert_eq!(along(&banded, 10.0, 0.0), 0.0);
        assert_eq!(along(&banded, 50.0, 0.0), 0.5);
        assert_eq!(along(&banded, 90.0, 0.0), 1.0);
    }

    #[test]
    fn toward_see_through_the_colour_fades_and_alpha_lock_keeps_coverage() {
        // two pixels a row, see-through to start
        let s = GradientSpec { x1: 2.0, ..spec(GradientKind::Linear, 0) };
        let out = blended(&s, &[0; 8], 2, None, false);
        let (first, second) = (out.get(..4).unwrap(), out.get(4..).unwrap());
        assert!(first[0] > 150 && first[3] > 150, "near the start, red and solid: {first:?}");
        assert!(second[3] < first[3] && second[2] == 0, "fading, still red: {second:?}");
        // over nothing, alpha-locked: nothing
        assert_eq!(blended(&s, &[0; 8], 2, None, true), vec![0; 8]);
    }
}
