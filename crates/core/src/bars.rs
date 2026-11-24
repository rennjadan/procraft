//! The interface's bars as Edit layout moves them: the top bar's left group (Gallery, Actions,
//! Adjustments, Selection, Transform), its right group (the tools, Layers and the colour), and the
//! side bar (size, modify, opacity, undo and redo). Until one is moved it is where the app always
//! has it; moved, it goes where it was let go, snapping to the window's edges and to the other bars
//! with no gap. Brought within [`TURN_IN`] of the window's left or right edge (and not of its top or
//! bottom) it stands upright against that edge, and it lies down again only once taken farther than
//! [`TURN_OUT`] from it, or within [`TURN_IN`] of the top or the bottom: so it doesn't flicker
//! between the two as it passes. Each way the window is turned keeps its own; Reset puts them all
//! back.
//!
//! The app tells it each bar's place as it always has it and its size laid down (its length and its
//! thickness), the window's size and how near the bars snap, and draws each where
//! [`BarLayout::shown`] says. Everything is in points as the window measures them.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::floating::FloatingRoom;
use crate::layout::{LayoutRect, snap_moved};

/// What the bars are kept under, with the interface's other settings.
const KEY: &str = "bar_layout";
/// A bar lying down whose end comes this near the window's left or right edge stands upright
/// against it (points) ...
pub const TURN_IN: f32 = 96.0;
/// ... and one standing upright lies down once taken farther than this from the edges (points).
pub const TURN_OUT: f32 = 176.0;

/// A bar of the interface Edit layout moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum LayoutBar {
    TopLeft,
    TopRight,
    SideBar,
}

const BARS: [LayoutBar; 3] = [LayoutBar::TopLeft, LayoutBar::TopRight, LayoutBar::SideBar];

impl LayoutBar {
    fn at(self) -> usize {
        match self {
            LayoutBar::TopLeft => 0,
            LayoutBar::TopRight => 1,
            LayoutBar::SideBar => 2,
        }
    }

    fn word(self) -> &'static str {
        match self {
            LayoutBar::TopLeft => "top_left",
            LayoutBar::TopRight => "top_right",
            LayoutBar::SideBar => "side_bar",
        }
    }
}

/// Where a bar shows: its top-left in the window, and whether it stands upright.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct BarPlace {
    pub x: f32,
    pub y: f32,
    pub vertical: bool,
}

/// Which side of a moved bar a panel opened from one of its buttons hangs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum HangSide {
    /// Under a bar lying in the window's upper half (its arrow up at the button).
    Below,
    /// Over a bar lying in the window's lower half (its arrow down).
    Above,
    /// Right of a bar standing in the window's left half (its arrow pointing left).
    Right,
    /// Left of a bar standing in the window's right half (its arrow pointing right).
    Left,
}

/// Where a panel opened from a button of a moved bar goes (points as the window measures them):
/// the side of the bar it hangs on; its left edge (the arrow's, beside); its top, or its bottom
/// hanging above; how far along its edge the arrow's tip is (from its left, or from its top
/// beside); and how tall it may be.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct BarHang {
    pub side: HangSide,
    pub x: f32,
    pub y: f32,
    pub arrow: f32,
    pub room: f32,
}

/// Beside a bar standing upright, how far over the button the panel's top starts, where it can.
const BESIDE_ABOVE: f32 = 100.0;
/// Beside it, the least room a panel is left under its top.
const BESIDE_ROOM: f32 = 320.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    bar: LayoutBar,
    from: (f32, f32),
    start: BarPlace,
    /// Whether it stands upright now, as the drag has turned it.
    upright: bool,
}

#[derive(Debug, Default)]
struct Bars {
    window: (f32, f32),
    snap: f32,
    /// Each bar's length and thickness, laid down.
    sizes: [(f32, f32); 3],
    /// Where the app always has each.
    own: [Option<BarPlace>; 3],
    /// Each moved bar's place, portrait and landscape.
    moved: [[Option<BarPlace>; 3]; 2],
    held: Option<Held>,
    editing: bool,
    /// The bars that stay as they stand, upright or lying down (an app that can't turn them).
    fixed: [bool; 3],
}

/// The bars' places; see the module's words.
#[derive(Debug, Default, uniffi::Object)]
pub struct BarLayout {
    inner: Mutex<Bars>,
}

/// What the bars are kept under, with the interface's other settings.
#[uniffi::export]
pub fn bar_layout_key() -> String {
    KEY.to_owned()
}

#[uniffi::export]
impl BarLayout {
    #[uniffi::constructor]
    pub fn new() -> Arc<BarLayout> {
        Arc::new(BarLayout::default())
    }

    /// The bars as kept under [`bar_layout_key`], leniently: what doesn't read is passed over.
    pub fn read_kept(&self, kept: String) {
        let mut b = self.lock();
        for bar in kept.split(';') {
            let words: Vec<&str> = bar.split_whitespace().collect();
            let Some((name, places)) = words.split_first() else { continue };
            let Some(at) = BARS.iter().find(|b| b.word() == *name).map(|b| b.at()) else { continue };
            for place in places.chunks(4) {
                let [way, x, y, vertical] = place else { break };
                let way = match *way {
                    "P" => 0,
                    "L" => 1,
                    _ => continue,
                };
                let number = |w: &str| w.parse::<f32>().ok().filter(|v| v.is_finite());
                let (Some(x), Some(y)) = (number(x), number(y)) else { continue };
                if let Some(slot) = b.moved.get_mut(way).and_then(|m| m.get_mut(at)) {
                    *slot = Some(BarPlace { x, y, vertical: *vertical == "1" });
                }
            }
        }
    }

    /// The bars as kept: each moved bar, and where in each way of turning it was moved to.
    pub fn kept(&self) -> String {
        let b = self.lock();
        let mut bars = Vec::new();
        for bar in BARS {
            let mut words = vec![bar.word().to_owned()];
            for (way, letter) in [(0, "P"), (1, "L")] {
                if let Some(p) = b.moved.get(way).and_then(|m| m.get(bar.at())).copied().flatten() {
                    words.push(format!("{letter} {} {} {}", p.x, p.y, u8::from(p.vertical)));
                }
            }
            if words.len() > 1 {
                bars.push(words.join(" "));
            }
        }
        bars.join(";")
    }

    /// The window's size, and how near the bars snap (points; 0 for none).
    pub fn set_window(&self, width: f32, height: f32, snap: f32) {
        let mut b = self.lock();
        let was = b.landscape();
        b.window = (
            if width.is_finite() { width.max(0.0) } else { 0.0 },
            if height.is_finite() { height.max(0.0) } else { 0.0 },
        );
        b.snap = if snap.is_finite() { snap.clamp(0.0, crate::layout::MOST_SNAP) } else { 0.0 };
        if b.landscape() != was {
            b.held = None;
        }
    }

    /// `bar` where the app always has it (`own`), `length` along it and `thickness` across.
    pub fn set_bar(&self, bar: LayoutBar, own: BarPlace, length: f32, thickness: f32) {
        let mut b = self.lock();
        if [own.x, own.y, length, thickness].iter().all(|v| v.is_finite()) && length > 0.0 && thickness > 0.0 {
            if let Some(size) = b.sizes.get_mut(bar.at()) {
                *size = (length, thickness);
            }
            if let Some(slot) = b.own.get_mut(bar.at()) {
                *slot = Some(own);
            }
        }
    }

    /// Where `bar` shows: where it was moved to in this way of turning, inside the window; else where
    /// the app always has it.
    pub fn shown(&self, bar: LayoutBar) -> Option<BarPlace> {
        let b = self.lock();
        b.moved_place(bar).or_else(|| b.own.get(bar.at()).copied().flatten())
    }

    /// What `bar` covers where it shows (see [`BarLayout::shown`]).
    pub fn rect(&self, bar: LayoutBar) -> Option<LayoutRect> {
        let b = self.lock();
        let place = b.moved_place(bar).or_else(|| b.own.get(bar.at()).copied().flatten())?;
        Some(b.rect(bar, place))
    }

    /// Where a panel `width` wide (as it shows, its arrow's `arrow_height` not counted) goes opened
    /// from the button whose middle is `along` from the start of `bar` where it shows (its left,
    /// lying down; its top, standing): see [`HangSide`]. The arrow's tip `shift` past the button's
    /// middle, the panel at least `gap` inside the window. None for a bar not known.
    pub fn hang(
        &self,
        bar: LayoutBar,
        along: f32,
        width: f32,
        shift: f32,
        gap: f32,
        arrow_height: f32,
    ) -> Option<BarHang> {
        let b = self.lock();
        let place = b.moved_place(bar).or_else(|| b.own.get(bar.at()).copied().flatten())?;
        let r = b.rect(bar, place);
        let (w, h) = b.window;
        let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
        let (along, width, shift, gap, arrow_height) =
            (finite(along), finite(width).max(0.0), finite(shift), finite(gap), finite(arrow_height));
        if !place.vertical {
            let tip = r.x + along + shift;
            let furthest = w - gap - width;
            let x =
                if furthest >= gap { (tip - width / 2.0).clamp(gap, furthest) } else { ((w - width) / 2.0).max(0.0) };
            let below = r.y + r.height / 2.0 < h / 2.0;
            let (side, y, room) =
                if below { (HangSide::Below, r.bottom(), h - r.bottom()) } else { (HangSide::Above, r.y, r.y) };
            return Some(BarHang { side, x, y, arrow: tip - x, room: room.max(0.0) });
        }
        let tip = r.y + along + shift;
        let right = r.x + r.width / 2.0 < w / 2.0;
        let (side, x) = if right { (HangSide::Right, r.right()) } else { (HangSide::Left, r.x - width - arrow_height) };
        let lowest = (h - gap - BESIDE_ROOM).max(gap);
        let y = (tip - BESIDE_ABOVE).clamp(gap, lowest);
        Some(BarHang { side, x, y, arrow: tip - y, room: (h - gap - y).max(0.0) })
    }

    /// Whether `bar` was moved from where the app has it, in this way of turning.
    pub fn is_moved(&self, bar: LayoutBar) -> bool {
        self.lock().moved_place(bar).is_some()
    }

    /// `bar` stays as it stands wherever it goes (`fixed`), for an app that can't turn it; or turns
    /// against an edge, as every bar does at first.
    pub fn set_fixed(&self, bar: LayoutBar, fixed: bool) {
        if let Some(slot) = self.lock().fixed.get_mut(bar.at()) {
            *slot = fixed;
        }
    }

    /// Edit layout's handles shown on the bars (they move), or not.
    pub fn set_editing(&self, editing: bool) {
        let mut b = self.lock();
        b.editing = editing;
        if !editing {
            b.held = None;
        }
    }

    pub fn editing(&self) -> bool {
        self.lock().editing
    }

    /// `bar`'s handle pressed with the pointer at (`x`, `y`), while editing: it moves with the
    /// pointer until let go.
    pub fn hold(&self, bar: LayoutBar, x: f32, y: f32) -> bool {
        let mut b = self.lock();
        let start = b.moved_place(bar).or_else(|| b.own.get(bar.at()).copied().flatten());
        let Some(start) = start.filter(|_| b.editing && x.is_finite() && y.is_finite()) else { return false };
        b.held = Some(Held { bar, from: (x, y), start, upright: start.vertical });
        true
    }

    /// The pointer at (`x`, `y`) with a bar held: where it shows now.
    pub fn drag(&self, x: f32, y: f32) -> Option<BarPlace> {
        let mut b = self.lock();
        let held = b.held?;
        if !(x.is_finite() && y.is_finite()) {
            return Some(held.start);
        }
        let place = b.dragged(held, x - held.from.0, y - held.from.1);
        if let Some(h) = b.held.as_mut() {
            h.upright = place.vertical;
        }
        let way = b.way();
        if let Some(slot) = b.moved.get_mut(way).and_then(|m| m.get_mut(held.bar.at())) {
            *slot = Some(place);
        }
        Some(place)
    }

    /// The bar held, if one is.
    pub fn holding(&self) -> Option<LayoutBar> {
        self.lock().held.map(|h| h.bar)
    }

    /// The bar held let go: true when there is something new to keep.
    pub fn let_go(&self) -> bool {
        self.lock().held.take().is_some()
    }

    /// Every bar back where the app always has it, in both ways of turning.
    pub fn reset(&self) {
        let mut b = self.lock();
        b.moved = [[None; 3]; 2];
        b.held = None;
    }
}

impl BarLayout {
    /// The bars as `kept` has them, and only those: the others where they always are (a named
    /// layout chosen).
    pub(crate) fn replace_kept(&self, kept: &str) {
        {
            let mut b = self.lock();
            b.moved = Default::default();
            b.held = None;
        }
        self.read_kept(kept.to_owned());
    }

    fn lock(&self) -> MutexGuard<'_, Bars> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Bars {
    fn landscape(&self) -> bool {
        self.window.0 >= self.window.1
    }

    fn way(&self) -> usize {
        usize::from(self.landscape())
    }

    fn room(&self) -> FloatingRoom {
        FloatingRoom { left: 0.0, top: 0.0, right: self.window.0, bottom: self.window.1 }
    }

    /// What `bar` covers at `place`.
    fn rect(&self, bar: LayoutBar, place: BarPlace) -> LayoutRect {
        let (length, thickness) = self.sizes.get(bar.at()).copied().unwrap_or((0.0, 0.0));
        let (width, height) = if place.vertical { (thickness, length) } else { (length, thickness) };
        LayoutRect { x: place.x, y: place.y, width, height }
    }

    /// `bar` at `place`, inside the window (from its top-left where it doesn't fit).
    fn inside(&self, bar: LayoutBar, place: BarPlace) -> BarPlace {
        let r = self.rect(bar, place);
        let x = place.x.min(self.window.0 - r.width).max(0.0);
        let y = place.y.min(self.window.1 - r.height).max(0.0);
        BarPlace { x, y, ..place }
    }

    fn moved_place(&self, bar: LayoutBar) -> Option<BarPlace> {
        let p = self.moved.get(self.way()).and_then(|m| m.get(bar.at())).copied().flatten()?;
        Some(self.inside(bar, p))
    }

    /// `held` dragged (`dx`, `dy`): moved, upright or lying down as it comes near the edges (see the
    /// module's words), snapped to the window's edges and the other bars, inside the window.
    fn dragged(&self, held: Held, dx: f32, dy: f32) -> BarPlace {
        let bar = held.bar;
        let room = self.room();
        let others: Vec<LayoutRect> = BARS
            .into_iter()
            .filter(|&o| o != bar)
            .filter_map(|o| {
                let p = self.moved_place(o).or_else(|| self.own.get(o.at()).copied().flatten())?;
                Some(self.rect(o, p))
            })
            .collect();
        let placed = |vertical: bool, x: f32, y: f32| {
            let inside = self.inside(bar, BarPlace { x, y, vertical });
            let snapped = snap_moved(self.rect(bar, inside), &others, room, -1.0, self.snap);
            self.inside(bar, BarPlace { x: snapped.x, y: snapped.y, vertical })
        };
        let start = held.start;
        let (x, y) = (start.x + dx, start.y + dy);
        if self.fixed.get(bar.at()).copied().unwrap_or(false) {
            return placed(start.vertical, x, y);
        }
        // how near the window's edges it comes as it stands now
        let r = self.rect(bar, self.inside(bar, BarPlace { x, y, vertical: held.upright }));
        let (left, right) = (r.x - room.left, room.right - r.right());
        let (top, bottom) = (r.y - room.top, room.bottom - r.bottom());
        let (side, end) = (left.min(right), top.min(bottom));
        let upright = if held.upright { side <= TURN_OUT && end > TURN_IN } else { side <= TURN_IN && end > TURN_IN };
        if upright == held.upright {
            return placed(upright, x, y);
        }
        // turned: standing against the side it came to, or lying down where it is
        let (length, thickness) = self.sizes.get(bar.at()).copied().unwrap_or((0.0, 0.0));
        if upright {
            let x = if left <= right { room.left } else { room.right - thickness };
            placed(true, x, y)
        } else {
            let x = if left <= right { r.x } else { r.right() - length };
            placed(false, x, y)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window 1600 by 1000 snapping within 16, the top bar's groups 420 and 300 long and 48 thick
    /// at its top corners, the side bar 519 long and 51 thick on its left.
    fn bars() -> Arc<BarLayout> {
        let bars = BarLayout::new();
        bars.set_window(1600.0, 1000.0, 16.0);
        bars.set_bar(LayoutBar::TopLeft, BarPlace { x: 0.0, y: 0.0, vertical: false }, 420.0, 48.0);
        bars.set_bar(LayoutBar::TopRight, BarPlace { x: 1300.0, y: 0.0, vertical: false }, 300.0, 48.0);
        bars.set_bar(LayoutBar::SideBar, BarPlace { x: 0.0, y: 240.0, vertical: true }, 519.0, 51.0);
        bars.set_editing(true);
        bars
    }

    fn at(x: f32, y: f32, vertical: bool) -> BarPlace {
        BarPlace { x, y, vertical }
    }

    #[test]
    fn a_bar_is_where_the_app_has_it_until_moved_and_moves_only_while_editing() {
        let bars = bars();
        assert_eq!(bars.shown(LayoutBar::TopLeft), Some(at(0.0, 0.0, false)));
        assert_eq!(bars.rect(LayoutBar::SideBar), Some(LayoutRect { x: 0.0, y: 240.0, width: 51.0, height: 519.0 }));
        assert!(!bars.is_moved(LayoutBar::TopLeft));
        bars.set_editing(false);
        assert!(!bars.hold(LayoutBar::TopLeft, 10.0, 10.0));
        assert_eq!(bars.drag(300.0, 300.0), None);
        bars.set_editing(true);
        assert!(bars.hold(LayoutBar::TopLeft, 10.0, 10.0));
        // along the top it stays lying down, snapping to the right group with no gap
        let moved = bars.drag(10.0 + 870.0, 10.0).unwrap();
        assert_eq!(moved, at(880.0, 0.0, false));
        assert!(bars.let_go());
        assert!(bars.is_moved(LayoutBar::TopLeft));
    }

    #[test]
    fn a_bar_near_a_side_stands_upright_and_lies_down_again_only_well_away_from_it() {
        let bars = bars();
        // the left half dragged down the left edge: once clear of the top, upright against the edge,
        // 48 wide and 420 tall
        bars.hold(LayoutBar::TopLeft, 10.0, 10.0);
        assert_eq!(bars.drag(10.0, 10.0 + 90.0).unwrap(), at(0.0, 90.0, false), "still near the top");
        let upright = bars.drag(10.0, 10.0 + 400.0).unwrap();
        assert_eq!(upright, at(0.0, 400.0, true));
        // taken 150 out from the edge it stays upright (within 176)...
        let held_out = bars.drag(10.0 + 150.0, 410.0).unwrap();
        assert_eq!(held_out, at(150.0, 400.0, true));
        // ...and lies down past 176
        let free = bars.drag(10.0 + 200.0, 410.0).unwrap();
        assert_eq!(free, at(200.0, 400.0, false));
        // brought back to 90 from the right edge, lying down: upright against it
        let right = bars.drag(10.0 + 1600.0 - 420.0 - 90.0, 410.0).unwrap();
        assert_eq!(right, at(1600.0 - 48.0, 400.0, true));
        bars.let_go();
        // the side bar dragged near the bottom: lying down along it, 519 long and 51 thick
        bars.hold(LayoutBar::SideBar, 20.0, 300.0);
        let down = bars.drag(620.0, 300.0 + 1000.0).unwrap();
        assert_eq!(down, at(600.0, 1000.0 - 51.0, false));
        // into the bottom left corner: near a side and an end both, it stays as it lies
        let corner = bars.drag(20.0, 300.0 + 1000.0).unwrap();
        assert_eq!(corner, at(0.0, 949.0, false));
        bars.let_go();
        // one the app can't turn stays upright wherever it goes
        bars.set_fixed(LayoutBar::TopRight, true);
        bars.hold(LayoutBar::TopRight, 1400.0, 10.0);
        let kept = bars.drag(1400.0 + 150.0, 10.0 + 400.0).unwrap();
        assert!(!kept.vertical, "{kept:?}");
    }

    #[test]
    fn a_panel_hangs_from_its_button_where_the_bar_was_moved() {
        let bars = bars();
        // the right half where it always is: the Layers panel, 384 wide, under its button 230 along
        let under = bars.hang(LayoutBar::TopRight, 230.0, 384.0, 2.0, 12.0, 12.0).unwrap();
        assert_eq!(under, BarHang { side: HangSide::Below, x: 1204.0, y: 48.0, arrow: 328.0, room: 952.0 });
        // near the window's right edge it stays 12 inside, the arrow still at the button
        assert_eq!(under.x + 384.0, 1588.0);
        // moved to the bottom: over the bar, as tall as the room above it
        bars.hold(LayoutBar::TopRight, 1400.0, 10.0);
        bars.drag(1400.0 - 700.0, 10.0 + 945.0);
        bars.let_go();
        let over = bars.hang(LayoutBar::TopRight, 230.0, 384.0, 2.0, 12.0, 12.0).unwrap();
        assert_eq!(over, BarHang { side: HangSide::Above, x: 640.0, y: 952.0, arrow: 192.0, room: 952.0 });
        // standing against the left edge: right of it, its arrow at the button
        bars.hold(LayoutBar::TopLeft, 10.0, 10.0);
        bars.drag(10.0, 10.0 + 400.0);
        bars.let_go();
        let beside = bars.hang(LayoutBar::TopLeft, 150.0, 390.0, 2.0, 12.0, 12.0).unwrap();
        assert_eq!(beside, BarHang { side: HangSide::Right, x: 48.0, y: 452.0, arrow: 100.0, room: 536.0 });
        // against the right edge: left of it, the arrow's room taken off too; low down, its top
        // goes no lower than leaves it 320 of room
        bars.hold(LayoutBar::TopLeft, 10.0, 410.0);
        assert!(bars.drag(10.0 + 1600.0, 410.0).unwrap().vertical);
        bars.let_go();
        let left = bars.hang(LayoutBar::TopLeft, 400.0, 390.0, 2.0, 12.0, 12.0).unwrap();
        assert_eq!((left.side, left.x), (HangSide::Left, 1600.0 - 48.0 - 390.0 - 12.0));
        assert_eq!((left.y, left.room), (1000.0 - 12.0 - 320.0, 320.0));
        assert_eq!(left.arrow, 400.0 + 402.0 - left.y);
    }

    #[test]
    fn each_way_of_turning_keeps_its_own_and_reset_puts_them_back() {
        let bars = bars();
        bars.hold(LayoutBar::TopRight, 1400.0, 10.0);
        bars.drag(1400.0, 10.0 + 945.0);
        bars.let_go();
        let landscape = bars.shown(LayoutBar::TopRight).unwrap();
        assert_eq!(landscape, at(1300.0, 952.0, false));
        // portrait: where the app has it
        bars.set_window(1000.0, 1600.0, 16.0);
        bars.set_bar(LayoutBar::TopRight, at(700.0, 0.0, false), 300.0, 48.0);
        assert_eq!(bars.shown(LayoutBar::TopRight), Some(at(700.0, 0.0, false)));
        let kept = bars.kept();
        assert_eq!(kept, "top_right L 1300 952 0");
        let next = BarLayout::new();
        next.read_kept(kept);
        next.set_window(1600.0, 1000.0, 16.0);
        next.set_bar(LayoutBar::TopRight, at(1300.0, 0.0, false), 300.0, 48.0);
        assert_eq!(next.shown(LayoutBar::TopRight), Some(landscape));
        next.reset();
        assert_eq!(next.shown(LayoutBar::TopRight), Some(at(1300.0, 0.0, false)));
        assert_eq!(next.kept(), "");
        // what doesn't read is passed over
        next.read_kept("nothing L 1 2 0;top_left X 1 2 0;top_left L nan 2 0;side_bar L 5".to_owned());
        assert_eq!(next.kept(), "");
    }
}
