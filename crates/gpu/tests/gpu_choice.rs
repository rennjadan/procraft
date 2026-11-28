//! The GPU a device is made on, on a computer of a graphics card and an integrated GPU, by the
//! card's memory as the system tells it. (A test binary of its own: what the system tells is the
//! whole process's.)
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use procraft_gpu::{Gpu, GpuMemory, wgpu};

/// This computer's graphics card and recent integrated GPU on the backend devices are first made
/// from, if it has both.
fn card_and_integrated() -> Option<(wgpu::AdapterInfo, wgpu::AdapterInfo)> {
    let backend = *Gpu::backends().first()?;
    let seen = Gpu::adapters(backend);
    let card = seen.iter().find(|a| a.device_type == wgpu::DeviceType::DiscreteGpu)?.clone();
    let integrated = seen.iter().find(|a| a.device_type == wgpu::DeviceType::IntegratedGpu)?.clone();
    Some((card, integrated))
}

/// The card told to have `memory` of its own, the integrated GPU 128 MB: the GPU a device is made
/// on, and whether it counts as a weak one.
fn made_on(card: &wgpu::AdapterInfo, integrated: &wgpu::AdapterInfo, memory: u64) -> (String, bool) {
    procraft_gpu::know_gpu_memory(vec![
        GpuMemory { vendor: card.vendor, device: card.device, dedicated: memory },
        GpuMemory { vendor: integrated.vendor, device: integrated.device, dedicated: 128 << 20 },
    ]);
    let gpu = Gpu::headless().unwrap();
    (gpu.adapter.get_info().name, gpu.is_weak())
}

/// A graphics card with a GT 710's 1 GB of its own gives way to the integrated GPU: the device is
/// made on it, and it draws a layer. With a card's 8 GB, or picked in the settings, the card is
/// drawn with; picked with 1 GB, it counts as a weak GPU. (On this PC: an RTX 5060 Laptop GPU and
/// an Intel Graphics; skipped where there aren't both, or `WGPU_POWER_PREF` chooses.)
#[test]
fn a_card_with_an_entry_level_card_s_memory_gives_way_to_the_integrated_gpu() {
    if std::env::var_os("WGPU_POWER_PREF").is_some() || std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    let Some((card, integrated)) = card_and_integrated() else {
        println!("no graphics card and integrated GPU both here: nothing to choose");
        return;
    };
    const GB: u64 = 1 << 30;
    let weak_card = made_on(&card, &integrated, GB);
    let capable_card = made_on(&card, &integrated, 8 * GB);
    procraft_gpu::prefer_gpu(Some((card.vendor, card.device)));
    let picked = made_on(&card, &integrated, GB);
    procraft_gpu::prefer_gpu(None);
    println!("card {}, integrated {}", card.name, integrated.name);
    println!("1 GB card: {weak_card:?}; 8 GB card: {capable_card:?}; 1 GB card picked: {picked:?}");
    assert_eq!(weak_card, (integrated.name.clone(), false));
    assert_eq!(capable_card, (card.name.clone(), false));
    assert_eq!(picked, (card.name.clone(), true));
    // the integrated GPU's device paints: a stamp over the whole layer, read back
    procraft_gpu::know_gpu_memory(vec![GpuMemory { vendor: card.vendor, device: card.device, dedicated: GB }]);
    let gpu = Gpu::headless().unwrap();
    assert_eq!(gpu.adapter.get_info().name, integrated.name);
    let mut doc = procraft_engine::document::Document::new(64, 64);
    let mut canvas = procraft_gpu::Canvas::new(&gpu, &doc).unwrap();
    let layer = doc.current();
    let style = procraft_gpu::StrokeStyle::new(procraft_gpu::StrokeTool::Paint, [1.0, 0.5, 0.0], 1.0);
    canvas.begin_stroke(layer, style);
    let middle = procraft_engine::geom::Vec2::new(32.0, 32.0);
    canvas.add_stamps(&[procraft_engine::stroke::Stamp::round(middle, 100.0, 1.0)]);
    canvas.end_stroke();
    canvas.render(&gpu, &mut doc, None);
    let back = canvas.read_layer(&gpu, layer).unwrap();
    let at = (32 * 64 + 32) * 4;
    assert_eq!(back.get(at..at + 4).map(|p| (p[0], p[3])), Some((255, 255)), "painted on {}", integrated.name);
    assert_eq!(gpu.errors(), 0);
}
