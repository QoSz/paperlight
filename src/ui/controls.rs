//! Apple-style controls: line icons painted as vectors, flat icon buttons and filled text fields.

use std::f32::consts::{FRAC_1_SQRT_2, TAU};

use egui::{
    Button, Color32, Frame, Margin, Painter, Pos2, Rect, Response, Shape, Stroke, StrokeKind,
    TextEdit, Ui, Vec2, pos2, vec2,
};

use super::theme::{self, RADIUS_MD};

/// Line width of icons, in points.
const ICON_STROKE: f32 = 1.5;
/// Half the side of the square an icon fits in.
const ICON_HALF: f32 = 5.0;
/// Width and opacity of the focus ring around a focused field: a soft accent halo.
const RING: f32 = 3.0;
const RING_ALPHA: f32 = 0.5;
/// Horizontal text padding of a field; with a leading icon, the left padding is `ICON_INSET`.
const FIELD_PAD: i8 = 8;
const ICON_INSET: i8 = 26;
/// Vertical text padding of a field.
const FIELD_PAD_Y: i8 = 5;

/// A line icon, after SF Symbols.
#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Search,
    Up,
    Down,
    Left,
    Right,
    Minus,
    Plus,
    RotateClockwise,
}

impl Icon {
    /// The shapes of `self` centred on `c`.
    fn shapes(self, c: Pos2, stroke: Stroke) -> Vec<Shape> {
        let h = ICON_HALF;
        let line = |points: &[Vec2]| Shape::line(points.iter().map(|&p| c + p).collect(), stroke);
        match self {
            Self::Search => {
                let (centre, r) = (c - Vec2::splat(1.0), h - 1.5);
                vec![
                    Shape::circle_stroke(centre, r, stroke),
                    Shape::line_segment(
                        [centre + Vec2::splat(r * FRAC_1_SQRT_2), c + vec2(h, h)],
                        stroke,
                    ),
                ]
            }
            Self::Up => vec![line(&[
                vec2(-h, h / 2.0),
                vec2(0.0, -h / 2.0),
                vec2(h, h / 2.0),
            ])],
            Self::Down => vec![line(&[
                vec2(-h, -h / 2.0),
                vec2(0.0, h / 2.0),
                vec2(h, -h / 2.0),
            ])],
            Self::Left => vec![line(&[
                vec2(h / 2.0, -h),
                vec2(-h / 2.0, 0.0),
                vec2(h / 2.0, h),
            ])],
            Self::Right => vec![line(&[
                vec2(-h / 2.0, -h),
                vec2(h / 2.0, 0.0),
                vec2(-h / 2.0, h),
            ])],
            Self::Minus => vec![line(&[vec2(-h, 0.0), vec2(h, 0.0)])],
            Self::Plus => vec![
                line(&[vec2(-h, 0.0), vec2(h, 0.0)]),
                line(&[vec2(0.0, -h), vec2(0.0, h)]),
            ],
            Self::RotateClockwise => rotate_clockwise(c, stroke),
        }
    }
}

/// An open circle running clockwise from 1:30 to 12 o'clock, with an arrowhead there
/// pointing on into the gap.
fn rotate_clockwise(c: Pos2, stroke: Stroke) -> Vec<Shape> {
    const STEPS: u8 = 32;
    const HEAD: f32 = 2.5;
    let r = ICON_HALF;
    let (start, end) = (-TAU / 8.0, TAU * 3.0 / 4.0);
    let arc = (0..=STEPS)
        .map(|k| c + r * Vec2::angled(start + (end - start) * f32::from(k) / f32::from(STEPS)))
        .collect();
    // Screen y points down, so a growing angle runs clockwise and its tangent is (-sin, cos).
    let out = Vec2::angled(end);
    let (tip, p) = (vec2(-out.y, out.x), c + r * out);
    vec![
        Shape::line(arc, stroke),
        Shape::convex_polygon(
            vec![p + HEAD * tip, p + HEAD * out, p - HEAD * out],
            stroke.color,
            Stroke::NONE,
        ),
    ]
}

/// Paints `icon` centred in `rect`.
pub fn paint_icon(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    painter.extend(icon.shapes(rect.center(), Stroke::new(ICON_STROKE, color)));
}

/// A flat square button `side` points wide showing `icon` in the button's text colour.
pub fn icon_button(ui: &mut Ui, icon: Icon, side: f32) -> Response {
    let resp = ui.add(Button::new("").min_size(Vec2::splat(side)));
    let color = ui.style().interact(&resp).fg_stroke.color;
    paint_icon(ui.painter(), icon, resp.rect, color);
    resp
}

/// Shows `edit` as a filled, rounded field with an optional leading `icon` and, while focused,
/// a soft accent ring outside it.
pub fn field(ui: &mut Ui, edit: TextEdit<'_>, icon: Option<Icon>) -> Response {
    let pal = theme::palette(ui.visuals().dark_mode);
    let left = if icon.is_some() {
        ICON_INSET
    } else {
        FIELD_PAD
    };
    let frame = Frame::new()
        .fill(pal.field_bg)
        .corner_radius(RADIUS_MD)
        .inner_margin(Margin {
            left,
            right: FIELD_PAD,
            top: FIELD_PAD_Y,
            bottom: FIELD_PAD_Y,
        });
    let resp = ui.add(edit.frame(frame));
    let painter = ui.painter();
    if let Some(icon) = icon {
        let c = pos2(
            resp.rect.left() + f32::from(ICON_INSET) / 2.0,
            resp.rect.center().y,
        );
        let rect = Rect::from_center_size(c, Vec2::splat(2.0 * ICON_HALF));
        paint_icon(painter, icon, rect, pal.text_secondary);
    }
    if resp.has_focus() {
        let ring = Stroke::new(RING, pal.accent.gamma_multiply(RING_ALPHA));
        painter.rect_stroke(resp.rect, RADIUS_MD, ring, StrokeKind::Outside);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes(icon: Icon) -> Vec<Shape> {
        icon.shapes(Pos2::ZERO, Stroke::new(1.0, Color32::WHITE))
    }

    #[test]
    fn icons_fit_their_box() {
        // The rotate arrowhead overhangs the arc; strokes add half their width.
        let limit = Rect::from_center_size(Pos2::ZERO, Vec2::splat(2.0 * ICON_HALF + 6.0));
        for icon in [
            Icon::Search,
            Icon::Up,
            Icon::Down,
            Icon::Left,
            Icon::Right,
            Icon::Minus,
            Icon::Plus,
            Icon::RotateClockwise,
        ] {
            let bounds = shapes(icon)
                .iter()
                .map(Shape::visual_bounding_rect)
                .fold(Rect::NOTHING, Rect::union);
            assert!(bounds.is_positive(), "{icon:?}: {bounds:?}");
            assert!(limit.contains_rect(bounds), "{icon:?}: {bounds:?}");
        }
    }

    #[test]
    fn rotate_arrowhead_points_clockwise_at_the_top() -> Result<(), String> {
        let Some(Shape::Path(head)) = shapes(Icon::RotateClockwise).pop() else {
            return Err("arrowhead is not the last shape".into());
        };
        let (tip, base) = (head.points[0], head.points[1].lerp(head.points[2], 0.5));
        // Above the centre, pointing right: clockwise.
        assert!(
            tip.y < 0.0 && base.y < 0.0 && tip.x > base.x,
            "{:?}",
            head.points
        );
        Ok(())
    }
}
