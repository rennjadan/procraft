//! A stroke's pressure as the plugins on the pressure hook give it (see
//! [`crate::hooks::Hooks::pressure`]): each sample asked once, in the order they come, before
//! anything else takes it; predicted input never asked, taking the last pressure given.

use procraft_engine::stroke::StrokeBuilder;

use crate::plugin_run::PressureAsked;
use crate::{PenSample, PointerKind, State};

/// The open stroke's pressure as the plugins give it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Pressing {
    /// The stroke's pointer, numbered as the hook numbers it, and when its first sample came
    /// (milliseconds, the input's clock).
    pointer: i32,
    first_ms: f64,
    /// A plugin gave its first sample a pressure: the stroke draws with the pressure given though
    /// its pointer reports none (a finger).
    pub(crate) given: bool,
    /// The last pressure given, which predicted input takes.
    last: Option<f32>,
    /// How far its pressure grows from nothing at its start and falls to nothing at its end,
    /// window points.
    pub(crate) tapers: [f32; 2],
}

impl Pressing {
    /// `builder` drawing with this pressure, `per_point` canvas pixels (as its samples come) a
    /// window point.
    pub(crate) fn applied(&self, builder: StrokeBuilder, per_point: f32) -> StrokeBuilder {
        let builder = if self.given { builder.with_given_pressure() } else { builder };
        if self.tapers == [0.0; 2] || !per_point.is_finite() {
            return builder;
        }
        builder.with_pressure_tapers(self.tapers[0] * per_point, self.tapers[1] * per_point)
    }
}

/// `builder` pressed as `like` was (the stroke it lays again).
pub(crate) fn pressed_like(builder: StrokeBuilder, like: &StrokeBuilder) -> StrokeBuilder {
    let (given, [start, end]) = like.pressing();
    let builder = if given { builder.with_given_pressure() } else { builder };
    builder.with_pressure_tapers(start, end)
}

/// The most a pen's pressure is scaled by (see [`pen_pressure_scale`]): past any pen's force.
const MOST_PRESSURE_SCALE: f32 = 16.0;

/// The pressure a pen's sample gives the strokes, from its force as its platform measures it and
/// the most force it can sense (on an iPad, a Pencil touch's `force` and `maximumPossibleForce`):
/// the share of the most, 0..1. A touch that senses no force (none is the most it can) presses
/// fully, as a finger does; a force that isn't finite presses not at all.
#[uniffi::export]
pub fn pen_pressure(force: f64, most: f64) -> f32 {
    if !(most.is_finite() && most > 0.0) {
        return 1.0;
    }
    if !force.is_finite() {
        return 0.0;
    }
    (force / most).clamp(0.0, 1.0) as f32
}

/// How far a pen's pressure reaches for the strokes (see
/// [`procraft_engine::stroke::InputSettings::pressure_scale`]), from the most force it can sense
/// in its platform's units: on an iPad, where an ordinary press is a force of 1, a Pencil's most
/// (about 4.17), so an ordinary press is full and a harder one presses past it. Never under 1 (a
/// touch that senses no force, or a pen whose pressure is already its share of full, is 1).
#[uniffi::export]
pub fn pen_pressure_scale(most: f64) -> f32 {
    if most.is_finite() && most > 1.0 { (most as f32).min(MOST_PRESSURE_SCALE) } else { 1.0 }
}

/// How the pressure hook numbers a pointer.
pub(crate) fn pointer_number(pointer: PointerKind) -> i32 {
    match pointer {
        PointerKind::Finger => 0,
        PointerKind::Stylus => 1,
        PointerKind::StylusEraser => 2,
        PointerKind::Mouse => 3,
    }
}

impl State {
    /// `samples` as the plugins on the pressure hook press them: a stroke's first by `pointer`, or
    /// more of the open one (`pointer` none). Nothing changes with none on it.
    pub(crate) fn press(&mut self, pointer: Option<PointerKind>, samples: &mut [PenSample]) {
        // a stroke starts with nothing given, whatever the last was given
        if let Some(pointer) = pointer {
            let first_ms = samples.first().map_or(0.0, |s| s.time_ms);
            self.pressing = Pressing { pointer: pointer_number(pointer), first_ms, ..Pressing::default() };
        }
        let Some(hooks) = self.hooks.clone().filter(|h| h.listens(crate::plugin_run::Hook::Pressure)) else { return };
        let per_point = if self.display_scale > 0.0 { 1.0 / self.display_scale } else { 1.0 };
        let p = &mut self.pressing;
        for (i, s) in samples.iter_mut().enumerate() {
            let first = pointer.is_some() && i == 0;
            let ms = (s.time_ms - p.first_ms) as f32;
            let asked = PressureAsked {
                pointer: p.pointer,
                first,
                x: s.x * per_point,
                y: s.y * per_point,
                ms,
                pressure: s.pressure,
            };
            let said = hooks.pressure(asked);
            if first {
                p.given = said.is_some() && !matches!(pointer, Some(PointerKind::Stylus | PointerKind::StylusEraser));
            }
            // a sample the plugins leave, in a stroke drawn with the pressure given, takes the last
            if let Some(pressure) = said.or(p.last.filter(|_| p.given)) {
                s.pressure = pressure;
                p.last = Some(pressure);
            }
        }
        if pointer.is_some() {
            p.tapers = hooks.pressure_tapers(p.pointer);
        }
    }

    /// Predicted input for the open stroke, pressed as its last sample was.
    pub(crate) fn press_predicted(&self, samples: &mut [PenSample]) {
        if let Some(last) = self.pressing.last {
            for s in samples {
                s.pressure = last;
            }
        }
    }

    /// Canvas pixels a window point, as the stroke's samples come onto the canvas.
    pub(crate) fn canvas_per_point(&self) -> f32 {
        self.view.map_or(1.0, |v| self.display_scale / v.zoom().max(1e-6))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An iPad's Pencil: UIKit's most force for it.
    const PENCIL_MOST: f64 = 25.0 / 6.0;

    #[test]
    fn a_pencils_force_is_its_share_of_the_most_it_senses() {
        // (force, the most it senses, the pressure the strokes get)
        let cases = [
            (0.0, PENCIL_MOST, 0.0),
            (0.1, PENCIL_MOST, 0.024),
            (0.5, PENCIL_MOST, 0.12),
            (1.0, PENCIL_MOST, 0.24),
            (2.0, PENCIL_MOST, 0.48),
            (PENCIL_MOST, PENCIL_MOST, 1.0),
            // pressed past what it says it senses, or below nothing
            (5.0, PENCIL_MOST, 1.0),
            (-0.3, PENCIL_MOST, 0.0),
            (f64::NAN, PENCIL_MOST, 0.0),
            (f64::INFINITY, PENCIL_MOST, 0.0),
            // a finger on an iPad senses no force: full, as a finger draws
            (0.0, 0.0, 1.0),
            (0.4, f64::NAN, 1.0),
            (0.4, -1.0, 1.0),
        ];
        for (force, most, want) in cases {
            let got = pen_pressure(force, most);
            println!("force {force:.3} of {most:.3}: pressure {got:.4}");
            assert!((got - want).abs() < 1e-6, "force {force} of {most}: {got}, not {want}");
        }
    }

    #[test]
    fn a_pencils_pressure_reaches_as_far_as_its_most_force() {
        let pencil = pen_pressure_scale(PENCIL_MOST);
        assert!((pencil - 25.0 / 6.0).abs() < 1e-6, "{pencil}");
        // an ordinary press (a force of 1) is full pressure once scaled
        assert!((pen_pressure(1.0, PENCIL_MOST) * pencil - 1.0).abs() < 1e-6);
        // no force sensed, or a pressure already its share of full: as it comes
        for most in [0.0, 0.5, 1.0, -2.0, f64::NAN, f64::NEG_INFINITY] {
            assert_eq!(pen_pressure_scale(most), 1.0, "{most}");
        }
        // never past the most any pen could be
        assert_eq!(pen_pressure_scale(1e9), MOST_PRESSURE_SCALE);
        assert_eq!(pen_pressure_scale(f64::INFINITY), 1.0);
    }

    /// A stock brush, by its set and name.
    fn stock(set: &str, name: &str) -> procraft_engine::brush::Brush {
        let sets = crate::stock_library::shipped();
        let found = sets.iter().filter(|s| s.name == set).flat_map(|s| &s.brushes).find(|b| b.name == name);
        found.unwrap_or_else(|| panic!("{name} in {set}")).clone()
    }

    /// The stamps a Pencil lays pressing with `force` all along a slow line (240 samples a second
    /// for two seconds), the pen's pressure reaching `scale` times its share of its most force:
    /// those of its middle third, past where any taper or the following of the pressure reaches.
    fn pressed_with(
        brush: &procraft_engine::brush::Brush,
        force: f64,
        scale: f32,
    ) -> Vec<procraft_engine::stroke::Stamp> {
        use procraft_engine::geom::Vec2;
        use procraft_engine::input::{Pointer, Sample};
        use procraft_engine::stroke::InputSettings;
        let pressure = pen_pressure(force, PENCIL_MOST);
        let samples: Vec<Sample> = (0..480)
            .map(|i| Sample::at(Vec2::new(100.0 + 0.5 * i as f32, 300.0), pressure, f64::from(i) * 1000.0 / 240.0))
            .collect();
        let input = InputSettings { pressure_scale: scale, ..InputSettings::default() };
        let mut b = StrokeBuilder::new(brush.clone(), 40.0, Pointer::Stylus).with_input(input);
        for chunk in samples.chunks(4) {
            b.add(chunk);
        }
        b.finish().into_iter().filter(|s| (180.0..260.0).contains(&s.center.x)).collect()
    }

    /// A stamp's opacity as a Pencil pressing with `force` lays it with `brush`, worked out the
    /// way the reference app works it out: the force as a share of the most (through the app's
    /// curve, straight as it ships), the brush's curve and its opacity curve, then times the
    /// most force; the brush's opacity amount `a` takes `1 - a + a·p` of it, the flow and its
    /// opacity range take that, held to 0.01..1, and the stamp is that to the power 2.5.
    fn reference_opacity(brush: &procraft_engine::brush::Brush, force: f64) -> f32 {
        let share = (force / PENCIL_MOST).clamp(0.0, 1.0) as f32;
        let pressed = brush.opacity_curve.eval(brush.pressure_curve.eval(share)) * (25.0 / 6.0);
        let a = brush.pressure_opacity;
        let factor = if a >= 0.0 { 1.0 - a + a * pressed } else { 1.0 + a * pressed };
        let control = brush.flow.powf(1.0 / procraft_engine::brush::FLOW_POWER) * factor;
        let ranged = (brush.min_opacity + (brush.max_opacity - brush.min_opacity) * control).clamp(0.01, 1.0);
        ranged.powf(procraft_engine::brush::FLOW_POWER)
    }

    #[test]
    fn a_pencils_light_press_lays_a_stock_brush_as_opaque_as_the_reference_does() {
        // a soft round brush whose opacity is all pressure, and a pencil with an opacity range,
        // flow and a pressure curve of its own (its opacity jitter off, so each stamp can be read)
        let mut pencil = stock("Pencils", "Ibis");
        pencil.set_setting("jitter_opacity", 0.0);
        for (name, brush) in [("Hyacinth", stock("Basics", "Hyacinth")), ("Ibis", pencil)] {
            println!(
                "{name}: force, the stamps' opacity (pressure not reaching past its share, then reaching as the reference's), the reference's"
            );
            for force in [0.1, 0.25, 0.5, 1.0, 1.5, 2.0, PENCIL_MOST] {
                let mean = |stamps: &[procraft_engine::stroke::Stamp]| {
                    stamps.iter().map(|s| s.alpha).sum::<f32>() / stamps.len().max(1) as f32
                };
                let (before, now) =
                    (pressed_with(&brush, force, 1.0), pressed_with(&brush, force, pen_pressure_scale(PENCIL_MOST)));
                let want = reference_opacity(&brush, force);
                println!("  {force:.2}: {:.4} -> {:.4} (reference {want:.4})", mean(&before), mean(&now));
                assert!(!now.is_empty());
                for stamp in &now {
                    assert!(
                        (stamp.alpha - want).abs() < 2e-3 + want * 0.01,
                        "{name} at {force}: {} not {want}",
                        stamp.alpha
                    );
                }
            }
        }
    }

    /// A stamp's width (canvas pixels) as a Pencil pressing with `force` lays it with `brush`, the
    /// side slider at `slider`, worked out the way the reference app works it out: the force as a
    /// share of the most (through the app's curve, straight as it ships), the brush's curve and its
    /// size curve, then times the most force; the brush's size amount `a` leaves `1 - a + a·p` of
    /// the slider's size above the brush's least (`1 + a·p` for one that thins as it is pressed),
    /// and a stamp grows no bigger than the brush's most.
    fn reference_width(brush: &procraft_engine::brush::Brush, slider: f32, force: f64) -> f32 {
        let share = (force / PENCIL_MOST).clamp(0.0, 1.0) as f32;
        let pressed = brush.size_curve.eval(brush.pressure_curve.eval(share)) * (25.0 / 6.0);
        let a = brush.pressure_size.clamp(-1.0, 1.0);
        let factor = if a >= 0.0 { 1.0 - a + a * pressed } else { 1.0 + a * pressed };
        let (most, at) = (brush.biggest(), brush.diameter(slider));
        let least = brush.min_size.min(most).min(at);
        (least + (at - least) * factor.max(0.0)).min(most.max(at))
    }

    /// A session as the iPad opens one, with nothing on it but the stock brushes.
    fn pencil_session() -> std::sync::Arc<crate::CanvasSession> {
        let side = 1024;
        let s = crate::CanvasSession::new(side, side);
        {
            let mut st = s.shared.lock();
            st.window_size = (side, side);
            st.view = Some(procraft_engine::view::View::fit((side, side), (side, side)));
        }
        s
    }

    /// Brush `index` of set `set_at` of `s`'s library chosen (its size jitter, speed size, lean,
    /// tapers and pressure response set aside if `plain`, so each stamp's width can be read), the
    /// side slider at `slider`; the brush as it draws.
    fn choose(
        s: &crate::CanvasSession,
        set_at: u32,
        index: u32,
        slider: f32,
        plain: bool,
    ) -> procraft_engine::brush::Brush {
        use procraft_engine::brush::Taper;
        assert!(s.select_brush(set_at, index));
        let brush = {
            let mut st = s.shared.lock();
            let tool = st.tool;
            let b = st.brush_mut(tool).expect("the chosen brush");
            if plain {
                b.pen_taper = Taper::NONE;
                b.tilt.size = 0.0;
                b.set_settings([
                    ("jitter_size", 0.0),
                    ("speed_size", 0.0),
                    ("pressure_size_response", 0.0),
                    ("pen_taper_pressure", 0.0),
                ]);
            }
            b.clone()
        };
        s.set_brush_size(slider);
        brush
    }

    /// The stamps `s` lays for a Pencil pressing with `force`, held upright, along a slow line,
    /// sampled as the iPad samples it: 240 a second, in pixels, its pressure the force's share of
    /// the most; the session told its pressure reaches `scale` times that share, as the iPad tells
    /// it as the Pencil comes down. The stroke is made as the session makes one (its brush and
    /// diameter at the slider, its input settings, the samples taken onto the canvas), without
    /// the canvas a GPU would give it to lay them on. Those of the line's middle third.
    fn pencil_line(s: &crate::CanvasSession, force: f64, scale: f32) -> Vec<procraft_engine::stroke::Stamp> {
        use procraft_engine::input::Pointer;
        use procraft_engine::stroke::StrokeBuilder;
        s.set_pen_pressure_scale(scale);
        let samples: Vec<crate::PenSample> = (0..1200u32)
            .map(|i| crate::PenSample {
                x: 100.0 + 0.5 * i as f32,
                y: 500.0,
                pressure: pen_pressure(force, PENCIL_MOST),
                tilt: 0.0,
                orientation: 0.0,
                time_ms: f64::from(i) * 1000.0 / 240.0,
                roll: 0.0,
            })
            .collect();
        let mut st = s.shared.lock();
        let (tool, current) = (st.tool, st.doc.current());
        let (brush, _, diameter) = st.stroke_brush(tool, current);
        let scaled = st.brush_scale();
        let mut b = StrokeBuilder::with_seed(brush, diameter * scaled, Pointer::Stylus, 1)
            .scaled(scaled)
            .with_input(st.input.clone());
        for chunk in st.to_canvas(&samples).chunks(4) {
            b.add(chunk);
        }
        b.stamps().into_iter().filter(|stamp| (200.0..400.0).contains(&stamp.distance)).collect()
    }

    #[test]
    fn an_ordinary_pencil_press_lays_stock_brushes_as_wide_as_the_reference_does_at_40_percent() {
        use procraft_engine::brush::SIZE_CURVE;
        // the side slider where its panel reads 40%
        let slider = 0.4f32.powf(1.0 / SIZE_CURVE);
        assert_eq!(crate::side_slider_shown_percent(true, slider), 40);
        // an ordinary press: a force of 1, the system's own ordinary touch
        let force = 1.0;
        // brushes whose size is mostly pressure, from a pencil to a pen and an ink (a curve of
        // their own among them), and one thinning as it is pressed
        let mut checked = 0;
        let s = pencil_session();
        let wanted: Vec<(u32, u32, String)> = {
            let st = s.shared.lock();
            st.library
                .iter()
                .enumerate()
                .filter(|(_, set)| ["Pencils", "Inking", "Sketching"].iter().any(|n| set.name == *n))
                .flat_map(|(k, set)| {
                    set.brushes
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.pressure_size.abs() >= 0.3 && b.secondary.is_none() && b.engine.is_none())
                        .map(move |(i, b)| (k as u32, i as u32, format!("{} / {}", set.name, b.name)))
                })
                .collect()
        };
        for (set_at, index, name) in wanted {
            let drawn = choose(&s, set_at, index, slider, true);
            // as v0.0.8 laid it (the pressure reaching no further than its share), and now
            let before = pencil_line(&s, force, 1.0);
            let now = pencil_line(&s, force, pen_pressure_scale(PENCIL_MOST));
            let want = reference_width(&drawn, slider, force);
            let mean = |stamps: &[procraft_engine::stroke::Stamp]| {
                stamps.iter().map(|s| 2.0 * s.radius).sum::<f32>() / stamps.len().max(1) as f32
            };
            let (was, is) = (mean(&before), mean(&now));
            println!(
                "{name}: pressure size {:.2}; a stamp {was:.1} px wide as v0.0.8 laid it ({:.0}% of the reference's), {is:.1} px now; the reference's {want:.1} px",
                drawn.pressure_size,
                100.0 * was / want.max(1e-3)
            );
            assert!(!now.is_empty(), "{name}: no stamps");
            for stamp in &now {
                let width = 2.0 * stamp.radius;
                assert!((width - want).abs() <= 0.02 * want + 0.05, "{name}: {width} wide, not {want}");
            }
            // pressed fully now, a brush that grows as it is pressed is never narrower (one
            // whose curve already reached full by a quarter of the force was as wide before)
            if drawn.pressure_size > 0.0 {
                assert!(was <= is + 1e-3, "{name}: narrower now");
            }
            checked += 1;
        }
        assert!(checked >= 5, "{checked} stock brushes checked");
    }

    /// What of a stamp a lean may change, but where it lies (a leaning Pencil moves every stamp a
    /// little towards its top, as the reference's does): its size, opacity, roundness, grain,
    /// colour, shading and wet attack.
    fn look(s: &procraft_engine::stroke::Stamp) -> [f32; 15] {
        let t = s.tint;
        [
            s.radius,
            s.alpha,
            s.roundness,
            s.grain_press,
            s.grain_bleed,
            s.secondary,
            s.grain_depth,
            s.grain_size,
            s.gradation,
            s.attack,
            t.hue,
            t.saturation[0],
            t.saturation[1],
            t.brightness[0],
            t.brightness[1],
        ]
    }

    /// Whether two runs of stamps look alike, but where they lie.
    fn look_alike(a: &[procraft_engine::stroke::Stamp], b: &[procraft_engine::stroke::Stamp]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(a, b)| look(a).iter().zip(look(b)).all(|(x, y)| (x - y).abs() <= 1e-4 * x.abs().max(1.0)))
    }

    #[test]
    fn a_leaning_pencil_changes_every_stock_brush_with_a_tilt_control_and_only_those() {
        use std::f32::consts::FRAC_PI_2;
        // held low for shading (30 degrees above the glass), and at a writing angle (55 degrees)
        let (shading, writing) = (30f32.to_radians(), 55f32.to_radians());
        let (mut with_tilt, mut without, mut wrong) = (Vec::new(), 0, Vec::new());
        // a Pencil pressing as usual along a line as a hand shading goes (3 pixels a sample: a
        // brush with size compression grows little from shorter steps), sampled as the iPad
        // samples it, held at `altitude` above the glass (its tilt a quarter turn less), its top
        // leaning up the screen, across the line (such a brush grows only so); the side slider at
        // 40%: the stamps of the line's middle third
        let line = |brush: &procraft_engine::brush::Brush, altitude: f32| {
            use procraft_engine::geom::Vec2;
            use procraft_engine::input::{Pointer, Sample};
            use procraft_engine::stroke::{InputSettings, StrokeBuilder};
            let input = InputSettings { pressure_scale: pen_pressure_scale(PENCIL_MOST), ..InputSettings::default() };
            let diameter = brush.diameter(0.4f32.powf(1.0 / procraft_engine::brush::SIZE_CURVE));
            let mut b = StrokeBuilder::with_seed(brush.clone(), diameter, Pointer::Stylus, 7).with_input(input);
            let samples: Vec<Sample> = (0..300)
                .map(|i| Sample {
                    tilt: FRAC_PI_2 - altitude,
                    azimuth: 0.0,
                    ..Sample::at(
                        Vec2::new(100.0 + 3.0 * i as f32, 500.0),
                        pen_pressure(1.0, PENCIL_MOST),
                        f64::from(i) * 1000.0 / 240.0,
                    )
                })
                .collect();
            for chunk in samples.chunks(4) {
                b.add(chunk);
            }
            b.stamps()
                .into_iter()
                .filter(|st| (300.0..600.0).contains(&st.distance))
                .collect::<Vec<procraft_engine::stroke::Stamp>>()
        };
        for set in crate::stock_library::shipped().iter() {
            for b in set.brushes.iter().filter(|b| b.secondary.is_none() && b.engine.is_none()) {
                let amounts = [
                    "tilt_gradation",
                    "tilt_bleed",
                    "tilt_roundness",
                    "tilt_hue",
                    "tilt_saturation",
                    "tilt_brightness",
                    "tilt_secondary",
                ];
                let switches = [
                    "count_tilt",
                    "grain_depth_tilt",
                    "stamp_hue_tilt",
                    "stamp_saturation_tilt",
                    "stamp_lightness_tilt",
                    "stamp_darkness_tilt",
                    "stamp_secondary_tilt",
                    "attack_tilt",
                    "jitter_lateral_tilt",
                    "jitter_linear_tilt",
                ];
                // what leaning does to it, of what it shows: size, opacity, shading, bleed, roundness
                let shows =
                    b.tilt.size != 0.0 || b.tilt.opacity != 0.0 || amounts.iter().take(3).any(|k| b.setting(k) != 0.0);
                let any = shows
                    || amounts.iter().any(|k| b.setting(k) != 0.0)
                    || switches.iter().any(|k| b.setting(k) >= 0.5);
                let (upright, low, mid) = (line(b, FRAC_PI_2), line(b, shading), line(b, writing));
                let responds = !look_alike(&upright, &low);
                if any {
                    // where its size or opacity control starts: degrees above the glass, as the
                    // reference's ramp has it (nothing until the altitude's share of a quarter turn
                    // is down to 0.382 + 0.618 of the control's angle)
                    let start = |theta: f32| 90.0 * (0.38198 + 0.61802 * theta);
                    let wide = |stamps: &[procraft_engine::stroke::Stamp]| {
                        stamps.iter().map(|s| 2.0 * s.radius).sum::<f32>() / stamps.len().max(1) as f32
                    };
                    with_tilt.push(format!(
                        "{} / {}: tilt size {:.2}, opacity {:.2}, gradation {:.2}, bleed {:.2}; size from {:.0} deg, opacity from {:.0} deg; {:.1} px wide upright, {:.1} at 30 deg (its own most {:.1}); at 30 deg {}, at 55 deg {}",
                        set.name,
                        b.name,
                        b.tilt.size,
                        b.tilt.opacity,
                        b.setting("tilt_gradation"),
                        b.setting("tilt_bleed"),
                        start(b.setting("size_tilt_angle")),
                        start(b.setting("opacity_tilt_angle")),
                        wide(&upright),
                        wide(&low),
                        b.biggest(),
                        if responds { "changes" } else { "same" },
                        if look_alike(&upright, &mid) { "same" } else { "changes" },
                    ));
                    if shows && !responds {
                        wrong.push(format!(
                            "{} / {}: a tilt control, but no change held at 30 degrees",
                            set.name, b.name
                        ));
                    }
                } else {
                    without += 1;
                    if responds {
                        wrong.push(format!("{} / {}: no tilt control, yet leaning changed it", set.name, b.name));
                    }
                }
            }
        }
        println!("{} stock brushes have a tilt control ({without} have none, as their files):", with_tilt.len());
        for line in &with_tilt {
            println!("  {line}");
        }
        assert!(with_tilt.len() >= 20, "{} brushes with tilt", with_tilt.len());
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn pointers_are_numbered_as_the_hook_says() {
        let numbers = [PointerKind::Finger, PointerKind::Stylus, PointerKind::StylusEraser, PointerKind::Mouse]
            .map(pointer_number);
        assert_eq!(numbers, [0, 1, 2, 3]);
    }

    #[test]
    fn with_nothing_given_a_builder_is_left_as_it_was() {
        use procraft_engine::{brush::Brush, input::Pointer};
        let plain = || StrokeBuilder::new(Brush::round_pen(), 10.0, Pointer::Finger);
        assert_eq!(Pressing::default().applied(plain(), 1.0).pressing(), (false, [0.0; 2]));
        let given = Pressing { given: true, tapers: [10.0, 20.0], ..Pressing::default() };
        assert_eq!(given.applied(plain(), 0.5).pressing(), (true, [5.0, 10.0]));
        let like = given.applied(plain(), 2.0);
        assert_eq!(pressed_like(plain(), &like).pressing(), (true, [20.0, 40.0]));
    }
}
