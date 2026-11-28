//! QuickShape as a hand meets it: small ellipses drawn by hand (a lumpy loop, the pen landing with
//! a tick, slowing into the hold, sampled as a tablet samples), the finger that snaps a held shape,
//! and how long a big shape takes to follow a drag.

use std::f32::consts::TAU;

use procraft_engine::geom::Vec2;

use crate::tests::session;
use crate::{CanvasSession, PenSample, PointerKind, QuickShapeInfo, QuickShapeKind, TouchCommand};

/// A loop as a hand draws it, in window points.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hand {
    pub(crate) centre: (f32, f32),
    /// Its half-axes, and the turn of the first from across.
    pub(crate) rx: f32,
    pub(crate) ry: f32,
    pub(crate) tilt: f32,
    /// Where on it the pen lands (a turn about the centre), which way it goes (1 or -1), and how
    /// far round (1 is once round, more runs on past its start).
    pub(crate) start: f32,
    pub(crate) way: f32,
    pub(crate) turns: f32,
    /// How long the loop takes, ms.
    pub(crate) ms: f64,
    /// How lumpy it is: the radius wanders by this share.
    pub(crate) lump: f32,
    /// How far in from its start the end comes as it runs on (window points), as a loop that
    /// crosses inside its start.
    pub(crate) end_in: f32,
    /// The tick the pen lands with (window points), outwards before it sets off round.
    pub(crate) tick: f32,
    pub(crate) seed: u32,
}

impl Hand {
    pub(crate) fn small(turns: f32) -> Hand {
        Hand {
            centre: (300.0, 300.0),
            rx: 48.0,
            ry: 34.0,
            tilt: 0.35,
            start: -1.9,
            way: -1.0,
            turns,
            ms: 620.0,
            lump: 0.05,
            end_in: 0.0,
            tick: 0.0,
            seed: 7,
        }
    }

    /// The pen's samples, 240 a second (with the hand's tremor of a third of a point), then held
    /// still for the shape to snap (`hold_ms`, trembling on the spot).
    pub(crate) fn samples(&self, hold_ms: f64) -> Vec<PenSample> {
        let mut seed = self.seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut tremor = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.7
        };
        let (sin, cos) = self.tilt.sin_cos();
        let at = |a: f32, inward: f32| {
            let lumps = 1.0 + self.lump * (0.6 * (2.0 * a + 1.3).sin() + 0.4 * (3.0 * a + 0.4).sin());
            let (x, y) = ((self.rx * lumps - inward) * a.cos(), (self.ry * lumps - inward) * a.sin());
            (self.centre.0 + x * cos - y * sin, self.centre.1 + x * sin + y * cos)
        };
        let step = 1000.0 / 240.0;
        let n = (self.ms / step).ceil() as usize;
        let mut out = Vec::with_capacity(n + 200);
        // the tick: the pen lands off the loop and sweeps onto it
        let first = at(self.start, 0.0);
        let out_way = (first.0 - self.centre.0, first.1 - self.centre.1);
        let len = (out_way.0 * out_way.0 + out_way.1 * out_way.1).sqrt().max(1.0);
        let ticks = if self.tick > 0.0 { 6 } else { 0 };
        for k in 0..ticks {
            let f = 1.0 - k as f32 / ticks as f32;
            let off = self.tick * f;
            out.push((first.0 + out_way.0 / len * off, first.1 + out_way.1 / len * off));
        }
        for i in 0..=n {
            let u = (i as f64 / n as f64) as f32;
            // setting off, going on, slowing into the hold
            let p = 0.25 * u + 0.75 * u * u * (3.0 - 2.0 * u);
            let a = self.start + self.way * self.turns * TAU * p;
            out.push(at(a, self.end_in * p.powi(8)));
        }
        let mut samples: Vec<PenSample> = out
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| PenSample {
                x: x + tremor(),
                y: y + tremor(),
                pressure: 0.6,
                tilt: 0.0,
                orientation: 0.0,
                time_ms: i as f64 * step,
                roll: 0.0,
            })
            .collect();
        if let Some(&end) = samples.last() {
            let held = (hold_ms / step) as usize;
            for k in 1..=held {
                let (x, y) = (end.x + tremor() * 0.6, end.y + tremor() * 0.6);
                samples.push(PenSample { x, y, time_ms: end.time_ms + k as f64 * step, ..end });
            }
        }
        samples
    }
}

/// The widest gap, in turns, between the turns about `centre` of the points of `outline`, the
/// ellipse with half-axes `a` and `b` (from its centre) taken as round.
fn widest_gap(outline: &[Vec2], centre: Vec2, a: Vec2, b: Vec2) -> f32 {
    let det = a.x * b.y - a.y * b.x;
    if det.abs() < 1e-6 || outline.is_empty() {
        return 1.0;
    }
    let mut turns: Vec<f32> = outline
        .iter()
        .map(|p| {
            let d = *p - centre;
            let (u, v) = ((d.x * b.y - d.y * b.x) / det, (a.x * d.y - a.y * d.x) / det);
            v.atan2(u).rem_euclid(TAU) / TAU
        })
        .collect();
    turns.sort_by(f32::total_cmp);
    let wrap = turns.first().copied().unwrap_or(0.0) + 1.0 - turns.last().copied().unwrap_or(0.0);
    turns.windows(2).map(|w| w[1] - w[0]).fold(wrap, f32::max)
}

/// What a held stroke of `samples` snapped to, and the widest gap (in turns) its outline leaves.
fn snap(s: &CanvasSession, samples: &[PenSample], kind: PointerKind) -> (Option<QuickShapeKind>, f32) {
    s.stroke_begin(kind, samples[..1].to_vec());
    s.stroke_add(samples[1..].to_vec());
    let shape = s.quickshape_hold().map(|i| i.kind);
    let gap = {
        let st = s.shared.lock();
        st.shape.as_ref().map_or(1.0, |edit| {
            let (_, nodes) = edit.geometry();
            let outline: Vec<Vec2> = edit.samples().iter().map(|s| s.pos).collect();
            match nodes.as_slice() {
                [c, a, b, ..] if matches!(shape, Some(QuickShapeKind::Ellipse | QuickShapeKind::Circle)) => {
                    widest_gap(&outline, *c, *a - *c, *b - *c)
                }
                _ => 1.0,
            }
        })
    };
    s.stroke_end(vec![]);
    s.quickshape_done();
    s.undo();
    (shape, gap)
}

/// An ellipse's or circle's centre and mean radius (window points), from its handles (the ends of
/// its axes).
fn round_of(info: &QuickShapeInfo) -> (Vec2, f32) {
    let h = |i: usize| Vec2::new(info.handles[2 * i], info.handles[2 * i + 1]);
    let c = (h(0) + h(2)) * 0.5;
    (c, (h(0).distance(c) + h(1).distance(c)) * 0.5)
}

fn touch_at(x: f32, y: f32, time_ms: f64) -> Vec<PenSample> {
    vec![PenSample { x, y, pressure: 0.6, tilt: 0.0, orientation: 0.0, time_ms, roll: 0.0 }]
}

/// A loop drawn by `kind` (pointer 1) and held until it snaps, through the touches as the app
/// hands them over. Its last sample.
fn draw_and_hold(s: &CanvasSession, kind: PointerKind, hand: &Hand) -> PenSample {
    let samples = hand.samples(700.0);
    s.touch_down(1, kind, samples[..1].to_vec());
    for chunk in samples[1..].chunks(4) {
        s.touch_move(1, chunk.to_vec());
    }
    let last = *samples.last().unwrap();
    // (the hand's tremor restarts the wait, so it runs out after the last sample)
    let out = s.touch_tick(last.time_ms + 700.0);
    assert!(out.contains(&TouchCommand::ShapeSnapped), "{kind:?}: no snap ({out:?})");
    last
}

/// #411: a held ellipse snapped to a circle by a second finger, sized by the pen still holding it
/// and dropped, keeps the size it was dragged to, wherever the snapping finger was: the pen's lift
/// lets the shape go as it shows, and the finger lifting after it does nothing more. (The finger's
/// lift ended the stroke, and the shape grew or shrank to where the finger was, here more than
/// four times as big.) A finger snaps beside a pen as beside a finger: it was taken for a hand.
#[test]
fn a_circle_snapped_by_a_finger_keeps_the_size_it_was_dragged_to_when_dropped() {
    for kind in [PointerKind::Finger, PointerKind::Stylus] {
        let Some(s) = session(1024, 1024) else { return };
        s.set_display_scale(2.0);
        s.set_brush_size(0.1);
        let hand = Hand::small(1.04);
        let last = draw_and_hold(&s, kind, &hand);
        let snapped = s.quickshape_info().expect("a shape");
        assert_eq!(snapped.kind, QuickShapeKind::Ellipse, "{kind:?}");
        // a finger down far off, in a corner: a circle
        let t = last.time_ms;
        let out = s.touch_down(2, PointerKind::Finger, touch_at(60.0, 980.0, t + 20.0));
        assert!(out.contains(&TouchCommand::ShapeChanged), "{kind:?}: {out:?}");
        let circle = s.quickshape_info().unwrap();
        assert_eq!(circle.kind, QuickShapeKind::Circle, "{kind:?}");
        let (centre, r0) = round_of(&circle);
        // the pen drags it out half as big again, straight away from its middle
        let from = Vec2::new(last.x, last.y);
        let away = (from - centre).normalized_or(Vec2::new(1.0, 0.0));
        let reach = from.distance(centre);
        for k in 1..=10 {
            let at = from + away * (reach * 0.5 * k as f32 / 10.0);
            s.touch_move(1, touch_at(at.x, at.y, t + 40.0 + f64::from(k) * 8.0));
        }
        let (c_drag, r_drag) = round_of(&s.quickshape_info().unwrap());
        assert!((r_drag / r0 - 1.5).abs() < 0.02, "{kind:?}: dragged from {r0} to {r_drag}");
        assert!(c_drag.distance(centre) < 0.5);
        // the pen lifts where it is; the finger then wanders off and lifts
        let end = from + away * (reach * 0.5);
        let out = s.touch_up(1, touch_at(end.x, end.y, t + 140.0));
        assert!(out.contains(&TouchCommand::StrokeEnded), "{kind:?}: {out:?}");
        s.touch_move(2, touch_at(40.0, 1000.0, t + 160.0));
        s.touch_up(2, touch_at(30.0, 1010.0, t + 180.0));
        let dropped = s.quickshape_info().expect("still open to edit");
        let (c_end, r_end) = round_of(&dropped);
        assert_eq!(dropped.kind, QuickShapeKind::Circle, "{kind:?}");
        assert!((r_end - r_drag).abs() < 0.5, "{kind:?}: dragged to {r_drag}, dropped at {r_end}");
        assert!(c_end.distance(c_drag) < 0.5, "{kind:?}: moved from {c_drag:?} to {c_end:?}");
        println!("{kind:?}: snapped r {r0:.1}, dragged to {r_drag:.1}, dropped at {r_end:.1}");
        s.quickshape_done();
    }
}

/// The finger that snapped a held shape lifting first shows it unsnapped again, as the pen holds
/// it; the pen's lift then keeps that.
#[test]
fn a_snapping_finger_lifted_first_leaves_the_shape_to_the_pen() {
    let Some(s) = session(1024, 1024) else { return };
    s.set_display_scale(2.0);
    s.set_brush_size(0.1);
    let last = draw_and_hold(&s, PointerKind::Finger, &Hand::small(1.04));
    let t = last.time_ms;
    s.touch_down(2, PointerKind::Finger, touch_at(60.0, 980.0, t + 20.0));
    assert_eq!(s.quickshape_info().unwrap().kind, QuickShapeKind::Circle);
    let out = s.touch_up(2, touch_at(60.0, 980.0, t + 60.0));
    assert!(out.contains(&TouchCommand::ShapeChanged), "{out:?}");
    let ellipse = s.quickshape_info().unwrap();
    assert_eq!(ellipse.kind, QuickShapeKind::Ellipse);
    s.touch_up(1, touch_at(last.x, last.y, t + 100.0));
    assert_eq!(s.quickshape_info().unwrap().handles, ellipse.handles);
    s.quickshape_done();
}

/// #411: small loops drawn by hand (lumpy, landing with a tick or not, slowing into the hold)
/// snap to ellipses: one run on past its start, crossing inside it or not, closes with no piece
/// missing; one stopped short half way round or more is the ellipse left open by the gap the
/// stroke left, as the reference has it (it goes by how far round the ellipse a stroke goes); one
/// that goes less far is an arc of it.
#[test]
fn small_hand_drawn_loops_snap_to_ellipses_closed_or_open_as_drawn() {
    let Some(s) = session(1024, 1024) else { return };
    s.set_display_scale(2.0);
    s.set_brush_size(0.1);
    let mut wrong = Vec::new();
    let mut loops = 0;
    for (rx, ry) in [(30.0, 22.0), (48.0, 34.0), (80.0, 58.0)] {
        for (turns, tick, end_in) in [
            (1.03, 0.0, 0.0),
            (1.08, 4.0, 0.0),
            (1.12, 0.0, 6.0),
            (1.18, 5.0, 4.0),
            (0.62, 0.0, 0.0),
            (0.75, 4.0, 0.0),
            (0.85, 0.0, 0.0),
            (0.92, 5.0, 0.0),
            (0.4, 0.0, 0.0),
        ] {
            for seed in [3, 11] {
                loops += 1;
                let hand = Hand { rx, ry, turns, tick, end_in, seed, ..Hand::small(turns) };
                let (kind, gap) = snap(&s, &hand.samples(700.0), PointerKind::Finger);
                let what =
                    format!("{rx}x{ry} {turns} round, tick {tick}, {end_in} in, seed {seed}: {kind:?}, gap {gap:.3}");
                let right = if turns < 0.5 {
                    kind == Some(QuickShapeKind::Arc)
                } else if turns >= 1.0 {
                    kind == Some(QuickShapeKind::Ellipse) && gap < 0.02
                } else {
                    kind == Some(QuickShapeKind::Ellipse) && (gap - (1.0 - turns)).abs() < 0.05
                };
                if !right {
                    wrong.push(what);
                }
            }
        }
    }
    println!("{} of {loops} hand-drawn loops snapped wrong", wrong.len());
    assert!(wrong.is_empty(), "{} of {loops} snapped wrong: {wrong:#?}", wrong.len());
}

#[test]
#[ignore]
fn survey_hand_drawn_small_ellipses() {
    let Some(s) = session(1024, 1024) else { return };
    s.set_display_scale(2.0);
    s.set_brush_size(0.1);
    for (rx, ry) in [(30.0, 22.0), (48.0, 34.0), (80.0, 58.0), (140.0, 100.0)] {
        for turns in [0.62, 0.75, 0.85, 0.92, 0.97, 1.0, 1.04, 1.1, 1.18] {
            for (lump, tick, end_in) in [(0.03, 0.0, 0.0), (0.06, 4.0, 0.0), (0.05, 0.0, 6.0), (0.08, 5.0, 4.0)] {
                let hand = Hand { rx, ry, turns, lump, tick, end_in, ..Hand::small(turns) };
                let (kind, gap) = snap(&s, &hand.samples(700.0), PointerKind::Finger);
                let want = (1.0 - turns).max(0.0);
                println!(
                    "r {rx:>3}x{ry:<3} turns {turns:.2} lump {lump:.2} tick {tick} in {end_in}: {kind:?}, gap {gap:.3} (drawn {want:.3})"
                );
            }
        }
    }
}

/// The brushes a big shape's drag is timed with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DragBrush {
    /// A plain round pen, packed tight.
    RoundPen,
    /// The stock pastel: a grain, big soft stamps.
    SoftPastel,
    /// Wet paint laid in place, with a second.
    WetDual,
}

/// What one drag of a big shape measured: per frame, how long it took (the move, the frame drawn
/// and the GPU waited for, ms) and how many moves the stroke laid trailed behind the shape; the
/// frames after the drag until the stroke caught up; the stamps laid; and the line's width across
/// the outline (canvas pixels) as the shape first snapped, once caught up after the drag, and kept.
struct DragTimes {
    frames: Vec<f64>,
    behind: Vec<u64>,
    catch_up: usize,
    stamps: usize,
    width_snapped: usize,
    width_dragged: usize,
    width_kept: usize,
}

fn median(v: &[f64]) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn worst(v: &[f64]) -> f64 {
    v.iter().copied().fold(0.0, f64::max)
}

/// The width (pixels) of the dark run crossing row `y` of `shown` (`side` square) nearest `x`.
fn width_at(shown: &[u8], side: u32, x: i32, y: i32) -> usize {
    let dark = |x: i32| (0..side as i32).contains(&x) && shown[((y as u32 * side + x as u32) * 4) as usize] < 128;
    let Some(near) = (0..side as i32).filter(|&i| dark(i)).min_by_key(|&i| (i - x).abs()) else { return 0 };
    let (mut a, mut b) = (near, near);
    while dark(a - 1) {
        a -= 1;
    }
    while dark(b + 1) {
        b += 1;
    }
    (b - a + 1) as usize
}

/// How many moves the stroke laid trails behind the shape.
fn behind(st: &crate::State) -> u64 {
    st.shape.as_ref().map_or(0, |edit| edit.moves_behind())
}

/// A circle of 50 window points (200 canvas pixels, the canvas 4096 square in a 1024 window)
/// drawn with `brush`, held until it snaps, let go, and its outline dragged out to 448 window points
/// (1792 canvas pixels) 16 at a time, a move and a frame (drawn and waited for) each display frame.
fn drag_a_big_circle(brush: DragBrush) -> Option<DragTimes> {
    use procraft_engine::brush::{Brush, Taper};
    use procraft_engine::profile::Profile;
    use procraft_gpu::wgpu;
    use std::time::Instant;
    const SIDE: u32 = 1024;
    const CANVAS: u32 = 4096;
    let s = session(CANVAS, SIDE)?;
    let gpu = s.shared.lock().gpu.clone()?;
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("quickshape drag"),
        size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let started = Instant::now();
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    {
        let mut st = s.shared.lock();
        let tool = st.tool;
        let pastel = st
            .library
            .iter()
            .flat_map(|set| set.brushes.iter())
            .find(|b| b.grain.image.is_some() && b.secondary.is_none() && !b.mixes_wet() && b.name.contains("Pastel"))
            .cloned();
        let chosen = st.brush_mut(tool)?;
        *chosen = match (brush, pastel) {
            (DragBrush::SoftPastel, Some(b)) => Brush { pen_taper: Taper::NONE, ..b },
            (DragBrush::SoftPastel, None) => return None,
            _ => Brush { spacing: 0.03, pen_taper: Taper::NONE, ..Brush::round_pen() },
        };
        if brush == DragBrush::WetDual {
            chosen.set_setting("wet_dilution", 0.6);
            chosen.set_setting("wet_charge", 0.5);
            chosen.set_setting("wet_pull", 0.5);
            chosen.secondary = Some(Box::new(Brush::round_pen()));
            chosen.set_setting("dual_blend", 1.0);
        }
    }
    s.set_color(0.0, 0.0, 0.0);
    s.set_brush_size(0.2);
    let frame = |st: &mut crate::State| {
        st.draw_frame(&view, wgpu::TextureFormat::Rgba8Unorm, (SIDE, SIDE), Profile::Srgb, started);
    };
    // the line's width across the outline's left, where it crosses the middle row (canvas pixels)
    let width = |radius: f32| -> Option<usize> {
        let mut guard = s.shared.lock();
        let st = &mut *guard;
        let (canvas, gpu) = (st.canvas.as_mut()?, st.gpu.as_ref()?);
        canvas.render(gpu, &mut st.doc, None);
        let shown = canvas.read_composite(gpu).ok()?;
        Some(width_at(&shown, CANVAS, (2048.0 - radius * 4.0) as i32, 2048))
    };
    let (cx, cy, r0) = (512.0f32, 512.0f32, 50.0f32);
    let pen: Vec<PenSample> = (0..=100)
        .map(|i| {
            let a = i as f32 / 100.0 * TAU;
            PenSample {
                x: cx + r0 * a.cos(),
                y: cy + r0 * a.sin(),
                pressure: 1.0,
                tilt: 0.0,
                orientation: 0.0,
                time_ms: f64::from(i) * 8.0,
                roll: 0.0,
            }
        })
        .collect();
    s.stroke_begin(PointerKind::Stylus, pen[..1].to_vec());
    s.stroke_add(pen[1..].to_vec());
    s.quickshape_hold()?;
    s.stroke_end(vec![]);
    frame(&mut s.shared.lock());
    gpu.wait().ok()?;
    let width_snapped = width(r0)?;
    let mut times = DragTimes {
        frames: Vec::new(),
        behind: Vec::new(),
        catch_up: 0,
        stamps: 0,
        width_snapped,
        width_dragged: 0,
        width_kept: 0,
    };
    // a move and a frame each display frame (60 a second), as a screen draws them
    let display = std::time::Duration::from_micros(16_667);
    let vsync = |since: Instant| std::thread::sleep(display.saturating_sub(since.elapsed()));
    s.quickshape_pin(cx + r0, cy, true)?;
    let mut x = cx + r0;
    while x < cx + 448.0 {
        x += 16.0;
        let t = Instant::now();
        s.quickshape_pin(x, cy, false)?;
        let mut st = s.shared.lock();
        frame(&mut st);
        gpu.wait().ok()?;
        times.frames.push(ms(t));
        times.behind.push(behind(&st));
        drop(st);
        vsync(t);
    }
    // let go: frames go on until the stroke laid is the shape's
    s.quickshape_unpin();
    while behind(&s.shared.lock()) > 0 && times.catch_up < 600 {
        let t = Instant::now();
        frame(&mut s.shared.lock());
        gpu.wait().ok()?;
        times.catch_up += 1;
        vsync(t);
    }
    times.stamps = s.shared.lock().stroke.as_ref().map_or(0, |b| b.stamps().len());
    let radius = x - cx;
    times.width_dragged = width(radius)?;
    s.quickshape_done();
    times.width_kept = width(radius)?;
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    Some(times)
}

/// How long a big shape takes to follow a drag (see [`drag_a_big_circle`]), by brush: a frame of
/// the drag at most as long as the bound says, the stroke caught up within a few frames of the
/// drag's end, and the line as wide dragged out as it was snapped and as it is kept (the stroke is
/// laid again along the shape, never stretched).
/// `cargo test -p procraft-core --release quickshape_drag_frames -- --ignored --nocapture --test-threads 1`
#[test]
#[ignore]
fn quickshape_drag_frames() {
    let mut wrong = Vec::new();
    for (brush, bound) in [(DragBrush::RoundPen, 12.0), (DragBrush::SoftPastel, 12.0), (DragBrush::WetDual, 60.0)] {
        let Some(t) = drag_a_big_circle(brush) else { return };
        let most_behind = t.behind.iter().copied().max().unwrap_or(0);
        println!(
            "{brush:?}: {} stamps; a drag frame {:.2} ms median, {:.2} worst ({} frames); the stroke trailing by {most_behind} moves at most, caught up {} frames after; width snapped {} px, dragged {} px, kept {} px",
            t.stamps,
            median(&t.frames),
            worst(&t.frames),
            t.frames.len(),
            t.catch_up,
            t.width_snapped,
            t.width_dragged,
            t.width_kept,
        );
        if median(&t.frames) >= bound {
            wrong.push(format!("{brush:?}: frames of {:.2} ms", median(&t.frames)));
        }
        if t.catch_up >= 120 {
            wrong.push(format!("{brush:?}: caught up {} frames after the drag", t.catch_up));
        }
        // (a soft grained line's width, read across it, goes with the grain where it is read)
        if t.width_snapped == 0 || t.width_dragged as f32 > t.width_snapped as f32 * 1.25 + 1.0 {
            wrong.push(format!("{brush:?}: stretched"));
        }
        if t.width_kept.abs_diff(t.width_dragged) > 1 {
            wrong.push(format!("{brush:?}: shown otherwise than kept"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// How the canvas lays a heavy shape's strokes in [`heavy_shape`]: at its own pace, all of it in
/// each frame, or as a slow GPU's pace allows (two million work a millisecond, set).
#[derive(Clone, Copy, Debug, PartialEq)]
enum ShapePace {
    Own,
    AtOnce,
    SlowGpu,
}

/// What [`heavy_shape`] measured (ms): the longest any frame held the canvas, and the longest a
/// frame's GPU work took after it (waited for); each frame's hold; the drag's frames, in how many
/// the picture changed and how many showed the shape by its preview; the frames and the time from
/// the lift until the shape's stroke was all laid, and from keeping it until it was in its layer;
/// the most GPU time a frame gave the stamps by the rates it went by (ms); the stamps along the
/// shape; and the layer as kept.
struct HeavyShape {
    longest_hold: f64,
    longest_gpu: f64,
    holds: Vec<f64>,
    drag_frames: usize,
    drag_shown: usize,
    drag_previewed: usize,
    spent_most: f64,
    drag_moves_behind: u64,
    settle_frames: usize,
    settle_ms: f64,
    keep_frames: usize,
    keep_ms: f64,
    stamps: usize,
    layer: Vec<u8>,
}

/// QuickShape with a heavy brush (`brush`, from the library, at full size) on a 1600 x 2560 canvas
/// in a window of that size, by finger as a tablet's test drives it: a circle of 250 pixels drawn
/// in a second, held until it snaps, dragged out to 650 pixels over 1.3 s (a move every 8 ms, as
/// many as came while the frame before was drawn going in at once), lifted (a time-lapse frame
/// asked for, as the app asks), frames drawn until its stroke is laid, then kept and frames drawn
/// until it is in its layer. Each frame's GPU work is waited for after it (outside the canvas's
/// lock).
fn heavy_shape(brush: &str, pace: ShapePace) -> Option<HeavyShape> {
    use procraft_engine::profile::Profile;
    use procraft_engine::view::View;
    use procraft_gpu::{Gpu, wgpu};
    use std::time::{Duration, Instant};
    let (width, height) = (1600u32, 2560u32);
    let gpu = Gpu::headless().ok()?;
    let s = CanvasSession::new(width, height);
    {
        let mut st = s.shared.lock();
        st.window_size = (width, height);
        st.view = Some(View::fit((width, height), (width, height)));
        st.gpu = Some(gpu.clone());
        st.ensure_canvas();
        let canvas = st.canvas.as_mut()?;
        match pace {
            ShapePace::Own => {}
            ShapePace::AtOnce => {
                canvas.set_dry_paced(false, None);
                canvas.set_in_place_paced(false);
                canvas.set_relay_at_once(Duration::from_secs(3600));
                canvas.set_record_most(Duration::from_secs(3600));
            }
            ShapePace::SlowGpu => {
                canvas.set_dry_paced(true, Some(2e6));
                canvas.set_in_place_rate(Some(2e6));
            }
        }
    }
    let at = {
        let st = s.shared.lock();
        st.library.iter().enumerate().find_map(|(set, entry)| {
            let at = entry.brushes.iter().position(|b| b.name == brush)?;
            Some((set as u32, at as u32))
        })
    };
    let (set, index) = at?;
    assert!(s.select_brush(set, index));
    crate::big_pictures::wait_for_big_pictures(Duration::from_secs(60));
    s.set_brush_size(1.0);
    s.set_color(0.1, 0.2, 0.5);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("window"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let started = Instant::now();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let (mut longest_hold, mut longest_gpu, mut holds) = (0.0f64, 0.0f64, Vec::new());
    let (changed, previewed) = (std::cell::Cell::new(false), std::cell::Cell::new(false));
    // a frame drawn (timed under the lock) and its GPU work waited for; whether more are wanted
    let mut frame = |s: &CanvasSession| -> bool {
        let more = {
            let mut st = s.shared.lock();
            let queued = st.canvas.as_ref().map(|c| c.queued_work()).unwrap_or_default();
            let before = st.canvas.as_ref().map_or(0, |c| c.composites());
            let t = Instant::now();
            st.draw_frame(&view, format, (width, height), Profile::Srgb, started);
            let held = ms(t.elapsed());
            changed.set(st.canvas.as_ref().map_or(0, |c| c.composites()) != before);
            previewed.set(st.shape.as_ref().is_some_and(|e| e.previewing()));
            if held > 200.0 && std::env::var("HEAVY_SHAPE_SAY").is_ok() {
                println!("  frame {} held {held:.0} ms; queued before it: {queued}", holds.len());
            }
            longest_hold = longest_hold.max(held);
            holds.push(held);
            st.needs_frame
                || st.shape.as_ref().is_some_and(|e| e.moves_behind() > 0)
                || st.canvas.as_ref().is_some_and(|c| c.lays_over_frames() || c.catching_up() || c.relaying())
        };
        let t = Instant::now();
        gpu.flush();
        longest_gpu = longest_gpu.max(ms(t.elapsed()));
        more
    };
    let (cx, cy) = (800.0f32, 1280.0f32);
    let pen =
        |x: f32, y: f32, t: f64| PenSample { x, y, pressure: 1.0, tilt: 0.0, orientation: 0.0, time_ms: t, roll: 0.0 };
    // the circle, a sample every 8 ms over a second, then held still
    let round: Vec<PenSample> = (0..=125)
        .map(|i| {
            let a = i as f32 / 125.0 * TAU - TAU / 4.0;
            pen(cx + 250.0 * a.cos(), cy + 250.0 * a.sin(), f64::from(i) * 8.0)
        })
        .collect();
    s.stroke_begin(PointerKind::Finger, round[..1].to_vec());
    for chunk in round[1..].chunks(2) {
        s.stroke_add(chunk.to_vec());
        frame(&s);
    }
    let last = round[round.len() - 1];
    s.stroke_add((1..60).map(|i| pen(last.x, last.y, last.time_ms + f64::from(i) * 8.0)).collect());
    s.quickshape_hold()?;
    frame(&s);
    // dragged from its top out to 650 pixels from the centre, a move every 8 ms in real time
    let drag: Vec<PenSample> =
        (0..=162).map(|i| pen(cx, cy - 250.0 - 400.0 * i as f32 / 162.0, 2000.0 + f64::from(i) * 8.0)).collect();
    let drag_began = Instant::now();
    let mut sent = 0;
    let mut drag_frames = 0;
    let (mut drag_shown, mut drag_previewed) = (0, 0);
    let mut drag_moves_behind = 0;
    while sent < drag.len() {
        let due = ((ms(drag_began.elapsed()) / 8.0) as usize + 1).min(drag.len());
        if due > sent {
            s.stroke_add(drag[sent..due].to_vec());
            sent = due;
        }
        frame(&s);
        drag_frames += 1;
        drag_shown += usize::from(changed.get());
        drag_previewed += usize::from(previewed.get());
        drag_moves_behind = drag_moves_behind.max(s.shared.lock().shape.as_ref().map_or(0, |e| e.moves_behind()));
        if drag_began.elapsed() > Duration::from_secs(300) {
            break;
        }
    }
    let dir = std::env::temp_dir().join(format!("procraft-heavy-shape-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lapse = crate::TimeLapse::new(dir.to_string_lossy().into_owned());
    s.stroke_end(vec![]);
    // (the app asks for a time-lapse frame as the pen lifts)
    s.record_time_lapse(std::sync::Arc::clone(&lapse));
    if std::env::var("HEAVY_SHAPE_SAY").is_ok() {
        let st = s.shared.lock();
        println!(
            "  lifted after {drag_frames} drag frames: {}",
            st.canvas.as_ref().map(|c| c.queued_work()).unwrap_or_default()
        );
    }
    let lifted = Instant::now();
    let mut settle_frames = 0;
    while frame(&s) && settle_frames < 5000 && lifted.elapsed() < Duration::from_secs(300) {
        settle_frames += 1;
    }
    let settle_ms = ms(lifted.elapsed());
    let stamps = s.shared.lock().stroke.as_ref().map_or(0, |b| b.stamps().len());
    let kept = Instant::now();
    s.quickshape_done();
    let mut keep_frames = 0;
    while frame(&s) && keep_frames < 5000 && kept.elapsed() < Duration::from_secs(300) {
        keep_frames += 1;
    }
    let keep_ms = ms(kept.elapsed());
    s.keep_time_lapse();
    let _ = std::fs::remove_dir_all(&dir);
    let spent_most = s.shared.lock().canvas.as_ref().map_or(0.0, |c| c.paced_spent().0);
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    let current = s.layers().iter().find(|l| l.current).map_or(0, |l| l.id);
    let layer = s.shared.lock().layer_pixels(procraft_engine::document::LayerId(current))?;
    Some(HeavyShape {
        longest_hold,
        longest_gpu,
        holds,
        drag_frames,
        drag_shown,
        drag_previewed,
        spent_most,
        drag_moves_behind,
        settle_frames,
        settle_ms,
        keep_frames,
        keep_ms,
        stamps,
        layer,
    })
}

/// A heavy brush's shape dragged big (see [`heavy_shape`]), Quicksilver at full size unless
/// `HEAVY_SHAPE_BRUSH` names another: what each frame holds the canvas for, how the picture follows
/// the drag, how long the stroke takes to be laid and kept, and whether it is kept as laid at once.
/// Bounds: no frame holds the canvas a second; the shape is laid within 30 s of the lift and kept
/// within 10 s; the picture changes in half the drag's frames; a slow GPU's frames give the stamps
/// at most 200 ms (150 ms and the least lot of each part); kept as laid at once, bit for bit.
/// (Before: one frame held it 2.1 s, the picture never changed through the drag, and the stroke
/// was laid 97 s after the lift, 28 moves of it queued and laid in turn.)
/// `cargo test -p procraft-core --release heavy_shape_frames -- --ignored --nocapture --test-threads 1`
#[test]
#[ignore]
fn heavy_shape_frames() {
    let brush = std::env::var("HEAVY_SHAPE_BRUSH").unwrap_or_else(|_| "Quicksilver".to_owned());
    let paces = std::env::var("HEAVY_SHAPE_PACES").unwrap_or_else(|_| "own,once,slow".to_owned());
    let mut kept: Vec<(ShapePace, Vec<u8>)> = Vec::new();
    let mut wrong = Vec::new();
    for pace in [ShapePace::Own, ShapePace::AtOnce, ShapePace::SlowGpu] {
        let name = match pace {
            ShapePace::Own => "own",
            ShapePace::AtOnce => "once",
            ShapePace::SlowGpu => "slow",
        };
        if !paces.split(',').any(|p| p == name) {
            continue;
        }
        let Some(h) = heavy_shape(&brush, pace) else { return };
        let mut sorted = h.holds.clone();
        sorted.sort_by(f64::total_cmp);
        let top: Vec<String> = sorted.iter().rev().take(6).map(|v| format!("{v:.0}")).collect();
        println!(
            "{brush} {pace:?}: {} stamps; longest hold {:.0} ms (top {}), longest GPU wait {:.0} ms; drag {} frames, \
             the picture changed in {}, {} by the preview, {} moves behind at most; settled {} frames {:.0} ms after the \
             lift; kept {} frames {:.0} ms; {} frames; a frame gave the stamps {:.0} ms of the GPU at most by its rates",
            h.stamps,
            h.longest_hold,
            top.join(" "),
            h.longest_gpu,
            h.drag_frames,
            h.drag_shown,
            h.drag_previewed,
            h.drag_moves_behind,
            h.settle_frames,
            h.settle_ms,
            h.keep_frames,
            h.keep_ms,
            h.holds.len(),
            h.spent_most
        );
        if h.longest_hold >= 1000.0 {
            wrong.push(format!("{pace:?}: a frame held the canvas {:.0} ms", h.longest_hold));
        }
        if h.settle_ms >= 30_000.0 || h.keep_ms >= 10_000.0 {
            wrong.push(format!("{pace:?}: laid {:.0} ms after the lift, kept {:.0} ms after", h.settle_ms, h.keep_ms));
        }
        if pace != ShapePace::AtOnce && h.drag_shown * 2 < h.drag_frames {
            wrong.push(format!("{pace:?}: the picture changed in {} of {} drag frames", h.drag_shown, h.drag_frames));
        }
        if pace == ShapePace::SlowGpu && h.spent_most > 200.0 {
            wrong.push(format!("{pace:?}: a frame gave the stamps {:.0} ms of the GPU", h.spent_most));
        }
        kept.push((pace, h.layer));
    }
    if let Some((_, at_once)) = kept.iter().find(|(p, _)| *p == ShapePace::AtOnce) {
        for (pace, layer) in kept.iter().filter(|(p, _)| *p != ShapePace::AtOnce) {
            let differ = layer.iter().zip(at_once).filter(|(a, b)| a != b).count();
            let most = layer.iter().zip(at_once).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            println!("{brush} {pace:?}: {differ} channels kept otherwise than laid at once, by {most} levels at most");
            if differ > 0 {
                wrong.push(format!("{pace:?}: {differ} channels kept otherwise than laid at once"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// What one drag of a heavy shape did (see [`drag_heavy_shape`]): the drag's frames, how many of
/// them showed the shape by its preview, how many of its moves were shown by the frame that laid
/// their stroke, and the most frames a move took to be laid; the frames after the lift until its stroke was laid as itself, and
/// after it was kept until it was in its layer; the most GPU time a frame was taken to give the
/// stroke's stamps (ms, by the rates set), and the most stamps queued after a frame (first, second);
/// the stamps renders that showed nothing laid from the lift on, and the time-lapse frames taken;
/// whether it was still shown by its preview once laid; the layer as kept.
struct HeavyDrag {
    drag_frames: usize,
    previewed: usize,
    moves_shown: usize,
    most_frames_a_move: usize,
    settle_frames: usize,
    keep_frames: usize,
    spent_most: f64,
    queued_most: (usize, usize),
    laid_unshown: u64,
    lapse_frames: u32,
    preview_left: bool,
    layer: Vec<u8>,
}

/// A shape of a heavy brush (wet paint laid in place, a second, big and packed tight) dragged out
/// by the pen that drew it on a 1024 square canvas in a 512 window: a circle of 60 points drawn,
/// held until it snaps, and dragged out to 140 points in 12 moves, frames drawn (their GPU work
/// waited for) after each until the stroke laid is made for it; lifted, and frames drawn until its
/// stroke is laid (a time-lapse frame asked for as it lifts, as the app asks); kept, and frames
/// drawn until it is in its layer. `slow`: the canvas lays its stamps as a slow GPU's pace allows (two million
/// work a millisecond, each frame its share of 150 ms), else all of them in each frame.
fn drag_heavy_shape(slow: bool) -> Option<HeavyDrag> {
    use procraft_engine::brush::{Brush, Taper};
    use procraft_engine::profile::Profile;
    use procraft_gpu::wgpu;
    use std::time::{Duration, Instant};
    const SIDE: u32 = 512;
    let s = session(1024, SIDE)?;
    let gpu = s.shared.lock().gpu.clone()?;
    {
        let mut st = s.shared.lock();
        let canvas = st.canvas.as_mut()?;
        // (recorded for as long as the pace allows, however long a debug build takes)
        canvas.set_record_most(Duration::from_secs(3600));
        if slow {
            canvas.set_in_place_rate(Some(2e6));
            canvas.set_dry_paced(true, Some(2e6));
        } else {
            canvas.set_in_place_paced(false);
            canvas.set_dry_paced(false, None);
            canvas.set_relay_at_once(Duration::from_secs(3600));
        }
        let tool = st.tool;
        let chosen = st.brush_mut(tool)?;
        *chosen = Brush { spacing: 0.03, pen_taper: Taper::NONE, ..Brush::round_pen() };
        chosen.set_setting("wet_dilution", 0.6);
        chosen.set_setting("wet_charge", 0.5);
        chosen.set_setting("wet_pull", 0.5);
        chosen.secondary = Some(Box::new(Brush::round_pen()));
        chosen.set_setting("dual_blend", 1.0);
    }
    s.set_color(0.1, 0.2, 0.6);
    s.set_brush_size(0.25);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("heavy shape"),
        size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let started = Instant::now();
    let mut queued_most = (0, 0);
    // a frame drawn and its GPU work waited for: whether it showed the preview, whether the picture
    // changed, how many moves the stroke trails, and whether more frames are wanted
    let mut frame = |s: &CanvasSession| -> Option<(bool, bool, u64, bool)> {
        let mut st = s.shared.lock();
        let before = st.canvas.as_ref()?.composites();
        st.draw_frame(&view, wgpu::TextureFormat::Rgba8Unorm, (SIDE, SIDE), Profile::Srgb, started);
        let canvas = st.canvas.as_ref()?;
        let queued = canvas.queued_stamps();
        queued_most = (queued_most.0.max(queued.0), queued_most.1.max(queued.1));
        let changed = canvas.composites() != before;
        let laying = canvas.lays_over_frames() || canvas.catching_up() || canvas.relaying();
        let edit = st.shape.as_ref();
        let (preview, behind) = (edit.is_some_and(|e| e.previewing()), edit.map_or(0, |e| e.moves_behind()));
        let more = st.needs_frame || behind > 0 || laying;
        drop(st);
        gpu.wait().ok()?;
        Some((preview, changed, behind, more))
    };
    let (cx, cy, r0) = (256.0f32, 256.0f32, 60.0f32);
    let pen =
        |x: f32, y: f32, t: f64| PenSample { x, y, pressure: 1.0, tilt: 0.0, orientation: 0.0, time_ms: t, roll: 0.0 };
    let round: Vec<PenSample> = (0..=100)
        .map(|i| {
            let a = i as f32 / 100.0 * TAU;
            pen(cx + r0 * a.cos(), cy + r0 * a.sin(), f64::from(i) * 8.0)
        })
        .collect();
    s.stroke_begin(PointerKind::Stylus, round[..1].to_vec());
    s.stroke_add(round[1..].to_vec());
    s.quickshape_hold()?;
    frame(&s)?;
    let mut drag = HeavyDrag {
        drag_frames: 0,
        previewed: 0,
        moves_shown: 0,
        most_frames_a_move: 0,
        settle_frames: 0,
        keep_frames: 0,
        spent_most: 0.0,
        queued_most: (0, 0),
        laid_unshown: 0,
        lapse_frames: 0,
        preview_left: false,
        layer: Vec::new(),
    };
    for k in 1..=12 {
        let x = cx + r0 + 80.0 * k as f32 / 12.0;
        s.stroke_add(vec![pen(x, cy, 1000.0 + f64::from(k) * 16.0)]);
        // (frames until the stroke made for it is laid: whether that frame shows it)
        let mut frames = 0;
        let shown = loop {
            let (preview, changed, behind, _) = frame(&s)?;
            frames += 1;
            drag.previewed += usize::from(preview);
            if behind == 0 || frames >= 500 {
                break changed;
            }
        };
        drag.drag_frames += frames;
        drag.most_frames_a_move = drag.most_frames_a_move.max(frames);
        drag.moves_shown += usize::from(shown);
        if std::env::var("HEAVY_DRAG_SAY").is_ok() {
            let st = s.shared.lock();
            let c = st.canvas.as_ref()?;
            let p = st.shape.as_ref().is_some_and(|e| e.previewing());
            println!(
                "move {k}: {frames} frames, shown {shown}, preview {p}, relaying {} catching up {}; {}",
                c.relaying(),
                c.catching_up(),
                c.queued_work()
            );
        }
    }
    let dir = std::env::temp_dir().join(format!("procraft-heavy-shape-lapse-{}-{slow}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lapse = crate::TimeLapse::new(dir.to_string_lossy().into_owned());
    let unshown = s.shared.lock().canvas.as_ref()?.laid_unshown();
    s.stroke_end(vec![]);
    s.record_time_lapse(std::sync::Arc::clone(&lapse));
    while drag.settle_frames < 3000 && frame(&s)?.3 {
        drag.settle_frames += 1;
    }
    drag.preview_left = s.shared.lock().shape.as_ref().is_some_and(|e| e.previewing());
    s.quickshape_done();
    while drag.keep_frames < 3000 && frame(&s)?.3 {
        drag.keep_frames += 1;
    }
    drag.laid_unshown = s.shared.lock().canvas.as_ref()?.laid_unshown() - unshown;
    s.keep_time_lapse();
    drag.lapse_frames = lapse.count();
    let _ = std::fs::remove_dir_all(&dir);
    drag.queued_most = queued_most;
    drag.spent_most = s.shared.lock().canvas.as_ref()?.paced_spent().0;
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    let current = s.layers().iter().find(|l| l.current).map_or(0, |l| l.id);
    drag.layer = s.shared.lock().layer_pixels(procraft_engine::document::LayerId(current))?;
    Some(drag)
}

/// A shape of a brush painting in place with a second, dragged out on a canvas that can't lay it
/// whole again at each move within a frame (a slow GPU's pace, set): it follows the drag, shown by
/// its preview (laid dry, without its second) until the drag ends, and is then laid as itself and
/// kept exactly as it is laid all at once. Before, its stroke was begun again at every move before
/// the last was laid, so the picture never changed through the drag, and each move's stroke was
/// queued (none let go: the second is laid again first, so the first's stamps never came last) to be
/// laid whole, one after another, after the lift: 28 moves of a big brush took a tablet a minute and
/// a half, the frame that laid them holding the canvas the whole time (a time-lapse frame, asked for
/// as the pen lifted, read the canvas with a render that laid all that was queued at once). Bounds:
/// none of its stamps laid by a render that shows nothing (the time-lapse frame waits); every move
/// shown by the frame that lays it, but the first whose stroke takes longer than a frame to lay
/// whole (the shape growing as it is dragged out); no more than one move's stamps queued; no frame
/// gives the stamps over 200 ms of the GPU (150 ms, and the least lot of each part a frame lays
/// however little time is left); laid as itself within 400 frames of the lift.
#[test]
fn a_heavy_shape_dragged_about_follows_the_drag_and_is_kept_as_laid_at_once() {
    let (Some(at_once), Some(slow)) = (drag_heavy_shape(false), drag_heavy_shape(true)) else { return };
    for (name, d) in [("at once", &at_once), ("slow", &slow)] {
        println!(
            "{name}: {} drag frames, {} by the preview; {} of 12 moves shown as laid, {} frames a move at most; \
             laid {} frames after the lift, kept {} frames after; {:.1} ms of the GPU a frame at most; {:?} stamps \
             queued at most; {} laid by renders showing nothing; {} time-lapse frames",
            d.drag_frames,
            d.previewed,
            d.moves_shown,
            d.most_frames_a_move,
            d.settle_frames,
            d.keep_frames,
            d.spent_most,
            d.queued_most,
            d.laid_unshown,
            d.lapse_frames
        );
    }
    assert_eq!(at_once.previewed, 0, "laid all at once, it needs no preview");
    // the time-lapse frame asked for as the pen lifted waited for the frames to lay the stroke (the
    // render it is read with laid what was left at once)
    assert_eq!(slow.laid_unshown, 0, "renders showing nothing laid stamps");
    assert!(slow.lapse_frames >= 1, "no time-lapse frame");
    assert!(slow.previewed > 0, "never shown by its preview");
    assert!(!slow.preview_left, "still shown by its preview once laid");
    // (all but the first whose stroke takes longer than a frame to lay, as the shape grows: from it
    // on, the preview)
    assert!(slow.moves_shown >= 11, "{} of 12 moves shown as laid", slow.moves_shown);
    assert!(slow.settle_frames < 400, "laid {} frames after the lift", slow.settle_frames);
    // one move's stamps queued (a dual brush's every move was)
    let (first, second) = slow.queued_most;
    assert!(first <= 3000 && second <= 3000, "{first} and {second} stamps queued");
    assert!(slow.spent_most <= 200.0, "a frame gave the stamps {:.0} ms of the GPU", slow.spent_most);
    let differ = at_once.layer.iter().zip(&slow.layer).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "kept otherwise than laid all at once");
}

/// A shape of wet paint laid in place shows, while it is still open, as it is kept: its stroke is
/// all there is of it, so its last display frame's stamps are laid with the rest rather than held
/// for more of that frame to come (a big circle showed a gap at its seam until it was kept).
#[test]
fn an_open_shape_of_wet_paint_shows_as_it_is_kept() {
    use procraft_engine::brush::{Brush, Taper};
    use std::time::Duration;
    let Some(s) = session(512, 512) else { return };
    {
        let mut st = s.shared.lock();
        let Some(canvas) = st.canvas.as_mut() else { return };
        canvas.set_relay_at_once(Duration::from_secs(3600));
        let tool = st.tool;
        let Some(chosen) = st.brush_mut(tool) else { return };
        *chosen = Brush { spacing: 0.05, pen_taper: Taper::NONE, ..Brush::round_pen() };
        chosen.set_setting("wet_dilution", 0.4);
        chosen.set_setting("wet_charge", 0.6);
        chosen.set_setting("wet_pull", 0.5);
    }
    s.set_color(0.1, 0.2, 0.6);
    s.set_brush_size(0.2);
    let composite = |s: &CanvasSession| -> Vec<u8> {
        let mut guard = s.shared.lock();
        let st = &mut *guard;
        let (Some(canvas), Some(gpu)) = (st.canvas.as_mut(), st.gpu.as_ref()) else { return Vec::new() };
        canvas.render_finished(gpu, &mut st.doc);
        canvas.read_composite(gpu).unwrap_or_default()
    };
    let pen =
        |x: f32, y: f32, t: f64| PenSample { x, y, pressure: 1.0, tilt: 0.0, orientation: 0.0, time_ms: t, roll: 0.0 };
    let round: Vec<PenSample> = (0..=120)
        .map(|i| {
            let a = i as f32 / 120.0 * TAU;
            pen(256.0 + 150.0 * a.cos(), 256.0 + 150.0 * a.sin(), f64::from(i) * 8.0)
        })
        .collect();
    s.stroke_begin(PointerKind::Stylus, round[..1].to_vec());
    s.stroke_add(round[1..].to_vec());
    assert!(s.quickshape_hold().is_some());
    s.stroke_end(vec![]);
    let open = composite(&s);
    s.quickshape_done();
    let kept = composite(&s);
    let differ = open.chunks(4).zip(kept.chunks(4)).filter(|(a, b)| a != b).count();
    println!("{differ} pixels shown otherwise while open than kept");
    assert_eq!(differ, 0, "shown otherwise while open than kept");
}
