//! Which of a computer's GPUs the app draws with, where it has several: a graphics card's, unless
//! that card is an old or entry-level one and the computer's own (integrated) GPU is a recent one;
//! or the one the person picked (the desktop's settings).
//!
//! Painting reads and writes whole layers, so what counts most is how fast a GPU's memory is. An
//! entry-level card of a few years ago has little of it, and slow: a GeForce GT 710 has 1 or 2 GB
//! of DDR3 at 14 GB/s, where the Intel UHD Graphics 630 in the same PC shares the computer's
//! dual-channel DDR4 at about 40. Taken for the faster as a graphics card (as wgpu takes any), a
//! GT 710 drew a 13-layer artwork at 100 to 900 ms a frame.

use std::sync::Mutex;

/// A GPU's own memory as the system tells it (Windows' DXGI), by its PCI vendor and device: wgpu
/// doesn't say how much a GPU has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuMemory {
    pub vendor: u32,
    pub device: u32,
    /// Its own memory (a graphics card's), bytes; an integrated GPU's is a small part of the
    /// computer's, set aside for it.
    pub dedicated: u64,
}

/// The GPUs' memory as the system told it (see [`know_gpu_memory`]).
static MEMORY: Mutex<Vec<GpuMemory>> = Mutex::new(Vec::new());
/// The GPU the person picked (vendor, device), if they did.
static PREFERRED: Mutex<Option<(u32, u32)>> = Mutex::new(None);

/// Tells the choice how much memory of its own each GPU has (see [`GpuMemory`]), as the system
/// says: before the device is made.
pub fn know_gpu_memory(gpus: Vec<GpuMemory>) {
    *MEMORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = gpus;
}

/// The memory of its own the GPU `vendor`, `device` has, as the system told it (see
/// [`know_gpu_memory`]); none where it didn't.
pub fn gpu_memory(vendor: u32, device: u32) -> Option<u64> {
    if vendor == 0 && device == 0 {
        return None;
    }
    let known = MEMORY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    known.iter().find(|g| g.vendor == vendor && g.device == device).map(|g| g.dedicated)
}

/// Draws with the GPU `vendor`, `device` wherever it is one of those to choose from (the person's
/// pick); none: as [`choose_gpu`] finds best.
pub fn prefer_gpu(gpu: Option<(u32, u32)>) {
    *PREFERRED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = gpu;
}

/// The GPU the person picked, if they did (see [`prefer_gpu`]).
pub fn preferred_gpu() -> Option<(u32, u32)> {
    *PREFERRED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the choice goes by, of a GPU.
#[derive(Clone, Debug)]
pub(crate) struct Candidate<'a> {
    pub(crate) name: &'a str,
    pub(crate) vendor: u32,
    pub(crate) device: u32,
    pub(crate) device_type: wgpu::DeviceType,
    /// Its own memory, bytes, where the system says.
    pub(crate) memory: Option<u64>,
}

impl<'a> Candidate<'a> {
    pub(crate) fn of(info: &'a wgpu::AdapterInfo) -> Candidate<'a> {
        Candidate {
            name: &info.name,
            vendor: info.vendor,
            device: info.device,
            device_type: info.device_type,
            memory: gpu_memory(info.vendor, info.device),
        }
    }
}

/// Which of `gpus` (as wgpu lists them) the app draws with: the person's pick where it is one of
/// them (see [`prefer_gpu`]), else as [`choose`] finds best; none if there are none.
pub fn choose_gpu(gpus: &[wgpu::AdapterInfo]) -> Option<usize> {
    let candidates: Vec<Candidate<'_>> = gpus.iter().map(Candidate::of).collect();
    choose(&candidates, preferred_gpu())
}

/// Which of `gpus` [`choose`] finds best, whatever the person picked: what "Automatic" takes.
pub fn best_gpu(gpus: &[wgpu::AdapterInfo]) -> Option<usize> {
    let candidates: Vec<Candidate<'_>> = gpus.iter().map(Candidate::of).collect();
    choose(&candidates, None)
}

/// The GPUs the window's device was chosen from (those that could show on it), not the software
/// one: what the person can pick from.
static TO_CHOOSE: Mutex<Vec<wgpu::AdapterInfo>> = Mutex::new(Vec::new());

/// Keeps `gpus` as those the window's device was chosen from (see [`gpus_to_choose`]).
pub(crate) fn keep_to_choose(gpus: &[wgpu::AdapterInfo]) {
    let real = gpus.iter().filter(|g| g.device_type != wgpu::DeviceType::Cpu).cloned().collect();
    *TO_CHOOSE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = real;
}

/// The GPUs the window's device was chosen from, not the software one: the person can pick one of
/// them to draw with (from the next start: see [`prefer_gpu`]).
pub fn gpus_to_choose() -> Vec<wgpu::AdapterInfo> {
    TO_CHOOSE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Why [`choose_gpu`] took the GPU it did over the graphics card wgpu would have taken, for the log;
/// none where it took that one.
pub fn passed_over(gpus: &[wgpu::AdapterInfo], chosen: usize) -> Option<String> {
    let candidates: Vec<Candidate<'_>> = gpus.iter().map(Candidate::of).collect();
    let card = candidates.iter().position(|g| g.device_type == wgpu::DeviceType::DiscreteGpu)?;
    if card == chosen {
        return None;
    }
    let (card, took) = (candidates.get(card)?, candidates.get(chosen)?);
    if preferred_gpu() == Some((took.vendor, took.device)) {
        return Some(format!("{} as picked in the settings, not {}", took.name, card.name));
    }
    let memory = card.memory.map(|m| format!(", {} MB of its own", m >> 20)).unwrap_or_default();
    Some(format!("{} over {} (an old or entry-level graphics card{memory})", took.name, card.name))
}

/// Which of `gpus` is best to draw with: `preferred` (vendor, device) where one is it; else a
/// graphics card, unless it is a weak one (see [`weak_card`]) and an integrated GPU is a recent one
/// (see [`old_integrated`]), which then is taken; else an integrated GPU, then any other, a
/// software one last. The first listed of those alike.
pub(crate) fn choose(gpus: &[Candidate<'_>], preferred: Option<(u32, u32)>) -> Option<usize> {
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, Other, VirtualGpu};
    if let Some(picked) = preferred
        && let Some(at) = gpus.iter().position(|g| (g.vendor, g.device) == picked)
    {
        return Some(at);
    }
    let recent_integrated = gpus.iter().any(|g| g.device_type == IntegratedGpu && !old_integrated(g.name));
    let rank = |g: &Candidate<'_>| match g.device_type {
        DiscreteGpu if !(recent_integrated && weak_card(g.name, g.memory)) => 0,
        IntegratedGpu => 1,
        DiscreteGpu => 2,
        VirtualGpu => 3,
        Other => 4,
        Cpu => 5,
    };
    gpus.iter().enumerate().min_by_key(|(i, g)| (rank(g), *i)).map(|(i, _)| i)
}

/// A graphics card with less of its own memory than this is an entry-level one, however it is
/// named: every card worth drawing with since 2016 has 2 GB or more.
const WEAK_CARD_MEMORY: u64 = 1536 << 20;

/// Whether a graphics card named `name`, with `memory` of its own (where the system says), is an
/// old or entry-level one, its memory slow (64-bit DDR3 or DDR4, 10 to 30 GB/s): NVIDIA's GeForce
/// GT 200 to 700s, GeForce 210 to 405, its laptops' 700M to 900M and MX110 and MX130, and NVS;
/// AMD's Radeon HD below their x700s, R5, R7 240 and 250, and the laptops' Radeon 520 to 630 (not
/// RX); and any card with less than [`WEAK_CARD_MEMORY`]. A GT 1030's (2017) GDDR5 is three times
/// a GT 710's, so it isn't one, nor is any card named otherwise.
pub(crate) fn weak_card(name: &str, memory: Option<u64>) -> bool {
    if memory.is_some_and(|m| m < WEAK_CARD_MEMORY) {
        return true;
    }
    let name = name.to_ascii_lowercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // the word after `word`, if there is one
    let after = |word: &str| words.iter().position(|w| *w == word).and_then(|i| words.get(i + 1)).copied();
    let number = |w: &str| -> Option<u32> { w.parse().ok() };
    if name.contains("nvidia") || name.contains("geforce") || name.contains("nvs") || name.contains("quadro") {
        if words.contains(&"nvs") {
            return true;
        }
        // GeForce GT 210 to GT 740 (GT 1010 and 1030 aren't counted)
        if let Some(n) = after("gt").and_then(number) {
            return (200..1000).contains(&n);
        }
        // the MX110 and MX130 (laptops, 2017: a GT 710's chip or a 920M's)
        if words.iter().any(|w| matches!(*w, "mx110" | "mx130")) {
            return true;
        }
        // GeForce 210 to 405, and the laptops' 700M to 900M ("GeForce 920M", "940MX")
        if let Some(w) = after("geforce") {
            if let Some(n) = number(w) {
                return (200..=405).contains(&n);
            }
            if let Some(digits) = w.strip_suffix("mx").or_else(|| w.strip_suffix('m')) {
                return number(digits).is_some_and(|n| (700..1000).contains(&n));
            }
        }
        return false;
    }
    if name.contains("radeon") {
        // never an RX (the RX 550 has GDDR5 at 112 GB/s)
        if words.contains(&"rx") || words.contains(&"pro") || words.contains(&"vega") {
            return false;
        }
        // Radeon HD 5450 to 8570: those below a series' x700 (HD 7750 and up have GDDR5)
        if let Some(n) = after("hd").and_then(number) {
            return (1000..10000).contains(&n) && n % 1000 < 700;
        }
        // Radeon R5 (any), R7 240 and 250 and the laptops' R7 M2xx to M4xx
        match after("radeon") {
            Some("r5") => return true,
            Some("r7") => {
                return words
                    .iter()
                    .skip_while(|w| **w != "r7")
                    .nth(1)
                    .is_some_and(|w| matches!(*w, "240" | "250") || w.starts_with('m'));
            }
            // the laptops' Radeon 520 to 630 (2017 to 2019, 64-bit memory)
            Some(w) => return number(w).is_some_and(|n| (520..=630).contains(&n)),
            None => return false,
        }
    }
    false
}

/// Whether an integrated GPU named `name` is an old one, slower than any graphics card: Intel's HD
/// Graphics before their 500s (2015's sixth generation: HD 4000 to 6000, and plain "HD Graphics" as
/// Atoms and old Celerons and Pentiums have), and AMD's APUs before Vega (Radeon R2 to R7
/// Graphics). Intel's HD 500s and 600s, UHD, Iris, Xe, Arc and plain "Graphics", AMD's Vega,
/// "Radeon Graphics" and 600M to 800M, and any other's (Apple's, Qualcomm's) are recent ones.
pub(crate) fn old_integrated(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if name.contains("intel") {
        if name.contains("uhd") || name.contains("iris") || name.contains("arc") || words.contains(&"xe") {
            return false;
        }
        let Some(at) = words.windows(2).position(|w| w == ["hd", "graphics"]) else { return false };
        // the number after "HD Graphics" (some are "P530"); none: an Atom's or an old Celeron's
        let number = words.get(at + 2).and_then(|w| w.trim_start_matches('p').parse::<u32>().ok());
        return number.is_none_or(|n| !(500..1000).contains(&n));
    }
    if name.contains("radeon") {
        return words.iter().any(|w| matches!(*w, "r2" | "r3" | "r4" | "r5" | "r6" | "r7"));
    }
    false
}

#[cfg(test)]
mod tests {
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu};

    use super::{Candidate, choose, old_integrated, weak_card};

    const GB: u64 = 1 << 30;

    fn gpu(name: &str, device_type: wgpu::DeviceType, memory: Option<u64>) -> Candidate<'_> {
        // a vendor and device from the name, so each is told apart
        let device = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
        Candidate { name, vendor: 0x10de, device, device_type, memory }
    }

    /// The PC in the report: a GeForce GT 710 (2014, 1 or 2 GB of DDR3) beside an Intel UHD
    /// Graphics 630. wgpu took the card; the integrated GPU is taken, whatever the system says of
    /// the card's memory (nothing on Linux, 2 GB for the larger model), on Vulkan's names and
    /// Direct3D 12's alike. The card is taken where it is the person's pick, and where it is
    /// alone, or the only GPU but the software one.
    #[test]
    fn an_entry_level_card_gives_way_to_a_recent_integrated_gpu() {
        for (card, uhd) in
            [("GeForce GT 710", "Intel(R) UHD Graphics 630"), ("NVIDIA GeForce GT 710", "Intel(R) UHD Graphics 630")]
        {
            for memory in [None, Some(GB), Some(2 * GB)] {
                let gpus = [gpu(card, DiscreteGpu, memory), gpu(uhd, IntegratedGpu, Some(128 << 20))];
                assert_eq!(choose(&gpus, None), Some(1), "{card}, {memory:?}");
                // listed the other way round
                let gpus = [gpu(uhd, IntegratedGpu, Some(128 << 20)), gpu(card, DiscreteGpu, memory)];
                assert_eq!(choose(&gpus, None), Some(0), "{card}, {memory:?}");
                // the person's pick
                let picked = gpus.get(1).map(|g| (g.vendor, g.device));
                assert_eq!(choose(&gpus, picked), Some(1));
            }
            let software = gpu("Microsoft Basic Render Driver", Cpu, None);
            assert_eq!(choose(&[software.clone(), gpu(card, DiscreteGpu, None)], None), Some(1));
            assert_eq!(choose(&[gpu(card, DiscreteGpu, None)], None), Some(0));
        }
        // a pick that isn't there counts for nothing
        let gpus = [gpu("GeForce GT 710", DiscreteGpu, None), gpu("Intel(R) UHD Graphics 630", IntegratedGpu, None)];
        assert_eq!(choose(&gpus, Some((0x1002, 0x1234))), Some(1));
        assert_eq!(choose(&[], None), None);
    }

    /// A card worth drawing with is taken over any integrated GPU, as wgpu takes it, and an old or
    /// entry-level card over an old integrated GPU (an HD 4600 is no faster than a GT 710).
    #[test]
    fn a_capable_card_or_an_old_integrated_gpu_keeps_the_card() {
        let cases = [
            ("NVIDIA GeForce RTX 5060 Laptop GPU", Some(8 * GB), "Intel(R) Graphics"),
            ("NVIDIA GeForce GTX 1050 Ti", Some(4 * GB), "Intel(R) UHD Graphics 630"),
            ("NVIDIA GeForce GT 1030", Some(2 * GB), "Intel(R) UHD Graphics 630"),
            ("NVIDIA GeForce MX150", Some(2 * GB), "Intel(R) UHD Graphics 620"),
            ("GeForce GTX 750 Ti", None, "Intel(R) HD Graphics 530"),
            ("AMD Radeon RX 550", Some(4 * GB), "Intel(R) UHD Graphics 630"),
            ("AMD Radeon RX 6600", Some(8 * GB), "AMD Radeon(TM) Graphics"),
            ("Intel(R) Arc(TM) A380 Graphics", Some(6 * GB), "Intel(R) UHD Graphics 770"),
            // an entry-level card beside an old integrated GPU
            ("GeForce GT 710", Some(GB), "Intel(R) HD Graphics 4600"),
            ("NVIDIA GeForce 920M", Some(2 * GB), "Intel(R) HD Graphics 5500"),
            ("AMD Radeon R5 M330", None, "Intel(R) HD Graphics"),
        ];
        for (card, memory, integrated) in cases {
            let gpus = [gpu(integrated, IntegratedGpu, Some(128 << 20)), gpu(card, DiscreteGpu, memory)];
            assert_eq!(choose(&gpus, None), Some(1), "{card} beside {integrated}");
        }
    }

    /// The old and entry-level cards, by name or by their memory, and those that aren't.
    #[test]
    fn entry_level_cards_are_told_by_name_or_memory() {
        let weak = [
            "GeForce GT 710",
            "NVIDIA GeForce GT 730",
            "NVIDIA GeForce GT 740",
            "NVIDIA GeForce GT 630",
            "GeForce GT 240",
            "NVIDIA GeForce 210",
            "NVIDIA GeForce 405",
            "NVIDIA GeForce 920M",
            "NVIDIA GeForce 940MX",
            "NVIDIA GeForce 710M",
            "NVIDIA GeForce MX110",
            "NVIDIA GeForce MX130",
            "NVIDIA NVS 315",
            "NVS 510",
            "AMD Radeon HD 5450",
            "AMD Radeon HD 6450",
            "AMD Radeon HD 8570",
            "AMD Radeon R5 230",
            "AMD Radeon R5 M330",
            "AMD Radeon R7 240",
            "AMD Radeon R7 M260",
            "AMD Radeon 520",
            "AMD Radeon 625",
        ];
        for name in weak {
            assert!(weak_card(name, None), "{name}");
        }
        let capable = [
            "NVIDIA GeForce GT 1030",
            "NVIDIA GeForce GTX 750 Ti",
            "NVIDIA GeForce GTX 1650",
            "NVIDIA GeForce MX150",
            "NVIDIA GeForce MX450",
            "NVIDIA GeForce RTX 5060 Laptop GPU",
            "NVIDIA GeForce GTX 980M",
            "Quadro P1000",
            "AMD Radeon HD 7750",
            "AMD Radeon HD 7970",
            "AMD Radeon R7 370",
            "AMD Radeon R9 290",
            "AMD Radeon RX 550",
            "AMD Radeon RX 7600",
            "AMD Radeon Pro 555X",
            "Radeon RX Vega 56",
            "Intel(R) Arc(TM) A750 Graphics",
            "Apple M2",
        ];
        for name in capable {
            assert!(!weak_card(name, None), "{name}");
            assert!(!weak_card(name, Some(2 * GB)), "{name}");
            // less than 1.5 GB of its own: entry-level whatever its name
            assert!(weak_card(name, Some(GB)), "{name}");
        }
    }

    /// The old integrated GPUs, and the recent ones.
    #[test]
    fn old_integrated_gpus_are_told_from_recent_ones() {
        let old = [
            "Intel(R) HD Graphics 4000",
            "Intel(R) HD Graphics 4600",
            "Intel(R) HD Graphics 5500",
            "Intel(R) HD Graphics 6000",
            "Intel(R) HD Graphics",
            "AMD Radeon R7 Graphics",
            "AMD Radeon(TM) R5 Graphics",
        ];
        for name in old {
            assert!(old_integrated(name), "{name}");
        }
        let recent = [
            "Intel(R) HD Graphics 520",
            "Intel(R) HD Graphics P530",
            "Intel(R) HD Graphics 630",
            "Intel(R) UHD Graphics 630",
            "Intel(R) UHD Graphics 620",
            "Intel(R) Iris(R) Plus Graphics",
            "Intel(R) Iris(R) Xe Graphics",
            "Intel(R) Arc(TM) Graphics",
            "Intel(R) Graphics",
            "AMD Radeon(TM) Graphics",
            "AMD Radeon(TM) Vega 8 Graphics",
            "AMD Radeon 780M Graphics",
            "Qualcomm(R) Adreno(TM) X1-85 GPU",
            "Apple M1",
        ];
        for name in recent {
            assert!(!old_integrated(name), "{name}");
        }
    }
}
