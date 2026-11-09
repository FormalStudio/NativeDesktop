//! An expanded view presented over a region: a now-playing screen over the page, a detail over
//! its list. Inside it a `.hero_with(..)` element travels from its source and back, so the
//! motion says where the view came from, while the backdrop fades in under it on the same
//! spring. The base stays mounted, keeping its scroll and state, and is hidden once covered.
//!
//! The hero is either a shared element or a container. A shared element (the artwork) flies
//! alone: the rest of the view marks itself `.reveal_with(&state)` and fades in once the hero has
//! landed and out before it leaves, so nothing draws over it in flight. A container (a card
//! opening into its detail) holds the view: it grows from the source's frame, corner radius,
//! border and fill to its own, and clips what it holds. Inside it, the source's content marked
//! `.hero_content()` rides along at the source's size and fades out early in the flight, and the
//! detail marked `.reveal_with(&state)` is laid out at its final size and fades in as the
//! source's goes, so the container is never empty.
//!
//! ```ignore
//! // A card opening into its detail: the card's frame is keyed "card:7".
//! presentation("detail", &self.detail).base(grid).content(
//!     div()
//!         .absolute()
//!         .top_0()
//!         .left_0()
//!         .size(px(width), px(height))
//!         .bg(cx.theme().surface.raised)
//!         .overflow_hidden()
//!         .hero_with(&self.detail, "card:7")
//!         .child(card_face(note).hero_content())
//!         .child(div().absolute().top_0().left_0().size_full().reveal_with(&self.detail).child(editor)),
//! )
//! ```
//!
//! ```ignore
//! // The view keeps one handle; open and close it, then render.
//! self.expanded.set_open(true);
//! presentation("now-playing", &self.expanded)
//!     .base(page)
//!     .backdrop(image(cover).blur(px(60.)))
//!     .content(self.now_playing.clone())
//!     .bg(cx.theme().surface.base)
//! // Inside the content, the artwork, and the rest revealed with it:
//! div().size(px(480.)).child(
//!     div().absolute().size(px(480.)).hero_with(&expanded, "player.bar.art").child(art),
//! )
//! v_flex().reveal_with(&expanded).child(title)
//! ```
//!
//! Interruptible: closing mid-open turns everything around from where it is. Reduce motion
//! leaves short fades and no travel: a crossfade from the base to the view.

use std::cell::Cell;
use std::rc::Rc;

use nd_gpui::gpui::Refineable;
use nd_ui::prelude::*;
use nd_ui::{HeroPart, Pace, StyleRefinement};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Closed,
    Opening,
    Open,
    Closing,
}

/// Whether a presentation is open, kept by the view that shows it. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct Presentation(Rc<State>);

#[derive(Debug, Default)]
struct State {
    phase: Cell<Phase>,
    /// While closing: the backdrop has faded out, and the hero (if one rendered) has landed.
    faded: Cell<bool>,
    landed: Cell<bool>,
    /// A hero was drawn heading back during this close, so a landing will come.
    leaving: Cell<bool>,
}

impl Presentation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens or closes it; the view renders again to show the change.
    pub fn set_open(&self, open: bool) {
        let next = match (self.0.phase.get(), open) {
            (Phase::Closed | Phase::Closing, true) => Phase::Opening,
            (Phase::Opening | Phase::Open, false) => {
                self.0.faded.set(false);
                self.0.landed.set(false);
                self.0.leaving.set(false);
                Phase::Closing
            }
            (phase, _) => phase,
        };
        self.0.phase.set(next);
    }

    /// Open, or opening.
    pub fn is_open(&self) -> bool {
        matches!(self.0.phase.get(), Phase::Opening | Phase::Open)
    }

    /// Going away: a hero inside travels back to its source.
    pub fn is_closing(&self) -> bool {
        self.0.phase.get() == Phase::Closing
    }

    /// On screen, coming, staying or going.
    pub fn is_shown(&self) -> bool {
        self.0.phase.get() != Phase::Closed
    }

    /// The backdrop's fade ended. Closing ends once the hero has landed too, so the content
    /// stays mounted under it the whole way back.
    fn faded(&self) {
        match self.0.phase.get() {
            Phase::Opening => self.0.phase.set(Phase::Open),
            Phase::Closing => {
                self.0.faded.set(true);
                self.close_if_done();
            }
            _ => {}
        }
    }

    fn landed(&self) {
        if self.0.phase.get() == Phase::Closing {
            self.0.landed.set(true);
            self.close_if_done();
        }
    }

    fn close_if_done(&self) {
        if self.0.faded.get() && (self.0.landed.get() || !self.0.leaving.get()) {
            self.0.phase.set(Phase::Closed);
        }
    }
}

pub struct PresentationView {
    id: SharedString,
    state: Presentation,
    base: Option<AnyElement>,
    backdrop: Option<AnyElement>,
    content: Option<AnyElement>,
    layer: StyleRefinement,
}

/// `content` presented over `base` while `state` is open. Style it with the backdrop's
/// background.
#[track_caller]
pub fn presentation(id: impl Into<SharedString>, state: &Presentation) -> PresentationView {
    PresentationView {
        id: id.into(),
        state: state.clone(),
        base: None,
        backdrop: None,
        content: None,
        layer: StyleRefinement::default(),
    }
}

impl PresentationView {
    /// What sits under the presentation, mounted throughout.
    pub fn base(mut self, base: impl IntoElement) -> Self {
        self.base = Some(base.into_any_element());
        self
    }

    /// Drawn over the background and under the content, fading with it: blurred artwork, say.
    /// It fills the region and is clipped to it.
    pub fn backdrop(mut self, backdrop: impl IntoElement) -> Self {
        self.backdrop = Some(backdrop.into_any_element());
        self
    }

    /// The presented view, mounted only while shown.
    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }
}

impl Styled for PresentationView {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.layer
    }
}

impl IntoElement for PresentationView {
    fn into_any_element(self) -> AnyElement {
        let PresentationView { id, state, base, backdrop, content, layer } = self;
        let covered = state.0.phase.get() == Phase::Open;
        let shown = state.is_shown();
        let target = if state.is_open() { 1. } else { 0. };
        // Comes in on the hero's spring, goes out with the hero once the content has faded.
        let pace = if state.is_closing() { Animation::Themed(Pace::Exit) } else { Animation::natural() };
        let mut fill = div()
            .id(id)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .overflow_hidden()
            .opacity(target)
            .animate(pace)
            // Mounted mid-close (its parent changed shape), it still fades out and reports it.
            .enter_from(|p| p.opacity(1. - target))
            .on_animation_end(move |_, _, cx| {
                state.faded();
                cx.refresh_windows();
            })
            .children(backdrop);
        fill.style().refine(&layer);
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .child(div().size_full().when(covered, |b| b.invisible()).children(base))
            .when(shown, |root| root.child(fill).child(div().absolute().top_0().left_0().size_full().children(content)))
            .into_any_element()
    }
}

pub trait PresentationExt: Styled + Sized {
    /// Beside a shared element, fades in once a presentation's hero has landed and out before
    /// it leaves, so it never draws over the hero in flight. Inside a container hero it is the
    /// destination's content ([`HeroPart::Destination`]): laid out at its final size while the
    /// container grows around it, and fading in as the source's content fades out.
    fn reveal_with(self, state: &Presentation) -> Self;

    /// The source's content inside a container hero ([`HeroPart::Source`]): a card's title and
    /// preview, pinned to the container's top-left at the card's size, fading out early as the
    /// container leaves the card and back in as it lands on it.
    fn hero_content(self) -> Self;

    /// The presentation's shared element: it travels from the element keyed `from` as the
    /// presentation opens and back onto it as it closes ([`Div::hero_from`]), and the
    /// presentation stays on screen until it has landed.
    fn hero_with(self, state: &Presentation, from: impl Into<SharedString>) -> Self;
}

impl PresentationExt for Div {
    fn reveal_with(self, state: &Presentation) -> Self {
        let pace = if state.is_open() { Animation::Themed(Pace::Settled) } else { Animation::fast() };
        self.opacity(if state.is_open() { 1. } else { 0. })
            .animate(pace)
            .enter_from(|p| p.opacity(0.))
            .hero_part(HeroPart::Destination)
    }

    fn hero_content(self) -> Self {
        self.absolute().top_0().left_0().hero_part(HeroPart::Source)
    }

    fn hero_with(self, state: &Presentation, from: impl Into<SharedString>) -> Self {
        if state.is_closing() {
            state.0.leaving.set(true);
        }
        let state = state.clone();
        self.hero_from(from, state.is_closing()).on_animation_end(move |_, _, cx| {
            state.landed();
            cx.refresh_windows();
        })
    }
}
