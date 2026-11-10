//! Element animation on the window's frame clock (ARCHITECTURE.md 3.7). A `div` with
//! `.animate(..)` moves to the opacity, offset, scale and size each render gives it instead of
//! jumping there; `.enter_from(..)` gives a freshly mounted one a pose to come in from, and
//! `.exit_to(..)` one to leave to while the tree keeps it drawn. `.animate_layout(..)` glides
//! it from where layout had it to where layout puts it now. The retained tree samples every motion at the frame's target time and sends the presented values
//! to the backend as ordinary props, so both backends animate the same way and a clean window
//! draws nothing. A new target mid-flight starts from the presented value and, for springs, the
//! velocity, so motion is interruptible. With reduce motion on nothing travels: a move that
//! would show (more than [`DISSOLVE_DISTANCE`] points, or a tenth of a scale) dissolves instead,
//! fading out where the element was and back in where it lands, an entrance or exit that would
//! travel only fades, and smaller moves (a hover lift, a press dip) take their place at once.

use std::time::Duration;

use nd_gesture::{Spring, SpringMotion};
use nd_gpui::gpui::{
    AbsoluteLength, DefiniteLength, Hsla, Length, Pixels, Rgba, StyleRefinement, px, solid_background,
};

/// An easing curve for a timed animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// A CSS `cubic-bezier(x1, y1, x2, y2)`.
    Bezier(f32, f32, f32, f32),
}

impl Curve {
    /// Progress at `t` in 0..=1.
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0., 1.);
        let (x1, y1, x2, y2) = match self {
            Curve::Linear => return t,
            Curve::EaseIn => (0.42, 0., 1., 1.),
            Curve::EaseOut => (0., 0., 0.58, 1.),
            Curve::EaseInOut => (0.42, 0., 0.58, 1.),
            Curve::Bezier(a, b, c, d) => (a, b, c, d),
        };
        let bezier = |a: f32, b: f32, s: f32| {
            let u = 1. - s;
            3. * u * u * s * a + 3. * u * s * s * b + s * s * s
        };
        // Solves x(s) = t by bisection: monotonic on 0..=1 for x1, x2 in 0..=1.
        let (mut lo, mut hi) = (0f32, 1f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.;
            if bezier(x1, x2, mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        bezier(y1, y2, (lo + hi) / 2.)
    }
}

/// How an animated value moves to a new target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Animation {
    /// Over `duration`, along `curve`.
    Timed { duration: Duration, curve: Curve },
    /// Holds for `delay`, then runs over `duration` along `curve`.
    Delayed { delay: Duration, duration: Duration, curve: Curve },
    /// A spring in Apple's terms (`damping` 1 settles without overshoot, `response` is roughly
    /// the seconds it takes). Carries velocity through a retarget.
    Spring { response: f64, damping: f64 },
    /// One of the platform's own timings, from the app's [`MotionTokens`]: the same feel in
    /// every widget and app on that platform.
    Themed(Pace),
}

/// A platform timing, see [`MotionTokens`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pace {
    /// Hover, press and small state changes.
    Fast,
    /// Things coming in or going out: a revealer, a presentation.
    Normal,
    /// Larger moves across the window.
    Slow,
    /// The platform's spring: interruptible moves that keep their velocity.
    Spring,
    /// Waits out the platform spring, then runs on the fast timing: what surrounds a shared
    /// element coming in once it has landed.
    Settled,
    /// Waits the fast timing, then runs as long as the platform spring: a backdrop going out
    /// with a shared element, after what surrounds it has faded.
    Exit,
}

/// The platform's motion: durations, curve and spring, set by the host from the theme. Every
/// [`Animation::Themed`] resolves through them, so hover fades, presentations and lyrics move
/// alike in every app on a platform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionTokens {
    pub fast: Duration,
    pub normal: Duration,
    pub slow: Duration,
    pub curve: Curve,
    /// Apple's response and damping.
    pub spring: (f64, f64),
}

impl Default for MotionTokens {
    fn default() -> Self {
        Self {
            fast: Duration::from_millis(150),
            normal: Duration::from_millis(250),
            slow: Duration::from_millis(400),
            curve: Curve::Bezier(0.25, 0.1, 0.25, 1.),
            spring: (0.35, 0.86),
        }
    }
}

/// The fade reduce motion keeps when the theme zeroed its durations.
pub const REDUCED_FADE: Duration = Duration::from_millis(150);

/// Under reduce motion, a move longer than this many points dissolves; a shorter one jumps.
pub const DISSOLVE_DISTANCE: f64 = 12.;
/// The same for a change of scale.
const DISSOLVE_SCALE: f64 = 0.1;

impl MotionTokens {
    /// `animation` with a themed pace replaced by its timing. With reduce motion a themed
    /// animation or a spring is the platform's fast fade on an ease-out: nothing travels, and
    /// a dissolve or a fade has no velocity to carry.
    pub fn resolve(&self, animation: Animation, reduce: bool) -> Animation {
        let k = slowdown();
        if reduce {
            return match animation {
                Animation::Themed(_) | Animation::Spring { .. } => {
                    let fade = if self.fast.is_zero() { REDUCED_FADE } else { self.fast };
                    Animation::ease_out(fade.mul_f64(k))
                }
                Animation::Timed { duration, curve } => Animation::timed(duration.mul_f64(k), curve),
                Animation::Delayed { delay, duration, curve } => {
                    Animation::Delayed { delay: delay.mul_f64(k), duration: duration.mul_f64(k), curve }
                }
            };
        }
        let pace = match animation {
            Animation::Themed(pace) => pace,
            Animation::Timed { duration, curve } => return Animation::timed(duration.mul_f64(k), curve),
            Animation::Delayed { delay, duration, curve } => {
                return Animation::Delayed { delay: delay.mul_f64(k), duration: duration.mul_f64(k), curve };
            }
            Animation::Spring { response, damping } => return Animation::spring(response * k, damping),
        };
        match pace {
            Pace::Fast => Animation::timed(self.fast.mul_f64(k), self.curve),
            Pace::Normal => Animation::timed(self.normal.mul_f64(k), self.curve),
            Pace::Slow => Animation::timed(self.slow.mul_f64(k), self.curve),
            Pace::Spring => Animation::spring(self.spring.0 * k, self.spring.1),
            Pace::Settled => Animation::Delayed {
                delay: Duration::from_secs_f64(self.spring.0 * k),
                duration: self.fast.mul_f64(k),
                curve: self.curve,
            },
            Pace::Exit => Animation::Delayed {
                delay: self.fast.mul_f64(k),
                duration: Duration::from_secs_f64(self.spring.0 * k),
                curve: self.curve,
            },
        }
    }

    /// How long a shared element waits before it travels home: the time what surrounds it takes
    /// to fade out. Nothing under reduce motion, where it does not travel.
    pub fn hero_hold(&self, reduce: bool) -> f64 {
        if reduce { 0. } else { self.fast.as_secs_f64() * slowdown() }
    }
}

/// `ND_ANIMATION_SLOWDOWN=<factor>` stretches every animation, themed or given explicitly, for a
/// test or a person to watch it frame by frame.
fn slowdown() -> f64 {
    static K: std::sync::LazyLock<f64> = std::sync::LazyLock::new(|| {
        std::env::var("ND_ANIMATION_SLOWDOWN").ok().and_then(|v| v.parse().ok()).filter(|v| *v >= 1.).unwrap_or(1.)
    });
    *K
}

impl Animation {
    pub fn timed(duration: Duration, curve: Curve) -> Self {
        Animation::Timed { duration, curve }
    }

    pub fn linear(duration: Duration) -> Self {
        Self::timed(duration, Curve::Linear)
    }

    pub fn ease_out(duration: Duration) -> Self {
        Self::timed(duration, Curve::EaseOut)
    }

    pub fn ease_in_out(duration: Duration) -> Self {
        Self::timed(duration, Curve::EaseInOut)
    }

    pub fn spring(response: f64, damping: f64) -> Self {
        Animation::Spring { response, damping }
    }

    /// A spring from its physical constants with a mass of 1: `stiffness` k and `damping` c, as
    /// a hand-written integrator takes them (`v += (k * (target - x) - c * v) * dt`). k 420 and
    /// c 31 settle in about a quarter of a second with a little overshoot.
    pub fn spring_physics(stiffness: f64, damping: f64) -> Self {
        let root = stiffness.max(1e-6).sqrt();
        Animation::Spring { response: std::f64::consts::TAU / root, damping: damping / (2. * root) }
    }

    /// The platform's hover and small state change timing.
    pub fn fast() -> Self {
        Animation::Themed(Pace::Fast)
    }

    /// The platform's timing for things coming in and going out.
    pub fn normal() -> Self {
        Animation::Themed(Pace::Normal)
    }

    pub fn slow() -> Self {
        Animation::Themed(Pace::Slow)
    }

    /// The platform's spring.
    pub fn natural() -> Self {
        Animation::Themed(Pace::Spring)
    }
}

/// Where a freshly mounted element starts before moving to its own values, or where one on its
/// way out goes; unset fields stay at its own values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub opacity: Option<f32>,
    pub offset: Option<(f32, f32)>,
    pub scale: Option<f32>,
}

impl Pose {
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }

    pub fn offset(mut self, x: Pixels, y: Pixels) -> Self {
        self.offset = Some((x.as_f32(), y.as_f32()));
        self
    }

    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = Some(scale);
        self
    }
}

/// A visual move and scale of a `div`, about its centre. Layout never sees it: siblings stay
/// where they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub offset: (f32, f32),
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self { offset: (0., 0.), scale: 1. }
    }
}

impl Transform {
    pub fn is_identity(&self) -> bool {
        *self == Transform::default()
    }
}

/// Reported to a node's `.on_animation_end` handler when every value it animates has settled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnimationEnded;

/// What `.animate`, `.enter_from`, `.exit_to` and `.animate_layout` asked of an element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MotionSpec {
    pub animation: Animation,
    pub enter: Option<Pose>,
    pub exit: Option<Pose>,
    /// Moves between layout frames glide on this.
    pub layout: Option<Animation>,
}

impl MotionSpec {
    pub fn new(animation: Animation) -> Self {
        Self { animation, enter: None, exit: None, layout: None }
    }
}

pub(crate) const CHANNELS: usize = 17;
pub(crate) const OPACITY: usize = 0;
const OFFSET_X: usize = 1;
const OFFSET_Y: usize = 2;
pub(crate) const SCALE: usize = 3;
const WIDTH: usize = 4;
const HEIGHT: usize = 5;
/// A hero's look, which it carries from its source's to its own: corner radius and border
/// width in points, fill and border colour as premultiplied RGBA.
const RADIUS: usize = 6;
pub(crate) const BORDER: usize = 7;
const FILL: usize = 8;
const STROKE: usize = 12;
/// A hero's flight: 0 on its source, 1 at home. What it holds follows it (see
/// [`crate::HeroPart`]).
pub(crate) const PROGRESS: usize = 16;
/// How far into a hero's flight its source's border has gone.
const BORDER_GONE: f64 = 0.5;
/// The channels that move the element, which reduce motion dissolves or jumps.
fn geometry() -> std::ops::RangeInclusive<usize> {
    OFFSET_X..=HEIGHT
}

/// The animatable values of one element: `None` where the element has none to animate (a
/// size that is not a length in points, a look on an element that is not a hero).
pub(crate) type Values = [Option<f64>; CHANNELS];

/// Values with only `channel` set.
pub(crate) fn only(channel: usize, value: f64) -> Values {
    let mut out = [None; CHANNELS];
    out[channel] = Some(value);
    out
}

fn px_of(length: Option<Length>) -> Option<f64> {
    match length? {
        Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(p))) => Some(f64::from(p.as_f32())),
        _ => None,
    }
}

/// The values a div's props and layout style give it.
pub(crate) fn values(style: &StyleRefinement, transform: &Transform) -> Values {
    let mut out = only(OPACITY, f64::from(style.opacity.unwrap_or(1.)));
    out[OFFSET_X] = Some(f64::from(transform.offset.0));
    out[OFFSET_Y] = Some(f64::from(transform.offset.1));
    out[SCALE] = Some(f64::from(transform.scale));
    out[WIDTH] = px_of(style.size.width);
    out[HEIGHT] = px_of(style.size.height);
    out
}

/// One length for all four corners or sides, in points: `None` when they differ.
fn uniform(lengths: [Option<f64>; 4]) -> Option<f64> {
    lengths.iter().all(|l| *l == lengths[0]).then_some(lengths[0].unwrap_or(0.))
}

fn absolute_px(length: Option<AbsoluteLength>) -> Option<f64> {
    match length? {
        AbsoluteLength::Pixels(p) => Some(f64::from(p.as_f32())),
        AbsoluteLength::Rems(_) => None,
    }
}

fn premultiplied(color: Option<Hsla>) -> [f64; 4] {
    let Some(color) = color else { return [0.; 4] };
    let c = Rgba::from(color);
    let a = f64::from(c.a);
    [f64::from(c.r) * a, f64::from(c.g) * a, f64::from(c.b) * a, a]
}

fn unpremultiplied(values: &Values, at: usize) -> Option<Hsla> {
    let [r, g, b, a] = [0, 1, 2, 3].map(|i| values[at + i].unwrap_or(0.).clamp(0., 1.));
    if a <= 1e-3 {
        return None;
    }
    let channel = |v: f64| (v / a).clamp(0., 1.) as f32;
    Some(Hsla::from(Rgba { r: channel(r), g: channel(g), b: channel(b), a: a as f32 }))
}

/// `values` with a hero's look added from `style`, the one it shows at rest: no radius, border
/// or fill where it sets none. Corners or sides that differ keep their own. `radius_cap` bounds
/// the radius to half the element's shorter side when that is known, as backends draw it.
pub(crate) fn with_look(mut values: Values, style: &StyleRefinement, radius_cap: Option<f64>) -> Values {
    let r = &style.corner_radii;
    values[RADIUS] = uniform([r.top_left, r.top_right, r.bottom_right, r.bottom_left].map(absolute_px))
        .map(|r| radius_cap.map_or(r, |cap| r.min(cap)));
    let b = &style.border_widths;
    values[BORDER] = uniform([b.top, b.right, b.bottom, b.left].map(absolute_px));
    let fill = style.background.as_ref().and_then(|f| f.color()).and_then(|b| b.as_solid());
    let solid = style.background.is_none() || fill.is_some();
    for (i, v) in premultiplied(fill).into_iter().enumerate() {
        values[FILL + i] = solid.then_some(v);
    }
    for (i, v) in premultiplied(style.border_color).into_iter().enumerate() {
        values[STROKE + i] = Some(v);
    }
    values[PROGRESS] = Some(1.);
    values
}

/// `look` with the fill of `own` where it has none: a hero landing on a source that draws no
/// surface of its own keeps its surface instead of thinning to nothing on the way.
pub(crate) fn fill_from(mut look: Values, own: &Values) -> Values {
    if look[FILL + 3].is_none_or(|a| a <= 1e-3) {
        look[FILL..FILL + 4].copy_from_slice(&own[FILL..FILL + 4]);
    }
    look
}

/// `style` and `transform` with `values` in place of their own.
pub(crate) fn present(style: &mut StyleRefinement, transform: &mut Transform, values: &Values) {
    if let Some(o) = values[OPACITY] {
        style.opacity = Some(o.clamp(0., 1.) as f32);
    }
    if let Some(x) = values[OFFSET_X] {
        transform.offset.0 = x as f32;
    }
    if let Some(y) = values[OFFSET_Y] {
        transform.offset.1 = y as f32;
    }
    if let Some(s) = values[SCALE] {
        transform.scale = s.max(0.) as f32;
    }
    if let Some(w) = values[WIDTH] {
        style.size.width = Some(px(w.max(0.) as f32).into());
    }
    if let Some(h) = values[HEIGHT] {
        style.size.height = Some(px(h.max(0.) as f32).into());
    }
    if let Some(r) = values[RADIUS] {
        let r = Some(px(r.max(0.) as f32).into());
        style.corner_radii.top_left = r;
        style.corner_radii.top_right = r;
        style.corner_radii.bottom_right = r;
        style.corner_radii.bottom_left = r;
    }
    if let Some(w) = values[BORDER] {
        let w = Some(px(w.max(0.) as f32).into());
        style.border_widths.top = w;
        style.border_widths.right = w;
        style.border_widths.bottom = w;
        style.border_widths.left = w;
    }
    if values[FILL + 3].is_some() {
        style.background = unpremultiplied(values, FILL).map(|c| solid_background(c).into());
    }
    if values[STROKE + 3].is_some() {
        style.border_color = Some(unpremultiplied(values, STROKE).unwrap_or_default());
    }
}

#[derive(Clone, Copy, Debug)]
enum Motion {
    Timed {
        from: f64,
        to: f64,
        start: f64,
        duration: f64,
        curve: Curve,
    },
    Spring(SpringMotion),
    /// Shows `from` until `at`, then `to`: a move reduce motion dissolves, switching places
    /// while the element is faded out.
    Step {
        from: f64,
        to: f64,
        at: f64,
    },
    /// The opacity of a dissolve: from `from` down to nothing by `at`, then up to `to` by `end`.
    Dip {
        from: f64,
        to: f64,
        start: f64,
        at: f64,
        end: f64,
    },
}

impl Motion {
    fn start(animation: Animation, from: f64, to: f64, velocity: f64, now: f64) -> Self {
        match animation {
            Animation::Timed { duration, curve } => {
                Motion::Timed { from, to, start: now, duration: duration.as_secs_f64(), curve }
            }
            Animation::Delayed { delay, duration, curve } => {
                Motion::Timed { from, to, start: now + delay.as_secs_f64(), duration: duration.as_secs_f64(), curve }
            }
            Animation::Spring { response, damping } => {
                Motion::Spring(Spring { response: response.max(0.01), damping }.start(from, to, velocity, now))
            }
            Animation::Themed(_) => {
                Self::start(MotionTokens::default().resolve(animation, false), from, to, velocity, now)
            }
        }
    }

    /// Value and velocity at `now`.
    fn sample(&self, now: f64) -> (f64, f64) {
        match *self {
            Motion::Timed { from, to, start, duration, curve } => {
                if duration <= 0. {
                    return (to, 0.);
                }
                let at = |t: f64| {
                    let p = (t - start) / duration;
                    match p {
                        ..=0. => from,
                        1.0.. => to,
                        p => from + (to - from) * f64::from(curve.at(p as f32)),
                    }
                };
                let dt = 1. / 240.;
                (at(now), (at(now + dt) - at(now)) / dt)
            }
            Motion::Spring(s) => s.sample(now),
            Motion::Step { from, to, at } => (if now < at { from } else { to }, 0.),
            Motion::Dip { from, to, start, at, end } => {
                let ease = |t: f64, a: f64, b: f64| f64::from(Curve::EaseOut.at(((t - a) / (b - a).max(1e-9)) as f32));
                if now < at { (from * (1. - ease(now, start, at)), 0.) } else { (to * ease(now, at, end), 0.) }
            }
        }
    }

    #[cfg(test)]
    fn target(&self) -> f64 {
        match *self {
            Motion::Timed { to, .. } | Motion::Step { to, .. } | Motion::Dip { to, .. } => to,
            Motion::Spring(s) => s.target(),
        }
    }

    /// At rest by `now`, to within `epsilon` of the target.
    fn settled(&self, now: f64, epsilon: f64) -> bool {
        match *self {
            Motion::Timed { start, duration, .. } => now >= start + duration,
            Motion::Step { at, .. } => now >= at,
            Motion::Dip { end, .. } => now >= end,
            Motion::Spring(s) => {
                let (x, v) = s.sample(now);
                (x - s.target()).abs() < epsilon && v.abs() < epsilon * 10.
            }
        }
    }
}

/// How close a channel must come to rest: a fraction of opacity or colour, a tenth of a point.
fn epsilon(channel: usize) -> f64 {
    match channel {
        OPACITY | FILL..PROGRESS => 0.002,
        SCALE | PROGRESS => 0.001,
        _ => 0.1,
    }
}

fn moves(channel: usize) -> bool {
    channel != OPACITY
}

/// Whether reduce motion dissolves a move of `channel` from `from` to `to` rather than jumping.
fn shows(channel: usize, from: f64, to: f64) -> bool {
    let limit = if channel == SCALE { DISSOLVE_SCALE } else { DISSOLVE_DISTANCE };
    (to - from).abs() > limit
}

/// How long a dissolve on `animation` fades out, then in: half its time out, all of it in.
fn dissolve_times(animation: Animation) -> (f64, f64) {
    let fade = match animation {
        Animation::Timed { duration, .. } | Animation::Delayed { duration, .. } => duration,
        _ => REDUCED_FADE,
    };
    let fade = fade.as_secs_f64();
    (fade / 2., fade)
}

/// One element's motions in flight.
#[derive(Clone, Debug)]
pub(crate) struct MotionState {
    animation: Animation,
    /// What the element asked for last.
    targets: Values,
    motions: [Option<Motion>; CHANNELS],
}

impl MotionState {
    /// A state at rest on `targets`, or coming in from `enter`.
    pub fn new(spec: MotionSpec, targets: Values, now: f64, reduce: bool) -> Self {
        let mut state = MotionState { animation: spec.animation, targets, motions: [None; CHANNELS] };
        if let Some(pose) = spec.enter {
            let mut from: Values = [None; CHANNELS];
            from[OPACITY] = pose.opacity.map(f64::from);
            from[OFFSET_X] = pose.offset.map(|o| f64::from(o.0));
            from[OFFSET_Y] = pose.offset.map(|o| f64::from(o.1));
            from[SCALE] = pose.scale.map(f64::from);
            state.start_from(&from, now, reduce);
        }
        state
    }

    /// Where channel `c` shows at `now` and how fast it moves.
    fn current(&self, c: usize, now: f64) -> Option<(f64, f64)> {
        self.motions[c].map(|m| m.sample(now)).or(self.targets[c].map(|v| (v, 0.)))
    }

    /// Whether channel `c` holds its old place in a dissolve still fading out.
    fn holding(&self, c: usize, now: f64) -> bool {
        matches!(self.motions[c], Some(Motion::Step { at, .. }) if now < at)
    }

    /// Fades the element out from where its opacity is and back in to `to`, joining a dissolve
    /// still fading out. Returns when it switches places, or `None` when the animation has no
    /// time to dissolve in.
    fn dissolve(&mut self, to: Option<f64>, now: f64) -> Option<f64> {
        if let Some(Motion::Dip { at, to: ref mut target, .. }) = self.motions[OPACITY]
            && now < at
        {
            *target = to.unwrap_or(1.);
            return Some(at);
        }
        let (out, back) = dissolve_times(self.animation);
        if out <= 0. {
            return None;
        }
        let from = self.current(OPACITY, now).map_or(1., |(o, _)| o);
        let at = now + out;
        self.motions[OPACITY] = Some(Motion::Dip { from, to: to.unwrap_or(1.), start: now, at, end: at + back });
        Some(at)
    }

    /// The element rendered again with `targets`: channels whose target moved start towards
    /// it from where they are now. Under reduce motion a move that shows dissolves and a
    /// smaller one jumps.
    pub fn retarget(&mut self, animation: Animation, targets: Values, now: f64, reduce: bool) {
        self.animation = animation;
        let mut switch = None;
        if reduce {
            let travels = geometry().any(|c| match (self.current(c, now), targets[c]) {
                (Some((from, _)), Some(to)) if targets[c] != self.targets[c] => {
                    shows(c, from, to) || self.holding(c, now)
                }
                _ => false,
            });
            if travels {
                switch = self.dissolve(targets[OPACITY], now);
            }
        }
        for (c, &target) in targets.iter().enumerate() {
            if target == self.targets[c] {
                continue;
            }
            if c == OPACITY
                && let Some(Motion::Dip { end, to: ref mut dip_to, .. }) = self.motions[OPACITY]
                && now < end
                && let Some(to) = target
            {
                *dip_to = to;
                continue;
            }
            let current = self.current(c, now);
            self.motions[c] = match (current, target) {
                (Some((from, velocity)), Some(to)) if from != to => match (reduce && moves(c), switch) {
                    (false, _) => Some(Motion::start(animation, from, to, velocity, now)),
                    (true, Some(at)) => Some(Motion::Step { from, to, at }),
                    (true, None) => None,
                },
                _ => None,
            };
        }
        self.targets = targets;
    }

    pub fn animation(&self) -> Animation {
        self.animation
    }

    /// The element's layout frame moved by `by`: its offset starts from where it showed before
    /// the move, so it travels on from there to its own offset with the velocity it had. Under
    /// reduce motion a move that shows dissolves, and a smaller one jumps.
    pub fn shift(&mut self, animation: Animation, by: (f64, f64), now: f64, reduce: bool) {
        if reduce {
            let held = self.holding(OFFSET_X, now) || self.holding(OFFSET_Y, now);
            if !held && !shows(OFFSET_X, 0., by.0) && !shows(OFFSET_Y, 0., by.1) {
                return;
            }
            let previous = self.animation;
            self.animation = animation;
            let switch = self.dissolve(self.targets[OPACITY], now);
            self.animation = previous;
            let Some(at) = switch else { return };
            for (c, by) in [(OFFSET_X, by.0), (OFFSET_Y, by.1)] {
                let Some(to) = self.targets[c] else { continue };
                let from = match self.motions[c] {
                    Some(Motion::Step { from, at, .. }) if now < at => from,
                    Some(m) => m.sample(now).0,
                    None => to,
                };
                if from + by != to {
                    self.motions[c] = Some(Motion::Step { from: from + by, to, at });
                }
            }
            return;
        }
        for (c, by) in [(OFFSET_X, by.0), (OFFSET_Y, by.1)] {
            let Some(to) = self.targets[c].filter(|_| by != 0.) else { continue };
            let (from, velocity) = self.motions[c].map_or((to, 0.), |m| m.sample(now));
            self.motions[c] = Some(Motion::start(animation, from + by, to, velocity, now));
        }
    }

    /// Starts towards `pose` from where the element is, for one on its way out. Reduce motion
    /// leaves its offset and scale where they are and fades it out instead, to the pose's
    /// opacity or to nothing. False when nothing moves.
    pub fn exit(&mut self, pose: Pose, now: f64, reduce: bool) -> bool {
        let mut targets = self.targets;
        if let Some(o) = pose.opacity {
            targets[OPACITY] = Some(f64::from(o));
        }
        if reduce {
            if pose.opacity.is_none() && (pose.offset.is_some() || pose.scale.is_some()) {
                targets[OPACITY] = Some(0.);
            }
        } else {
            if let Some((x, y)) = pose.offset {
                targets[OFFSET_X] = Some(f64::from(x));
                targets[OFFSET_Y] = Some(f64::from(y));
            }
            if let Some(s) = pose.scale {
                targets[SCALE] = Some(f64::from(s));
            }
        }
        self.retarget(self.animation, targets, now, reduce);
        self.is_moving()
    }

    pub fn targets(&self) -> Values {
        self.targets
    }

    /// Starts every channel `from` names towards its target, as an enter pose does. Under
    /// reduce motion one that would travel fades in where it lands instead, from the pose's
    /// opacity or from nothing.
    pub fn start_from(&mut self, from: &Values, now: f64, reduce: bool) {
        let mut from = *from;
        if reduce
            && from[OPACITY].is_none()
            && geometry().any(|c| matches!((from[c], self.targets[c]), (Some(f), Some(t)) if f != t))
        {
            from[OPACITY] = Some(0.);
        }
        for (c, from) in from.iter().enumerate() {
            if let (Some(from), Some(to)) = (*from, self.targets[c])
                && !(reduce && moves(c))
                && from != to
            {
                self.motions[c] = Some(Motion::start(self.animation, from, to, 0., now));
            }
        }
    }

    /// The values to show at `now`. A hero's border leaves faster than the rest of its look:
    /// a source's ring is gone by [`BORDER_GONE`] of the flight, so it never shows as an outline
    /// around a container that has nearly grown into its own look.
    pub fn sample(&self, now: f64) -> Values {
        let mut out = self.targets;
        for (c, motion) in self.motions.iter().enumerate() {
            if let Some(m) = motion {
                out[c] = Some(m.sample(now).0);
            }
        }
        if let Some(p) = out[PROGRESS].filter(|_| self.is_flying()) {
            // A look channel sits a share 1 - p of the way from home to the source, which is
            // (1 - p) of its distance to the target going home and p of it going back. The
            // border's share is 1 - k instead, nothing from `BORDER_GONE` on.
            let k = (p / BORDER_GONE).min(1.);
            let home = self.targets[PROGRESS].is_some_and(|t| t > 0.5);
            let scale = match home {
                true if p < 1. - 1e-6 => (1. - k) / (1. - p),
                false if p > 1e-6 => k / p,
                _ => 0.,
            };
            for c in [BORDER, STROKE, STROKE + 1, STROKE + 2, STROKE + 3] {
                if let (Some(v), Some(t)) = (out[c], self.targets[c]) {
                    out[c] = Some(t + (v - t) * scale);
                }
            }
        }
        out
    }

    /// Drops the motions that came to rest by `now`. True when the last one just did.
    pub fn settle(&mut self, now: f64) -> bool {
        let was = self.is_moving();
        for (c, motion) in self.motions.iter_mut().enumerate() {
            if motion.is_some_and(|m| m.settled(now, epsilon(c))) {
                *motion = None;
            }
        }
        was && !self.is_moving()
    }

    pub fn is_moving(&self) -> bool {
        self.motions.iter().any(Option::is_some)
    }

    /// A hero between its source and home.
    pub fn is_flying(&self) -> bool {
        self.motions[PROGRESS].is_some()
    }

    #[cfg(test)]
    fn target(&self, c: usize) -> Option<f64> {
        self.motions[c].map(|m| m.target())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(animation: Animation) -> MotionSpec {
        MotionSpec::new(animation)
    }

    fn opacity(v: f64) -> Values {
        let mut out = only(OPACITY, v);
        out[OFFSET_X] = Some(0.);
        out[OFFSET_Y] = Some(0.);
        out[SCALE] = Some(1.);
        out
    }

    #[test]
    fn curves_start_and_end_in_place() {
        for curve in [Curve::Linear, Curve::EaseIn, Curve::EaseOut, Curve::EaseInOut, Curve::Bezier(0.2, 0.9, 0.3, 1.)]
        {
            assert!(curve.at(0.).abs() < 1e-3, "{curve:?}");
            assert!((curve.at(1.) - 1.).abs() < 1e-3, "{curve:?}");
        }
        assert!(Curve::EaseOut.at(0.5) > 0.5, "ease out is ahead at the midpoint");
        assert!(Curve::EaseIn.at(0.5) < 0.5);
    }

    #[test]
    fn a_timed_fade_samples_its_curve_and_settles_on_time() {
        let anim = Animation::linear(Duration::from_millis(200));
        let mut state = MotionState::new(spec(anim), opacity(1.), 0., false);
        state.retarget(anim, opacity(0.), 1., false);
        assert_eq!(state.sample(1.)[OPACITY], Some(1.));
        assert!((state.sample(1.1)[OPACITY].unwrap() - 0.5).abs() < 1e-3);
        assert!(!state.settle(1.15));
        assert!(state.settle(1.2), "settled exactly at the end");
        assert_eq!(state.sample(5.)[OPACITY], Some(0.));
    }

    #[test]
    fn a_spring_retargeted_mid_flight_keeps_its_velocity() {
        let anim = Animation::spring(0.4, 1.);
        let mut state = MotionState::new(spec(anim), opacity(0.), 0., false);
        state.retarget(anim, opacity(1.), 0., false);
        let (mid, v) = state.motions[OPACITY].unwrap().sample(0.1);
        assert!(mid > 0. && mid < 1. && v > 0.);
        state.retarget(anim, opacity(0.), 0.1, false);
        let (again, v2) = state.motions[OPACITY].unwrap().sample(0.1);
        assert!((again - mid).abs() < 1e-9, "no jump on retarget");
        assert!((v2 - v).abs() < 1e-6, "the velocity carries through");
        assert_eq!(state.target(OPACITY), Some(0.));
        let mut t = 0.1;
        while state.is_moving() {
            t += 1. / 60.;
            state.settle(t);
            assert!(t < 3., "a critically damped spring settles");
        }
    }

    #[test]
    fn reduce_motion_jumps_movement_but_still_fades() {
        let anim = Animation::ease_out(Duration::from_millis(300));
        let pose = Pose::default().opacity(0.).offset(px(0.), px(12.)).scale(0.5);
        let state = MotionState::new(MotionSpec { enter: Some(pose), ..spec(anim) }, opacity(1.), 0., true);
        let now = state.sample(0.);
        assert!(now[OPACITY].unwrap().abs() < 1e-9, "the fade starts at the pose");
        assert_eq!(now[OFFSET_Y], Some(0.), "the offset does not move");
        assert_eq!(now[SCALE], Some(1.));
        let moving = MotionState::new(MotionSpec { enter: Some(pose), ..spec(anim) }, opacity(1.), 0., false);
        assert_eq!(moving.sample(0.)[OFFSET_Y], Some(12.));
        assert_eq!(moving.sample(0.)[SCALE], Some(0.5));
    }

    #[test]
    fn spring_physics_matches_the_integrators_constants() {
        let Animation::Spring { response, damping } = Animation::spring_physics(420., 31.) else { unreachable!() };
        assert!((response - 0.3066).abs() < 1e-3, "{response}");
        assert!((damping - 0.7563).abs() < 1e-3, "{damping}");
        let Animation::Spring { damping, .. } = Animation::spring_physics(400., 40.) else { unreachable!() };
        assert!((damping - 1.).abs() < 1e-9, "c = 2 sqrt(k) is critical");
    }

    #[test]
    fn a_shift_starts_from_the_old_place_and_keeps_the_velocity() {
        let anim = Animation::spring(0.3, 1.);
        let mut state = MotionState::new(spec(anim), opacity(1.), 0., false);
        state.shift(anim, (0., 40.), 0., false);
        assert_eq!(state.sample(0.)[OFFSET_Y], Some(40.), "drawn where it was");
        let (mid, v) = state.motions[OFFSET_Y].unwrap().sample(0.05);
        state.shift(anim, (0., -10.), 0.05, false);
        let (again, v2) = state.motions[OFFSET_Y].unwrap().sample(0.05);
        assert!((again - (mid - 10.)).abs() < 1e-9, "no jump: {mid} then {again}");
        assert!((v2 - v).abs() < 1e-6);
        let mut reduced = MotionState::new(spec(anim), opacity(1.), 0., true);
        reduced.shift(anim, (0., 6.), 0., true);
        assert!(!reduced.is_moving(), "reduce motion jumps a small move");
    }

    #[test]
    fn reduce_motion_dissolves_a_move_that_shows() {
        let fade = MotionTokens::default().resolve(Animation::spring(0.3, 1.), true);
        assert_eq!(fade, Animation::ease_out(Duration::from_millis(150)), "a spring becomes the fast fade");
        let mut state = MotionState::new(spec(fade), opacity(1.), 0., true);
        state.shift(fade, (0., 40.), 0., true);
        let out = state.sample(0.05);
        assert_eq!(out[OFFSET_Y], Some(40.), "held where it was while it fades out");
        assert!(out[OPACITY].unwrap() < 1., "{out:?}");
        let gone = state.sample(0.075);
        assert!(gone[OPACITY].unwrap() < 1e-3, "out by the switch: {gone:?}");
        assert_eq!(gone[OFFSET_Y], Some(0.), "then at its new place");
        let back = state.sample(0.15);
        assert!(back[OPACITY].unwrap() > 0. && back[OPACITY].unwrap() < 1., "fading in: {back:?}");
        state.shift(fade, (0., -3.), 0.02, true);
        assert_eq!(state.sample(0.03)[OFFSET_Y], Some(37.), "a move mid fade-out keeps the old place");
        assert!(!state.settle(0.2));
        assert!(state.settle(0.225), "out for half the fade and in for all of it");
        assert_eq!(state.sample(0.3), opacity(1.));

        let mut card = MotionState::new(spec(fade), opacity(1.), 0., true);
        let mut lifted = opacity(1.);
        lifted[OFFSET_Y] = Some(-3.);
        card.retarget(fade, lifted, 0., true);
        assert!(!card.is_moving(), "a hover lift jumps");
        let mut moved = opacity(1.);
        moved[OFFSET_X] = Some(200.);
        card.retarget(fade, moved, 0., true);
        assert_eq!(card.sample(0.05)[OFFSET_X], Some(0.), "a long move dissolves");
        assert_eq!(card.sample(0.1)[OFFSET_X], Some(200.));
    }

    #[test]
    fn under_reduce_motion_an_entrance_that_travels_fades_in() {
        let anim = Animation::ease_out(Duration::from_millis(150));
        let rise = Pose::default().offset(px(0.), px(12.)).scale(0.9);
        let state = MotionState::new(MotionSpec { enter: Some(rise), ..spec(anim) }, opacity(1.), 0., true);
        assert_eq!(state.sample(0.)[OPACITY], Some(0.), "from nothing");
        assert_eq!(state.sample(0.)[OFFSET_Y], Some(0.), "where it lands");
        assert_eq!(state.sample(0.)[SCALE], Some(1.));
        assert_eq!(state.sample(0.15)[OPACITY], Some(1.));
    }

    #[test]
    fn an_exit_under_reduce_motion_only_fades() {
        let anim = Animation::linear(Duration::from_millis(100));
        let pose = Pose::default().opacity(0.).scale(0.5);
        let mut state = MotionState::new(spec(anim), opacity(1.), 0., true);
        assert!(state.exit(pose, 0., true));
        assert_eq!(state.target(OPACITY), Some(0.));
        assert_eq!(state.target(SCALE), None, "the scale stays");
        let mut shrink = MotionState::new(spec(anim), opacity(1.), 0., true);
        assert!(shrink.exit(Pose::default().scale(0.5), 0., true), "a pose that only shrinks fades out");
        assert_eq!(shrink.target(OPACITY), Some(0.));
        assert_eq!(shrink.target(SCALE), None);
        let mut still = MotionState::new(spec(anim), opacity(1.), 0., true);
        assert!(!still.exit(Pose::default(), 0., true), "nothing left to animate");
    }

    #[test]
    fn sizes_animate_only_when_given_in_points() {
        let mut style = StyleRefinement::default();
        style.size.width = Some(px(100.).into());
        style.size.height = Some(nd_gpui::gpui::relative(0.5).into());
        let v = values(&style, &Transform::default());
        assert_eq!(v[WIDTH], Some(100.));
        assert_eq!(v[HEIGHT], None);
        let mut t = Transform::default();
        let mut v = opacity(0.5);
        (v[OFFSET_X], v[OFFSET_Y], v[SCALE], v[WIDTH]) = (Some(3.), Some(4.), Some(2.), Some(50.));
        present(&mut style, &mut t, &v);
        assert_eq!(style.opacity, Some(0.5));
        assert_eq!(t, Transform { offset: (3., 4.), scale: 2. });
        assert_eq!(px_of(style.size.width), Some(50.));
    }
}
