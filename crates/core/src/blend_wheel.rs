//! The blend picker's modes: a list turning under a band fixed in its middle, the row in the band
//! the layer's mode. How the list moves is worked out here for every interface: how far it goes
//! once let go (slowing as a scrolled list does, straight onto a row), how far a pull past an end
//! stretches it, and how a tapped mode scrolls to the band.
//!
//! Distances are points, the list at 0 with its first mode in the band and one row `row` further
//! for each mode after; speeds are points a second, positive down the list.

use crate::scroll::{ScrollMotion, scroll_motion, scroll_motion_at, stretch, unstretch};

/// A tapped mode scrolls to the band over this long (seconds); meanwhile the list takes no
/// touches and the modes it passes don't show on the layer.
pub const BLEND_TAP_SECONDS: f32 = 0.3;

/// Where a moving list is: its `position` and `speed`, and whether it is `done` (at rest where it
/// stops, `position` exactly that).
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct BlendStep {
    pub position: f32,
    pub speed: f32,
    pub done: bool,
}

/// The farthest the list scrolls: the last of `rows` rows, `row` apart, in the band.
fn last(row: f32, rows: u32) -> f32 {
    row * rows.saturating_sub(1) as f32
}

/// `v`, or 0 if it isn't a number.
fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// A row's height as given, or a point if it isn't one.
fn row_or_one(row: f32) -> f32 {
    if row.is_finite() && row > 0.0 { row } else { 1.0 }
}

/// How the list of `rows` rows, `row` apart, moves on, let go at `from` going `speed`: slowing as
/// a scrolled list does, where it would come to rest moved to the nearest row, so it slows
/// straight onto that row in one motion (a release too slow to glide there springs there); let go
/// past an end, it springs back to it; flicked into an end, it overshoots and springs back.
#[uniffi::export]
pub fn blend_coast(from: f32, speed: f32, row: f32, rows: u32) -> ScrollMotion {
    let row = row_or_one(row);
    scroll_motion(from, speed, 0.0, last(row, rows), row)
}

/// Where the blend wheel is after the shared scroll motion has run for `seconds`.
#[uniffi::export]
pub fn blend_coast_at(coast: ScrollMotion, seconds: f32) -> BlendStep {
    let sample = scroll_motion_at(coast, seconds);
    BlendStep { position: sample.position, speed: sample.velocity, done: sample.done }
}

/// Pulled `past` points beyond an end, how far beyond it the list goes: about half the pull at
/// first, less and less the farther it goes, with no hard stop (`height` the list's height).
#[uniffi::export]
pub fn blend_stretch(past: f32, height: f32) -> f32 {
    stretch(past, height)
}

/// How far beyond an end a pull is that takes the list `shown` points beyond it: what
/// [`blend_stretch`] undoes.
#[uniffi::export]
pub fn blend_unstretch(shown: f32, height: f32) -> f32 {
    unstretch(shown, height)
}

/// Where the list of `rows` rows, `row` apart, `height` tall, shows when a finger has taken it to
/// `pull`: there between the ends, stretched past them.
#[uniffi::export]
pub fn blend_pulled(pull: f32, row: f32, rows: u32, height: f32) -> f32 {
    let pull = finite(pull);
    let edge = pull.clamp(0.0, last(row_or_one(row), rows));
    edge + blend_stretch(pull - edge, height)
}

/// Where a finger has to have taken the list for it to show at `position`: what [`blend_pulled`]
/// undoes.
#[uniffi::export]
pub fn blend_pull_for(position: f32, row: f32, rows: u32, height: f32) -> f32 {
    let position = finite(position);
    let edge = position.clamp(0.0, last(row_or_one(row), rows));
    edge + blend_unstretch(position - edge, height)
}

/// The row in the band with the list at `position`: the nearest, changing half way between two;
/// the first or the last while it is stretched past an end.
#[uniffi::export]
pub fn blend_centred(position: f32, row: f32, rows: u32) -> u32 {
    let i = (finite(position) / row_or_one(row) + 0.5).floor();
    i.clamp(0.0, rows.saturating_sub(1) as f32) as u32
}

/// A tap on row `tapped` with the list at `position`: where the list goes, the row's mode becoming
/// the layer's at once; none for the row in the band (a tap there changes nothing) or no row.
#[uniffi::export]
pub fn blend_tap(tapped: u32, position: f32, row: f32, rows: u32) -> Option<f32> {
    (tapped < rows && tapped != blend_centred(position, row, rows)).then(|| tapped as f32 * row_or_one(row))
}

/// Where a list sent from `from` to `to` by a tap is `seconds` later: easing in and out over
/// [`BLEND_TAP_SECONDS`] however far it goes.
#[uniffi::export]
pub fn blend_tap_at(from: f32, to: f32, seconds: f32) -> BlendStep {
    let u = finite(seconds) / BLEND_TAP_SECONDS;
    if u >= 1.0 {
        return BlendStep { position: to, speed: 0.0, done: true };
    }
    let u = u.max(0.0);
    BlendStep {
        position: from + (to - from) * u * u * (3.0 - 2.0 * u),
        speed: (to - from) * 6.0 * u * (1.0 - u) / BLEND_TAP_SECONDS,
        done: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The desktop app's rows and list.
    const ROW: f32 = 38.8;
    const HEIGHT: f32 = 349.0;
    const ROWS: u32 = 27;

    /// A coast followed at 120 frames a second: each frame's step, the last done.
    fn follow(coast: ScrollMotion) -> Vec<(f32, BlendStep)> {
        let mut steps = Vec::new();
        for frame in 0..2400 {
            let t = frame as f32 / 120.0;
            let step = blend_coast_at(coast, t);
            steps.push((t, step));
            if step.done {
                return steps;
            }
        }
        panic!("{coast:?} still moving after 20 s");
    }

    fn on_a_row(position: f32) -> bool {
        (position - (position / ROW).round() * ROW).abs() < 1e-3
    }

    #[test]
    fn a_flick_at_1000_points_a_second_goes_about_500_points_over_about_2_seconds() {
        let coast = blend_coast(0.0, 1000.0, ROW, ROWS);
        assert!(matches!(coast, ScrollMotion::Coast { .. }), "{coast:?}");
        let steps = follow(coast);
        let (_, end) = steps[steps.len() - 1];
        // 13 rows: 504.4, the nearest row to 499.5
        assert!((end.position - 500.0).abs() <= 50.0, "went {}", end.position);
        assert!(on_a_row(end.position));
        // all but still (under 10 points a second) after about 2 s
        let (slow, _) = steps.iter().find(|(_, s)| s.speed.abs() < 10.0).copied().unwrap();
        assert!((1.8..=2.8).contains(&slow), "under 10 points a second after {slow} s");
        eprintln!("1000 pt/s: {} pt, under 10 pt/s after {slow:.2} s", end.position);
    }

    #[test]
    fn let_go_anywhere_it_comes_to_rest_exactly_on_a_row() {
        for from in [0.0, 3.3, 77.6, 120.0, 400.7, 1000.0] {
            for speed in [-3000.0, -1000.0, -250.0, -40.0, -5.0, 0.0, 5.0, 40.0, 250.0, 1000.0, 3000.0] {
                let coast = blend_coast(from, speed, ROW, ROWS);
                let steps = follow(coast);
                let (_, end) = steps[steps.len() - 1];
                assert!(on_a_row(end.position), "from {from} at {speed}: {coast:?} ends at {}", end.position);
                assert!((0.0..=last(ROW, ROWS)).contains(&end.position));
            }
        }
    }

    /// Flicked, it slows straight onto its row: its speed only ever falls and never turns, no
    /// second move to centre it.
    #[test]
    fn a_flick_slows_straight_onto_its_row_in_one_motion() {
        for from in [0.0, 13.0, 200.0, 333.3] {
            for speed in [200.0, 600.0, 1000.0, 1500.0, -200.0, -600.0] {
                if !(0.0..=last(ROW, ROWS)).contains(&(from + speed / crate::scroll::coast_rate())) {
                    continue;
                }
                let coast = blend_coast(from, speed, ROW, ROWS);
                assert!(matches!(coast, ScrollMotion::Coast { .. }), "from {from} at {speed}: {coast:?}");
                let steps = follow(coast);
                for pair in steps.windows(2) {
                    let ((_, a), (_, b)) = (pair[0], pair[1]);
                    assert!(b.speed.abs() <= a.speed.abs() + 1e-3, "from {from} at {speed}: sped up");
                    assert!((b.position - a.position) * speed >= 0.0, "from {from} at {speed}: turned");
                }
            }
        }
    }

    #[test]
    fn let_go_still_between_rows_it_settles_on_the_nearest() {
        for (from, to) in [(2.3 * ROW, 2.0 * ROW), (2.6 * ROW, 3.0 * ROW), (0.4 * ROW, 0.0)] {
            let steps = follow(blend_coast(from, 0.0, ROW, ROWS));
            assert_eq!(steps[steps.len() - 1].1.position, to);
            // from rest it doesn't overshoot
            assert!(steps.iter().all(|(_, s)| (s.position - to) * (from - to) >= 0.0));
        }
    }

    #[test]
    fn pulled_past_an_end_it_stretches_about_half_as_far_with_no_hard_stop() {
        let at = |pull: f32| blend_stretch(pull, HEIGHT);
        // 100 points of pull about 48 points, 200 about 84
        assert!((at(100.0) - 48.0).abs() < 2.0, "{}", at(100.0));
        assert!((at(200.0) - 84.0).abs() < 2.0, "{}", at(200.0));
        assert!((at(-100.0) + 48.0).abs() < 2.0);
        // ever farther, ever less for each point more
        let mut last_gain = f32::MAX;
        for pull in (1..40).map(|i| i as f32 * 25.0) {
            let gain = at(pull + 25.0) - at(pull);
            assert!(gain > 0.0 && gain < last_gain, "at {pull}");
            last_gain = gain;
        }
        for pull in [0.0, 10.0, 100.0, 300.0, -50.0] {
            assert!((blend_unstretch(at(pull), HEIGHT) - pull).abs() < 0.01 * pull.abs().max(1.0));
        }
        eprintln!("stretch: 100 pt -> {:.1} pt, 200 pt -> {:.1} pt", at(100.0), at(200.0));
    }

    #[test]
    fn a_pull_shows_between_the_ends_as_it_is_and_past_them_stretched() {
        let most = last(ROW, ROWS);
        assert_eq!(blend_pulled(200.0, ROW, ROWS, HEIGHT), 200.0);
        assert!((blend_pulled(-100.0, ROW, ROWS, HEIGHT) + blend_stretch(100.0, HEIGHT)).abs() < 1e-3);
        assert!((blend_pulled(most + 200.0, ROW, ROWS, HEIGHT) - most - blend_stretch(200.0, HEIGHT)).abs() < 1e-3);
        for pull in [-150.0, -3.0, 0.0, 500.0, most, most + 40.0] {
            let shown = blend_pulled(pull, ROW, ROWS, HEIGHT);
            assert!((blend_pull_for(shown, ROW, ROWS, HEIGHT) - pull).abs() < 0.05, "{pull}");
        }
    }

    #[test]
    fn let_go_past_an_end_it_springs_back_onto_it() {
        let most = last(ROW, ROWS);
        for (from, edge) in [(-48.0, 0.0), (most + 30.0, most)] {
            let coast = blend_coast(from, 0.0, ROW, ROWS);
            let steps = follow(coast);
            assert_eq!(steps[steps.len() - 1].1.position, edge);
            assert!(steps.iter().all(|(_, s)| (s.position - edge) * (from - edge) >= 0.0), "overshot");
        }
    }

    #[test]
    fn flicked_into_an_end_it_overshoots_and_springs_back() {
        let most = last(ROW, ROWS);
        for (from, speed, edge) in [(2.0 * ROW, -2000.0, 0.0), (most - 2.0 * ROW, 2000.0, most)] {
            let coast = blend_coast(from, speed, ROW, ROWS);
            assert!(matches!(coast, ScrollMotion::Bounce { .. }), "{coast:?}");
            let steps = follow(coast);
            let beyond = steps.iter().map(|(_, s)| (s.position - edge) * speed.signum()).fold(0.0, f32::max);
            assert!(beyond > 5.0, "overshot {beyond}");
            assert_eq!(steps[steps.len() - 1].1.position, edge);
        }
    }

    #[test]
    fn the_row_in_the_band_changes_half_way_between_rows() {
        assert_eq!(blend_centred(2.49 * ROW, ROW, ROWS), 2);
        assert_eq!(blend_centred(2.5 * ROW, ROW, ROWS), 3);
        assert_eq!(blend_centred(-60.0, ROW, ROWS), 0);
        assert_eq!(blend_centred(30.0 * ROW, ROW, ROWS), ROWS - 1);
        assert_eq!(blend_centred(f32::NAN, ROW, ROWS), 0);
    }

    #[test]
    fn a_tap_scrolls_its_row_to_the_band_in_the_same_time_however_far() {
        // the row in the band: nothing
        assert_eq!(blend_tap(2, 2.0 * ROW, ROW, ROWS), None);
        assert_eq!(blend_tap(ROWS, 2.0 * ROW, ROW, ROWS), None);
        for rows_away in [1, 2, 4] {
            let to = blend_tap(2 + rows_away, 2.0 * ROW, ROW, ROWS).unwrap();
            assert_eq!(to, (2 + rows_away) as f32 * ROW);
            let step = blend_tap_at(2.0 * ROW, to, BLEND_TAP_SECONDS - 0.01);
            assert!(!step.done && step.position < to);
            let step = blend_tap_at(2.0 * ROW, to, BLEND_TAP_SECONDS);
            assert!(step.done && step.position == to);
        }
        assert_eq!(blend_tap_at(0.0, 100.0, 0.15).position, 50.0);
    }
}
