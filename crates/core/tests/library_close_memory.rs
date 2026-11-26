#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
//! The brush library looked through and closed, as a phone of 4 GB froze and closed opening its
//! Gouache and Oils sets (discord): the stock library, a 2000 × 1200 canvas open on a GPU held to
//! wgpu's downlevel limits, the sample stroke of each brush of those two sets drawn as the
//! library's rows ask for them (530 × 162). Printed: what the GPU holds (its allocator's blocks)
//! over the canvas's own with the strokes drawn and once the library has closed, and the most the
//! canvas the strokes are drawn on kept of its work from one stroke to the next.
//!
//! Measured on an NVIDIA laptop GPU. Before: 68 MB held over the canvas's with the strokes drawn,
//! all of it still held with the library closed (until the system said it was short); the strokes'
//! canvas kept 52 MB of its work's buffers from one stroke to the next (16 MB of wet passes'
//! uniforms, 24 MB of a dual brush's carried floats, 8 MB of the rule's output). After: 4 MB once
//! the library closed, and 4 MB once 2.0 s had gone by with no stroke drawn, the library open; 11
//! MB of work's buffers kept at most, 2 MB of them lists of tiles. Bounds: what the library closed
//! may leave (readback buffers the device keeps, the allocator's rounding); what the strokes'
//! canvas may keep of its work, that measured and half again; and its lists alone, eight of 1 MB
//! (each past that is let go after its stroke).
//!
//! On the tablet (an Adreno 750, Vulkan) the GPU memory the system counts for the app was still 40
//! to 48 MB over the canvas's after the library closed, besides the 31 MB the interface's own
//! drawing caches took. Logged there, the allocator held 0 to 8 MB of it (the readback buffer
//! kept, in a block of its own); the rest, 33 to 40 MB, was the driver's own, taken the first time
//! any stamp was laid in the run, by the library's strokes or the canvas's: a first pen stroke on
//! the canvas took the same (allocator 80 MB, the driver 35 MB more), and whichever came second
//! took next to none of it (the library after a stroke 0.5 to 10 MB, a stroke after the library
//! 3.7 MB), the Pencils' ten strokes as much as two heavy sets' twenty; it stays with the device,
//! the canvas left and opened again. So it isn't the library's to give back. What is, the third
//! test below looks at as the tablet does it: the canvas drawn a frame at a time with the
//! library's frosted panel over it while its strokes are drawn, then closed, what is left printed
//! resource by resource.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use procraft_core::CanvasSession;
use procraft_engine::profile::Profile;
use procraft_gpu::{Gpu, wgpu};

const SIDE: (u32, u32) = (2000, 1200);
const MB: u64 = 1 << 20;

/// The most the library closed may leave held over the canvas's own, MB.
const CLOSED_MOST: u64 = 16;

/// The most the strokes' canvas may keep of its work's buffers between strokes, and of that in
/// lists of tiles, MB.
const WORK_KEPT_MOST: u64 = 16;
const LISTS_KEPT_MOST: u64 = 8;

/// The GPU's allocator's blocks, once what was let go is gone.
fn reserved(gpu: &Gpu) -> u64 {
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    gpu.device.generate_allocator_report().map_or(0, |r| r.total_reserved_bytes)
}

/// A session on a canvas the library's size, on a GPU of wgpu's downlevel limits (none without
/// one).
fn session() -> Option<(std::sync::Arc<CanvasSession>, Gpu)> {
    let gpu = match Gpu::headless_limited(wgpu::Limits::downlevel_defaults()) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return None;
        }
    };
    let s = CanvasSession::new(SIDE.0, SIDE.1);
    s.use_gpu(gpu.clone());
    s.show_in(SIDE.0, SIDE.1);
    Some((s, gpu))
}

#[test]
fn the_library_closed_gives_back_what_its_strokes_took() {
    let Some((s, gpu)) = session() else { return };
    let canvas_only = reserved(&gpu);
    let library = s.brush_library();
    let (mut drawn, mut brushes, mut work_most, mut lists_most) = (0, 0, 0, 0);
    let started = Instant::now();
    for name in ["Gouache", "Oils"] {
        let set = library.iter().find(|set| set.name == name).unwrap();
        for b in &set.brushes {
            brushes += 1;
            if s.brush_preview(b.set, b.index, 530, 162).is_some() {
                drawn += 1;
            }
            // what the canvas kept for the next stroke
            if let Some([_, work, lists]) = s.preview_canvas_bytes() {
                work_most = work_most.max(work);
                lists_most = lists_most.max(lists);
            }
        }
    }
    let kept = s.preview_canvas_bytes();
    let open = reserved(&gpu);
    s.let_go_of_previews();
    let closed = reserved(&gpu);
    let over = |v: u64| v.saturating_sub(canvas_only) / MB;
    println!(
        "{drawn} of {brushes} strokes in {:.1} s: the GPU {} MB over the canvas's with them drawn, {} MB once \
         the library closed; the strokes' canvas kept {} MB of its work's buffers at most, {} MB in lists \
         of tiles (its textures {} MB)",
        started.elapsed().as_secs_f32(),
        over(open),
        over(closed),
        work_most / MB,
        lists_most / MB,
        kept.map_or(0, |[textures, _, _]| textures / MB),
    );
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    assert!(drawn * 10 >= brushes * 9, "{drawn} of {brushes} drawn");
    assert!(s.preview_canvas_bytes().is_none(), "the strokes' canvas let go of");
    assert!(over(closed) <= CLOSED_MOST, "{} MB held with the library closed", over(closed));
    assert!(work_most <= WORK_KEPT_MOST * MB, "{} MB of work kept", work_most / MB);
    assert!(lists_most <= LISTS_KEPT_MOST * MB, "{} MB of lists kept", lists_most / MB);
    // another set's stroke drawn as the library opens again (those drawn come from what is kept,
    // drawing nothing), then the artwork left
    let b = &library.iter().find(|set| set.name == "Inks").unwrap().brushes[0];
    assert!(s.brush_preview(b.set, b.index, 530, 162).is_some());
    assert!(s.preview_canvas_bytes().is_some(), "a canvas made again for the next");
    s.release_canvas();
    assert!(s.preview_canvas_bytes().is_none(), "the strokes' canvas let go of with the canvas");
}

#[test]
fn the_strokes_canvas_goes_a_while_after_the_last() {
    let Some((s, gpu)) = session() else { return };
    let canvas_only = reserved(&gpu);
    let library = s.brush_library();
    let set = library.iter().find(|set| set.name == "Gouache").unwrap();
    for b in set.brushes.iter().take(3) {
        assert!(s.brush_preview(b.set, b.index, 530, 162).is_some());
    }
    let last = Instant::now();
    assert!(s.preview_canvas_bytes().is_some(), "kept for the next stroke");
    // gone once no stroke has been drawn for a while, by itself, the library still open
    while s.preview_canvas_bytes().is_some() {
        assert!(last.elapsed() < Duration::from_secs(30), "still kept after {:?}", last.elapsed());
        std::thread::sleep(Duration::from_millis(50));
    }
    let gone = last.elapsed();
    let left = reserved(&gpu).saturating_sub(canvas_only) / MB;
    println!(
        "the strokes' canvas let go of {:.1} s after the last stroke, {left} MB left over the canvas's",
        gone.as_secs_f32()
    );
    assert!(gone >= Duration::from_millis(1500), "let go of after {gone:?}: too soon for the next row's");
    assert!(left <= CLOSED_MOST, "{left} MB left");
}

/// The allocator's allocations by resource (its label), count and bytes.
fn by_resource(gpu: &Gpu) -> BTreeMap<String, (usize, u64)> {
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    let mut out: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for a in gpu.device.generate_allocator_report().map(|r| r.allocations).unwrap_or_default() {
        let e = out.entry(a.name).or_default();
        e.0 += 1;
        e.1 += a.size;
    }
    out
}

#[test]
fn the_library_over_a_canvas_being_drawn_leaves_little_when_it_closes() {
    let Some((s, gpu)) = session() else { return };
    // the window the canvas is drawn into, a frame at a time
    let window = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("window"),
        size: wgpu::Extent3d { width: SIDE.0, height: SIDE.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = window.create_view(&wgpu::TextureViewDescriptor::default());
    let began = Instant::now();
    let frames = |n: usize| {
        for _ in 0..n {
            let _ = s.draw(&view, wgpu::TextureFormat::Rgba8Unorm, SIDE, Profile::Srgb, began);
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        }
    };
    frames(10);
    let (canvas_only, before) = (reserved(&gpu), by_resource(&gpu));
    // the library open, its frosted panel over the canvas, its strokes drawn with frames between
    let panel = vec![1040.0, 90.0, 1960.0, 1160.0, 26.0, 1.0];
    s.set_glass(panel, Vec::new());
    frames(5);
    let library = s.brush_library();
    let mut drawn = 0;
    for name in ["Gouache", "Oils"] {
        let set = library.iter().find(|set| set.name == name).unwrap();
        for b in &set.brushes {
            if s.brush_preview(b.set, b.index, 530, 162).is_some() {
                drawn += 1;
            }
            frames(2);
        }
    }
    let open = reserved(&gpu);
    // closed
    s.set_glass(Vec::new(), Vec::new());
    s.let_go_of_previews();
    frames(5);
    let closed = reserved(&gpu);
    let after = by_resource(&gpu);
    let left: Vec<String> = after
        .iter()
        .filter_map(|(name, &(n, bytes))| {
            let (was_n, was) = before.get(name).copied().unwrap_or_default();
            (bytes > was)
                .then(|| format!("{name} x{} {:.2} MB", n.saturating_sub(was_n), (bytes - was) as f64 / MB as f64))
        })
        .collect();
    let over = |v: u64| v.saturating_sub(canvas_only) / MB;
    println!(
        "{drawn} strokes drawn over the canvas being drawn: the GPU {} MB over the canvas's with them, {} MB once closed; left over the canvas's, by resource: {}",
        over(open),
        over(closed),
        if left.is_empty() { "nothing".to_owned() } else { left.join(", ") }
    );
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    assert!(drawn >= 18, "{drawn} strokes drawn");
    assert!(s.preview_canvas_bytes().is_none(), "the strokes' canvas let go of");
    assert!(over(closed) <= CLOSED_MOST, "{} MB held with the library closed", over(closed));
    // nothing the strokes drew with but the readback buffer the device keeps and the panel's glass
    // (the canvas's own, made as the panel first showed)
    for (name, &(_, bytes)) in &after {
        let was = before.get(name).map_or(0, |&(_, b)| b);
        assert!(bytes <= was || ["readback", "backdrop"].contains(&name.as_str()), "{name} left: {left:?}");
    }
}
