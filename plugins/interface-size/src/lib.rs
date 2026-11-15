//! Interface size: the whole interface a little bigger or smaller, for a screen it doesn't suit, and
//! on a phone kept clear of the status bar and the camera. It's a plugin, not part of the app:
//! `tools/pack_plugin.py` makes a `.procraft-plugin` file of it.
//!
//! The app asks how big to show and draws at that number, and whether to keep clear, and lays the
//! interface out inside the system's bars and the screen's cutouts if so. The slider, its range,
//! the rule that fits a bigger size to the window and the switch are all here. The interface is
//! written down at the top of `crates/core/src/plugin_run.rs`.

#![cfg_attr(not(test), no_std)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The steps it moves in as a window gets smaller, percent, so a window dragged smaller doesn't lay
/// everything out again at each pixel.
const STEP: f32 = 5.0;

/// What a bigger size must leave the interface (points): the smallest window the desktop opens,
/// which every panel fits. A window with less shows at 100%.
const LEAST_WIDTH: f32 = 640.0;
const LEAST_HEIGHT: f32 = 480.0;

/// The slider, as an `f32`'s bits, and the switch; the app makes its calls one at a time, so this
/// is only to need no `unsafe`.
static CHOSEN: AtomicU32 = AtomicU32::new(1.0_f32.to_bits());
static CLEAR: AtomicBool = AtomicBool::new(false);

/// As chosen, but a bigger size only as far as it leaves the interface the room it needs, on a step.
fn fitted(chosen: f32, width: f32, height: f32) -> f32 {
    let room = (width / LEAST_WIDTH).min(height / LEAST_HEIGHT) * 100.0;
    let steps = (room / STEP) as i32 as f32;
    chosen.min((steps * STEP / 100.0).max(1.0))
}

/// A control's value, by its place in the manifest: the slider, then the switch (0 or 1).
#[unsafe(no_mangle)]
pub extern "C" fn procraft_control(index: i32, value: f32) {
    match index {
        0 => CHOSEN.store(value.to_bits(), Ordering::Relaxed),
        1 => CLEAR.store(value != 0.0, Ordering::Relaxed),
        _ => {}
    }
}

/// How big the interface shows in a window `width` × `height` points at 100%.
#[unsafe(no_mangle)]
pub extern "C" fn procraft_interface_scale(width: f32, height: f32) -> f32 {
    fitted(f32::from_bits(CHOSEN.load(Ordering::Relaxed)), width, height)
}

/// Whether the interface keeps clear of the status bar and the screen's cutouts (not 0).
#[unsafe(no_mangle)]
pub extern "C" fn procraft_keep_clear() -> i32 {
    i32::from(CLEAR.load(Ordering::Relaxed))
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[cfg(test)]
mod tests {
    use super::{fitted, procraft_control, procraft_interface_scale, procraft_keep_clear};

    #[test]
    fn the_switch_keeps_clear_and_leaves_the_size_alone() {
        procraft_control(0, 0.8);
        assert_eq!(procraft_keep_clear(), 0);
        procraft_control(1, 1.0);
        assert_eq!(procraft_keep_clear(), 1);
        assert_eq!(procraft_interface_scale(1440.0, 900.0), 0.8);
        procraft_control(1, 0.0);
        assert_eq!(procraft_keep_clear(), 0);
        // a control it doesn't have changes nothing
        procraft_control(2, 1.0);
        assert_eq!((procraft_keep_clear(), procraft_interface_scale(1440.0, 900.0)), (0, 0.8));
    }

    #[test]
    fn a_bigger_size_goes_no_further_than_the_window_has_room() {
        // room to spare: as chosen
        assert_eq!(fitted(1.25, 1440.0, 900.0), 1.25);
        assert_eq!(fitted(1.25, 800.0, 600.0), 1.25);
        // the smallest window keeps the interface it always had
        assert_eq!(fitted(1.25, 640.0, 480.0), 1.0);
        // the shorter side decides, by steps
        assert_eq!(fitted(1.25, 1440.0, 540.0), 1.1);
        assert_eq!(fitted(1.25, 710.0, 900.0), 1.1);
        // a window smaller than that never goes below 100%
        assert_eq!(fitted(1.25, 500.0, 400.0), 1.0);
        // and a smaller size is as chosen, whatever the window
        assert_eq!(fitted(0.75, 500.0, 400.0), 0.75);
        // a window not known yet leaves it alone
        assert_eq!(fitted(0.9, 0.0, 0.0), 0.9);
        assert_eq!(fitted(1.25, f32::NAN, f32::NAN), 1.0);
    }
}
