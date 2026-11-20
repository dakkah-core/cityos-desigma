//! Prototype "Control animation" actions: the editor UI, and playing them in Present.

use super::*;
use crate::showcase;
use desigma_doc::{Action, AnimationOp, Reaction};

fn sim(select: &str) -> Sim {
    let mut s =
        Sim::with(Options { fixture: Some(showcase::MOTION_FIXTURE.into()), select: Some(select.into()), tab: Some("prototype".into()), ..Default::default() });
    s.frame(vec![]);
    s
}

fn reaction0(s: &Sim, name: &str) -> Reaction {
    s.app().doc.get(s.by_name(name)).unwrap().prototype.reactions[0].clone()
}

fn settle(s: &mut Sim) {
    s.frame(vec![]);
    s.frame(vec![]);
}

fn pick(s: &mut Sim, name: &str, i: usize) {
    let ctl = s.rect_of(name);
    s.click(ctl.center());
    let y = ctl.bottom() + 2.0 + 4.0 + i as f32 * theme::MENU_ITEM_HEIGHT + theme::MENU_ITEM_HEIGHT / 2.0;
    s.click(Pos2::new(ctl.left() + 20.0, y));
    settle(s);
}

#[test]
fn the_editor_edits_animation_actions_and_undo_restores_them() {
    let mut s = sim("Replay");
    assert!(matches!(reaction0(&s, "Replay").actions[0], Action::Animation { dest: None, op: AnimationOp::Restart }));
    s.click(s.rect_of("proto-row-0").center());
    assert!(s.app().proto.editing.is_some());
    settle(&mut s);
    assert!(s.rect_of("proto-pop-anim-dest-0").width() > 40.0 && s.rect_of("proto-pop-anim-op-0").width() > 40.0);
    // Pause (index 2)
    pick(&mut s, "proto-pop-anim-op-0", 2);
    assert!(matches!(reaction0(&s, "Replay").actions[0], Action::Animation { op: AnimationOp::Pause, .. }));
    // aim it at the Onboarding frame (entry 0 is "This frame")
    pick(&mut s, "proto-pop-anim-dest-0", 1);
    let onboarding = s.by_name("Onboarding");
    assert!(
        matches!(reaction0(&s, "Replay").actions[0], Action::Animation { dest: Some(d), .. } if d == onboarding || s.app().doc.get(d).is_some()),
        "{:?}",
        reaction0(&s, "Replay").actions[0]
    );
    // the action menu can switch to and from it
    pick(&mut s, "proto-pop-action-0", 1);
    assert_eq!(reaction0(&s, "Replay").actions[0], Action::Back);
    pick(&mut s, "proto-pop-action-0", 12);
    assert!(matches!(reaction0(&s, "Replay").actions[0], Action::Animation { dest: None, op: AnimationOp::Play { from_start: false } }));
    // every edit is undoable
    s.key(Key::Escape);
    for _ in 0..8 {
        s.cmd_key(Key::Z);
    }
    assert!(matches!(reaction0(&s, "Replay").actions[0], Action::Animation { dest: None, op: AnimationOp::Restart }));
}

fn present(s: &mut Sim) {
    s.click(s.rect_of("present-button").center());
    assert!(s.app().proto.present.is_some());
    for _ in 0..4 {
        s.frame(vec![]);
    }
}

fn clock(s: &Sim, frame: &str) -> (f32, bool) {
    let st = s.app().proto.present.as_ref().unwrap();
    st.player.motion_state(s.by_name(frame), st.player.now)
}

fn run(s: &mut Sim, frames: usize) {
    for _ in 0..frames {
        s.frame(vec![]);
    }
}

#[test]
fn present_plays_on_arrival_and_the_replay_button_restarts() {
    let mut s = sim("Onboarding");
    present(&mut s);
    let (t0, playing) = clock(&s, "Onboarding");
    assert!(playing, "the After delay trigger plays the animation");
    run(&mut s, 60);
    let (t1, playing) = clock(&s, "Onboarding");
    assert!(playing && t1 > t0 + 2500.0, "{t0} -> {t1}");
    // click Replay: the timeline starts over
    let id = s.by_name("Replay");
    let at = s.app().proto.present.as_ref().unwrap().screen_center_of(id).unwrap();
    s.click(at);
    let (t2, playing) = clock(&s, "Onboarding");
    assert!(playing && t2 < 400.0, "restarted: {t2}");
}

#[test]
fn hovering_the_card_pauses_and_leaving_resumes() {
    let mut s = sim("Onboarding");
    present(&mut s);
    run(&mut s, 40);
    let id = s.by_name("Card");
    let at = s.app().proto.present.as_ref().unwrap().screen_center_of(id).unwrap();
    s.mv(at);
    s.frame(vec![]);
    let (held, playing) = clock(&s, "Onboarding");
    assert!(!playing, "paused while hovered");
    run(&mut s, 10);
    assert_eq!(clock(&s, "Onboarding").0, held, "the position holds");
    s.mv(Pos2::new(s.app().canvas_rect.left() + 4.0, s.app().canvas_rect.bottom() - 4.0));
    s.frame(vec![]);
    run(&mut s, 5);
    let (t, playing) = clock(&s, "Onboarding");
    assert!(playing && t > held, "resumed from where it was: {held} -> {t}");
}

#[test]
fn smart_animate_arrival_plays_the_destination_timeline() {
    let mut s = sim("Onboarding");
    present(&mut s);
    run(&mut s, 70);
    let id = s.by_name("Get started");
    let at = s.app().proto.present.as_ref().unwrap().screen_center_of(id).unwrap();
    s.click(at);
    let ready = s.by_name("Ready");
    assert_eq!(s.app().proto.present.as_ref().unwrap().player.current, ready);
    assert!(s.app().proto.present.as_ref().unwrap().player.anim.is_some(), "smart animate runs");
    assert_eq!(clock(&s, "Ready"), (0.0, false), "the destination waits for the transition");
    for _ in 0..60 {
        s.frame(vec![]);
        if s.app().proto.present.as_ref().unwrap().player.anim.is_none() {
            break;
        }
    }
    run(&mut s, 70);
    let (t, playing) = clock(&s, "Ready");
    assert!(playing && t > 300.0, "plays after arriving: {t}");
}
