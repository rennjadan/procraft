//! What each library brush costs a frame through a long stroke, at a few sizes: the stamps each
//! frame lays, its CPU and GPU time, the stroke's start and its lift, and probes of where a heavy
//! brush's frames and ends go. Benches, run by hand with `--ignored --nocapture` (on a tablet too,
//! built with cargo-ndk). A tablet's GPU waited for after each frame slows its clocks between them
//! (a big brush's frame took three times as long so): its frame rate is measured with the GPU kept
//! busy, as the app keeps it ([`heavy_pipelined`]).

use std::io::Write;
use std::time::Instant;

use procraft_engine::view::View;
use procraft_gpu::wgpu;

use crate::tests::session;
use crate::{PenSample, PointerKind};

/// Renders what is queued and reads the composite.
fn composite(s: &crate::CanvasSession) -> Vec<u8> {
    let mut guard = s.shared.lock();
    let st = &mut *guard;
    let (canvas, gpu) = (st.canvas.as_mut().unwrap(), st.gpu.as_ref().unwrap());
    canvas.render(gpu, &mut st.doc, None);
    canvas.read_composite(gpu).unwrap()
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// The `k`th sample of the stroke (a pen at 240 Hz): back and forth across the canvas fitted in a
/// 2560 × 1600 window, a slow wave up and down, pressure rising and falling.
fn point(k: usize, width: f32, height: f32) -> PenSample {
    let side = height;
    let left = (width - side) / 2.0 + 80.0;
    let span = side - 160.0;
    let along = k as f32 * 6.0;
    let line = (along / span) as usize;
    let into = along % span;
    let x = if line.is_multiple_of(2) { left + into } else { left + span - into };
    let y = 160.0 + (line % 5) as f32 * 260.0 + (along * 0.01).sin() * 90.0;
    PenSample {
        x,
        y,
        pressure: 0.35 + 0.6 * (k as f32 * 0.02).sin().abs(),
        tilt: 0.0,
        orientation: 0.0,
        time_ms: k as f64 * 4.1667,
        roll: 0.0,
    }
}

/// Every library brush (or the sets `HEAVY_SETS`, the brushes `HEAVY_BRUSHES` names, `,` between)
/// drawn in a stroke `HEAVY_FRAMES` frames long (two pen samples a frame) at the size sliders
/// `HEAVY_SIZES`, a frame drawn into a 2560 × 1600 window and the GPU waited for after each. Writes
/// a row a stroke to `HEAVY_OUT` and a row a frame to `HEAVY_FRAMES_OUT`.
#[test]
#[ignore]
fn heavy_brush_frames() {
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let frames: usize = env("HEAVY_FRAMES", "240").parse().unwrap_or(240);
    let sizes: Vec<f32> = env("HEAVY_SIZES", "0.1,0.5,1.0").split(',').filter_map(|s| s.parse().ok()).collect();
    let sets = env("HEAVY_SETS", "");
    let names = env("HEAVY_BRUSHES", "");
    let side: u32 = env("HEAVY_CANVAS", "2048").parse().unwrap_or(2048);
    let aside = env("HEAVY_ASIDE", "1") == "1";
    let (width, height) = (2560u32, 1600u32);
    let mut out = std::fs::File::create(env("HEAVY_OUT", "heavy-strokes.tsv")).unwrap();
    let mut per_frame = std::fs::File::create(env("HEAVY_FRAMES_OUT", "heavy-frames.tsv")).unwrap();
    writeln!(
        out,
        "set\tbrush\tsize\tdiameter\tspacing\tcount\tdual\tin_place\tshape_side\tgrain_side\tstabilisation\tmotion\tstreamline\tselect_ms\tbegin_ms\tfirst_cpu\tfirst_gpu\tpictures_ms\tframes\tstamps_med\tstamps_max\tstamps_all\tadd_med\tadd_max\tcpu_med\tcpu_p95\tcpu_max\tgpu_med\tgpu_p95\tgpu_max\tframe_med\tframe_p95\tframe_max\tend_ms\tlift_cpu\tlift_gpu\tlift_frames\tlift_total\tlift_worst"
    )
    .unwrap();
    writeln!(per_frame, "set\tbrush\tsize\tframe\tadd\tstamps\tcpu\tgpu").unwrap();
    if aside {
        crate::big_pictures::ASIDE.with(|a| a.set(true));
    }
    let Some(s) = session(side, width) else { return };
    s.resize_window(width, height);
    s.set_display_scale(2.0);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = {
        let mut st = s.shared.lock();
        st.view = Some(View::fit((side, side), (width, height)));
        let gpu = st.gpu.as_ref().unwrap();
        let info = gpu.adapter.get_info();
        println!("{} ({:?}), window {width}x{height}, canvas {side} square", info.name, info.backend);
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("window"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    };
    let view = target.create_view(&Default::default());
    let started = Instant::now();
    let gpu = s.shared.lock().gpu.clone().unwrap();
    // a frame: its CPU time (recorded and sent), then the GPU's (waited for)
    let frame = || {
        let t = Instant::now();
        let more = {
            let mut st = s.shared.lock();
            st.draw_frame(&view, format, (width, height), procraft_engine::profile::Profile::Srgb, started);
            st.needs_frame || st.canvas.as_ref().is_some_and(|c| c.lays_over_frames())
        };
        let cpu = ms(t);
        let t = Instant::now();
        gpu.flush();
        (cpu, ms(t), more)
    };
    let queued = || s.shared.lock().canvas.as_ref().map_or(0, |c| c.stamps_queued());
    let at = |v: &[f64], q: f64| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v.get(((v.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or(0.0)
    };
    let most = |v: &[f64]| v.iter().copied().fold(0.0, f64::max);
    // paint to mix into
    let library = s.brush_library();
    if let Some((set, index)) = library
        .iter()
        .enumerate()
        .find_map(|(i, set)| set.brushes.iter().position(|b| b.name == "Soft Airbrush").map(|j| (i as u32, j as u32)))
    {
        s.select_brush(set, index);
        s.set_brush_size(1.0);
        for k in 0..4 {
            s.set_color(k as f32 / 3.0, 0.4, 1.0 - k as f32 / 3.0);
            s.stroke_begin(PointerKind::Stylus, vec![point(k * 300, width as f32, height as f32)]);
            s.stroke_add((1..80).map(|i| point(k * 300 + i * 3, width as f32, height as f32)).collect());
            s.stroke_end(vec![]);
            frame();
        }
    }
    s.set_color(0.8, 0.2, 0.1);
    for set in &library {
        if !sets.is_empty() && !sets.split(',').any(|n| n == set.name) {
            continue;
        }
        for b in &set.brushes {
            if !names.is_empty() && !names.split(',').any(|n| n == b.name) {
                continue;
            }
            let t = Instant::now();
            s.select_brush(b.set, b.index);
            let select = ms(t);
            // settings changed for the run (`HEAVY_SETTINGS`: key=value, `,` between)
            let settings = env("HEAVY_SETTINGS", "");
            if !settings.is_empty() {
                s.studio_begin();
                for kv in settings.split(',') {
                    if let Some((k, v)) = kv.split_once('=')
                        && let Ok(v) = v.parse::<f32>()
                    {
                        s.set_studio_setting(k.to_owned(), v);
                    }
                }
                s.studio_end(true);
            }
            let brush = s.shared.lock().brush(crate::ToolKind::Paint).clone();
            let style = procraft_gpu::StrokeStyle::for_brush(
                &brush,
                procraft_gpu::StrokeTool::Paint,
                [0.8, 0.2, 0.1],
                1.0,
                false,
            );
            let pictures = Instant::now();
            crate::big_pictures::wait_for_big_pictures(std::time::Duration::from_secs(60));
            let pictures = ms(pictures);
            let shape_side = brush
                .shape_file_now()
                .map(|f| f.full.side)
                .or_else(|| crate::big_pictures::shape_of(&brush).map(|b| b.side))
                .unwrap_or(0);
            let grain_side = brush
                .grain_file_now()
                .and_then(|f| f.full.as_ref().map(|g| g.side))
                .or_else(|| crate::big_pictures::grain_of(&brush).map(|b| b.side))
                .unwrap_or(0);
            for (n, &size) in sizes.iter().enumerate() {
                s.set_brush_size(size);
                let diameter = brush.diameter(size);
                let t = Instant::now();
                s.stroke_begin(PointerKind::Stylus, vec![point(0, width as f32, height as f32)]);
                let begin = ms(t);
                let (first_cpu, first_gpu, _) = frame();
                let mut last = queued();
                let (mut adds, mut cpus, mut gpus, mut whole, mut stamps) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for f in 1..=frames {
                    let t = Instant::now();
                    s.stroke_add(vec![
                        point(2 * f - 1, width as f32, height as f32),
                        point(2 * f, width as f32, height as f32),
                    ]);
                    let add = ms(t);
                    let (cpu, gpu_ms, _) = frame();
                    let now = queued();
                    let laid = now.saturating_sub(last) as f64;
                    last = now;
                    writeln!(
                        per_frame,
                        "{}\t{}\t{size}\t{f}\t{add:.3}\t{laid}\t{cpu:.3}\t{gpu_ms:.3}",
                        set.name, b.name
                    )
                    .unwrap();
                    adds.push(add);
                    cpus.push(cpu);
                    gpus.push(gpu_ms);
                    whole.push(add + cpu + gpu_ms);
                    stamps.push(laid);
                }
                let t = Instant::now();
                s.stroke_end(vec![]);
                let end = ms(t);
                let (mut lift_cpu, mut lift_gpu, mut lift_frames, mut lift_worst) = (0.0, 0.0, 0usize, 0.0f64);
                loop {
                    let (cpu, gpu_ms, more) = frame();
                    lift_cpu += cpu;
                    lift_gpu += gpu_ms;
                    lift_frames += 1;
                    lift_worst = lift_worst.max(cpu + gpu_ms);
                    if !more || lift_frames >= 400 {
                        break;
                    }
                }
                writeln!(
                    per_frame,
                    "{}\t{}\t{size}\tlift\t{end:.3}\t{}\t{lift_cpu:.3}\t{lift_gpu:.3}",
                    set.name,
                    b.name,
                    queued().saturating_sub(last)
                )
                .unwrap();
                let lift_total = end + lift_cpu + lift_gpu;
                let row = format!(
                    "{}\t{}\t{size}\t{diameter:.1}\t{:.4}\t{}\t{}\t{}\t{shape_side}\t{grain_side}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{begin:.2}\t{first_cpu:.2}\t{first_gpu:.2}\t{:.1}\t{frames}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{end:.2}\t{lift_cpu:.2}\t{lift_gpu:.2}\t{lift_frames}\t{lift_total:.2}\t{lift_worst:.2}",
                    set.name,
                    b.name,
                    brush.spacing,
                    brush.shape.count,
                    u8::from(brush.secondary.is_some()),
                    u8::from(style.in_place()),
                    brush.setting("stabilization"),
                    brush.setting("motion_filtering"),
                    brush.streamline,
                    if n == 0 { select } else { 0.0 },
                    if n == 0 { pictures } else { 0.0 },
                    at(&stamps, 0.5),
                    most(&stamps),
                    stamps.iter().sum::<f64>(),
                    at(&adds, 0.5),
                    most(&adds),
                    at(&cpus, 0.5),
                    at(&cpus, 0.95),
                    most(&cpus),
                    at(&gpus, 0.5),
                    at(&gpus, 0.95),
                    most(&gpus),
                    at(&whole, 0.5),
                    at(&whole, 0.95),
                    most(&whole),
                );
                writeln!(out, "{row}").unwrap();
                println!("{row}");
                s.undo();
                frame();
            }
        }
    }
}

/// Why a stroke's end lays what it lays: for the brushes `HEAVY_BRUSHES` names at `HEAVY_SIZES`,
/// what each part has laid when the pen lifts and where its end would lay from.
#[test]
#[ignore]
fn heavy_lift_probe() {
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let frames: usize = env("HEAVY_FRAMES", "240").parse().unwrap_or(240);
    let sizes: Vec<f32> = env("HEAVY_SIZES", "0.1,1.0").split(',').filter_map(|s| s.parse().ok()).collect();
    let names = env("HEAVY_BRUSHES", "Verbena,Elfin Saddle,Easterly,Hamilton,Torrent");
    let (width, height) = (2560u32, 1600u32);
    let Some(s) = session(2048, width) else { return };
    s.resize_window(width, height);
    {
        let mut st = s.shared.lock();
        st.view = Some(View::fit((2048, 2048), (width, height)));
    }
    let library = s.brush_library();
    for name in names.split(',') {
        let Some(b) = library.iter().flat_map(|set| set.brushes.iter()).find(|b| b.name == name) else {
            println!("no {name}");
            continue;
        };
        s.select_brush(b.set, b.index);
        for &size in &sizes {
            s.set_brush_size(size);
            s.stroke_begin(PointerKind::Stylus, vec![point(0, width as f32, height as f32)]);
            for f in 1..=frames {
                s.stroke_add(vec![
                    point(2 * f - 1, width as f32, height as f32),
                    point(2 * f, width as f32, height as f32),
                ]);
                let mut st = s.shared.lock();
                let st = &mut *st;
                if let (Some(c), Some(g)) = (st.canvas.as_mut(), st.gpu.as_ref()) {
                    c.render(g, &mut st.doc, None);
                }
            }
            {
                let st = s.shared.lock();
                let c = st.canvas.as_ref().unwrap();
                let first = st.stroke.clone().map(|mut b| {
                    let n = c.laid_count();
                    let placed = b.stamp_count();
                    (n, placed, n.and_then(|n| b.finish_changed(n).map(|(from, rest)| (from, rest.len()))))
                });
                let second = st.second.clone().map(|mut b| {
                    let n = c.laid_second_count();
                    (n, n.and_then(|n| b.finish_changed(n).map(|(from, rest)| (from, rest.len()))))
                });
                println!(
                    "{name} {size}: first laid {:?} placed {:?} -> {:?}; second laid,end {:?}; keeps_laid {}",
                    first.map(|f| f.0),
                    first.map(|f| f.1),
                    first.map(|f| f.2),
                    second,
                    c.keeps_laid()
                );
            }
            let before = s.shared.lock().canvas.as_ref().map_or(0, |c| c.stamps_queued());
            s.stroke_end(vec![]);
            let after = s.shared.lock().canvas.as_ref().map_or(0, |c| c.stamps_queued());
            println!("   end queued {}", after - before);
            s.undo();
        }
    }
}

/// Where a big brush's frame goes: frames with new samples drawn whole, frames with none, and
/// frames with new samples whose canvas work alone is drawn (no window), the GPU waited for after
/// each; medians, for the brushes `HEAVY_BRUSHES` names at `HEAVY_SIZES`.
#[test]
#[ignore]
fn heavy_frame_parts() {
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let sizes: Vec<f32> = env("HEAVY_SIZES", "1.0").split(',').filter_map(|s| s.parse().ok()).collect();
    let names = env("HEAVY_BRUSHES", "Chroma,Severn,Holdencroft,Longstone");
    let side: u32 = env("HEAVY_CANVAS", "2048").parse().unwrap_or(2048);
    let (width, height) = (2560u32, 1600u32);
    let Some(s) = session(side, width) else { return };
    s.resize_window(width, height);
    s.set_display_scale(2.0);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = {
        let mut st = s.shared.lock();
        st.view = Some(View::fit((side, side), (width, height)));
        st.gpu.as_ref().unwrap().device.create_texture(&wgpu::TextureDescriptor {
            label: Some("window"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    };
    let view = target.create_view(&Default::default());
    let gpu = s.shared.lock().gpu.clone().unwrap();
    let started = Instant::now();
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let library = s.brush_library();
    for name in names.split(',') {
        let Some(b) = library.iter().flat_map(|set| set.brushes.iter()).find(|b| b.name == name) else {
            println!("no {name}");
            continue;
        };
        s.select_brush(b.set, b.index);
        crate::big_pictures::wait_for_big_pictures(std::time::Duration::from_secs(60));
        for &size in &sizes {
            s.set_brush_size(size);
            s.stroke_begin(PointerKind::Stylus, vec![point(0, width as f32, height as f32)]);
            let (mut whole, mut idle, mut canvas_only, mut cpu_whole, mut cpu_canvas) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for f in 1..=120 {
                s.stroke_add(vec![
                    point(2 * f - 1, width as f32, height as f32),
                    point(2 * f, width as f32, height as f32),
                ]);
                let t = Instant::now();
                match f % 3 {
                    0 => {
                        s.shared.lock().draw_frame(
                            &view,
                            format,
                            (width, height),
                            procraft_engine::profile::Profile::Srgb,
                            started,
                        );
                        cpu_whole.push(ms(t));
                        gpu.flush();
                        whole.push(ms(t));
                        // and one with nothing new
                        let t = Instant::now();
                        s.shared.lock().needs_frame = true;
                        s.shared.lock().draw_frame(
                            &view,
                            format,
                            (width, height),
                            procraft_engine::profile::Profile::Srgb,
                            started,
                        );
                        gpu.flush();
                        idle.push(ms(t));
                    }
                    1 => {
                        {
                            let mut st = s.shared.lock();
                            let st = &mut *st;
                            st.draw_ahead();
                            if let (Some(c), Some(g)) = (st.canvas.as_mut(), st.gpu.as_ref()) {
                                c.render(g, &mut st.doc, None);
                            }
                        }
                        cpu_canvas.push(ms(t));
                        gpu.flush();
                        canvas_only.push(ms(t));
                    }
                    _ => {
                        s.shared.lock().draw_frame(
                            &view,
                            format,
                            (width, height),
                            procraft_engine::profile::Profile::Srgb,
                            started,
                        );
                        gpu.flush();
                    }
                }
            }
            s.stroke_end(vec![]);
            s.shared.lock().draw_frame(
                &view,
                format,
                (width, height),
                procraft_engine::profile::Profile::Srgb,
                started,
            );
            gpu.flush();
            println!(
                "{name} {size}: whole frame {:.2} ms (cpu {:.2}), nothing new {:.2}, canvas work alone {:.2} (cpu {:.2})",
                median(&mut whole),
                median(&mut cpu_whole),
                median(&mut idle),
                median(&mut canvas_only),
                median(&mut cpu_canvas)
            );
            s.undo();
            s.shared.lock().draw_frame(
                &view,
                format,
                (width, height),
                procraft_engine::profile::Profile::Srgb,
                started,
            );
            gpu.flush();
        }
    }
}

/// What the first stroke after choosing a brush waits for: each library brush (or `HEAVY_BRUSHES`)
/// chosen, its pictures drawn aside as the app draws them, and the pen put down `HEAVY_AFTER_MS`
/// (0) later: how long the stroke's start took, and how long drawing its pictures took in all.
#[test]
#[ignore]
fn heavy_first_stroke() {
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let names = env("HEAVY_BRUSHES", "");
    let after: u64 = env("HEAVY_AFTER_MS", "0").parse().unwrap_or(0);
    // big pictures made aside, as the app makes them (a stroke meanwhile reads those brushes hold)
    crate::big_pictures::ASIDE.with(|a| a.set(true));
    let Some(s) = session(2048, 2048) else { return };
    let library = s.brush_library();
    let mut rows = Vec::new();
    for b in library.iter().flat_map(|set| set.brushes.iter()) {
        if !names.is_empty() && !names.split(',').any(|n| n == b.name) {
            continue;
        }
        let brush = s.shared.lock().library[b.set as usize].brushes[b.index as usize].clone();
        let pictures: Vec<procraft_engine::brush::Pixels> = std::iter::once(&brush)
            .chain(brush.secondary.as_deref())
            .flat_map(|b| [&b.shape.source, &b.grain.source])
            .flatten()
            .map(|s| s.pixels.clone())
            .collect();
        let undrawn = pictures.iter().filter(|p| !p.is_drawn()).count();
        let chosen = Instant::now();
        s.select_brush(b.set, b.index);
        std::thread::sleep(std::time::Duration::from_millis(after));
        let t = Instant::now();
        s.stroke_begin(PointerKind::Stylus, vec![point(0, 2048.0, 2048.0)]);
        let begin = ms(t);
        s.stroke_cancel();
        // how long its pictures took to draw from its choosing
        while !pictures.iter().all(procraft_engine::brush::Pixels::is_drawn) && chosen.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        let drawn = ms(chosen);
        rows.push((begin, drawn, undrawn, b.name.clone()));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let waited: Vec<f64> = rows.iter().map(|r| r.0).collect();
    let over = |ms: f64| waited.iter().filter(|&&w| w > ms).count();
    println!(
        "{} brushes, pen down {after} ms after choosing: start waited over 16 ms {}, over 50 ms {}, over 100 ms {}",
        rows.len(),
        over(16.0),
        over(50.0),
        over(100.0)
    );
    for (begin, drawn, undrawn, name) in rows.iter().take(15) {
        println!("  {begin:7.1} ms  {name} ({undrawn} pictures to draw, all drawn {drawn:.0} ms after choosing)");
    }
}

/// Every library brush that lays a part by its rule (or `HEAVY_BRUSHES`), drawn on a 768 square
/// canvas at a few sizes: the picture mid-stroke (its tip showing) and the layer once it ends,
/// written to `HEAVY_PIXELS_OUT` to set beside another build's.
#[test]
#[ignore]
fn heavy_rule_pixels() {
    use procraft_engine::brush::Rendering;
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let names = env("HEAVY_BRUSHES", "");
    let side = 768u32;
    let Some(s) = session(side, side) else { return };
    let mut out = std::fs::File::create(env("HEAVY_PIXELS_OUT", "rule-pixels.bin")).unwrap();
    let library = s.brush_library();
    let by_rule = |b: &procraft_engine::brush::Brush| matches!(b.rendering, Rendering::Gradual | Rendering::Capped);
    let point = |k: usize| PenSample {
        x: 40.0 + (k % 240) as f32 * 2.9,
        y: 120.0 + (k / 240) as f32 * 260.0 + (k as f32 * 0.03).sin() * 90.0,
        pressure: 0.3 + 0.65 * (k as f32 * 0.021).sin().abs(),
        tilt: 0.0,
        orientation: 0.0,
        time_ms: k as f64 * 4.17,
        roll: 0.0,
    };
    let mut drawn = 0;
    for b in library.iter().flat_map(|set| set.brushes.iter()) {
        let brush = s.shared.lock().library[b.set as usize].brushes[b.index as usize].clone();
        let wanted = if names.is_empty() {
            (by_rule(&brush) || brush.secondary.as_deref().is_some_and(by_rule)) && !brush.mixes_wet()
        } else {
            names.split(',').any(|n| n == b.name)
        };
        if !wanted {
            continue;
        }
        s.select_brush(b.set, b.index);
        for size in [0.25f32, 0.7, 1.0] {
            s.set_brush_size(size);
            s.set_color(0.2, 0.45, 0.8);
            s.stroke_begin(PointerKind::Stylus, vec![point(0)]);
            let mut mid = Vec::new();
            for e in 1..300 {
                s.stroke_add(vec![point(2 * e - 1), point(2 * e)]);
                if e % 3 == 0 {
                    let pic = composite(&s);
                    if e == 150 {
                        mid = pic;
                    }
                }
            }
            s.stroke_end(vec![]);
            composite(&s);
            let current = s.layers().iter().find(|l| l.current).map_or(0, |l| l.id);
            let layer = s.shared.lock().layer_pixels(procraft_engine::document::LayerId(current)).unwrap();
            let head = format!("{}/{}/{size}\n", b.set, b.name);
            out.write_all(head.as_bytes()).unwrap();
            out.write_all(&(mid.len() as u64).to_le_bytes()).unwrap();
            out.write_all(&mid).unwrap();
            out.write_all(&(layer.len() as u64).to_le_bytes()).unwrap();
            out.write_all(&layer).unwrap();
            s.undo();
            composite(&s);
            drawn += 1;
        }
    }
    println!("{drawn} strokes written");
}

/// A stroke's frames one after another as fast as they go, the GPU waited for only at the end (so
/// that it is kept busy, as the app keeps it, its clocks up): milliseconds a frame, for the
/// brushes `HEAVY_BRUSHES` names at `HEAVY_SIZES`, `HEAVY_ROUNDS` times each.
#[test]
#[ignore]
fn heavy_pipelined() {
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let sizes: Vec<f32> = env("HEAVY_SIZES", "1.0").split(',').filter_map(|s| s.parse().ok()).collect();
    let names = env("HEAVY_BRUSHES", "Chroma");
    let rounds: usize = env("HEAVY_ROUNDS", "2").parse().unwrap_or(2);
    let (width, height) = (2560u32, 1600u32);
    let side = 2048;
    let Some(s) = session(side, width) else { return };
    s.resize_window(width, height);
    s.set_display_scale(2.0);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = {
        let mut st = s.shared.lock();
        st.view = Some(View::fit((side, side), (width, height)));
        st.gpu.as_ref().unwrap().device.create_texture(&wgpu::TextureDescriptor {
            label: Some("window"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    };
    let view = target.create_view(&Default::default());
    let gpu = s.shared.lock().gpu.clone().unwrap();
    let started = Instant::now();
    let library = s.brush_library();
    for _ in 0..rounds {
        for name in names.split(',') {
            let Some(b) = library.iter().flat_map(|set| set.brushes.iter()).find(|b| b.name == name) else { continue };
            s.select_brush(b.set, b.index);
            for &size in &sizes {
                s.set_brush_size(size);
                s.stroke_begin(PointerKind::Stylus, vec![point(0, width as f32, height as f32)]);
                let frame = |f: usize| {
                    s.stroke_add(vec![
                        point(2 * f - 1, width as f32, height as f32),
                        point(2 * f, width as f32, height as f32),
                    ]);
                    s.shared.lock().draw_frame(
                        &view,
                        format,
                        (width, height),
                        procraft_engine::profile::Profile::Srgb,
                        started,
                    );
                };
                for f in 1..=20 {
                    frame(f);
                }
                gpu.flush();
                let t = Instant::now();
                for f in 21..=140 {
                    frame(f);
                    // (a frame or two ahead at most, as a window's images allow)
                    if f.is_multiple_of(2) {
                        gpu.flush();
                    }
                }
                gpu.flush();
                let each = ms(t) / 120.0;
                s.stroke_end(vec![]);
                s.shared.lock().draw_frame(
                    &view,
                    format,
                    (width, height),
                    procraft_engine::profile::Profile::Srgb,
                    started,
                );
                gpu.flush();
                s.undo();
                s.shared.lock().draw_frame(
                    &view,
                    format,
                    (width, height),
                    procraft_engine::profile::Profile::Srgb,
                    started,
                );
                gpu.flush();
                println!("{name} {size}: {each:.2} ms a frame");
            }
        }
    }
}
