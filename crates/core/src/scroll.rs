//! Kinetic scrolling in logical points, with positive offsets toward the end of the content.
//! The coast uses Apple's normal UIScrollView rate. Edge resistance follows the curve described
//! in Holko's CustomScrollView, and a ballistic scroll hands its velocity to a spring at the
//! exact edge-crossing time, as in Flutter's BouncingScrollSimulation.

const RETENTION_PER_MS: f64 = 0.998;
const RUBBER_BAND_SLOPE: f32 = 0.55;
const EDGE_RATE: f32 = 14.0;
const RETURN_HALF_LIFE: f64 = 0.07;
const SNAP_RATE: f32 = 12.0;
const STOP_DISTANCE: f32 = 0.25;
const STOP_SPEED: f32 = 5.0;
const CONTINUATION_WINDOW: f32 = 0.35;

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct ScrollSample {
    pub position: f32,
    pub velocity: f32,
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum ScrollMotion {
    Coast { from: f32, velocity: f32, target: f32, rate: f32 },
    Spring { from: f32, velocity: f32, target: f32, rate: f32 },
    Return { from: f32, velocity: f32, edge: f32 },
    Bounce { from: f32, velocity: f32, edge: f32, hit_time: f32, hit_velocity: f32 },
    Reenter { from: f32, velocity: f32, edge: f32, hit_time: f32, hit_velocity: f32, min: f32, max: f32, snap: f32 },
}

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

fn bounds(min: f32, max: f32) -> (f32, f32) {
    let min = finite(min);
    (min, finite(max).max(min))
}

pub(crate) fn coast_rate() -> f32 {
    (-1000.0 * RETENTION_PER_MS.ln()) as f32
}

/// Signed displacement after applying the resistance to a pull beyond either edge.
pub(crate) fn stretch(pull: f32, viewport: f32) -> f32 {
    let viewport = finite(viewport).max(1.0);
    let pull = finite(pull);
    pull.signum() * viewport * (1.0 - 1.0 / (1.0 + RUBBER_BAND_SLOPE * pull.abs() / viewport))
}

pub(crate) fn unstretch(displacement: f32, viewport: f32) -> f32 {
    let viewport = finite(viewport).max(1.0);
    let shown = finite(displacement);
    let fraction = (shown.abs() / viewport).min(0.999);
    shown.signum() * viewport / RUBBER_BAND_SLOPE * fraction / (1.0 - fraction)
}

/// Apply an input delta to the currently visible position. Undoing the prior resistance lets
/// a new drag catch a bouncing list without jumping, and makes the curve independent of sampling.
#[uniffi::export]
pub fn scroll_drag(position: f32, delta: f32, min: f32, max: f32, viewport: f32) -> f32 {
    let (min, max) = bounds(min, max);
    let position = finite(position);
    if finite(delta) == 0.0 {
        return position;
    }
    let edge = position.clamp(min, max);
    let next = edge + unstretch(position - edge, viewport) + finite(delta);
    let edge = next.clamp(min, max);
    edge + stretch(next - edge, viewport)
}

/// Convert pointer speed into the velocity of the visible content at release.
#[uniffi::export]
pub fn scroll_drag_velocity(position: f32, velocity: f32, min: f32, max: f32, viewport: f32) -> f32 {
    let (min, max) = bounds(min, max);
    let position = finite(position);
    let over = position - position.clamp(min, max);
    let velocity = finite(velocity);
    if over == 0.0 {
        return velocity;
    }
    let remaining = (1.0 - over.abs() / finite(viewport).max(1.0)).max(0.0);
    velocity * RUBBER_BAND_SLOPE * remaining * remaining
}

/// Rapid same-direction flicks build speed. A stopped or redirected drag uses its own speed.
#[uniffi::export]
pub fn scroll_fling_velocity(velocity: f32, previous: f32, pause: f32) -> f32 {
    let velocity = finite(velocity);
    let previous = finite(previous);
    if !(0.0..CONTINUATION_WINDOW).contains(&pause) || velocity * previous <= 0.0 {
        return velocity;
    }
    // Flutter's fit from comparing repeated flings with native UIScrollView.
    let gain = (0.000816 * previous.abs().powf(1.967)).min(40_000.0);
    if velocity.abs() > gain * 0.5 { velocity + gain.copysign(velocity) } else { velocity }
}

/// Build motion for an offset and its velocity. A positive snap interval lands on evenly spaced
/// rows measured from the minimum. Ordinary lists pass zero and keep their continuous offset.
#[uniffi::export]
pub fn scroll_motion(from: f32, velocity: f32, min: f32, max: f32, snap: f32) -> ScrollMotion {
    let (min, max) = bounds(min, max);
    let from = finite(from);
    let velocity = finite(velocity);
    let snap = finite(snap).max(0.0);
    let edge = from.clamp(min, max);
    let over = from - edge;
    if over != 0.0 {
        if velocity.abs() < STOP_SPEED {
            return ScrollMotion::Return { from, velocity, edge };
        }
        let slope = velocity + EDGE_RATE * over;
        if slope * over < 0.0 {
            let hit_time = -over / slope;
            let hit_velocity = slope * (-EDGE_RATE * hit_time).exp();
            return ScrollMotion::Reenter { from, velocity, edge, hit_time, hit_velocity, min, max, snap };
        }
        return ScrollMotion::Spring { from, velocity, target: edge, rate: EDGE_RATE };
    }

    let rate = coast_rate();
    let target = from + velocity / rate;
    if target < min || target > max {
        let edge = target.clamp(min, max);
        let remaining = (1.0 - rate * (edge - from) / velocity).clamp(f32::MIN_POSITIVE, 1.0);
        let hit_time = -remaining.ln() / rate;
        return ScrollMotion::Bounce { from, velocity, edge, hit_time, hit_velocity: velocity * remaining };
    }
    if snap > 0.0 {
        let row = (min + ((target - min) / snap).round() * snap).clamp(min, max);
        let landing_rate = velocity / (row - from);
        if (0.8 * rate..=2.5 * rate).contains(&landing_rate) {
            return ScrollMotion::Coast { from, velocity, target: row, rate: landing_rate };
        }
        return ScrollMotion::Spring { from, velocity, target: row, rate: SNAP_RATE };
    }
    ScrollMotion::Coast { from, velocity, target, rate }
}

/// Sample an unbounded coast. A toolkit may own layout bounds and report unconsumed deltas to
/// its overscroll adapter, which then takes over the remaining velocity using the same engine.
#[uniffi::export]
pub fn scroll_coast_at(velocity: f32, seconds: f32) -> ScrollSample {
    let rate = coast_rate();
    let velocity = finite(velocity);
    coast(0.0, velocity, velocity / rate, rate, finite(seconds).max(0.0), true)
}

/// Speed at the consumed distance when a toolkit hits a layout boundary between frames.
#[uniffi::export]
pub fn scroll_coast_velocity(velocity: f32, travelled: f32) -> f32 {
    let velocity = finite(velocity);
    let left = velocity - coast_rate() * finite(travelled);
    left.clamp(velocity.min(0.0), velocity.max(0.0))
}

/// Unresisted finger distance represented by a visible rubber-band displacement.
#[uniffi::export]
pub fn scroll_pull_distance(displacement: f32, viewport: f32) -> f32 {
    unstretch(displacement, viewport)
}

/// Sample by elapsed real time, including the remainder of any frame that crossed a boundary.
#[uniffi::export]
pub fn scroll_motion_at(motion: ScrollMotion, seconds: f32) -> ScrollSample {
    let time = finite(seconds).max(0.0);
    match motion {
        ScrollMotion::Coast { from, velocity, target, rate } => coast(from, velocity, target, rate, time, true),
        ScrollMotion::Spring { from, velocity, target, rate } => spring(from, velocity, target, rate, time, true),
        ScrollMotion::Return { from, velocity, edge } => stationary_return(from, velocity, edge, time),
        ScrollMotion::Bounce { from, velocity, edge, hit_time, hit_velocity } => {
            if time < hit_time {
                coast(from, velocity, edge, coast_rate(), time, false)
            } else {
                spring(edge, hit_velocity, edge, EDGE_RATE, time - hit_time, true)
            }
        }
        ScrollMotion::Reenter { from, velocity, edge, hit_time, hit_velocity, min, max, snap } => {
            if time < hit_time {
                spring(from, velocity, edge, EDGE_RATE, time, false)
            } else {
                scroll_motion_at(scroll_motion(edge, hit_velocity, min, max, snap), time - hit_time)
            }
        }
    }
}

fn stationary_return(from: f32, velocity: f32, edge: f32, time: f32) -> ScrollSample {
    // Flutter's fit for releasing a stationary rubber band. The fast term lets it start with
    // the supplied velocity, then the 70 ms half-life controls the visible return.
    let slow_rate = core::f64::consts::LN_2 / RETURN_HALF_LIFE;
    let fast_rate = slow_rate * 100_000.0;
    let offset = f64::from(from - edge);
    let time = f64::from(time);
    let slow = (f64::from(velocity) + fast_rate * offset) / (fast_rate - slow_rate);
    let fast = offset - slow;
    let slow = slow * (-slow_rate * time).exp();
    let fast = fast * (-fast_rate * time).exp();
    let offset = (slow + fast) as f32;
    let velocity = (-slow_rate * slow - fast_rate * fast) as f32;
    let done = offset.abs() < STOP_DISTANCE && velocity.abs() < STOP_SPEED;
    ScrollSample {
        position: if done { edge } else { edge + offset },
        velocity: if done { 0.0 } else { velocity },
        done,
    }
}

fn coast(from: f32, velocity: f32, target: f32, rate: f32, time: f32, settle: bool) -> ScrollSample {
    let left = velocity * (-rate * time).exp();
    let done = settle && (left / rate).abs() < STOP_DISTANCE;
    let position = if done { target } else { from - velocity * (-rate * time).exp_m1() / rate };
    ScrollSample { position, velocity: if done { 0.0 } else { left }, done }
}

fn spring(from: f32, velocity: f32, target: f32, rate: f32, time: f32, settle: bool) -> ScrollSample {
    let offset = from - target;
    let slope = velocity + rate * offset;
    let fade = (-rate * time).exp();
    let position = (offset + slope * time) * fade;
    let velocity = slope * fade - rate * position;
    let done = settle && position.abs() < STOP_DISTANCE && velocity.abs() < STOP_SPEED;
    ScrollSample {
        position: if done { target } else { target + position },
        velocity: if done { 0.0 } else { velocity },
        done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_resistance_uses_the_total_pull_and_retraces_it() {
        let first = scroll_drag(0.0, -40.0, 0.0, 500.0, 844.0);
        let second = scroll_drag(first, -60.0, 0.0, 500.0, 844.0);
        assert!((second + 51.635).abs() < 0.01);
        assert!((second - scroll_drag(0.0, -100.0, 0.0, 500.0, 844.0)).abs() < 0.001);
        assert!((scroll_drag(second, 110.0, 0.0, 500.0, 844.0) - 10.0).abs() < 0.001);
    }

    #[test]
    fn coast_transfers_position_and_speed_to_the_edge_spring() {
        let motion = scroll_motion(490.0, 1000.0, 0.0, 500.0, 0.0);
        let ScrollMotion::Bounce { hit_time, hit_velocity, .. } = motion else {
            panic!("the coast should reach the edge");
        };
        let edge = scroll_motion_at(motion, hit_time);
        assert!((edge.position - 500.0).abs() < 0.001);
        assert!((edge.velocity - hit_velocity).abs() < 0.001);
        assert!(scroll_motion_at(motion, hit_time + 0.05).position > 500.0);
        assert_eq!(scroll_motion_at(motion, 3.0), ScrollSample { position: 500.0, velocity: 0.0, done: true });
    }

    #[test]
    fn stationary_overscroll_returns_and_inward_release_can_reenter() {
        let held = scroll_motion(-40.0, 0.0, 0.0, 500.0, 0.0);
        assert_eq!(scroll_motion_at(held, 0.0).position, -40.0);
        assert_eq!(scroll_motion_at(held, 3.0).position, 0.0);
        let flick = scroll_motion(-40.0, 1200.0, 0.0, 500.0, 0.0);
        assert!(scroll_motion_at(flick, 0.2).position > 0.0);
    }

    #[test]
    fn free_lists_keep_their_projected_offset_and_pickers_snap() {
        let free = scroll_motion(100.0, 1000.0, 0.0, 1000.0, 0.0);
        let snapped = scroll_motion(100.0, 1000.0, 0.0, 1000.0, 25.0);
        assert!((scroll_motion_at(free, 10.0).position - 599.5).abs() < 0.01);
        assert!((scroll_motion_at(snapped, 10.0).position - 600.0).abs() < 0.01);
    }

    #[test]
    fn only_a_quick_same_direction_flick_carries_momentum() {
        assert!(scroll_fling_velocity(1000.0, 800.0, 0.1) > 1000.0);
        assert_eq!(scroll_fling_velocity(1000.0, -800.0, 0.1), 1000.0);
        assert_eq!(scroll_fling_velocity(1000.0, 800.0, 0.5), 1000.0);
        assert_eq!(scroll_fling_velocity(0.0, 800.0, 0.1), 0.0);
    }

    /// UIScrollView's normal deceleration: the share of its speed a coast keeps each millisecond.
    const KEPT_PER_MS: f32 = 0.998;
    /// The rubber band's constant.
    const C: f32 = 0.55;

    /// The rubber band: a pull `x` past an edge of a view `d` long shows `x·d·c / (d + c·x)` past it.
    fn rubber_band(x: f32, d: f32) -> f32 {
        x * d * C / (d + C * x)
    }

    /// Followed at 120 frames a second until done: each frame's time and sample.
    fn follow(motion: ScrollMotion) -> Vec<(f32, ScrollSample)> {
        let mut steps = Vec::new();
        for frame in 0..3600 {
            let t = frame as f32 / 120.0;
            let sample = scroll_motion_at(motion, t);
            steps.push((t, sample));
            if sample.done {
                return steps;
            }
        }
        panic!("{motion:?} still moving after 30 s");
    }

    #[test]
    fn a_coast_keeps_0_998_of_its_speed_each_millisecond() {
        let motion = scroll_motion(0.0, 3000.0, 0.0, 100_000.0, 0.0);
        assert!(matches!(motion, ScrollMotion::Coast { .. }), "{motion:?}");
        for fps in [60.0_f32, 120.0] {
            let dt = 1.0 / fps;
            let kept = KEPT_PER_MS.powf(dt * 1000.0);
            let mut worst = 0.0_f32;
            for frame in 0..fps as usize {
                let a = scroll_motion_at(motion, frame as f32 * dt);
                let b = scroll_motion_at(motion, (frame + 1) as f32 * dt);
                worst = worst.max((b.velocity / a.velocity - kept).abs());
            }
            eprintln!(
                "{fps} fps: {kept:.5} of the speed kept a frame (0.998^{:.2}), worst {worst:.1e} off",
                dt * 1000.0
            );
            assert!(worst < 1e-4, "{fps} fps: {worst} off");
        }
        // 0.998^1000 of it left after a second
        let left = scroll_motion_at(motion, 1.0).velocity / 3000.0;
        assert!((left - KEPT_PER_MS.powf(1000.0)).abs() < 1e-3, "{left} left after 1 s");
    }

    #[test]
    fn a_flick_goes_as_far_as_the_deceleration_projects() {
        // the distance a rate r a millisecond projects for a speed v: v / 1000 · r / (1 − r)
        for v in [200.0_f32, 1000.0, 2500.0, -1500.0] {
            let projected = v / 1000.0 * KEPT_PER_MS / (1.0 - KEPT_PER_MS);
            let end = scroll_motion_at(scroll_motion(0.0, v, -100_000.0, 100_000.0, 0.0), 30.0);
            assert!(end.done, "{v}: {end:?}");
            let off = (end.position - projected) / projected;
            eprintln!("{v} pt/s: went {:.1} pt, projected {projected:.1} ({:+.2}%)", end.position, off * 100.0);
            assert!(off.abs() < 0.002, "{v}: {off}");
        }
    }

    #[test]
    fn a_pull_past_an_edge_shows_the_rubber_band() {
        for d in [351.0_f32, 744.0, 1133.0] {
            for x in [1.0_f32, 10.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0] {
                let want = rubber_band(x, d);
                let past_top = -scroll_drag(0.0, -x, 0.0, 2000.0, d);
                let past_bottom = scroll_drag(2000.0, x, 0.0, 2000.0, d) - 2000.0;
                assert!((past_top - want).abs() <= want * 1e-4 + 1e-3, "{x} on {d}: {past_top} for {want}");
                assert!((past_bottom - want).abs() <= want * 1e-4 + 1e-3, "{x} on {d}: {past_bottom} for {want}");
                // the same pull in four moves ends in the same place
                let mut at = 0.0;
                for _ in 0..4 {
                    at = scroll_drag(at, -x / 4.0, 0.0, 2000.0, d);
                }
                assert!((-at - want).abs() <= want * 1e-3 + 1e-2, "{x} on {d} in four: {} for {want}", -at);
            }
            // no hard stop, but never as far as the view is long
            let far = -scroll_drag(0.0, -1e7, 0.0, 2000.0, d);
            assert!(far > 0.99 * d && far < d, "{far} on {d}");
            eprintln!(
                "on {d}: pulls of 50, 100, 500 pt show {:.1}, {:.1}, {:.1}",
                rubber_band(50.0, d),
                rubber_band(100.0, d),
                rubber_band(500.0, d)
            );
        }
    }

    #[test]
    fn let_go_past_an_edge_the_content_keeps_the_rubber_bands_slope() {
        // the content moves at the finger's speed times f'(x) = c·d² / (d + c·x)²
        let d = 744.0;
        for x in [5.0_f32, 20.0, 100.0, 400.0, 2000.0] {
            let slope = C * d * d / ((d + C * x) * (d + C * x));
            let got = scroll_drag_velocity(-rubber_band(x, d), -1000.0, 0.0, 2000.0, d);
            assert!((got + 1000.0 * slope).abs() < 0.5, "{x} past: {got} for {}", -1000.0 * slope);
        }
        // between the ends the speed is the finger's
        assert_eq!(scroll_drag_velocity(300.0, -1000.0, 0.0, 2000.0, d), -1000.0);
    }

    #[test]
    fn let_go_stretched_it_springs_back_in_about_half_a_second_without_overshoot() {
        for shown in [20.0_f32, 50.0, 120.0] {
            let motion = scroll_motion(-shown, 0.0, 0.0, 2000.0, 0.0);
            let steps = follow(motion);
            for pair in steps.windows(2) {
                let ((_, a), (_, b)) = (pair[0], pair[1]);
                assert!((a.position..=0.0).contains(&b.position), "{shown}: {a:?} then {b:?}");
            }
            // half way back in 70 ms
            let half = scroll_motion_at(motion, RETURN_HALF_LIFE as f32).position;
            assert!((half + shown / 2.0).abs() < 0.02 * shown, "{shown}: {half} after 70 ms");
            let near = steps.iter().find(|(_, s)| s.position > -1.0).map_or(f32::MAX, |(t, _)| *t);
            let rest = steps[steps.len() - 1].0;
            eprintln!("let go {shown} pt past the top: within 1 pt after {near:.3} s, at rest after {rest:.3} s");
            assert!((0.25..=0.6).contains(&near), "{shown}: within 1 pt after {near}");
            assert!(rest <= 0.75, "{shown}: at rest after {rest}");
        }
    }

    #[test]
    fn flicked_into_an_end_it_overshoots_by_its_speed_over_14e_and_comes_back() {
        for v in [500.0_f32, 1000.0, 2000.0] {
            let motion = scroll_motion(1900.0, v, 0.0, 2000.0, 0.0);
            let ScrollMotion::Bounce { hit_time, hit_velocity, .. } = motion else {
                panic!("{v}: {motion:?} doesn't reach the end");
            };
            let steps = follow(motion);
            let peak = steps.iter().map(|(_, s)| s.position - 2000.0).fold(0.0, f32::max);
            // a critically damped spring at rate k let go at the edge going v peaks at v / (k·e)
            let expected = hit_velocity / (EDGE_RATE * core::f32::consts::E);
            let back = steps[steps.len() - 1].0 - hit_time;
            eprintln!("{v} pt/s, {hit_velocity:.0} at the end: overshoots {peak:.1} pt, back {back:.3} s after");
            assert!((peak - expected).abs() < 0.01 * expected, "{v}: {peak} for {expected}");
            assert!(back < 0.75, "{v}: back after {back}");
        }
    }

    #[test]
    fn a_flicked_timeline_slows_straight_onto_a_frame() {
        let pitch = 54.0;
        let end = pitch * 39.0;
        let (mut glides, mut springs) = (0, 0);
        for from in [0.0_f32, 10.0, 300.0, 1000.0, end] {
            for v in [-2500.0_f32, -800.0, -150.0, -20.0, 20.0, 150.0, 800.0, 2500.0] {
                let motion = scroll_motion(from, v, 0.0, end, pitch);
                let steps = follow(motion);
                let last = steps[steps.len() - 1].1.position;
                assert!((last / pitch - (last / pitch).round()).abs() < 1e-4, "from {from} at {v}: ends at {last}");
                assert!((0.0..=end).contains(&last));
                if matches!(motion, ScrollMotion::Coast { .. }) {
                    glides += 1;
                    for pair in steps.windows(2) {
                        let ((_, a), (_, b)) = (pair[0], pair[1]);
                        assert!((b.position - a.position) * v >= 0.0, "from {from} at {v}: turned back");
                    }
                } else if matches!(motion, ScrollMotion::Spring { .. }) {
                    springs += 1;
                }
            }
        }
        eprintln!("{glides} flicks glided onto their frame, {springs} sprang onto it");
        assert!(glides > springs);
    }

    #[test]
    #[ignore = "timing: run alone, --ignored --test-threads=1"]
    fn sampling_a_motion_costs_well_under_a_microsecond() {
        let motions = [
            scroll_motion(0.0, 1500.0, 0.0, 5000.0, 0.0),
            scroll_motion(4900.0, 1500.0, 0.0, 5000.0, 0.0),
            scroll_motion(-40.0, 1200.0, 0.0, 5000.0, 54.0),
            scroll_motion(-40.0, 0.0, 0.0, 5000.0, 0.0),
        ];
        let count = 200_000;
        let start = std::time::Instant::now();
        let mut sum = 0.0;
        for i in 0..count {
            sum += scroll_motion_at(motions[i % 4], (i % 240) as f32 / 120.0).position;
        }
        std::hint::black_box(sum);
        let each = start.elapsed().as_nanos() as f64 / count as f64;
        eprintln!("{each:.0} ns a sample");
        // a frame samples once: 58 ns in a debug build on 2026-11-26
        assert!(each < 2_000.0, "{each} ns a sample");
    }
}
