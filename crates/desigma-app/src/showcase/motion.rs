//! The Motion page: a two-screen onboarding flow that shows off the timeline.
//!
//! * **Onboarding**: a night-sky hero with drifting glows, a payment card that springs in and then
//!   floats, two chips that drop and glide in, staggered "Rise" text, pagination dots whose active
//!   pill stretches with a spring, a call-to-action that bursts in and breathes.
//! * **Ready**: a badge that pops and rotates into place inside a rippling ring, three feature
//!   rows that glide in one after another.
//!
//! Each effect is either hand-placed keyframes with spring easing or one of the library's Enter
//! and Cycle presets. Every cycle is timed to end exactly at [`REST`] (`delay + k * period ==
//! REST`), so the scene settles at rest and holds. The last stretch of the timeline is an exit
//! phase: everything eases out in a short stagger, back to the empty state the timeline starts
//! from, so the first and last frames are identical and the animation loops without a seam.
//!
//! The prototype is wired with "Control animation" actions: each screen plays its timeline when
//! it appears, the Replay pill restarts it, hovering the card pauses it, and the call-to-action
//! Smart Animates to the next screen, which plays on arrival.

use super::icons::Ic;
use super::*;
use desigma_doc::motion::{find_preset, AnimProperty as P, AnimValue as V, Clip, ClipDirection, ClipKind, ClipMode, ClipRepeat, Ease, TimelineSettings};

/// Timeline length of the Motion page in ms.
pub const DURATION: u32 = 4800;
/// When the scene is fully settled and the exit phase begins (ms).
pub const REST: u32 = 4000;
/// Frames per second of the page's timeline (and of the exported animation).
pub const FPS: u32 = 30;

/// The screens of the Motion page and the layers the prototype starts from.
pub struct Motion {
    pub onboarding: NodeId,
    pub ready: NodeId,
}

impl Motion {
    pub fn all(&self) -> Vec<NodeId> {
        vec![self.onboarding, self.ready]
    }
}

const W: f32 = 400.0;
const H: f32 = 640.0;
/// Rest position of the payment card on the first screen.
const CARD_Y: f32 = 112.0;

fn night() -> Paint {
    grad_v(&[(0.0, hex(0x262A66)), (0.55, hex(0x14163F)), (1.0, hex(0x0A0B22))])
}

fn indigo() -> Color {
    hex(0x4C5BF0)
}

fn violet() -> Color {
    hex(0x8D5CF6)
}

fn mint() -> Color {
    hex(0x36D3A8)
}

fn key(s: &mut S, id: NodeId, p: P, ks: &[(u32, V, Option<Ease>)]) {
    let t = s.n(id).motion.ensure_track(p);
    for (ms, v, e) in ks {
        t.set_key(*ms, *v, *e);
    }
}

fn v2(x: f32, y: f32) -> V {
    V::Vec2(Vec2::new(x, y))
}

fn sc(f: f32) -> V {
    V::Vec2(Vec2::new(f, f))
}

/// Adds a library preset starting at `delay` ms.
fn preset(s: &mut S, id: NodeId, name: &str, delay: u32) {
    let n = s.n(id);
    let first = n.motion.next_clip_id();
    n.motion.clips.extend(find_preset(name).unwrap_or_else(|| panic!("no preset {name}")).instantiate(delay, first));
}

fn clip(s: &mut S, id: NodeId, mut c: Clip) {
    let n = s.n(id);
    c.id = n.motion.next_clip_id();
    c.sanitize();
    n.motion.clips.push(c);
}

/// Cycle count of a back-and-forth clip that comes back to rest exactly at [`REST`].
fn cycles(delay: u32, half: u32) -> ClipRepeat {
    assert_eq!((REST - delay) % (2 * half), 0, "cycles end at rest");
    ClipRepeat::Times((REST - delay) / half)
}

/// An outbound clip of the exit phase: `ms` long from `at`, eased in-out.
fn out(s: &mut S, id: NodeId, kind: ClipKind, dir: ClipDirection, amount: f32, at: u32, ms: u32) {
    assert!(at >= REST && at + ms <= DURATION, "exits stay inside the exit phase");
    clip(s, id, Clip::new(kind, ClipMode::Out).dir(dir).amount(amount).dur(ms).ease(Ease::EaseInOut).delay(at));
}

/// Fades a layer out and nudges it `dist` px towards `dir` (0 to only fade).
fn exit(s: &mut S, id: NodeId, dir: ClipDirection, dist: f32, at: u32, ms: u32) {
    out(s, id, ClipKind::Opacity, ClipDirection::Bottom, 0.0, at, ms);
    if dist > 0.0 {
        out(s, id, ClipKind::Position, dir, dist, at, ms);
    }
}

/// A back-and-forth offset: `half` ms out, `half` ms back, until [`REST`].
fn drift(s: &mut S, id: NodeId, dir: ClipDirection, dist: f32, delay: u32, half: u32) {
    let repeat = cycles(delay, half);
    clip(s, id, Clip::new(ClipKind::Position, ClipMode::Out).dir(dir).amount(dist).dur(half).ease(Ease::EaseInOut).repeat(repeat).yoyo().delay(delay));
}

/// A back-and-forth scale (breathing).
fn breathe(s: &mut S, id: NodeId, factor: f32, delay: u32, half: u32) {
    let repeat = cycles(delay, half);
    clip(s, id, Clip::new(ClipKind::Scale, ClipMode::Out).amount(factor).dur(half).ease(Ease::EaseInOut).repeat(repeat).yoyo().delay(delay));
}

#[allow(clippy::too_many_arguments)]
fn glow(s: &mut S, parent: NodeId, name: &str, x: f32, y: f32, d: f32, c: Color, alpha: f32) -> NodeId {
    let g = s.ellipse(parent, name, d, d, Cl::C(c));
    s.at(g, x, y);
    s.opacity(g, alpha);
    s.effect(g, Effect::LayerBlur { radius: 64.0, visible: true }, None);
    g
}

fn screen(s: &mut S, page: NodeId, name: &str, x: f32) -> NodeId {
    let f = s.frame(page, name);
    s.at(f, x, 0.0);
    s.size(f, W, H);
    s.fill_paint(f, night());
    s.radius(f, 32.0);
    s.clip(f);
    f
}

/// A pill-shaped frame with a translucent fill and hairline stroke.
fn glass_pill(s: &mut S, parent: NodeId, name: &str, w: f32, h: f32) -> NodeId {
    let p = s.frame(parent, name);
    s.size(p, w, h);
    s.fill_paint(p, Paint::solid(Color::WHITE.with_alpha(0.12)));
    s.stroke(p, Cl::C(Color::WHITE.with_alpha(0.22)), 1.0);
    s.radius(p, h / 2.0);
    p
}

fn centered_in(s: &mut S, id: NodeId, cx: f32, cy: f32) {
    let sz = s.n(id).size;
    s.at(id, cx - sz.x / 2.0, cy - sz.y / 2.0);
}

fn payment_card(s: &mut S, parent: NodeId) -> NodeId {
    let card = s.frame(parent, "Card");
    s.at(card, 50.0, CARD_Y);
    s.size(card, 300.0, 188.0);
    s.fill_paint(card, grad_d(&[(0.0, indigo()), (1.0, violet())]));
    s.radius(card, 24.0);
    s.clip(card);
    s.effect(
        card,
        Effect::DropShadow { color: Color::rgba(8, 10, 40, 150), offset: Vec2::new(0.0, 22.0), radius: 36.0, spread: -6.0, visible: true, show_behind: false },
        None,
    );
    // soft discs that give the gradient some depth
    let a = s.ellipse(card, "Disc 1", 190.0, 190.0, Cl::C(Color::WHITE.with_alpha(0.10)));
    s.at(a, 170.0, -70.0);
    let b = s.ellipse(card, "Disc 2", 150.0, 150.0, Cl::C(Color::WHITE.with_alpha(0.08)));
    s.at(b, -50.0, 108.0);
    let brand = s.text(card, "Marlow", "Text/Body strong", WHITE);
    s.at(brand, 24.0, 22.0);
    let c1 = s.ellipse(card, "Network 1", 28.0, 28.0, Cl::C(Color::WHITE.with_alpha(0.85)));
    s.at(c1, 232.0, 22.0);
    let c2 = s.ellipse(card, "Network 2", 28.0, 28.0, Cl::C(hex(0xF5B544).with_alpha(0.9)));
    s.at(c2, 248.0, 22.0);
    let chip = s.rect(card, "Chip", 40.0, 30.0, Cl::C(hex(0xF5C76B)));
    s.radius(chip, 7.0);
    s.fill_paint(chip, grad_d(&[(0.0, hex(0xFBE3A4)), (1.0, hex(0xE0A93E))]));
    s.at(chip, 24.0, 72.0);
    let num = s.text(card, "\u{2022}\u{2022}\u{2022}\u{2022}  \u{2022}\u{2022}\u{2022}\u{2022}  \u{2022}\u{2022}\u{2022}\u{2022}  4242", "Text/Heading", WHITE);
    s.at(num, 24.0, 118.0);
    let name = s.text(card, "ALEX MORGAN", "Text/Label", white(0.85));
    s.at(name, 24.0, 154.0);
    let exp = s.text(card, "12/28", "Text/Label", white(0.85));
    let w = s.n(exp).size.x;
    s.at(exp, 276.0 - w, 154.0);
    card
}

fn chip(s: &mut S, parent: NodeId, name: &str, label: &str, x: f32, y: f32, dot: Color) -> NodeId {
    let p = glass_pill(s, parent, name, 10.0, 40.0);
    s.at(p, x, y);
    let t = s.text(p, label, "Text/Body strong", WHITE);
    let w = s.n(t).size.x + 50.0;
    s.size(p, w, 40.0);
    s.at(t, 34.0, 9.0);
    let d = s.ellipse(p, "Dot", 10.0, 10.0, Cl::C(dot));
    s.at(d, 16.0, 15.0);
    p
}

fn cta(s: &mut S, parent: NodeId, label: &str, y: f32) -> NodeId {
    let b = s.frame(parent, "Button");
    s.at(b, 32.0, y);
    s.size(b, 336.0, 56.0);
    s.fill_paint(b, Paint::solid(Color::WHITE));
    s.radius(b, 18.0);
    s.effect(
        b,
        Effect::DropShadow {
            color: Color::rgba(76, 91, 240, 110),
            offset: Vec2::new(0.0, 10.0),
            radius: 24.0,
            spread: -4.0,
            visible: true,
            show_behind: false,
        },
        None,
    );
    let t = s.text(b, label, "Text/Button", Cl::C(hex(0x14163F)));
    let w = s.n(t).size.x;
    s.at(t, (336.0 - w - 28.0) / 2.0, 17.0);
    let ar = s.vicon(b, Ic::ArrowRight, 20.0, Cl::C(hex(0x14163F)));
    s.at(ar, (336.0 - w - 28.0) / 2.0 + w + 8.0, 18.0);
    b
}

fn onboarding(s: &mut S, page: NodeId) -> NodeId {
    let f = screen(s, page, "Onboarding", 0.0);

    // ambient glows, drifting for the whole timeline
    let ga = glow(s, f, "Glow A", -100.0, -70.0, 320.0, indigo(), 0.65);
    key(s, ga, P::Opacity, &[(0, V::Scalar(0.0), Some(Ease::EaseOut)), (900, V::Scalar(0.65), None)]);
    breathe(s, ga, 1.14, 0, 2000);
    let gb = glow(s, f, "Glow B", 190.0, 330.0, 280.0, violet(), 0.5);
    key(s, gb, P::Opacity, &[(0, V::Scalar(0.0), Some(Ease::EaseOut)), (1100, V::Scalar(0.5), None)]);
    drift(s, gb, ClipDirection::Top, 34.0, 0, 2000);

    // logo tile
    let logo = s.frame(f, "Logo");
    s.at(logo, 174.0, 44.0);
    s.size(logo, 52.0, 52.0);
    s.fill_paint(logo, grad_d(&[(0.0, hex(0x7C88FF)), (1.0, violet())]));
    s.radius(logo, 16.0);
    s.effect(
        logo,
        Effect::DropShadow { color: Color::rgba(76, 91, 240, 140), offset: Vec2::new(0.0, 8.0), radius: 18.0, spread: -2.0, visible: true, show_behind: false },
        None,
    );
    let m = s.text(logo, "M", "Text/Headline", WHITE);
    centered_in(s, m, 26.0, 26.0);
    key(s, logo, P::Scale, &[(80, sc(0.3), Some(Ease::SpringBouncy)), (900, sc(1.0), None)]);
    key(s, logo, P::Rotation, &[(80, V::Scalar(-28.0), Some(Ease::SpringGentle)), (900, V::Scalar(0.0), None)]);
    key(s, logo, P::Opacity, &[(80, V::Scalar(0.0), Some(Ease::EaseOut)), (320, V::Scalar(1.0), None)]);

    // replay pill (also the click target that restarts the animation)
    let replay = glass_pill(s, f, "Replay", 92.0, 32.0);
    s.at(replay, 284.0, 54.0);
    let ri = s.vicon(replay, Ic::Repeat, 14.0, WHITE);
    s.at(ri, 14.0, 9.0);
    let rt = s.text(replay, "Replay", "Text/Label", WHITE);
    s.at(rt, 36.0, 8.0);
    key(s, replay, P::Opacity, &[(2500, V::Scalar(0.0), Some(Ease::EaseOut)), (2900, V::Scalar(1.0), None)]);

    // the payment card: springs up and tilts into place, then floats
    let card = payment_card(s, f);
    key(s, card, P::Position, &[(300, v2(50.0, 300.0), Some(Ease::SpringBouncy)), (1250, v2(50.0, CARD_Y), None)]);
    key(s, card, P::Rotation, &[(300, V::Scalar(-16.0), Some(Ease::SpringGentle)), (1250, V::Scalar(5.0), None)]);
    key(s, card, P::Opacity, &[(300, V::Scalar(0.0), Some(Ease::EaseOut)), (700, V::Scalar(1.0), None)]);
    drift(s, card, ClipDirection::Top, 12.0, 1200, 1400);

    // floating chips
    let a = chip(s, f, "Chip income", "+$240.00", 24.0, 50.0, mint());
    preset(s, a, "Drop", 1000);
    drift(s, a, ClipDirection::Top, 8.0, 1500, 1250);
    let b = chip(s, f, "Chip saved", "Saved 12%", 24.0, 322.0, hex(0xF5B544));
    preset(s, b, "Glide", 1400);
    drift(s, b, ClipDirection::Bottom, 8.0, 1900, 1050);

    // copy
    let title = s.text(f, "Money, in motion.", "Text/Headline", WHITE);
    s.at(title, 32.0, 382.0);
    preset(s, title, "Rise", 1300);
    let sub = s.text_w(f, "Track spending, save automatically and watch every move happen in real time.", "Text/Body", white(0.66), 336.0);
    s.at(sub, 32.0, 426.0);
    preset(s, sub, "Rise", 1500);

    // pagination: the active dot stretches with a spring
    let mut dots = Vec::new();
    for (i, active) in [true, false, false].into_iter().enumerate() {
        let (w, c) = if active { (24.0, Color::WHITE) } else { (8.0, Color::WHITE.with_alpha(0.3)) };
        let dot = s.rect(f, &format!("Dot {}", i + 1), w, 8.0, Cl::C(c));
        s.radius(dot, 4.0);
        let x = 32.0 + if i == 0 { 0.0 } else { 24.0 + (i as f32 - 1.0) * 16.0 };
        s.at(dot, x, 502.0);
        dots.push(dot);
        key(s, dot, P::Opacity, &[(1800 + 80 * i as u32, V::Scalar(0.0), Some(Ease::EaseOut)), (2000 + 80 * i as u32, V::Scalar(1.0), None)]);
        if active {
            key(s, dot, P::Width, &[(1900, V::Scalar(8.0), Some(Ease::SpringBouncy)), (2600, V::Scalar(24.0), None)]);
        }
    }

    // call to action
    let c = cta(s, f, "Get started", 556.0);
    preset(s, c, "Burst", 2100);
    breathe(s, c, 1.03, 2400, 800);

    // exit phase: the scene eases out, top to bottom reversed, back to the empty start state
    exit(s, c, ClipDirection::Bottom, 22.0, 4000, 450);
    for (i, d) in dots.into_iter().enumerate() {
        exit(s, d, ClipDirection::Bottom, 0.0, 4040, 400 + 40 * i as u32);
    }
    exit(s, sub, ClipDirection::Top, 14.0, 4060, 450);
    exit(s, title, ClipDirection::Top, 14.0, 4120, 450);
    exit(s, b, ClipDirection::Left, 24.0, 4120, 480);
    exit(s, a, ClipDirection::Right, 24.0, 4180, 480);
    exit(s, replay, ClipDirection::Bottom, 0.0, 4000, 350);
    exit(s, card, ClipDirection::Bottom, 56.0, 4200, 520);
    out(s, card, ClipKind::Scale, ClipDirection::Bottom, 0.94, 4200, 520);
    exit(s, logo, ClipDirection::Top, 12.0, 4300, 450);
    out(s, logo, ClipKind::Scale, ClipDirection::Bottom, 0.7, 4300, 450);
    exit(s, ga, ClipDirection::Bottom, 0.0, 4200, 550);
    exit(s, gb, ClipDirection::Bottom, 0.0, 4200, 550);
    f
}

fn badge(s: &mut S, parent: NodeId) -> NodeId {
    let b = s.frame(parent, "Badge");
    s.at(b, 145.0, 104.0);
    s.size(b, 110.0, 110.0);
    let disc = s.ellipse(b, "Disc", 110.0, 110.0, Cl::C(mint()));
    s.fill_paint(disc, grad_d(&[(0.0, hex(0x12A67F)), (1.0, hex(0x5BE3BC))]));
    s.effect(
        disc,
        Effect::DropShadow {
            color: Color::rgba(54, 211, 168, 120),
            offset: Vec2::new(0.0, 12.0),
            radius: 28.0,
            spread: -4.0,
            visible: true,
            show_behind: false,
        },
        None,
    );
    let check = Path {
        cmds: vec![PathCmd::MoveTo(Vec2::new(0.0, 14.0)), PathCmd::LineTo(Vec2::new(14.0, 28.0)), PathCmd::LineTo(Vec2::new(40.0, 0.0))],
        fill_rule: FillRule::default(),
    };
    let c = s.vector(b, "Check", 40.0, 28.0, vec![check]);
    s.stroke(c, WHITE, 7.0);
    s.at(c, 35.0, 41.0);
    b
}

fn row(s: &mut S, parent: NodeId, label: &str, state: &str, y: f32) -> NodeId {
    let r = s.frame(parent, &format!("Row {label}"));
    s.at(r, 32.0, y);
    s.size(r, 336.0, 48.0);
    s.fill_paint(r, Paint::solid(Color::WHITE.with_alpha(0.08)));
    s.stroke(r, Cl::C(Color::WHITE.with_alpha(0.14)), 1.0);
    s.radius(r, 16.0);
    let d = s.ellipse(r, "Dot", 10.0, 10.0, Cl::C(mint()));
    s.at(d, 18.0, 19.0);
    let t = s.text(r, label, "Text/Body", WHITE);
    s.at(t, 40.0, 13.0);
    let st = s.text(r, state, "Text/Label", Cl::C(mint()));
    let w = s.n(st).size.x;
    s.at(st, 336.0 - 18.0 - w, 16.0);
    r
}

fn ready(s: &mut S, page: NodeId) -> NodeId {
    let f = screen(s, page, "Ready", 480.0);
    // same layer names as on the first screen, so Smart Animate morphs the glows between them
    let ga = glow(s, f, "Glow A", 60.0, -10.0, 300.0, mint(), 0.35);
    key(s, ga, P::Opacity, &[(0, V::Scalar(0.0), Some(Ease::EaseOut)), (900, V::Scalar(0.35), None)]);
    breathe(s, ga, 1.12, 0, 2000);
    let gb = glow(s, f, "Glow B", 140.0, 380.0, 300.0, violet(), 0.45);
    key(s, gb, P::Opacity, &[(0, V::Scalar(0.0), Some(Ease::EaseOut)), (1100, V::Scalar(0.45), None)]);
    drift(s, gb, ClipDirection::Top, 30.0, 0, 2000);

    // rippling ring behind the badge
    let ring = s.ellipse(f, "Ring", 150.0, 150.0, Cl::C(mint()));
    s.n(ring).fills = Vec::new();
    s.stroke(ring, Cl::C(mint()), 3.0);
    s.at(ring, 125.0, 84.0);
    key(s, ring, P::Opacity, &[(0, V::Scalar(0.0), Some(Ease::Hold)), (1200, V::Scalar(1.0), None)]);
    assert_eq!((REST - 1200) % 1400, 0, "ripples end at rest");
    let ring_cycles = ClipRepeat::Times((REST - 1200) / 1400);
    clip(s, ring, Clip::new(ClipKind::Scale, ClipMode::Out).amount(1.55).dur(1400).ease(Ease::EaseOut).repeat(ring_cycles).delay(1200));
    clip(s, ring, Clip::new(ClipKind::Opacity, ClipMode::Out).amount(0.0).dur(1400).ease(Ease::EaseOut).repeat(ring_cycles).delay(1200));

    let b = badge(s, f);
    key(s, b, P::Scale, &[(200, sc(0.2), Some(Ease::SpringBouncy)), (1000, sc(1.0), None)]);
    key(s, b, P::Rotation, &[(200, V::Scalar(-45.0), Some(Ease::SpringGentle)), (1000, V::Scalar(0.0), None)]);
    key(s, b, P::Opacity, &[(200, V::Scalar(0.0), Some(Ease::EaseOut)), (380, V::Scalar(1.0), None)]);

    let title = s.text(f, "You're all set", "Text/Headline", WHITE);
    s.at(title, 32.0, 262.0);
    preset(s, title, "Rise", 900);
    let sub = s.text(f, "Everything is switched on for you.", "Text/Body", white(0.66));
    s.at(sub, 32.0, 306.0);
    preset(s, sub, "Rise", 1050);

    let mut rows = Vec::new();
    for (i, (label, state)) in [("Round-ups", "On"), ("Weekly summary", "Mondays"), ("Spending limits", "Set")].into_iter().enumerate() {
        let r = row(s, f, label, state, 366.0 + i as f32 * 58.0);
        preset(s, r, "Glide", 1300 + 180 * i as u32);
        rows.push(r);
    }

    let c = cta(s, f, "Start over", 560.0);
    preset(s, c, "Feather", 2100);
    breathe(s, c, 1.03, 2400, 800);

    // exit phase, as on the first screen
    exit(s, c, ClipDirection::Bottom, 22.0, 4000, 450);
    for (i, r) in rows.into_iter().rev().enumerate() {
        exit(s, r, ClipDirection::Left, 28.0, 4040 + 80 * i as u32, 420);
    }
    exit(s, sub, ClipDirection::Top, 14.0, 4120, 450);
    exit(s, title, ClipDirection::Top, 14.0, 4160, 450);
    exit(s, ring, ClipDirection::Bottom, 0.0, 4200, 300);
    exit(s, b, ClipDirection::Top, 0.0, 4250, 450);
    out(s, b, ClipKind::Scale, ClipDirection::Bottom, 0.6, 4250, 450);
    exit(s, ga, ClipDirection::Bottom, 0.0, 4200, 550);
    exit(s, gb, ClipDirection::Bottom, 0.0, 4200, 550);
    f
}

fn click_to(dest: NodeId, kind: TransitionKind, ms: u32) -> Reaction {
    Reaction {
        trigger: Trigger::OnClick,
        actions: vec![Action::Navigate { dest, preserve_scroll: false }],
        transition: Some(Transition { kind, easing: Easing::EaseInOut, duration_ms: ms }),
    }
}

fn animation(trigger: Trigger, dest: Option<NodeId>, op: AnimationOp) -> Reaction {
    Reaction { trigger, actions: vec![Action::Animation { dest, op }], transition: None }
}

/// Builds the Motion page (the last page of the document) and wires its prototype.
pub fn build(s: &mut S) -> Motion {
    let page = s.d.pages[s.d.pages.len() - 1];
    s.d.timelines.insert(page, TimelineSettings { duration_ms: DURATION, fps: FPS, looping: true });
    let o = onboarding(s, page);
    let r = ready(s, page);

    // each screen plays its animation when it appears; the card pauses it while hovered
    s.n(o).prototype.reactions.push(animation(Trigger::AfterDelay { delay_ms: 0 }, None, AnimationOp::Play { from_start: true }));
    s.n(r).prototype.reactions.push(animation(Trigger::AfterDelay { delay_ms: 0 }, None, AnimationOp::Play { from_start: true }));
    if let Some(card) = s.find(o, "Card") {
        s.n(card).prototype.reactions.push(animation(Trigger::WhileHovering, None, AnimationOp::Pause));
    }
    if let Some(replay) = s.find(o, "Replay") {
        s.n(replay).prototype.reactions.push(animation(Trigger::OnClick, None, AnimationOp::Restart));
    }
    if let Some(go) = s.find(o, "Button") {
        s.n(go).prototype.reactions.push(click_to(r, TransitionKind::SmartAnimate, 600));
    }
    if let Some(back) = s.find(r, "Button") {
        s.n(back).prototype.reactions.push(click_to(o, TransitionKind::SmartAnimate, 600));
    }
    s.d.flows.push(Flow {
        name: "Onboarding motion".into(),
        page,
        start: o,
        description: "Animated welcome that plays on arrival, then Smart Animates to the next screen.".into(),
    });
    Motion { onboarding: o, ready: r }
}
