//! The picture a drag carries: a borderless panel holding a snapshot of the source's view, kept
//! under the pointer and ignoring the mouse so the windows under it keep reporting where the
//! pointer is. A lifted drag shows it over its own window too, a little larger with a shadow; a
//! plain one only once the pointer leaves its window. When the drag ends, a lifted picture
//! shrinks into the target that took it or settles back on its source before it goes.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use nd_ui::DragPreview;
use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSBackingStoreType, NSColor, NSImage, NSImageScaling,
    NSImageView, NSPanel, NSScreen, NSView, NSWindowLevel, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use objc2_quartz_core::CAMediaTimingFunction;

/// How much larger a lifted picture rides than its source, as Finder's does.
const LIFT: f64 = 1.04;
/// How small a picture dropped into a target gets before it is gone.
const INTO: f64 = 0.5;
const POP: f64 = 0.15;
const SETTLE: f64 = 0.25;
const CURVE: [f32; 4] = [0.23, 1., 0.32, 1.];

struct Picture {
    panel: Retained<NSPanel>,
    view: Retained<NSImageView>,
}

thread_local! {
    static PICTURE: RefCell<Option<Picture>> = const { RefCell::new(None) };
}

fn slowdown() -> f64 {
    std::env::var("ND_ANIMATION_SLOWDOWN").ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| *v >= 1.).unwrap_or(1.)
}

fn curve() -> Retained<CAMediaTimingFunction> {
    let [a, b, c, d] = CURVE;
    CAMediaTimingFunction::functionWithControlPoints(a, b, c, d)
}

/// `frame` (nd global points, top-left origin) grown by `scale` about its centre, in Cocoa
/// screen coordinates.
fn cocoa(mtm: MainThreadMarker, frame: nd_core::Rect, scale: f64) -> NSRect {
    let primary = NSScreen::screens(mtm).firstObject().map_or(0., |s| s.frame().size.height);
    let (w, h) = (f64::from(frame.size.width), f64::from(frame.size.height));
    let (sw, sh) = (w * scale, h * scale);
    let x = f64::from(frame.origin.x) - (sw - w) / 2.;
    let y = primary - f64::from(frame.origin.y) - h + (h - sh) / 2.;
    NSRect::new(NSPoint::new(x, y), NSSize::new(sw, sh))
}

/// Does what `preview` asks with the picture of `view`, which is only read when there is no
/// picture yet.
pub(crate) fn show(mtm: MainThreadMarker, view: Option<&NSView>, preview: DragPreview, reduce: bool) {
    match preview {
        DragPreview::At { frame, lifted } => {
            let existing = PICTURE.with(|p| p.borrow().as_ref().map(|p| (p.panel.clone(), p.view.clone())));
            let fresh = existing.is_none();
            let Some((panel, picture)) = existing.or_else(|| {
                let made = make(mtm, view?, lifted)?;
                let pair = (made.panel.clone(), made.view.clone());
                PICTURE.with(|p| *p.borrow_mut() = Some(made));
                Some(pair)
            }) else {
                return;
            };
            let scale = if lifted && !reduce { LIFT } else { 1. };
            let rect = cocoa(mtm, frame, scale);
            panel.setFrame_display(rect, false);
            if fresh {
                appear(&panel, &picture, rect.size, lifted, reduce);
            }
        }
        DragPreview::Land { frame, into } => {
            let Some(picture) = PICTURE.with(|p| p.borrow_mut().take()) else { return };
            land(mtm, picture, frame, into, reduce);
        }
        DragPreview::Gone => {
            if let Some(picture) = PICTURE.with(|p| p.borrow_mut().take()) {
                picture.panel.orderOut(None);
            }
        }
    }
}

/// Brings a new picture in: a lifted one grows from its source's size into the lift, or fades
/// in under Reduce Motion; a plain one is just there.
fn appear(panel: &NSPanel, picture: &NSImageView, size: NSSize, lifted: bool, reduce: bool) {
    let full = NSRect::new(NSPoint::new(0., 0.), size);
    if !lifted {
        picture.setFrame(full);
        panel.orderFrontRegardless();
        return;
    }
    if reduce {
        picture.setFrame(full);
        panel.setAlphaValue(0.);
    } else {
        let (w, h) = (size.width / LIFT, size.height / LIFT);
        picture.setFrame(NSRect::new(NSPoint::new((size.width - w) / 2., (size.height - h) / 2.), NSSize::new(w, h)));
    }
    panel.orderFrontRegardless();
    let (p, v) = (panel.retain(), picture.retain());
    let changes = RcBlock::new(move |ctx: NonNull<NSAnimationContext>| {
        let ctx = unsafe { ctx.as_ref() };
        ctx.setDuration(POP * slowdown());
        ctx.setTimingFunction(Some(&curve()));
        if reduce {
            p.animator().setAlphaValue(1.);
        } else {
            v.animator().setFrame(full);
        }
    });
    NSAnimationContext::runAnimationGroup_completionHandler(&changes, None);
}

/// Sends the picture into `frame` and orders it out once it gets there. Under Reduce Motion it
/// fades where it is.
fn land(mtm: MainThreadMarker, picture: Picture, frame: nd_core::Rect, into: bool, reduce: bool) {
    let Picture { panel, view } = picture;
    let to = cocoa(mtm, frame, if into { INTO } else { 1. });
    let p = panel.clone();
    let changes = RcBlock::new(move |ctx: NonNull<NSAnimationContext>| {
        let ctx = unsafe { ctx.as_ref() };
        ctx.setDuration(SETTLE * slowdown());
        ctx.setTimingFunction(Some(&curve()));
        if reduce || into {
            p.animator().setAlphaValue(0.);
        }
        if !reduce {
            p.animator().setFrame_display(to, true);
            view.animator().setFrame(NSRect::new(NSPoint::new(0., 0.), to.size));
        }
    });
    let p = panel.clone();
    let done = RcBlock::new(move || p.orderOut(None));
    NSAnimationContext::runAnimationGroup_completionHandler(&changes, Some(&done));
}

fn make(mtm: MainThreadMarker, view: &NSView, lifted: bool) -> Option<Picture> {
    let bounds = view.bounds();
    if bounds.size.width < 1. || bounds.size.height < 1. {
        return None;
    }
    let rep = view.bitmapImageRepForCachingDisplayInRect(bounds)?;
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let image = NSImage::initWithSize(NSImage::alloc(), bounds.size);
    image.addRepresentation(&rep);
    let frame = NSRect::new(NSPoint::new(0., 0.), bounds.size);
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        frame,
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(true);
    panel.setIgnoresMouseEvents(true);
    panel.setAlphaValue(if lifted { 1. } else { 0.9 });
    panel.setLevel(NSWindowLevel::from(objc2_app_kit::NSPopUpMenuWindowLevel));
    unsafe { panel.setReleasedWhenClosed(false) };
    let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
    let picture = NSImageView::imageViewWithImage(&image, mtm);
    picture.setImageScaling(NSImageScaling::ScaleAxesIndependently);
    picture.setFrame(frame);
    content.addSubview(&picture);
    panel.setContentView(Some(&content));
    Some(Picture { panel, view: picture })
}
