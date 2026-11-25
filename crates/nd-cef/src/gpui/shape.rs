//! The shape of a Chrome-style page's native window under the overlay layer. The window system
//! stacks a child window above everything its parent draws, so GPUI's overlays would sit under
//! the page: the window leaves out what the overlay layer draws this frame
//! (`nd_backend_gpui::occlusion`) and stops taking the pointer where the layer takes it, and a
//! snapshot of the page fills the holes so the overlay draws over the page instead of over
//! nothing.

/// Shadows and rounded corners of an overlay reach past its bounds: the hole it cuts in the page
/// is this much larger, in device pixels at scale 1.
const SHADOW: i32 = 12;

/// A rectangle in device pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct R {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl R {
    pub(crate) fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    fn right(self) -> i32 {
        self.x + self.w
    }

    fn bottom(self) -> i32 {
        self.y + self.h
    }

    fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub(crate) fn intersect(self, o: R) -> Option<R> {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        let r = R::new(x, y, self.right().min(o.right()) - x, self.bottom().min(o.bottom()) - y);
        (!r.is_empty()).then_some(r)
    }

    pub(crate) fn offset(self, dx: i32, dy: i32) -> R {
        R::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub(crate) fn inflate(self, by: i32) -> R {
        R::new(self.x - by, self.y - by, self.w + 2 * by, self.h + 2 * by)
    }
}

/// `region` without `hole`, as disjoint rectangles: each one it overlaps splits into the bands
/// above and below the hole and the pieces left and right of it.
pub(crate) fn subtract(region: &[R], hole: R) -> Vec<R> {
    let mut out = Vec::with_capacity(region.len() + 3);
    for &r in region {
        let Some(i) = r.intersect(hole) else {
            out.push(r);
            continue;
        };
        let pieces = [
            R::new(r.x, r.y, r.w, i.y - r.y),
            R::new(r.x, i.bottom(), r.w, r.bottom() - i.bottom()),
            R::new(r.x, i.y, i.x - r.x, i.h),
            R::new(i.right(), i.y, r.right() - i.right(), i.h),
        ];
        out.extend(pieces.into_iter().filter(|p| !p.is_empty()));
    }
    out
}

/// What of the page's window shows and what takes the pointer, in its own coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Shape {
    pub bounding: Vec<R>,
    pub input: Vec<R>,
    /// Some overlay cuts into what would show.
    pub cut: bool,
}

/// The shape of a page whose window covers `frame` and shows `visible` of it (both in window
/// coordinates), with its corners (top left, top right, bottom right, bottom left) rounded by
/// `radii` device pixels, under overlays given as (bounds, draws, casts a shadow, takes the
/// pointer) at `scale`.
pub(crate) fn shape(
    frame: R,
    visible: Option<R>,
    overlays: &[(R, bool, bool, bool)],
    scale: f32,
    radii: [i32; 4],
) -> Shape {
    let Some(visible) = visible.and_then(|v| v.intersect(frame)) else { return Shape::default() };
    let pad = (SHADOW as f32 * scale).round() as i32;
    let mut bounding = vec![visible];
    for corner in corners(frame, radii) {
        bounding = subtract(&bounding, corner);
    }
    let mut cut = false;
    for &(bounds, draws, shadow, _) in overlays {
        let drawn = if shadow { bounds.inflate(pad) } else { bounds };
        // An element laid out to nothing draws nothing, and its shadow pad would still cut.
        if draws && !bounds.is_empty() && drawn.intersect(visible).is_some() {
            bounding = subtract(&bounding, drawn);
            cut = true;
        }
    }
    let mut input = bounding.clone();
    for &(bounds, _, _, blocks) in overlays {
        if blocks {
            input = subtract(&input, bounds);
        }
    }
    let local = |rs: Vec<R>| rs.into_iter().map(|r| r.offset(-frame.x, -frame.y)).collect();
    Shape { bounding: local(bounding), input: local(input), cut }
}

/// What rounded corners leave out of `frame`: a strip per pixel row at each corner, as wide as
/// the arc is from the edge on that row. A window shape is all or nothing per pixel, so the arc
/// is the nearest pixel, not antialiased; whatever draws around the page covers the step.
fn corners(frame: R, radii: [i32; 4]) -> Vec<R> {
    let mut out = Vec::new();
    for (corner, radius) in radii.into_iter().enumerate() {
        let r = radius.min(frame.w / 2).min(frame.h / 2);
        for row in 0..r {
            let dy = r as f32 - row as f32 - 0.5;
            let inset = (r as f32 - (r as f32 * r as f32 - dy * dy).sqrt()).round() as i32;
            if inset <= 0 {
                continue;
            }
            let y = if corner < 2 { frame.y + row } else { frame.bottom() - 1 - row };
            let x = if corner == 0 || corner == 3 { frame.x } else { frame.right() - inset };
            out.push(R::new(x, y, inset, 1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(rs: &[R]) -> i32 {
        rs.iter().map(|r| r.w * r.h).sum()
    }

    #[test]
    fn rounded_corners_leave_the_corner_pixels_out_and_the_edges_in() {
        let frame = R::new(10, 20, 200, 100);
        let s = shape(frame, Some(frame), &[], 2., [20; 4]);
        let shows = |x: i32, y: i32| s.bounding.iter().any(|r| r.intersect(R::new(x, y, 1, 1)).is_some());
        for (x, y) in [(0, 0), (199, 0), (0, 99), (199, 99), (3, 3), (196, 96)] {
            assert!(!shows(x, y), "corner pixel {x},{y} shows");
        }
        for (x, y) in [(100, 0), (0, 50), (199, 50), (100, 99), (20, 20), (180, 80)] {
            assert!(shows(x, y), "pixel {x},{y} is cut");
        }
        assert_eq!(s.input, s.bounding);
        assert!(!s.cut, "corners are not an overlay's hole");
        let square = shape(frame, Some(frame), &[], 2., [0; 4]);
        assert_eq!(square.bounding, vec![R::new(0, 0, 200, 100)]);
        // Only the left corners, as for a page with the DevTools docked to its right.
        let left = shape(frame, Some(frame), &[], 2., [20, 0, 0, 20]);
        let shows = |x: i32, y: i32| left.bounding.iter().any(|r| r.intersect(R::new(x, y, 1, 1)).is_some());
        assert!(!shows(0, 0) && !shows(0, 99) && shows(199, 0) && shows(199, 99));
    }

    #[test]
    fn a_hole_in_the_middle_leaves_four_pieces() {
        let out = subtract(&[R::new(0, 0, 100, 100)], R::new(40, 40, 20, 20));
        assert_eq!(out.len(), 4);
        assert_eq!(area(&out), 100 * 100 - 20 * 20);
        for (i, a) in out.iter().enumerate() {
            assert!(out[i + 1..].iter().all(|b| a.intersect(*b).is_none()), "pieces overlap");
            assert!(a.intersect(R::new(40, 40, 20, 20)).is_none(), "a piece covers the hole");
        }
    }

    #[test]
    fn a_hole_over_everything_leaves_nothing_and_one_outside_changes_nothing() {
        assert!(subtract(&[R::new(10, 10, 50, 50)], R::new(0, 0, 100, 100)).is_empty());
        let page = [R::new(10, 10, 50, 50)];
        assert_eq!(subtract(&page, R::new(70, 70, 10, 10)), page);
    }

    #[test]
    fn an_overlay_laid_out_to_nothing_cuts_nothing() {
        let frame = R::new(0, 0, 800, 600);
        let s = shape(frame, Some(frame), &[(R::new(10, 590, 0, 0), true, true, false)], 1., [0; 4]);
        assert!(!s.cut);
        assert_eq!(s.bounding, vec![frame]);
    }

    #[test]
    fn a_modal_backdrop_takes_all_input_and_cuts_nothing() {
        let frame = R::new(0, 40, 800, 560);
        let s = shape(frame, Some(frame), &[(R::new(0, 0, 800, 600), false, false, true)], 1., [0; 4]);
        assert_eq!(s.bounding, vec![R::new(0, 0, 800, 560)]);
        assert!(s.input.is_empty());
        assert!(!s.cut);
    }

    #[test]
    fn window_chrome_above_the_page_casts_no_shadow_into_it() {
        let frame = R::new(0, 32, 800, 568);
        let s = shape(frame, Some(frame), &[(R::new(660, 0, 140, 32), true, false, true)], 1., [0; 4]);
        assert!(!s.cut);
        assert_eq!(s.bounding, vec![R::new(0, 0, 800, 568)]);
    }

    #[test]
    fn a_popover_cuts_a_padded_hole_in_window_coordinates() {
        let frame = R::new(100, 100, 400, 300);
        let popover = R::new(150, 120, 100, 50);
        let s = shape(frame, Some(frame), &[(popover, true, true, true)], 2., [0; 4]);
        assert!(s.cut);
        let hole = popover.inflate(24).offset(-100, -100);
        assert!(s.bounding.iter().all(|r| r.intersect(hole).is_none()));
        assert_eq!(area(&s.bounding), 400 * 300 - area(&[hole.intersect(R::new(0, 0, 400, 300)).unwrap()]));
    }

    #[test]
    fn a_page_scrolled_out_of_its_clip_shows_nothing() {
        let frame = R::new(0, 0, 100, 100);
        assert_eq!(shape(frame, Some(R::new(200, 0, 10, 10)), &[], 1., [0; 4]), Shape::default());
        let s = shape(frame, Some(R::new(0, 50, 100, 100)), &[], 1., [0; 4]);
        assert_eq!(s.bounding, vec![R::new(0, 50, 100, 50)]);
    }
}
