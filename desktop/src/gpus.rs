//! The computer's GPUs as the system tells them, before any device is made: how much memory of its
//! own each has (Windows' DXGI; wgpu doesn't say), for choosing which to draw with and how much of
//! it the layers may take. Listing them loads no driver's graphics API, so it is safe at each start.

/// A GPU as the system tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemGpu {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    /// Its own memory (a graphics card's), bytes; an integrated GPU's is a small part of the
    /// computer's set aside for it.
    pub dedicated: u64,
    /// The computer's memory it may use besides, bytes.
    pub shared: u64,
}

/// The GPUs the system lists (not its software one), once a run; none where it can't say.
pub fn system_gpus() -> &'static [SystemGpu] {
    static GPUS: std::sync::OnceLock<Vec<SystemGpu>> = std::sync::OnceLock::new();
    GPUS.get_or_init(|| {
        let started = std::time::Instant::now();
        let gpus = listed();
        for gpu in &gpus {
            log::info!(
                "the system's GPU: {} (vendor {:#06x}, device {:#06x}), {} MB of its own, {} MB of the computer's",
                gpu.name,
                gpu.vendor,
                gpu.device,
                gpu.dedicated >> 20,
                gpu.shared >> 20
            );
        }
        if !gpus.is_empty() {
            log::info!("the system's GPUs listed in {} ms", started.elapsed().as_millis());
        }
        gpus
    })
}

/// Tells the GPU choice how much memory of its own each GPU has (see [`system_gpus`]): before the
/// window's device is made.
pub fn tell_gpu_memory() {
    let known = system_gpus()
        .iter()
        .map(|g| procraft_gpu::GpuMemory { vendor: g.vendor, device: g.device, dedicated: g.dedicated })
        .collect();
    procraft_gpu::know_gpu_memory(known);
}

/// The GPUs DXGI lists, as [`system_gpus`] gives them.
#[cfg(windows)]
#[allow(unsafe_code)]
fn listed() -> Vec<SystemGpu> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
    // SAFETY: a factory asked for by its interface, which the call fills in or fails on
    let factory = match unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        Ok(f) => f,
        Err(e) => {
            log::warn!("the system's GPUs couldn't be listed: {e}");
            return Vec::new();
        }
    };
    let mut gpus = Vec::new();
    // (a computer has a few at most; the list ends with an error past the last)
    for i in 0..16 {
        // SAFETY: the factory is a live one; an index past the last fails (DXGI_ERROR_NOT_FOUND)
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        // SAFETY: the adapter is a live one, and the call fills in its description or fails
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(desc.Description.get(..end).unwrap_or_default()).trim().to_owned();
        gpus.push(SystemGpu {
            name,
            vendor: desc.VendorId,
            device: desc.DeviceId,
            dedicated: desc.DedicatedVideoMemory as u64,
            shared: desc.SharedSystemMemory as u64,
        });
    }
    gpus
}

/// Elsewhere the system isn't asked: the GPUs' memory is taken as unknown.
#[cfg(not(windows))]
fn listed() -> Vec<SystemGpu> {
    Vec::new()
}

/// What the layers' textures may take on a graphics card with `dedicated` memory of its own
/// (bytes), where the system says: half of it, 256 MiB to 2 GiB. A card's own memory runs out
/// before the layers' 2 GiB on one of 1 or 2 GB, and Windows then moves textures to and from the
/// computer's memory over the bus as they are drawn with: 100 to 900 ms frames on a 13-layer
/// artwork, ten seconds as the window came back to the front. Half leaves the rest for the
/// canvas's own textures, the window and the system; at 1280 MiB or less the canvas takes the
/// device for one of little memory (its history's older steps kept compressed off the GPU).
pub fn layers_on_card(dedicated: Option<u64>, most: u64) -> u64 {
    const LEAST: u64 = 256 << 20;
    dedicated.map_or(most, |d| (d / 2).clamp(LEAST, most))
}

#[cfg(test)]
mod tests {
    use super::layers_on_card;

    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;

    /// A card of 1 GB gives the layers 512 MiB, one of 2 GB 1 GiB (both taken for devices of little
    /// memory), 4 GB or more the 2 GiB they always had; unknown, 2 GiB as before.
    #[test]
    fn the_layers_take_half_a_cards_own_memory() {
        let most = 2 * GB;
        assert_eq!(layers_on_card(Some(GB), most), 512 * MB);
        assert_eq!(layers_on_card(Some(2 * GB), most), GB);
        assert_eq!(layers_on_card(Some(3 * GB), most), 1536 * MB);
        assert_eq!(layers_on_card(Some(4 * GB), most), most);
        assert_eq!(layers_on_card(Some(8 * GB), most), most);
        assert_eq!(layers_on_card(Some(256 * MB), most), 256 * MB);
        assert_eq!(layers_on_card(None, most), most);
    }

    /// On Windows the system lists this computer's GPUs, each with its vendor and a name.
    #[test]
    #[cfg(windows)]
    fn the_system_lists_the_gpus() {
        let gpus = super::listed();
        for gpu in &gpus {
            println!(
                "{} ({:#06x}:{:#06x}): {} MB own, {} MB shared",
                gpu.name,
                gpu.vendor,
                gpu.device,
                gpu.dedicated >> 20,
                gpu.shared >> 20
            );
            assert!(!gpu.name.is_empty());
            assert_ne!(gpu.vendor, 0);
        }
    }
}
