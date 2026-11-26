//! How far each library brush's stroke, as it shows while drawn, trails the pen: a straight stroke
//! at a steady speed and pressure, at a few input rates, under a pen and a finger, the app's own
//! smoothing off. Benches, run by hand with `--ignored --nocapture`.

use procraft_engine::stroke::Stamp;

use crate::tests::session;
use crate::{PenSample, PointerKind, State};

/// Where the stroke as shown reaches along x (canvas pixels): its stamps' middles, those still at
/// least half as wide and strong as the stroke's middle (an end taper thins the tip to the pen),
/// and its stamps' front edges (a brush spaced wide reaches the pen with a stamp's width). And
/// whether it shows anything past what it lays for good.
fn shown_reach(st: &mut State) -> Option<([f32; 3], bool)> {
    let canvas = st.canvas.as_ref()?;
    let builder = st.stroke.as_mut()?;
    let laid = builder.placed_stamps();
    let (tip, shows) = if canvas.shows_tip() && builder.shows_tip() {
        (builder.clone().tip_now(), true)
    } else if canvas.shows_tip() {
        let mut copy = (**builder).clone();
        let mut tip = copy.held_back();
        tip.extend(copy.preview_to_input(&[]));
        (tip, true)
    } else {
        (Vec::new(), false)
    };
    let strength = |s: &Stamp| s.radius * s.taper * s.alpha;
    let mut middle: Vec<f32> = laid.iter().chain(&tip).map(strength).filter(|v| *v > 0.0).collect();
    middle.sort_by(f32::total_cmp);
    let middle = middle.get(middle.len() / 2).copied().unwrap_or(0.0);
    let all = laid.iter().chain(&tip).filter(|s| s.alpha > 0.0 && s.radius > 0.0);
    let furthest = |of: &dyn Fn(&Stamp) -> Option<f32>| all.clone().filter_map(of).fold(f32::NEG_INFINITY, f32::max);
    let reach = furthest(&|s| Some(s.center.x));
    let strong = furthest(&|s| (strength(s) >= 0.5 * middle).then_some(s.center.x));
    let edge = furthest(&|s| Some(s.center.x + s.radius * s.taper));
    Some(([reach, strong, edge], shows))
}

/// Every library brush (or those `TRAIL_BRUSHES` names) drawn in a straight stroke at
/// `TRAIL_SPEED` screen pixels a second (2000), steady pressure 0.6, at each rate of `TRAIL_RATES`
/// (120,240,500) under a pen and a finger; a frame every 16.7 ms. Per brush, pointer and rate: how
/// far the stroke as shown (laid and tip) trails the pen over the frames after the first 150 ms,
/// mean and most (ms); the same counting only the stroke's full width, and its stamps' front edges;
/// and how far what it lays for good trails. With `TRAIL_UNSMOOTHED=1`, each brush's own StreamLine,
/// pressure smoothing, stabilisation and motion filtering are set to none first. Tab-separated, to
/// `TRAIL_OUT` too if set.
#[test]
#[ignore]
fn trail_by_brush() {
    use std::io::Write;
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let speed: f32 = env("TRAIL_SPEED", "2000").parse().unwrap_or(2000.0);
    let rates: Vec<f64> = env("TRAIL_RATES", "120,240,500").split(',').filter_map(|r| r.parse().ok()).collect();
    let wanted = env("TRAIL_BRUSHES", "");
    let wanted: Vec<&str> = wanted.split(',').filter(|n| !n.is_empty()).collect();
    let pointers: Vec<PointerKind> = match env("TRAIL_POINTERS", "pen,finger").as_str() {
        "pen" => vec![PointerKind::Stylus],
        "finger" => vec![PointerKind::Finger],
        _ => vec![PointerKind::Stylus, PointerKind::Finger],
    };
    let unsmoothed = env("TRAIL_UNSMOOTHED", "") == "1";
    let side = 1600u32;
    let Some(s) = session(side, side) else { return };
    s.set_input_settings(0.0, 0.0, 0.0, 0.0, true, Vec::new());
    let zoom = s.shared.lock().view.map_or(1.0, |v| v.zoom());
    let names: Vec<(u32, u32, String, String)> = {
        let st = s.shared.lock();
        st.library
            .iter()
            .enumerate()
            .flat_map(|(set, entry)| {
                entry
                    .brushes
                    .iter()
                    .enumerate()
                    .map(move |(i, b)| (set as u32, i as u32, entry.name.clone(), b.name.clone()))
            })
            .collect()
    };
    let mut out = std::env::var("TRAIL_OUT").ok().and_then(|p| std::fs::File::create(p).ok());
    let header = "set\tbrush\tpointer\trate\tshows\tdual\tstreamline\tpressure smoothing\tstabilization\tmotion\t\
                  trail mean\ttrail max\tfull-width mean\tfull-width max\tedge mean\tedge max\tlaid mean";
    println!("{header}");
    if let Some(f) = out.as_mut() {
        let _ = writeln!(f, "{header}");
    }
    for (set, index, set_name, name) in names {
        if !wanted.is_empty() && !wanted.contains(&name.as_str()) {
            continue;
        }
        if !s.select_brush(set, index) {
            continue;
        }
        if unsmoothed {
            s.studio_begin();
            for key in ["streamline", "streamline_pressure", "stabilization", "motion_filtering"] {
                s.set_studio_setting(key.to_owned(), 0.0);
            }
            s.studio_end(true);
        }
        for &pointer in &pointers {
            for &rate in &rates {
                let y = side as f32 / 2.0;
                let at = |k: usize| {
                    let t = k as f64 * 1000.0 / rate;
                    PenSample {
                        x: 100.0 + speed * (t / 1000.0) as f32,
                        y,
                        pressure: 0.6,
                        tilt: 0.0,
                        orientation: 0.0,
                        time_ms: t,
                        roll: 0.0,
                    }
                };
                let n = (0.6 * rate) as usize;
                s.stroke_begin(pointer, vec![at(0)]);
                // (each frame's: how far the shown stroke's middles, full width and edges trail)
                let (mut shown, mut laid) = (Vec::<[f32; 3]>::new(), Vec::new());
                let (mut shows, mut dual) = (false, false);
                let mut smoothing = [0.0f32; 4];
                let mut next_frame = 1000.0 / 60.0;
                for k in 1..n {
                    s.stroke_add(vec![at(k)]);
                    let t = k as f64 * 1000.0 / rate;
                    if t < next_frame {
                        continue;
                    }
                    next_frame += 1000.0 / 60.0;
                    if t < 150.0 {
                        continue;
                    }
                    let mut guard = s.shared.lock();
                    let st = &mut *guard;
                    let pen = at(k).x / zoom;
                    let to_ms = |px: f32| (pen - px) / (speed / zoom) * 1000.0;
                    dual = st.second.is_some();
                    if let Some(b) = st.stroke.as_ref() {
                        let brush = b.brush();
                        smoothing = [
                            brush.streamline,
                            brush.pressure_smoothing,
                            brush.setting("stabilization"),
                            brush.setting("motion_filtering"),
                        ];
                        if let Some(last) = b.placed_stamps().last() {
                            laid.push(to_ms(last.center.x));
                        }
                    }
                    if let Some((reach, tip)) = shown_reach(st) {
                        shows = tip;
                        shown.push(reach.map(to_ms));
                    }
                }
                s.stroke_end(vec![at(n)]);
                s.undo();
                let mean = |v: &mut dyn Iterator<Item = f32>| {
                    let (sum, count) = v.fold((0.0, 0), |(s, c), x| (s + x, c + 1));
                    if count == 0 { f32::NAN } else { sum / count as f32 }
                };
                let most = |v: &mut dyn Iterator<Item = f32>| v.fold(f32::NAN, f32::max);
                let mut row = format!(
                    "{set_name}\t{name}\t{}\t{rate}\t{shows}\t{dual}",
                    if pointer == PointerKind::Finger { "finger" } else { "pen" }
                );
                for v in smoothing {
                    row.push_str(&format!("\t{v:.3}"));
                }
                for i in 0..3 {
                    let of = |r: &[f32; 3]| r.get(i).copied().unwrap_or(f32::NAN);
                    row.push_str(&format!(
                        "\t{:.1}\t{:.1}",
                        mean(&mut shown.iter().map(of)),
                        most(&mut shown.iter().map(of))
                    ));
                }
                row.push_str(&format!("\t{:.1}", mean(&mut laid.iter().copied())));
                println!("{row}");
                if let Some(f) = out.as_mut() {
                    let _ = writeln!(f, "{row}");
                }
            }
        }
    }
}

/// As [`trail_by_brush`], but where the ink reaches on the canvas as each frame is drawn into a
/// window (the furthest column a band about the line differs from the blank canvas in), for the
/// brushes `TRAIL_BRUSHES` names, a pen at each rate of `TRAIL_RATES` with a frame every
/// `TRAIL_FRAME_MS` (16.7): mean and most (ms) over the frames after the first 150 ms.
#[test]
#[ignore]
fn trail_on_screen() {
    use procraft_gpu::wgpu;
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
    let speed: f32 = env("TRAIL_SPEED", "2000").parse().unwrap_or(2000.0);
    let rates: Vec<f64> = env("TRAIL_RATES", "120,240,500").split(',').filter_map(|r| r.parse().ok()).collect();
    let frame_ms: f64 = env("TRAIL_FRAME_MS", "16.7").parse().unwrap_or(16.7);
    let names = env("TRAIL_BRUSHES", "");
    let side = 1200u32;
    let Some(s) = session(side, side) else { return };
    s.set_input_settings(0.0, 0.0, 0.0, 0.0, true, Vec::new());
    let zoom = s.shared.lock().view.map_or(1.0, |v| v.zoom());
    let y = side as f32 / 2.0;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let window = s.shared.lock().gpu.as_ref().unwrap().device.create_texture(&wgpu::TextureDescriptor {
        label: Some("window"),
        size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = window.create_view(&Default::default());
    let started = std::time::Instant::now();
    let read = |s: &crate::CanvasSession| {
        let mut guard = s.shared.lock();
        let st = &mut *guard;
        st.draw_frame(&view, format, (side, side), procraft_engine::profile::Profile::Srgb, started);
        let (canvas, gpu) = (st.canvas.as_mut().unwrap(), st.gpu.as_ref().unwrap());
        canvas.read_composite(gpu).unwrap()
    };
    println!("brush\trate\ton-screen mean\ton-screen max");
    for name in names.split(',').filter(|n| !n.is_empty()) {
        let found = {
            let st = s.shared.lock();
            st.library.iter().enumerate().find_map(|(set, entry)| {
                let at = entry.brushes.iter().position(|b| b.name == name)?;
                Some((set as u32, at as u32))
            })
        };
        let Some((set, index)) = found else { continue };
        s.select_brush(set, index);
        for &rate in &rates {
            let at = |k: usize| {
                let t = k as f64 * 1000.0 / rate;
                PenSample {
                    x: 60.0 + speed * (t / 1000.0) as f32,
                    y,
                    pressure: 0.6,
                    tilt: 0.0,
                    orientation: 0.0,
                    time_ms: t,
                    roll: 0.0,
                }
            };
            let total = ((side as f32 - 120.0) / speed * rate as f32) as usize;
            let blank = read(&s);
            s.stroke_begin(PointerKind::Stylus, vec![at(0)]);
            let mut gaps = Vec::new();
            let mut next_frame = frame_ms;
            for k in 1..total {
                s.stroke_add(vec![at(k)]);
                let t = k as f64 * 1000.0 / rate;
                if t < next_frame {
                    continue;
                }
                next_frame += frame_ms;
                let pixels = read(&s);
                let row = |yy: u32| (yy * side) as usize * 4;
                let mut reach = 0u32;
                for yy in (y as u32 - 40)..(y as u32 + 40) {
                    for x in (reach + 1)..side {
                        let i = row(yy) + x as usize * 4;
                        if pixels[i..i + 3].iter().zip(&blank[i..i + 3]).any(|(&c, &d)| c.abs_diff(d) > 1) {
                            reach = x;
                        }
                    }
                }
                if t > 150.0 {
                    gaps.push((at(k).x / zoom - reach as f32) / (speed / zoom) * 1000.0);
                }
            }
            s.stroke_end(vec![at(total)]);
            let _ = read(&s);
            s.undo();
            let mean = gaps.iter().sum::<f32>() / gaps.len().max(1) as f32;
            let most = gaps.iter().copied().fold(f32::NAN, f32::max);
            println!("{name}\t{rate}\t{mean:.1}\t{most:.1}");
        }
    }
}

#[test]
fn a_brush_showing_its_tip_ends_the_line_at_the_pen_at_any_input_rate() {
    // stock brushes with StreamLine, pressure smoothing and stabilisation (laying their paint 75
    // to 200 ms behind the pen) and one of the Legacy set, drawn at 2000 px a second for 250 ms,
    // their samples handed over as a tablet's events bring them, 120 a second
    let brushes = [("Pens", "Shannon"), ("Inking", "Studio Pen"), ("Legacy", "Monoline")];
    let side = 1200u32;
    let Some(s) = session(side, side) else { return };
    s.set_input_settings(0.0, 0.0, 0.0, 0.0, true, Vec::new());
    let zoom = s.shared.lock().view.map_or(1.0, |v| v.zoom());
    let speed = 2000.0f32;
    for (set_name, name) in brushes {
        let found = {
            let st = s.shared.lock();
            st.library.iter().enumerate().find_map(|(set, entry)| {
                let at = entry.brushes.iter().position(|b| b.name == name).filter(|_| entry.name == set_name)?;
                Some((set as u32, at as u32))
            })
        };
        let Some((set, index)) = found else { panic!("{set_name}/{name} isn't in the library") };
        assert!(s.select_brush(set, index));
        for (pointer, rate) in [
            (PointerKind::Stylus, 120.0),
            (PointerKind::Stylus, 240.0),
            (PointerKind::Stylus, 500.0),
            (PointerKind::Finger, 240.0),
        ] {
            let at = |k: usize| {
                let t = k as f64 * 1000.0 / rate;
                PenSample {
                    x: 100.0 + speed * (t / 1000.0) as f32,
                    y: 600.0,
                    pressure: 0.6,
                    tilt: 0.0,
                    orientation: 0.0,
                    time_ms: t,
                    roll: 0.0,
                }
            };
            let event = |e: usize| ((e as f64 * rate / 120.0) as usize).max(1);
            s.stroke_begin(pointer, vec![at(0)]);
            let (mut shown, mut laid) = (Vec::new(), Vec::new());
            for e in 1..=30 {
                s.stroke_add((event(e - 1)..event(e)).map(at).collect());
                // looked at 150, 200 and 250 ms in
                if e % 6 != 0 || e < 18 {
                    continue;
                }
                let mut guard = s.shared.lock();
                let st = &mut *guard;
                let pen = at(event(e) - 1).x / zoom;
                let to_ms = |px: f32| (pen - px) / (speed / zoom) * 1000.0;
                if let Some(last) = st.stroke.as_ref().and_then(|b| b.placed_stamps().last().copied()) {
                    laid.push(to_ms(last.center.x));
                }
                let (reach, tip) = shown_reach(st).unwrap_or(([f32::NAN; 3], false));
                assert!(tip, "{name}: no tip");
                shown.push(to_ms(reach[0]));
            }
            s.stroke_end(vec![at(event(30))]);
            s.undo();
            // what is laid trails by the smoothing's time; the line shown reaches the pen (0.1 to
            // 0.5 ms behind, as measured)
            let most = shown.iter().copied().fold(f32::MIN, f32::max);
            let laid = laid.iter().copied().fold(f32::MAX, f32::min);
            assert!(shown.len() == 3 && most < 3.0, "{name} {pointer:?} at {rate} Hz: shown {shown:?} ms behind");
            assert!(laid > 40.0, "{name} {pointer:?} at {rate} Hz: laid only {laid} ms behind");
        }
    }
}
