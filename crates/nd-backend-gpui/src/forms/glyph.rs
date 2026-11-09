//! Every icon the controls draw, as alpha masks of `nd-icons`' Lucide shapes (Segoe Fluent
//! Icons under the Fluent preset on Windows), plus the arcs of spinners and circular progress.

use std::f32::consts::PI;

use nd_components::Icon;
use nd_gpui::gpui::{
    self, AnyElement, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Point, SharedString, Styled,
    TransformationMatrix, canvas, point, radians,
};
use nd_icons::{Seg, Shape};

/// An icon drawn as one alpha mask: Lucide's strokes (round caps and joins), tinted by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Glyph(Icon);

#[allow(non_upper_case_globals)]
impl Glyph {
    pub const Check: Glyph = Glyph(Icon::Check);
    pub const Dash: Glyph = Glyph(Icon::Minus);
    pub const Minus: Glyph = Glyph(Icon::Minus);
    pub const Plus: Glyph = Glyph(Icon::Plus);
    pub const ChevronDown: Glyph = Glyph(Icon::ChevronDown);
    pub const ChevronUp: Glyph = Glyph(Icon::ChevronUp);
    pub const ChevronLeft: Glyph = Glyph(Icon::ChevronLeft);
    pub const ChevronRight: Glyph = Glyph(Icon::ChevronRight);
    pub const Search: Glyph = Glyph(Icon::Search);
    pub const Clear: Glyph = Glyph(Icon::X);
    pub const LocateFixed: Glyph = Glyph(Icon::LocateFixed);
    pub const Person: Glyph = Glyph(Icon::User);

    pub fn of(icon: Icon) -> Self {
        Glyph(icon)
    }

    /// The glyph for a Lucide icon name (what a button's `icon` carries), when nd or the app
    /// has an icon by that name.
    pub fn named(name: &str) -> Option<Self> {
        Icon::from_name(name).map(Glyph)
    }
}

fn arc_points(center: (f32, f32), radius: f32, start: f32, sweep: f32, segments: usize) -> Vec<(f32, f32)> {
    (0..=segments)
        .map(|i| {
            let a = start + sweep * i as f32 / segments as f32;
            (center.0 + radius * a.cos(), center.1 + radius * a.sin())
        })
        .collect()
}

fn stroke(bounds: Bounds<Pixels>, lines: &[Vec<(f32, f32)>], width: Pixels, color: Hsla, window: &mut gpui::Window) {
    let at = |(x, y): (f32, f32)| -> Point<Pixels> {
        point(bounds.origin.x + bounds.size.width * x, bounds.origin.y + bounds.size.height * y)
    };
    for line in lines {
        let mut path = PathBuilder::stroke(width);
        let mut points = line.iter();
        let Some(first) = points.next() else { continue };
        path.move_to(at(*first));
        for p in points {
            path.line_to(at(*p));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
}

/// `glyph` as an SVG on its own grid, its strokes `width` grid units thick.
fn svg(shape: &Shape, width: f32) -> String {
    use std::fmt::Write;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {0} {0}" fill="none" stroke="black" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round">"#,
        shape.size
    );
    for piece in shape.paths {
        out.push_str(r#"<path d=""#);
        for seg in piece.segs {
            let _ = match *seg {
                Seg::Move(x, y) => write!(out, "M{x} {y}"),
                Seg::Line(x, y) => write!(out, "L{x} {y}"),
                Seg::Cubic(a, b, c, d, x, y) => write!(out, "C{a} {b} {c} {d} {x} {y}"),
                Seg::Close => write!(out, "Z"),
            };
        }
        out.push_str(if piece.filled { r#"" fill="black" stroke="none"/>"# } else { r#""/>"# });
    }
    out.push_str("</svg>");
    out
}

/// `glyph` drawn in `bounds`, its strokes `width` thick however big the box is, turned `turn`
/// radians clockwise about the box's centre. It is rasterized as one alpha mask and tinted once,
/// so where its strokes cross a translucent colour does not build up as overlapping paths do.
fn paint_glyph(
    glyph: Glyph,
    bounds: Bounds<Pixels>,
    color: Hsla,
    width: Pixels,
    turn: f32,
    window: &mut gpui::Window,
    cx: &gpui::App,
) {
    let Some(shape) = glyph.0.shape() else { return };
    if bounds.size.width <= Pixels::ZERO {
        return;
    }
    // Rounded so every frame of a box this size names the same atlas tile.
    let units = (width / bounds.size.width * shape.size * 100.).round() / 100.;
    let key = SharedString::from(format!("nd-glyph/{}/{units}", glyph.0.lucide()));
    let transformation = if turn == 0. {
        TransformationMatrix::unit()
    } else {
        let centre = bounds.center();
        let scale = window.scale_factor();
        TransformationMatrix::unit()
            .translate(centre.scale(scale))
            .rotate(radians(turn))
            .translate(centre.scale(-scale))
    };
    let _ = window.paint_svg(bounds, key, Some(svg(&shape, units).as_bytes()), transformation, color, cx);
}

/// [`glyph`] turned `turn` radians clockwise about its centre: a disclosure chevron mid-turn.
/// Segoe Fluent Icons' glyphs are text and do not turn: past halfway they show `then`.
pub(crate) fn turned_glyph(
    glyph: Glyph,
    then: Glyph,
    turn: f32,
    size: Pixels,
    color: Hsla,
    width: Pixels,
) -> AnyElement {
    if ICON_FONT.get() {
        return self::glyph(if turn.abs() < PI / 2. { glyph } else { then }, size, color, width);
    }
    canvas(|_, _, _| (), move |bounds, _, window, cx| paint_glyph(glyph, bounds, color, width, turn, window, cx))
        .size(size)
        .flex_none()
        .into_any_element()
}

/// `icon` drawn as [`glyph`] draws it, for chrome outside this crate (the OS hooks' window
/// controls).
pub fn icon_glyph(icon: Icon, size: Pixels, color: Hsla, width: Pixels) -> AnyElement {
    glyph(Glyph(icon), size, color, width)
}

/// `glyph` in a `size` square.
pub(crate) fn glyph(glyph: Glyph, size: Pixels, color: Hsla, width: Pixels) -> AnyElement {
    if ICON_FONT.get()
        && let Some(code) = glyph.0.segoe_fluent()
    {
        return crate::fluent_icons::glyph(code, size, color).into_any_element();
    }
    canvas(|_, _, _| (), move |bounds, _, window, cx| paint_glyph(glyph, bounds, color, width, 0., window, cx))
        .size(size)
        .flex_none()
        .into_any_element()
}

thread_local! {
    /// Whether [`glyph`] draws from Segoe Fluent Icons, for the window being built: the
    /// Fluent preset on Windows, the one OS that ships the font.
    static ICON_FONT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn set_icon_font(theme: &nd_theme::Theme) {
    ICON_FONT.set(cfg!(target_os = "windows") && theme.preset == nd_theme::Preset::Fluent);
}

/// An arc from `start` (radians, clockwise from three o'clock) through `sweep`, inset by half
/// the stroke so it stays inside a `size` square.
pub(crate) fn arc(size: Pixels, start: f32, sweep: f32, color: Hsla, width: Pixels) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if sweep.abs() < 0.01 {
                return;
            }
            let inset = (width / bounds.size.width) / 2.;
            let segments = ((sweep.abs() / (2. * PI)) * 48.).ceil().max(4.) as usize;
            let points = arc_points((0.5, 0.5), 0.5 - inset, start, sweep, segments);
            stroke(bounds, &[points], width, color, window);
        },
    )
    .size(size)
    .flex_none()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nd_components::Icon;
    use nd_gpui::gpui::SvgRenderer;

    use super::{Glyph, svg};

    #[test]
    fn every_icon_parses_as_svg() {
        let renderer = SvgRenderer::new(Arc::new(()));
        for icon in Icon::ALL {
            let shape = icon.shape().unwrap_or_else(|| panic!("{} has no shape", icon.lucide()));
            if let Err(e) = renderer.parse_svg(svg(&shape, 2.).as_bytes()) {
                panic!("{}: {e}", icon.lucide());
            }
        }
    }

    #[test]
    fn a_name_resolves_to_a_glyph() {
        assert!(Glyph::named("trash").is_some());
        assert!(Glyph::named("no-such-icon").is_none());
    }
}
