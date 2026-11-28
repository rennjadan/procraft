use egui::scroll_area::{DragScroll, ScrollAreaOutput, ScrollBarVisibility, ScrollSource};
use egui::{AsIdSalt, IdSalt, MouseWheelSource, PointerButton, Rect, Sense, Ui, UiKind, Vec2, Vec2b};
use procraft_core::{
    ScrollMotion, scroll_drag, scroll_drag_velocity, scroll_fling_velocity, scroll_motion, scroll_motion_at,
};

#[derive(Clone, Copy, Default)]
pub struct ScrollOffset {
    pub position: f32,
    motion: Option<(ScrollMotion, f64)>,
    caught: Option<(f32, f64)>,
}

impl ScrollOffset {
    pub fn stop(&mut self) {
        self.motion = None;
        self.caught = None;
    }

    pub fn catch(&mut self, now: f64) {
        self.caught = self
            .motion
            .take()
            .map(|(motion, started)| (scroll_motion_at(motion, (now - started) as f32).velocity, now));
    }

    pub fn drag(&mut self, delta: f32, max: f32, viewport: f32) {
        self.motion = None;
        self.position = scroll_drag(self.position, delta, 0.0, max, viewport);
    }

    pub fn wheel(&mut self, delta: f32, max: f32) {
        self.stop();
        self.position = (self.position + delta).clamp(0.0, max);
    }

    pub fn release(&mut self, velocity: f32, max: f32, viewport: f32, now: f64) {
        let velocity = scroll_drag_velocity(self.position, velocity, 0.0, max, viewport);
        let velocity = self
            .caught
            .take()
            .map_or(velocity, |(previous, touched)| scroll_fling_velocity(velocity, previous, (now - touched) as f32));
        self.motion = Some((scroll_motion(self.position, velocity, 0.0, max, 0.0), now));
    }

    pub fn advance(&mut self, now: f64) -> bool {
        let Some((motion, start)) = self.motion else { return false };
        let sample = scroll_motion_at(motion, (now - start) as f32);
        self.position = sample.position;
        if sample.done {
            self.stop();
        }
        true
    }
}

/// egui lays out the contents, clips the viewport, handles scrollbars and locates
/// programmatic targets. Its stock drag and wheel input are replaced here.
pub struct ScrollArea {
    layout: egui::ScrollArea,
    horizontal: bool,
    key: IdSalt,
    source: ScrollSource,
    initial: Option<f32>,
    row_drag: Option<RowDrag>,
    scroll_child_drags: bool,
}

#[derive(Clone, Copy)]
pub enum RowDrag {
    Move(f32),
    Release(f32),
}

#[derive(Clone, Copy, Default)]
struct AreaState {
    viewport: Option<Rect>,
    max: f32,
    scroll: ScrollOffset,
    held: bool,
    drag_displaced: bool,
    direction_chosen: bool,
    redirected: bool,
    trackpad: Option<(f32, f64)>,
    suppress_system_momentum: bool,
}

impl ScrollArea {
    pub fn horizontal() -> Self {
        Self::new(true, egui::ScrollArea::horizontal())
    }

    pub fn vertical() -> Self {
        Self::new(false, egui::ScrollArea::vertical())
    }

    fn new(horizontal: bool, layout: egui::ScrollArea) -> Self {
        Self {
            layout,
            horizontal,
            key: IdSalt::new("scroll area"),
            source: ScrollSource::ALL,
            initial: None,
            row_drag: None,
            scroll_child_drags: false,
        }
    }

    pub fn id_salt(mut self, salt: impl AsIdSalt) -> Self {
        self.key = IdSalt::new(&salt);
        self.layout = self.layout.id_salt(salt);
        self
    }

    pub fn scroll_bar_visibility(mut self, visibility: ScrollBarVisibility) -> Self {
        self.layout = self.layout.scroll_bar_visibility(visibility);
        self
    }

    pub fn auto_shrink(mut self, shrink: impl Into<Vec2b>) -> Self {
        self.layout = self.layout.auto_shrink(shrink);
        self
    }

    pub fn scroll_source(mut self, source: ScrollSource) -> Self {
        self.source = source;
        self
    }

    pub fn vertical_scroll_offset(mut self, offset: f32) -> Self {
        self.initial = Some(offset);
        self
    }

    pub fn row_drag(mut self, input: Option<RowDrag>) -> Self {
        self.row_drag = input;
        self
    }

    /// Rows that swipe sideways can give their perpendicular drag to the list.
    pub fn scroll_child_drags(mut self, enable: bool) -> Self {
        self.scroll_child_drags = enable;
        self
    }

    pub fn show<R>(self, ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R> {
        self.show_viewport(ui, |ui, _| content(ui))
    }

    pub fn show_viewport<R>(self, ui: &mut Ui, content: impl FnOnce(&mut Ui, Rect) -> R) -> ScrollAreaOutput<R> {
        let Self { mut layout, horizontal, key, source, initial, row_drag, scroll_child_drags } = self;
        let along = |v: Vec2| if horizontal { v.x } else { v.y };
        let across = |v: Vec2| if horizontal { v.y } else { v.x };
        let id = ui.make_persistent_id(("kinetic", key));
        let mut state = ui.data(|data| data.get_temp::<AreaState>(id)).unwrap_or_default();
        let rect = state.viewport;
        let scrollable = ui.is_enabled() && state.max > 0.0;
        let enabled = scrollable && source.drag != DragScroll::Never;
        let background = rect.filter(|_| enabled).map(|rect| ui.interact(rect, id, Sense::drag()));
        let (now, wheel_source, frame_time) = ui.input(|input| (input.time, input.scroll_source(), input.stable_dt));
        let on_trackpad = wheel_source == Some(MouseWheelSource::Trackpad);
        let viewport = rect.map_or(1.0, |rect| along(rect.size()));
        let mut repaint = false;
        if on_trackpad {
            state.suppress_system_momentum = false;
        } else if let Some((speed, last_move)) = state.trackpad.take()
            && source.mouse_wheel
            && scrollable
        {
            // A pause with fingers still down dissipates the launch velocity.
            let speed = if now - last_move < 0.12 { speed } else { 0.0 };
            state.scroll.release(speed, state.max, viewport, now);
            // The Rust coast replaces the momentum events that the OS may send next.
            state.suppress_system_momentum = true;
        }
        if let Some(offset) = initial {
            state.scroll.stop();
            state.scroll.position = offset;
        } else if !scrollable || (!enabled && !source.mouse_wheel) {
            state.held = false;
            state.redirected = false;
            state.trackpad = None;
            state.suppress_system_momentum = false;
            state.scroll.stop();
            state.scroll.position = state.scroll.position.clamp(0.0, state.max);
        } else {
            let (velocity, pressed, down) = ui.input(|input| {
                (along(input.pointer.velocity()), input.pointer.primary_pressed(), input.pointer.primary_down())
            });
            if enabled && pressed && rect.is_some_and(|rect| ui.rect_contains_pointer(rect)) {
                state.scroll.catch(now);
                state.held = true;
                state.drag_displaced = false;
                state.direction_chosen = false;
                state.redirected = false;
            }
            let scaling = ui.ctx().layer_transform_from_global(ui.layer_id()).map_or(1.0, |t| t.scaling);
            let mut first_move = None;
            if scroll_child_drags
                && state.held
                && !state.direction_chosen
                && down
                && ui.ctx().dragged_id().is_some_and(|dragged| dragged != id)
                && let Some(total) = ui.input(|i| i.pointer.total_drag_delta())
                && total.length() * scaling > 6.0
            {
                state.direction_chosen = true;
                if along(total).abs() > across(total).abs() {
                    ui.ctx().set_dragged_id(id);
                    state.redirected = true;
                    first_move = Some(along(total));
                }
            }
            if let Some(RowDrag::Move(delta)) = row_drag {
                state.scroll.drag(delta, state.max, viewport);
            } else if let Some(RowDrag::Release(velocity)) = row_drag {
                state.scroll.release(velocity, state.max, viewport, now);
                state.held = false;
            } else if state.redirected {
                if down {
                    let delta = first_move.unwrap_or_else(|| ui.input(|i| along(i.pointer.delta())));
                    state.scroll.drag(-delta * scaling, state.max, viewport);
                } else {
                    state.scroll.release(-velocity * scaling, state.max, viewport, now);
                    state.held = false;
                    state.redirected = false;
                }
            } else if let Some(hit) = background.as_ref().filter(|hit| hit.dragged_by(PointerButton::Primary)) {
                state.held = true;
                // A new touch can move the pointer to a different spot in the press frame.
                let delta = if pressed { hit.total_drag_delta().unwrap_or_default() } else { hit.drag_delta() };
                let delta = along(delta);
                state.drag_displaced |= delta != 0.0;
                state.scroll.drag(-delta, state.max, viewport);
            } else if background.as_ref().is_some_and(|hit| hit.drag_stopped_by(PointerButton::Primary)) {
                let speed = if state.drag_displaced { -velocity * scaling } else { 0.0 };
                state.scroll.release(speed, state.max, viewport, now);
                state.held = false;
            } else if state.held && !down {
                state.held = false;
                if !(0.0..=state.max).contains(&state.scroll.position) {
                    state.scroll.release(0.0, state.max, viewport, now);
                }
            }
            if !state.held && state.trackpad.is_none() {
                repaint = state.scroll.advance(now);
            }
        }

        layout = if horizontal {
            layout.horizontal_scroll_offset(state.scroll.position)
        } else {
            layout.vertical_scroll_offset(state.scroll.position)
        };
        let source_for_layout =
            ScrollSource { drag: DragScroll::Never, mouse_wheel: false, scroll_bar: source.scroll_bar };
        let native_bounce = ui.spacing().scroll.kinetic.rubber_band;
        ui.style_mut().spacing.scroll.kinetic.rubber_band = false;
        let result = layout.scroll_source(source_for_layout).show_viewport(ui, content);
        ui.style_mut().spacing.scroll.kinetic.rubber_band = native_bounce;

        state.viewport = Some(result.inner_rect);
        let max = along(result.content_size - result.inner_rect.size()).max(0.0);
        let actual = along(result.state.clamped_offset());
        if (actual - state.scroll.position.clamp(0.0, max)).abs() > 0.1 || (state.max - max).abs() > 0.1 {
            // A scrollbar, scroll-to request or content size change superseded the gesture.
            state.scroll.stop();
            state.scroll.position = actual;
        }
        state.max = max;
        // Inner scroll areas and child widgets get the first opportunity to take wheel input.
        if source.mouse_wheel && ui.is_enabled() && max > 0.0 && ui.rect_contains_pointer(result.inner_rect) {
            let only_direction = ui.style().always_scroll_the_only_direction;
            let nested = ui.stack().iter().any(|parent| parent.kind() == Some(UiKind::ScrollArea));
            let delta = ui.input(|input| {
                let wheel = input.smooth_scroll_delta;
                if only_direction { -wheel.x - wheel.y } else { -along(wheel) }
            });
            let position = state.scroll.position;
            let can_scroll = if delta < 0.0 { position > 0.0 } else { position < max };
            let suppress = state.suppress_system_momentum && wheel_source == Some(MouseWheelSource::Momentum);
            if delta != 0.0 && (can_scroll || !nested || state.trackpad.is_some() || suppress) {
                ui.input_mut(|input| {
                    if only_direction {
                        input.smooth_scroll_delta = egui::Vec2::ZERO;
                    } else if horizontal {
                        input.smooth_scroll_delta.x = 0.0;
                    } else {
                        input.smooth_scroll_delta.y = 0.0;
                    }
                });
                if on_trackpad {
                    let (speed, elapsed) = if let Some((previous, last_move)) = state.trackpad {
                        (previous, (now - last_move) as f32)
                    } else {
                        state.scroll.catch(now);
                        (0.0, frame_time)
                    };
                    let sample = delta / elapsed.clamp(1.0 / 240.0, 0.05);
                    state.trackpad = Some((if elapsed < 0.05 { sample * 0.65 + speed * 0.35 } else { sample }, now));
                    state.scroll.drag(delta, max, along(result.inner_rect.size()));
                    repaint = true;
                } else if !suppress {
                    state.scroll.wheel(delta, max);
                    repaint = true;
                }
            }
        }
        if repaint {
            ui.request_repaint();
        }
        ui.data_mut(|data| data.insert_temp(id, state));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, Event, Modifiers, MouseWheelUnit, Pos2, RawInput, TouchPhase, pos2, vec2};

    fn scroll_harness(rows: Sense) -> impl FnMut(Vec<Event>) -> f32 {
        let ctx = Context::default();
        let mut time = 0.0;
        move |events| {
            time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(240.0, 200.0))),
                time: Some(time),
                events,
                ..RawInput::default()
            };
            let mut offset = 0.0;
            let mut output = ctx.run_ui(input, |ui| {
                offset = ScrollArea::vertical()
                    .scroll_child_drags(true)
                    .show(ui, |ui| {
                        for _ in 0..16 {
                            ui.allocate_exact_size(vec2(220.0, 40.0), rows);
                        }
                    })
                    .state
                    .clamped_offset()
                    .y;
            });
            output.textures_delta.clear();
            offset
        }
    }

    fn click(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    #[test]
    fn scroll_area_turns_vertical_row_drags_into_a_flick() {
        let mut show = scroll_harness(Sense::click_and_drag());
        show(vec![]);
        show(vec![]);
        let start = pos2(40.0, 131.0);
        show(vec![Event::PointerMoved(start), click(start, true)]);
        show(vec![Event::PointerMoved(pos2(41.0, 112.0))]);
        show(vec![Event::PointerMoved(pos2(41.0, 95.0))]);
        let dragged = show(vec![Event::PointerMoved(pos2(42.0, 77.0))]);
        show(vec![click(pos2(42.0, 77.0), false)]);
        let mut coasted = 0.0;
        for _ in 0..8 {
            coasted = show(vec![]);
        }
        assert!(dragged > 40.0, "the list moved only {dragged} during the drag");
        assert!(coasted > dragged + 20.0, "the list moved from {dragged} to {coasted} after release");
    }

    #[test]
    fn a_press_after_repositioning_the_pointer_does_not_move_the_list() {
        let mut show = scroll_harness(Sense::hover());
        let from = pos2(40.0, 70.0);
        let to = pos2(40.0, 160.0);
        show(vec![Event::PointerMoved(to)]);
        show(vec![]);
        show(vec![click(to, true)]);
        show(vec![Event::PointerMoved(from)]);
        for _ in 0..10 {
            show(vec![Event::PointerMoved(from)]);
        }
        show(vec![click(from, false)]);
        let offset = show(vec![]);
        assert!(offset > 80.0);
        assert_eq!(show(vec![Event::PointerMoved(to), click(to, true)]), offset);
        show(vec![click(to, false)]);
        assert_eq!(show(vec![]), offset);
    }

    #[test]
    fn scroll_area_coasts_after_a_trackpad_gesture_without_adding_system_momentum() {
        let with_flick = |system_momentum| {
            let mut show = scroll_harness(Sense::hover());
            let event = |phase, source, y| Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: vec2(0.0, y),
                phase,
                source,
                modifiers: Modifiers::NONE,
            };
            let trackpad = |phase, y| event(phase, MouseWheelSource::Trackpad, y);
            show(vec![Event::PointerMoved(pos2(80.0, 80.0))]);
            show(vec![trackpad(TouchPhase::Start, 0.0)]);
            for _ in 0..4 {
                show(vec![trackpad(TouchPhase::Move, -14.0)]);
            }
            let released = show(vec![trackpad(TouchPhase::End, 0.0)]);
            let mut after = released;
            for _ in 0..8 {
                let events = if system_momentum {
                    vec![event(TouchPhase::Move, MouseWheelSource::Momentum, -25.0)]
                } else {
                    vec![]
                };
                after = show(events);
            }
            (released, after)
        };
        let (released, after) = with_flick(false);
        assert!(after > released + 20.0);
        assert_eq!(with_flick(true), (released, after));
    }

    #[test]
    fn a_mouse_wheel_scrolls_by_its_notches_and_never_coasts() {
        let mut show = scroll_harness(Sense::hover());
        let wheel = |y| Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, y),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        };
        show(vec![Event::PointerMoved(pos2(80.0, 80.0))]);
        show(vec![wheel(-50.0)]);
        // egui spreads a notch over a few frames
        let mut settled = 0.0;
        for _ in 0..60 {
            settled = show(vec![]);
        }
        assert!((settled - 50.0).abs() < 0.5, "a 50 point notch scrolled {settled}");
        // and nothing moves on after it
        for _ in 0..30 {
            assert_eq!(show(vec![]), settled);
        }
    }
}
