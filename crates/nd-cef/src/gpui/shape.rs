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
/// coordinates), under overlays given as (bounds, draws, takes the pointer) at `scale`.
pub(crate) fn shape(frame: R, visible: Option<R>, overlays: &[(R, bool, bool)], scale: f32) -> Shape {
    let Some(visible) = visible.and_then(|v| v.intersect(frame)) else { return Shape::default() };
    let pad = (SHADOW as f32 * scale).round() as i32;
    let mut bounding = vec![visible];
    let mut cut = false;
    for &(bounds, draws, _) in overlays {
        if draws && bounds.inflate(pad).intersect(visible).is_some() {
            bounding = subtract(&bounding, bounds.inflate(pad));
            cut = true;
        }
    }
    let mut input = bounding.clone();
    for &(bounds, _, blocks) in overlays {
        if blocks {
            input = subtract(&input, bounds);
        }
    }
    let local = |rs: Vec<R>| rs.into_iter().map(|r| r.offset(-frame.x, -frame.y)).collect();
    Shape { bounding: local(bounding), input: local(input), cut }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(rs: &[R]) -> i32 {
        rs.iter().map(|r| r.w * r.h).sum()
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
    fn a_modal_backdrop_takes_all_input_and_cuts_nothing() {
        let frame = R::new(0, 40, 800, 560);
        let s = shape(frame, Some(frame), &[(R::new(0, 0, 800, 600), false, true)], 1.);
        assert_eq!(s.bounding, vec![R::new(0, 0, 800, 560)]);
        assert!(s.input.is_empty());
        assert!(!s.cut);
    }

    #[test]
    fn a_popover_cuts_a_padded_hole_in_window_coordinates() {
        let frame = R::new(100, 100, 400, 300);
        let popover = R::new(150, 120, 100, 50);
        let s = shape(frame, Some(frame), &[(popover, true, true)], 2.);
        assert!(s.cut);
        let hole = popover.inflate(24).offset(-100, -100);
        assert!(s.bounding.iter().all(|r| r.intersect(hole).is_none()));
        assert_eq!(area(&s.bounding), 400 * 300 - area(&[hole.intersect(R::new(0, 0, 400, 300)).unwrap()]));
    }

    #[test]
    fn a_page_scrolled_out_of_its_clip_shows_nothing() {
        let frame = R::new(0, 0, 100, 100);
        assert_eq!(shape(frame, Some(R::new(200, 0, 10, 10)), &[], 1.), Shape::default());
        let s = shape(frame, Some(R::new(0, 50, 100, 100)), &[], 1.);
        assert_eq!(s.bounding, vec![R::new(0, 50, 100, 50)]);
    }
}
