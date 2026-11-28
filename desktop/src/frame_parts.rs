//! Where a window event's time went, part by part, for the log when it is slow: "a frame took
//! 131 ms" alone couldn't tell a GPU the frames waited on (a window picture handed out late) from
//! the app's own work.

use std::cell::RefCell;
use std::time::{Duration, Instant};

thread_local! {
    /// The parts timed so far in the window event under way (the window's thread's own).
    static PARTS: RefCell<Vec<(&'static str, Duration)>> = const { RefCell::new(Vec::new()) };
}

/// The parts shorter than this aren't told.
const TOLD_FROM: Duration = Duration::from_millis(2);

/// A window event begins: the parts of the one before are let go.
pub fn begin() {
    PARTS.with(|p| p.borrow_mut().clear());
}

/// Part `name` of the window event under way took from `since` until now.
pub fn part(name: &'static str, since: Instant) {
    let took = since.elapsed();
    PARTS.with(|p| p.borrow_mut().push((name, took)));
}

/// The part of the window event under way that took longest, and how long.
pub fn longest() -> Option<(&'static str, Duration)> {
    PARTS.with(|p| p.borrow().iter().copied().max_by_key(|(_, took)| *took))
}

/// What the parts timed in the window event under way say, for the end of its line in the log
/// (" (the canvas 120 ms, ...)"); nothing where none took long.
pub fn said() -> String {
    PARTS.with(|p| told(&p.borrow()))
}

/// [`said`] for `parts`.
fn told(parts: &[(&'static str, Duration)]) -> String {
    let long: Vec<String> = parts
        .iter()
        .filter(|(_, took)| *took >= TOLD_FROM)
        .map(|(name, took)| format!("{name} {} ms", took.as_millis()))
        .collect();
    if long.is_empty() { String::new() } else { format!(" ({})", long.join(", ")) }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::told;

    /// A slow frame's line says which parts took long, in order, and leaves out those that didn't.
    #[test]
    fn a_slow_frame_says_which_parts_took_long() {
        let ms = Duration::from_millis;
        let parts = [
            ("the interface", ms(3)),
            ("the window's picture (the GPU's earlier frames)", ms(120)),
            ("the canvas", ms(1)),
            ("the interface drawn", ms(4)),
        ];
        assert_eq!(
            told(&parts),
            " (the interface 3 ms, the window's picture (the GPU's earlier frames) 120 ms, the interface drawn 4 ms)"
        );
        assert_eq!(told(&[("the canvas", ms(1))]), "");
        assert_eq!(told(&[]), "");
    }
}
