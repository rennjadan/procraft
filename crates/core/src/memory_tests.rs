//! Memory over strokes: a heavy dual brush's big fast strokes, a long session of many brushes, and
//! big wet brushes with heavy smoothing on a tablet of little memory. Measured as they go: what the
//! canvas holds (its textures, the buffers its strokes' work takes, the history on the GPU and
//! packed off it) and what the GPU has allocated, where its backend says (not on OpenGL).

use std::time::Instant;

use procraft_engine::profile::Profile;
use procraft_engine::view::View;
use procraft_gpu::{Gpu, wgpu};

use crate::{CanvasSession, PenSample, PointerKind, ToolKind};

const MB: u64 = 1 << 20;

/// What a session holds now, MB.
#[derive(Clone, Copy, Debug, Default)]
struct Held {
    /// The canvas's textures and the buffers its strokes' work takes.
    canvas: u64,
    /// The history's pixels on the GPU, and packed off it.
    history: u64,
    packed: u64,
    /// The GPU's memory allocated (0 where the backend doesn't say).
    gpu: u64,
}

impl Held {
    fn of(s: &CanvasSession) -> Held {
        let st = s.shared.lock();
        let (Some(c), Some(gpu)) = (st.canvas.as_ref(), st.gpu.as_ref()) else { return Held::default() };
        let allocated = gpu.device.generate_allocator_report().map_or(0, |r| r.total_allocated_bytes);
        Held {
            canvas: (c.texture_bytes() + c.work_buffer_bytes()) / MB,
            history: c.history_bytes() / MB,
            packed: c.history_packed_bytes() / MB,
            gpu: allocated / MB,
        }
    }

    fn most(self, other: Held) -> Held {
        Held {
            canvas: self.canvas.max(other.canvas),
            history: self.history.max(other.history),
            packed: self.packed.max(other.packed),
            gpu: self.gpu.max(other.gpu),
        }
    }
}

/// A window the session's frames are drawn into, the most each held kept, and the longest frame.
struct Probe {
    view: wgpu::TextureView,
    size: (u32, u32),
    began: Instant,
    most: Held,
    longest: f64,
}

impl Probe {
    fn new(gpu: &Gpu, size: (u32, u32)) -> Probe {
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        Probe { view, size, began: Instant::now(), most: Held::default(), longest: 0.0 }
    }

    /// A frame drawn and its GPU work waited for, what it held noted (before the wait: what the
    /// GPU still works on counts); whether more are wanted.
    fn frame(&mut self, s: &CanvasSession) -> bool {
        let t = Instant::now();
        let more = {
            let mut st = s.shared.lock();
            st.draw_frame(&self.view, wgpu::TextureFormat::Rgba8Unorm, self.size, Profile::Srgb, self.began);
            let c = st.canvas.as_ref();
            st.needs_frame || c.is_some_and(|c| c.wants_render() || c.lays_over_frames() || c.catching_up())
        };
        self.most = self.most.most(Held::of(s));
        let gpu = s.shared.lock().gpu.clone();
        if let Some(gpu) = gpu {
            let _ = gpu.wait();
        }
        self.longest = self.longest.max(t.elapsed().as_secs_f64() * 1e3);
        more
    }

    /// Frames until the canvas wants no more (at most `most`).
    fn settle(&mut self, s: &CanvasSession, most: usize) {
        for _ in 0..most {
            if !self.frame(s) {
                break;
            }
        }
    }
}

/// A session on its own GPU, its canvas `w` × `h` shown whole in a window its size, the canvas's
/// layers given `budget` MB as a tablet's are (0: none, as a computer's).
fn session_sized(w: u32, h: u32, budget: u64) -> Option<std::sync::Arc<CanvasSession>> {
    let gpu = match Gpu::headless() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return None;
        }
    };
    let s = CanvasSession::new(w, h);
    {
        let mut st = s.shared.lock();
        st.window_size = (w, h);
        st.view = Some(View::fit((w, h), (w, h)));
        st.gpu = Some(gpu);
        st.ensure_canvas();
        st.canvas.as_mut()?.set_resident_budget(budget * MB);
    }
    Some(s)
}

/// The stock brush `name` chosen.
fn pick(s: &CanvasSession, name: &str) -> bool {
    let found = s
        .brush_library()
        .iter()
        .enumerate()
        .find_map(|(i, set)| set.brushes.iter().position(|b| b.name == name).map(|j| (i as u32, j as u32)));
    found.is_some_and(|(set, index)| s.select_brush(set, index))
}

/// A fast stroke of `n` samples (4 ms apart, 10 canvas pixels) zigzagging across a canvas `w` × `h`
/// from `from`, a frame drawn every four, then frames until it is laid.
fn zigzag(s: &CanvasSession, p: &mut Probe, (w, h): (u32, u32), from: (f32, f32), n: usize, clock: &mut f64) {
    let (mut x, mut y) = from;
    let (mut dx, mut dy) = (6.0f32, 8.0f32);
    let pen = |x: f32, y: f32, t: f64, k: usize| PenSample {
        x,
        y,
        pressure: 0.6 + 0.4 * (k as f32 * 0.07).sin().abs(),
        tilt: 0.5,
        orientation: 0.8,
        time_ms: t,
        roll: 0.0,
    };
    s.stroke_begin(PointerKind::Stylus, vec![pen(x, y, *clock, 0)]);
    let mut batch = Vec::new();
    for k in 1..n {
        x += dx;
        y += dy;
        if x < 50.0 || x > w as f32 - 50.0 {
            dx = -dx;
        }
        if y < 50.0 || y > h as f32 - 50.0 {
            dy = -dy;
        }
        *clock += 4.0;
        batch.push(pen(x, y, *clock, k));
        if k % 4 == 0 {
            s.stroke_add(std::mem::take(&mut batch));
            p.frame(s);
        }
    }
    s.stroke_end(batch);
    p.settle(s, 400);
    *clock += 300.0;
}

/// Ironwood (a dual brush both of whose parts are built up by their rule and carried from lot to
/// lot) at its biggest, five fast strokes zigzagging across a 2160 × 1440 canvas with an 8 GB
/// tablet's budget (a tester's ran out of memory with it): what the canvas holds levels off at what
/// its first stroke made (what the parts are carried in, kept for the next) and each stroke's undo
/// step, and trimming memory gives the carried parts back. Printed: what it held before, after each
/// stroke and at the most, and trimmed. Bounds: the canvas's textures and work buffers at most
/// 360 MB over what they were before (measured 246 MB on Vulkan and OpenGL; 47 MB before dual
/// brushes were carried and showed their tip), each stroke after the first adding no more than its
/// undo step and 16 MB; trimmed, at least the 100 MB the parts were carried in given back (142).
#[test]
fn a_heavy_dual_brush_s_big_fast_strokes_level_off_and_give_their_memory_back() {
    let (w, h) = (2160, 1440);
    let Some(s) = session_sized(w, h, 1993) else { return };
    assert!(pick(&s, "Ironwood"));
    s.set_brush_size(1.0);
    s.set_color(0.8, 0.3, 0.1);
    let gpu = s.shared.lock().gpu.clone().unwrap();
    let mut p = Probe::new(&gpu, (w, h));
    p.frame(&s);
    let before = Held::of(&s);
    let mut clock = 0.0;
    let mut after = Vec::new();
    for n in 0..5 {
        zigzag(&s, &mut p, (w, h), (100.0 + n as f32 * 300.0, 100.0), 300, &mut clock);
        after.push(Held::of(&s));
    }
    s.trim_memory();
    p.frame(&s);
    gpu.wait().unwrap();
    let trimmed = Held::of(&s);
    println!(
        "Ironwood on {w}×{h} ({:?}): before {before:?}; after each stroke {after:?}; at the most {:?}; trimmed \
         {trimmed:?}; longest frame {:.0} ms",
        gpu.backend(),
        p.most,
        p.longest
    );
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    let made = p.most.canvas.saturating_sub(before.canvas);
    assert!(made <= 360, "the canvas held {made} MB more at the most");
    for pair in after.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let stroke = b.canvas.saturating_sub(a.canvas);
        let step = b.history.saturating_sub(a.history);
        assert!(stroke <= step + 16, "a stroke added {stroke} MB, its undo step {step} MB");
    }
    let last = after.last().copied().unwrap_or_default();
    let given = last.canvas.saturating_sub(trimmed.canvas);
    assert!(given >= 100, "trimmed, the canvas gave back {given} MB");
}

/// A long session as a tablet of 12 GB has it (its layers given 2.8 GB): strokes of the paint sets'
/// brushes (dry, wet, dual, laid in place), a smudge and an erasing stroke now and then, at sizes
/// from a fifth up, with undo and redo, new layers and layers switched to, and the library's sample
/// strokes drawn now and then (`SOAK_STROKES` strokes, 300 if not set). What the canvas holds levels
/// off once the history is full: its older steps kept packed off the GPU, the newest quarter on it.
/// All on the GPU, the history held 768 MB of the 1.2 GB the session held on a tester's tablet,
/// which the system closed as soon as it was put away. Printed every 50 strokes. Bounds: what the
/// canvas holds at the end no more than half way and 64 MB; the history on the GPU at most a quarter
/// of its room and two full layers. Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn a_long_session_s_memory_levels_off_with_the_history_packed() {
    let (w, h) = (2160, 1440);
    let Some(s) = session_sized(w, h, 2800) else { return };
    let strokes = std::env::var("SOAK_STROKES").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let sets = ["Oils", "Paints", "Watercolors", "Gouache", "Artistic", "Inks", "Painting"];
    let report = soak(&s, (w, h), &sets, strokes, (0.2, None), 50);
    let room = s.shared.lock().canvas.as_ref().map_or(0, |c| c.history_most()) / MB;
    let layer = u64::from(w * h * 4) / MB + 1;
    let (half, end) = (report[report.len() / 2], report[report.len() - 1]);
    assert!(end.canvas <= half.canvas + 64, "held {} MB half way, {} MB at the end", half.canvas, end.canvas);
    assert!(end.history <= room / 4 + 2 * layer, "{} MB of history on the GPU (room {room} MB)", end.history);
}

/// Big wet and oil brushes with every smoothing at its most (StreamLine, stabilisation and motion
/// filtering), at sizes from 0.6 up, on a 2048 square canvas of a 4 GB tablet (its layers given
/// 912 MB: a device of little memory), as a tester's Galaxy Tab S4 lagged and closed (#416): 60
/// strokes, a smudge and an erasing stroke now and then, undo and redo. What the canvas holds levels
/// off. Printed every 10 strokes. Bounds: at the end no more than half way and 64 MB; at the most
/// within the layers' budget. Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn big_wet_brushes_smoothed_their_most_on_a_small_tablet_level_off() {
    let (w, h) = (2048, 2048);
    let budget = 912;
    let Some(s) = session_sized(w, h, budget) else { return };
    let report = soak(&s, (w, h), &["Oils", "Paints", "Watercolors", "Gouache"], 60, (0.6, Some(1.0)), 10);
    let (half, end) = (report[report.len() / 2], report[report.len() - 1]);
    let most = report.iter().fold(Held::default(), |m, h| m.most(*h));
    assert!(end.canvas <= half.canvas + 64, "held {} MB half way, {} MB at the end", half.canvas, end.canvas);
    assert!(most.canvas <= budget, "held {} MB at the most", most.canvas);
}

/// `strokes` strokes on `s` (a canvas `w` × `h`) with the brushes of the stock `sets` in turn, at
/// sizes from `least` up, every smoothing at `smoothing` if given: a smudge and an erasing stroke
/// now and then, undo and redo every 10 strokes, a layer added or switched to every 25, the
/// library's sample strokes of four brushes every 30. What it held after every `every` strokes.
fn soak(
    s: &CanvasSession,
    (w, h): (u32, u32),
    sets: &[&str],
    strokes: usize,
    (least, smoothing): (f32, Option<f32>),
    every: usize,
) -> Vec<Held> {
    let brushes: Vec<(u32, u32, String)> = s
        .brush_library()
        .iter()
        .enumerate()
        .filter(|(_, set)| sets.contains(&set.name.as_str()))
        .flat_map(|(i, set)| set.brushes.iter().enumerate().map(move |(j, b)| (i as u32, j as u32, b.name.clone())))
        .collect();
    assert!(!brushes.is_empty(), "none of {sets:?}");
    let gpu = s.shared.lock().gpu.clone().unwrap();
    let mut p = Probe::new(&gpu, (w, h));
    p.frame(s);
    let before = Held::of(s);
    println!("{} brushes, {strokes} strokes on {w}×{h} ({:?}): before {before:?}", brushes.len(), gpu.backend());
    let mut seed = 0x1234_5678u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 10_000) as f32 / 10_000.0
    };
    let mut clock = 0.0f64;
    let mut layers = vec![s.layers().iter().find(|l| l.current).map_or(0, |l| l.id)];
    let mut report = Vec::new();
    let began = Instant::now();
    for k in 0..strokes {
        let (set, index, name) = &brushes[k * 7 % brushes.len()];
        assert!(s.select_brush(*set, *index));
        if let Some(amount) = smoothing {
            s.studio_begin();
            for key in ["streamline", "stabilization", "motion_filtering"] {
                s.set_studio_setting(key.to_owned(), amount);
            }
            s.studio_end(true);
        }
        s.set_tool(match k % 9 {
            4 => ToolKind::Smudge,
            7 => ToolKind::Erase,
            _ => ToolKind::Paint,
        });
        s.set_brush_size(least + (1.0 - least) * rnd());
        s.set_color(rnd(), rnd(), rnd());
        let n = 60 + (rnd() * 200.0) as usize;
        let from = (50.0 + rnd() * (w as f32 - 100.0), 50.0 + rnd() * (h as f32 - 100.0));
        zigzag(s, &mut p, (w, h), from, n, &mut clock);
        if k % 10 == 9 {
            for _ in 0..3 {
                s.undo();
                p.frame(s);
            }
            for _ in 0..2 {
                s.redo();
                p.frame(s);
            }
        }
        if k % 25 == 24 {
            if layers.len() < 6 {
                layers.push(s.add_layer());
            } else {
                s.select_layer(layers[k / 25 % layers.len()]);
            }
            p.frame(s);
        }
        if k % 30 == 29 {
            for b in brushes.iter().skip(k % 11).step_by(9).take(4) {
                let _ = s.brush_preview(b.0, b.1, 1060, 324);
            }
            p.frame(s);
        }
        if k % every == every - 1 || k + 1 == strokes {
            // (the history's packing, begun as the last stroke ended, done)
            p.settle(s, 50);
            let now = Held::of(s);
            report.push(now);
            println!(
                "stroke {} ({name}): {now:?}; at the most {:?}; longest frame {:.0} ms; {:.0} s",
                k + 1,
                p.most,
                p.longest,
                began.elapsed().as_secs_f32()
            );
        }
    }
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    report
}
