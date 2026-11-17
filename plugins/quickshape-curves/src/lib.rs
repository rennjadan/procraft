//! QuickShape curves: a stroke that bends more than once, held, snaps to a curve through its bends
//! rather than a polyline, its points there to reshape it. The app hands over the stroke's points
//! and what QuickShape found first (the plugin kit passes them); this finds the bends and lays a chain of cubics through them,
//! each anchor's controls leaning the way the stroke came and went so the joins are smooth. The app
//! draws the curve and lets it be reshaped. It's a plugin, not part of the app:
//! `tools/pack_plugin.py` makes a `.procraft-plugin` file of it.
//!
//! A stroke that comes back round to where it began, bending in and out on its way (a cloud, a
//! flower), snaps to a closed curve rather than the ellipse QuickShape makes of it: its last anchor
//! is its first again, smooth there as anywhere. A loop that only bends one way is left the ellipse
//! (or circle) it is.
//!
//! The interface is written down at the top of `crates/core/src/plugin_run.rs`.

/// A stroke's bend is a corner when the path strays this far (as a part of the stroke's size) from
/// the straight line between its neighbours.
const CORNER_TOLERANCE: f32 = 0.08;
/// Round a closed stroke, a bend is kept where the path strays this far: closer than an open
/// stroke's, as its waves are what it is drawn for.
const CLOSED_TOLERANCE: f32 = 0.05;
/// A stroke whose ends are this close (a part of its length) comes back round: QuickShape's own
/// measure.
const CLOSED: f32 = 0.18;
/// A closed stroke's bend turns the other way from the rest when it does by more than this (the
/// sine of the angle): less is a hand's wobble.
const TURNS_BACK: f32 = 0.1;

kit::quickshape_curve!(offer);

/// The curve offered for a stroke of `points`, QuickShape having found `found` first: one for a
/// polyline (it bends more than once), a closed one for a round loop that bends in and out, or
/// when a curve is asked for by name (none found: closed if the stroke comes back round), else none.
fn offer(points: &[P], found: Option<kit::ShapeKind>) -> Option<Vec<P>> {
    let pts: Vec<P> = points.iter().copied().filter(|p| p.0.is_finite() && p.1.is_finite()).collect();
    match found {
        None if closed(&pts) => fit_closed(&pts).map(|(nodes, _)| nodes),
        None | Some(kit::ShapeKind::Polyline) => Some(fit(&pts)).filter(|nodes| nodes.len() >= 4),
        Some(kit::ShapeKind::Ellipse | kit::ShapeKind::Circle) if closed(&pts) => {
            fit_closed(&pts).filter(|(_, wavy)| *wavy).map(|(nodes, _)| nodes)
        }
        Some(_) => None,
    }
}

type P = (f32, f32);

fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

fn add(a: P, b: P) -> P {
    (a.0 + b.0, a.1 + b.1)
}

fn scale(a: P, k: f32) -> P {
    (a.0 * k, a.1 * k)
}

fn length(a: P) -> f32 {
    a.0.hypot(a.1)
}

/// How far `p` is from the segment `a`–`b`.
fn distance_to_segment(p: P, a: P, b: P) -> f32 {
    let ab = sub(b, a);
    let len2 = ab.0 * ab.0 + ab.1 * ab.1;
    if len2 <= 1e-9 {
        return length(sub(p, a));
    }
    let ap = sub(p, a);
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / len2).clamp(0.0, 1.0);
    length(sub(p, add(a, scale(ab, t))))
}

/// How big the stroke is: its box's diagonal.
fn size(pts: &[P]) -> f32 {
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for &(x, y) in pts {
        (lo.0, lo.1, hi.0, hi.1) = (lo.0.min(x), lo.1.min(y), hi.0.max(x), hi.1.max(y));
    }
    if pts.is_empty() { 1.0 } else { length(sub(hi, lo)).max(1.0) }
}

/// The corners that keep the path within `tolerance` (Ramer–Douglas–Peucker), ends included.
fn simplify(pts: &[P], tolerance: f32) -> Vec<P> {
    fn keep(pts: &[P], tolerance: f32, out: &mut Vec<P>) {
        let (Some(&a), Some(&b)) = (pts.first(), pts.last()) else { return };
        let far = pts
            .iter()
            .enumerate()
            .map(|(i, &p)| (i, distance_to_segment(p, a, b)))
            .fold((0, 0.0f32), |best, x| if x.1 > best.1 { x } else { best });
        if far.1 > tolerance && far.0 > 0 && far.0 + 1 < pts.len() {
            keep(&pts[..=far.0], tolerance, out);
            out.pop();
            keep(&pts[far.0..], tolerance, out);
        } else {
            out.push(a);
            out.push(b);
        }
    }
    let mut out = Vec::new();
    keep(pts, tolerance, &mut out);
    out.dedup();
    out
}

/// How far the stroke goes, along its points.
fn path_length(pts: &[P]) -> f32 {
    pts.windows(2).map(|w| length(sub(w[1], w[0]))).sum()
}

/// Whether the stroke comes back round to where it began, as QuickShape tells a closed one.
fn closed(pts: &[P]) -> bool {
    let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { return false };
    let way = path_length(pts);
    way > 0.0 && length(sub(last, first)) <= CLOSED * way
}

/// The closed curve round the stroke's bends, and whether it bends in and out (some bend turns
/// the other way from the rest, as an ellipse's never do). None with fewer than three bends.
///
/// The stroke stops where it comes nearest its start again, once past halfway: what runs on
/// beyond lies over its start. The bends are found going once round from the point farthest from
/// the start, so the start, where the stroke closes, is no bend unless it is one; they are laid
/// from the one nearest the start, as the stroke went, and the first again at the end closes it.
fn fit_closed(pts: &[P]) -> Option<(Vec<P>, bool)> {
    let start = *pts.first()?;
    // where it closes: the point nearest its start over the second half of its way
    let half = path_length(pts) / 2.0;
    let mut gone = 0.0;
    let mut close_at = pts.len() - 1;
    let mut nearest = f32::MAX;
    for (i, w) in pts.windows(2).enumerate() {
        gone += length(sub(w[1], w[0]));
        let d = length(sub(w[1], start));
        if gone >= half && d < nearest {
            (close_at, nearest) = (i + 1, d);
        }
    }
    let pts = &pts[..=close_at];
    let far = pts.iter().enumerate().fold((0, 0.0f32), |best, (i, &p)| {
        let d = length(sub(p, start));
        if d > best.1 { (i, d) } else { best }
    });
    // once round, from the far point back to it
    let mut ring: Vec<P> = pts[far.0..].to_vec();
    ring.extend_from_slice(&pts[..=far.0]);
    let mut anchors = simplify(&ring, CLOSED_TOLERANCE * size(pts));
    anchors.pop();
    let n = anchors.len();
    if n < 3 {
        return None;
    }
    let first = (0..n).min_by(|&a, &b| length(sub(anchors[a], start)).total_cmp(&length(sub(anchors[b], start))))?;
    anchors.rotate_left(first);
    let at = |i: usize| anchors[i % n];
    // which way each bend turns, of those that clearly do
    let turns: Vec<f32> = (0..n)
        .filter_map(|i| {
            let (u, v) = (sub(at(i), at(i + n - 1)), sub(at(i + 1), at(i)));
            let sine = (u.0 * v.1 - u.1 * v.0) / (length(u) * length(v)).max(1e-6);
            (sine.abs() > TURNS_BACK).then_some(sine)
        })
        .collect();
    let wavy = turns.iter().any(|&t| t > 0.0) && turns.iter().any(|&t| t < 0.0);
    let mut nodes = Vec::with_capacity(3 * n + 1);
    for i in 0..n {
        let (before, here, after, beyond) = (at(i + n - 1), at(i), at(i + 1), at(i + 2));
        nodes.push(here);
        nodes.push(add(here, scale(sub(after, before), 1.0 / 6.0)));
        nodes.push(sub(after, scale(sub(beyond, here), 1.0 / 6.0)));
    }
    nodes.push(at(0));
    Some((nodes, wavy))
}

/// The curve through the stroke's bends: an anchor at each corner, and between each two the two
/// controls of a cubic, leaning the way the stroke came and went (a sixth of the way to the
/// neighbours either side), so the joins are smooth.
fn fit(pts: &[P]) -> Vec<P> {
    let pts: Vec<P> = pts.iter().copied().filter(|p| p.0.is_finite() && p.1.is_finite()).collect();
    let mut anchors = simplify(&pts, CORNER_TOLERANCE * size(&pts));
    if anchors.len() < 2 {
        let (Some(&first), Some(&last)) = (pts.first(), pts.last()) else { return Vec::new() };
        anchors = vec![first, last];
    }
    let n = anchors.len();
    let mut nodes = Vec::with_capacity(n * 3);
    for i in 0..n {
        let here = anchors[i];
        let before = if i == 0 { here } else { anchors[i - 1] };
        let after = if i + 1 == n { here } else { anchors[i + 1] };
        nodes.push(here);
        if i + 1 < n {
            let beyond = anchors.get(i + 2).copied().unwrap_or(after);
            nodes.push(add(here, scale(sub(after, before), 1.0 / 6.0)));
            nodes.push(sub(after, scale(sub(beyond, here), 1.0 / 6.0)));
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stroke across, back and across again.
    fn zigzag() -> Vec<P> {
        let corners = [(0.0, 0.0), (200.0, 10.0), (10.0, 90.0), (200.0, 160.0)];
        corners
            .windows(2)
            .flat_map(|w| (0..25).map(move |i| add(w[0], scale(sub(w[1], w[0]), i as f32 / 25.0))))
            .collect()
    }

    #[test]
    fn a_stroke_bending_twice_gets_a_curve_through_its_bends() {
        let nodes = fit(&zigzag());
        // anchors at 0, 3, 6 ...: each span's two controls between them; one at each corner
        assert_eq!(nodes.len() % 3, 1, "{nodes:?}");
        assert!(nodes.len() >= 10, "{nodes:?}");
        assert_eq!(nodes[0], (0.0, 0.0));
        // each anchor's controls either side on one line (smooth), the first's toward the next
        let (before, at, after) = (nodes[2], nodes[3], nodes[4]);
        let (u, v) = (sub(at, before), sub(after, at));
        assert!((u.0 * v.1 - u.1 * v.0).abs() < 1e-2 * length(u) * length(v), "{u:?} {v:?}");
    }

    #[test]
    fn a_curve_is_offered_for_a_polyline_or_when_asked_and_not_otherwise() {
        let stroke = zigzag();
        assert!(offer(&stroke, Some(kit::ShapeKind::Polyline)).is_some_and(|n| n.len() >= 10));
        assert!(offer(&stroke, None).is_some_and(|n| n.len() >= 10));
        assert_eq!(offer(&stroke, Some(kit::ShapeKind::Line)), None);
        assert_eq!(offer(&stroke, Some(kit::ShapeKind::Ellipse)), None);
    }

    /// Once round a loop `r` from (300, 300) at each turn, starting on the left, in `n` steps, and
    /// on a further `over` of the way round (past its start, or short of it if less than 0).
    fn loop_of(r: impl Fn(f32) -> f32, n: usize, over: f32) -> Vec<P> {
        let steps = ((1.0 + over) * n as f32) as usize;
        (0..=steps)
            .map(|i| {
                let t = std::f32::consts::PI + i as f32 / n as f32 * std::f32::consts::TAU;
                (300.0 + r(t) * t.cos(), 300.0 + 0.7 * r(t) * t.sin())
            })
            .collect()
    }

    /// A cloud: a loop bulging out and dipping in five times round.
    fn cloud(over: f32) -> Vec<P> {
        loop_of(|t| 150.0 * (1.0 + 0.25 * (5.0 * t).cos()), 200, over)
    }

    /// Whether `nodes` close: the last anchor the first, the controls either side of it on a line.
    fn closes_smoothly(nodes: &[P]) -> bool {
        let n = nodes.len();
        let (before, at, after) = (nodes[n - 2], nodes[0], nodes[1]);
        let (u, v) = (sub(at, before), sub(after, at));
        n % 3 == 1 && nodes[n - 1] == nodes[0] && (u.0 * v.1 - u.1 * v.0).abs() < 1e-2 * length(u) * length(v)
    }

    #[test]
    fn a_loop_that_bends_in_and_out_closes_as_a_curve_not_an_ellipse() {
        for over in [0.0, 0.1, -0.08] {
            let stroke = cloud(over);
            let nodes = offer(&stroke, Some(kit::ShapeKind::Ellipse)).expect("a closed curve");
            assert!(closes_smoothly(&nodes), "{over}: {nodes:?}");
            // a bend at each bulge and dip at least, and it starts where the stroke did
            assert!(nodes.len() / 3 >= 10, "{over}: {} anchors", nodes.len() / 3);
            assert!(length(sub(nodes[0], stroke[0])) < 0.1 * size(&stroke), "{over}: {:?}", nodes[0]);
            assert!(offer(&stroke, Some(kit::ShapeKind::Circle)).is_some());
        }
        // as it was drawn when reported: a zigzag down and up, round to the right and back along the top
        let corners = [
            (20.0, 0.0),
            (0.0, 60.0),
            (40.0, 40.0),
            (30.0, 100.0),
            (60.0, 60.0),
            (90.0, 90.0),
            (120.0, 30.0),
            (60.0, 0.0),
            (28.0, 4.0),
        ];
        let stroke: Vec<P> = corners
            .windows(2)
            .flat_map(|w| (0..20).map(move |i| add(w[0], scale(sub(w[1], w[0]), i as f32 / 20.0))))
            .collect();
        assert!(offer(&stroke, Some(kit::ShapeKind::Ellipse)).is_some_and(|n| closes_smoothly(&n)));
    }

    #[test]
    fn a_loop_that_only_bends_one_way_is_left_the_ellipse_it_is() {
        let wobbly = loop_of(|t| 150.0 + 4.0 * (9.0 * t).sin(), 200, 0.05);
        assert_eq!(offer(&wobbly, Some(kit::ShapeKind::Ellipse)), None);
        assert_eq!(offer(&wobbly, Some(kit::ShapeKind::Circle)), None);
        // asked for by name, it closes as a curve all the same
        assert!(offer(&wobbly, None).is_some_and(|n| closes_smoothly(&n)));
        // and polygons stay QuickShape's
        assert_eq!(offer(&cloud(0.0), Some(kit::ShapeKind::Quadrilateral)), None);
    }
}
