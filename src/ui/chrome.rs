//! Window chrome for the undecorated window: GNOME-style window buttons (on the right, in
//! GNOME's order), a title bar that moves and maximizes the window, resize bands along the
//! edges and a hairline outline.

use egui::{
    Color32, Context, CursorIcon, Event, Id, PointerButton, Pos2, RawInput, Rect, ResizeDirection,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2, ViewportCommand, pos2, vec2,
};

use super::theme::{self, RESIZE_EDGE};

/// Diameter of a window button, and the gap between two.
const BUTTON: f32 = 24.0;
const BUTTON_GAP: f32 = 6.0;
/// Width of the three buttons.
const BUTTONS_WIDTH: f32 = 3.0 * BUTTON + 2.0 * BUTTON_GAP;
/// Button fill: the text colour at these opacities (at rest, hovered, pressed), as libadwaita.
const FILL_ALPHA: [f32; 3] = [0.1, 0.15, 0.3];
/// Half the side of the square a button's icon fits in, and its line width.
const ICON_HALF: f32 = 4.0;
const ICON_STROKE: f32 = 1.2;

/// A window button.
#[derive(Clone, Copy, Debug, Hash)]
enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

/// Left to right, as GNOME's default `button-layout` (`appmenu:minimize,maximize,close`).
const BUTTONS: [WindowButton; 3] = [
    WindowButton::Minimize,
    WindowButton::Maximize,
    WindowButton::Close,
];
/// Side of the resize square at each corner (the edge bands are `RESIZE_EDGE` wide).
const CORNER: f32 = 12.0;

/// Minimize, maximize (restore while maximized) and close, GNOME style: round grey buttons
/// with symbolic icons. The icons are muted while the window is unfocused.
pub fn window_buttons(ui: &mut Ui) {
    let pal = theme::palette(ui.visuals().dark_mode);
    let (rect, _) = ui.allocate_exact_size(vec2(BUTTONS_WIDTH, BUTTON), Sense::hover());
    let (focused, maximized) = ui.input(|i| {
        let v = i.viewport();
        (v.focused.unwrap_or(true), v.maximized.unwrap_or(false))
    });
    let icon = if focused {
        pal.text_primary
    } else {
        pal.text_secondary
    };
    let mut c = pos2(rect.left() + BUTTON / 2.0, rect.center().y);
    for button in BUTTONS {
        let r = Rect::from_center_size(c, Vec2::splat(BUTTON));
        // Not focusable: Tab must not land on Close, where Enter would click it.
        let resp = ui.interact(r, Id::unique(("window-button", button)), Sense::CLICK);
        let state = if resp.is_pointer_button_down_on() {
            2
        } else {
            usize::from(resp.hovered())
        };
        let painter = ui.painter();
        painter.circle_filled(
            c,
            BUTTON / 2.0,
            pal.text_primary.gamma_multiply(FILL_ALPHA[state]),
        );
        painter.extend(glyph(button, maximized, c, icon));
        if resp.clicked() {
            ui.ctx().send_viewport_cmd(match button {
                WindowButton::Minimize => ViewportCommand::Minimized(true),
                WindowButton::Maximize => ViewportCommand::Maximized(!maximized),
                WindowButton::Close => ViewportCommand::Close,
            });
        }
        c.x += BUTTON + BUTTON_GAP;
    }
}

/// `button`'s symbolic icon centred on `c`: a low bar, a square (two stacked squares to
/// restore while `maximized`), or a cross.
fn glyph(button: WindowButton, maximized: bool, c: Pos2, color: Color32) -> Vec<Shape> {
    let (h, stroke) = (ICON_HALF, Stroke::new(ICON_STROKE, color));
    let line = |points: &[Vec2]| Shape::line(points.iter().map(|&p| c + p).collect(), stroke);
    let square = |min: Vec2, max: Vec2| {
        Shape::rect_stroke(
            Rect::from_min_max(c + min, c + max),
            1.0,
            stroke,
            StrokeKind::Middle,
        )
    };
    match (button, maximized) {
        (WindowButton::Minimize, _) => vec![line(&[vec2(-h, h - 1.0), vec2(h, h - 1.0)])],
        (WindowButton::Maximize, false) => vec![square(vec2(-h, -h), vec2(h, h))],
        (WindowButton::Maximize, true) => vec![
            square(vec2(-h, -h + 2.5), vec2(h - 2.5, h)),
            line(&[
                vec2(-h + 2.5, -h + 2.5),
                vec2(-h + 2.5, -h),
                vec2(h, -h),
                vec2(h, h - 2.5),
                vec2(h - 2.5, h - 2.5),
            ]),
        ],
        (WindowButton::Close, _) => vec![
            line(&[vec2(-h, -h), vec2(h, h)]),
            line(&[vec2(-h, h), vec2(h, -h)]),
        ],
    }
}

/// Lets the bar move the window when dragged and toggle maximize on double-click. Call it
/// before adding the bar's controls, so they sit on top and keep their clicks.
pub fn title_bar(ui: &mut Ui) {
    // Not focusable: a focused bar would swallow the canvas keys (`free` in app.rs).
    let bar = ui.interact(
        ui.clip_rect(),
        Id::unique("title-bar"),
        Sense::CLICK | Sense::DRAG,
    );
    if bar.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if bar.drag_started_by(PointerButton::Primary) {
        start_grab(ui.ctx(), ViewportCommand::StartDrag);
    }
}

/// Resize bands along the window edges and a hairline outline, neither while the window is
/// maximized or fullscreen. Call it after the panels, so the bands sit on top of them.
pub fn window_edges(ui: &mut Ui) {
    let (maximized, fullscreen) = ui.input(|i| {
        let v = i.viewport();
        (v.maximized.unwrap_or(false), v.fullscreen.unwrap_or(false))
    });
    if maximized || fullscreen {
        return;
    }
    let pal = theme::palette(ui.visuals().dark_mode);
    let window = ui.ctx().viewport_rect();
    ui.painter().rect_stroke(
        window,
        0.0,
        Stroke::new(1.0, pal.toolbar_border),
        StrokeKind::Inside,
    );
    for (k, (band, direction, cursor)) in resize_bands(window).into_iter().enumerate() {
        // Sensing only drags makes the press start the drag at once, with no threshold.
        let resp = ui.interact(band, Id::unique(("resize", k)), Sense::DRAG);
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        // Unfocused, an X11 resize would keep the pointer grabbed (egui-winit guards
        // `StartDrag` the same way).
        if resp.drag_started_by(PointerButton::Primary)
            && ui.input(|i| i.viewport().focused.unwrap_or(true))
        {
            start_grab(ui.ctx(), ViewportCommand::BeginResize(direction));
        }
    }
}

/// The compositor takes over a move or resize and keeps the button release, so egui would
/// hold the drag (and hide hover) until the next click. Marks it for `release_after_grab`.
fn start_grab(ctx: &Context, command: ViewportCommand) {
    ctx.send_viewport_cmd(command);
    ctx.data_mut(|d| d.insert_temp(grab_id(), true));
}

fn grab_id() -> Id {
    Id::unique("compositor-grab")
}

/// After `start_grab`, puts the missing primary-button release at the front of the next
/// input. Call from `App::raw_input_hook`.
pub fn release_after_grab(ctx: &Context, raw_input: &mut RawInput) {
    if ctx.data_mut(|d| d.remove_temp::<bool>(grab_id())).is_none() {
        return;
    }
    let pos = raw_input
        .events
        .iter()
        .find_map(|e| match e {
            Event::PointerMoved(p) => Some(*p),
            _ => None,
        })
        .or_else(|| ctx.input(|i| i.pointer.latest_pos()))
        .unwrap_or_default();
    let modifiers = ctx.input(|i| i.modifiers);
    raw_input.events.insert(
        0,
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
    );
}

/// The resize bands of `window`: four edges, then four corners, so a corner wins where it
/// overlaps an edge (egui gives the pointer to the widget added last).
fn resize_bands(window: Rect) -> [(Rect, ResizeDirection, CursorIcon); 8] {
    let r = window;
    let corner = |x: f32, y: f32| Rect::from_center_size(pos2(x, y), Vec2::splat(2.0 * CORNER));
    [
        (
            Rect::from_min_max(r.left_top(), pos2(r.right(), r.top() + RESIZE_EDGE)),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(pos2(r.left(), r.bottom() - RESIZE_EDGE), r.right_bottom()),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(r.left_top(), pos2(r.left() + RESIZE_EDGE, r.bottom())),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(pos2(r.right() - RESIZE_EDGE, r.top()), r.right_bottom()),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
        (
            corner(r.left(), r.top()),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNorthWest,
        ),
        (
            corner(r.right(), r.top()),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            corner(r.left(), r.bottom()),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (
            corner(r.right(), r.bottom()),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeSouthEast,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direction of the last band containing `p`, as egui would pick it.
    fn direction_at(window: Rect, p: Pos2) -> Option<ResizeDirection> {
        resize_bands(window)
            .into_iter()
            .rev()
            .find(|(band, ..)| band.contains(p))
            .map(|(_, d, _)| d)
    }

    #[test]
    fn bands_cover_edges_and_corners_only() {
        let w = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        for (p, want) in [
            (pos2(400.0, 1.0), Some(ResizeDirection::North)),
            (pos2(400.0, 598.0), Some(ResizeDirection::South)),
            (pos2(1.0, 300.0), Some(ResizeDirection::West)),
            (pos2(798.0, 300.0), Some(ResizeDirection::East)),
            (pos2(2.0, 2.0), Some(ResizeDirection::NorthWest)),
            (pos2(8.0, 1.0), Some(ResizeDirection::NorthWest)),
            (pos2(797.0, 3.0), Some(ResizeDirection::NorthEast)),
            (pos2(3.0, 597.0), Some(ResizeDirection::SouthWest)),
            (pos2(790.0, 598.0), Some(ResizeDirection::SouthEast)),
            (pos2(400.0, 300.0), None),
            (pos2(400.0, RESIZE_EDGE + 1.0), None),
            (pos2(20.0, 20.0), None),
        ] {
            assert_eq!(direction_at(w, p), want, "{p:?}");
        }
    }

    #[test]
    fn glyphs_stay_inside_the_button() {
        for button in BUTTONS {
            for maximized in [false, true] {
                for shape in glyph(button, maximized, Pos2::ZERO, Color32::WHITE) {
                    let b = shape.visual_bounding_rect();
                    // Corners of the icon's bounds stay well inside the round button.
                    assert!(
                        b.min.distance(Pos2::ZERO) < BUTTON / 2.0 - 2.0,
                        "{button:?}: {b:?}"
                    );
                    assert!(
                        b.max.distance(Pos2::ZERO) < BUTTON / 2.0 - 2.0,
                        "{button:?}: {b:?}"
                    );
                }
            }
        }
    }
}
