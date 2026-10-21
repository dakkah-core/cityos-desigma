//! Facilitation widgets of a board: a timer and a voting session in a pill at the top of the
//! canvas. Both live in the document ([`BoardSession`]) and every change is an undo step.
//! While a vote is open, clicking an item adds a vote dot to it (Alt-click takes one back);
//! ending the vote lists the items by votes.

use crate::app::App;
use crate::icons::Icon;
use crate::theme;
use crate::widgets;
use desigma_doc::*;
use egui::{Align2, Color32, Pos2, Rect as ERect, Sense, Stroke, StrokeKind, Ui, Vec2 as EV};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const H: f32 = 36.0;
const BTN: f32 = 28.0;
const PAD: f32 = 4.0;
const SEP: f32 = 9.0;
const VOTE_PURPLE: Color32 = Color32::from_rgb(0x97, 0x47, 0xFF);

/// Unix time in seconds.
pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Whether a vote can be put on `id`: any item that is not a container or a connector.
pub fn votable(doc: &Document, id: NodeId) -> bool {
    doc.get(id).is_some_and(|n| !n.kind.is_container() && !matches!(n.kind, NodeKind::Connector(_) | NodeKind::Page { .. } | NodeKind::Unknown { .. }))
}

/// Applies `f` to a copy of the session and stores the result as one undo step (nothing when
/// it did not change).
pub fn set_session(app: &mut App, f: impl FnOnce(&mut BoardSession)) {
    let mut s = app.doc.board_session.clone();
    f(&mut s);
    if s == app.doc.board_session {
        return;
    }
    if let Err(e) = app.hist.exec(&mut app.doc, Command::SetBoardSession { session: s }) {
        app.show_toast(format!("Edit failed: {e}"), true);
    }
    app.changed();
}

/// Adds (or, with `add == false`, takes back) one vote on `id` while a vote is open.
pub fn vote(app: &mut App, id: NodeId, add: bool) {
    if add && app.doc.board_session.vote.as_ref().is_some_and(|v| !v.ended && v.left() == 0) {
        app.show_toast("No votes left", false);
        return;
    }
    set_session(app, |s| {
        if let Some(v) = s.vote.as_mut() {
            if add {
                v.add(id);
            } else {
                v.remove(id);
            }
        }
    });
}

pub fn start_timer(app: &mut App, minutes: u32) {
    set_session(app, |s| s.timer = Some(TimerState::new(minutes)));
}

pub fn toggle_timer(app: &mut App) {
    let t = now();
    set_session(app, |s| {
        if let Some(timer) = s.timer.as_mut() {
            if timer.running() {
                timer.pause(t);
            } else {
                timer.start(t);
            }
        }
    });
}

pub fn add_timer_minute(app: &mut App) {
    let t = now();
    set_session(app, |s| {
        if let Some(timer) = s.timer.as_mut() {
            timer.add_minute(t);
        }
    });
}

pub fn reset_timer(app: &mut App) {
    set_session(app, |s| {
        if let Some(timer) = s.timer.as_mut() {
            timer.reset();
        }
    });
}

pub fn close_timer(app: &mut App) {
    set_session(app, |s| s.timer = None);
}

pub fn start_vote(app: &mut App) {
    let n = app.board.votes_each;
    app.board.vote_menu = false;
    app.board.results_open = true;
    set_session(app, |s| s.vote = Some(VoteSession::new(n)));
}

pub fn end_vote(app: &mut App) {
    set_session(app, |s| {
        if let Some(v) = s.vote.as_mut() {
            v.ended = true;
        }
    });
}

pub fn clear_vote(app: &mut App) {
    app.board.vote_menu = false;
    set_session(app, |s| s.vote = None);
}

/// A short name for a result row: the node's text, else its layer name.
pub fn item_label(doc: &Document, id: NodeId) -> String {
    let Some(n) = doc.get(id) else { return String::new() };
    let text = n.kind.any_text().map(|t| t.content.trim().replace('\n', " ")).filter(|t| !t.is_empty());
    let label = text.unwrap_or_else(|| n.name.clone());
    if label.chars().count() > 28 {
        format!("{}...", label.chars().take(27).collect::<String>())
    } else {
        label
    }
}

/// A flat text button; `primary` fills it with the brand colour.
fn text_button(ui: &mut Ui, r: ERect, key: &str, label: &str, primary: bool, tip: &str) -> bool {
    let resp = ui.interact(r, ui.id().with(key), Sense::click());
    let p = ui.painter().clone();
    let radius = theme::cr(theme::RADIUS_MEDIUM);
    if primary {
        let fill = if resp.hovered() { theme::BG_BRAND_HOVER } else { theme::BG_BRAND };
        p.rect_filled(r, radius, fill);
    } else {
        widgets::button_bg(&p, r, &resp, false, radius);
    }
    p.text(r.center(), Align2::CENTER_CENTER, label, theme::font_body_strong(), theme::TEXT);
    widgets::record(ui, key, r);
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    resp.clicked()
}

fn icon_btn(ui: &mut Ui, r: ERect, key: &str, icon: Icon, selected: bool, tip: &str) -> bool {
    let resp = widgets::icon_button(ui, r, key.to_string(), icon, selected, tip);
    let clicked = resp.clicked();
    widgets::record(ui, key, r);
    clicked
}

fn timer_width(s: &BoardSession) -> f32 {
    if s.timer.is_some() {
        BTN + 54.0 + BTN * 4.0
    } else {
        92.0
    }
}

fn vote_width(s: &BoardSession) -> f32 {
    match &s.vote {
        None => 84.0,
        Some(v) if !v.ended => BTN + 70.0 + 48.0,
        Some(_) => BTN + 66.0 + 58.0,
    }
}

/// The pill at the top centre of the canvas, the vote setup and the results list.
pub fn show(app: &mut App, ui: &mut Ui, canvas: ERect) {
    if !app.is_board() || app.history.viewing.is_some() {
        return;
    }
    let t = now();
    let sess = app.doc.board_session.clone();
    if sess.timer.as_ref().is_some_and(|t| t.running()) {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    let width = PAD * 2.0 + timer_width(&sess) + SEP + vote_width(&sess);
    let bar = ERect::from_min_size(Pos2::new(canvas.center().x - width / 2.0, canvas.top() + 8.0), EV::new(width, H));
    let p = ui.painter().clone();
    widgets::shadow(&p, bar, theme::RADIUS_LARGE, EV::new(0.0, 3.0), 8.0, theme::SHADOW_COLOR);
    widgets::shadow(&p, bar, theme::RADIUS_LARGE, EV::new(0.0, 1.0), 3.0, theme::SHADOW_COLOR_STRONG);
    p.rect_filled(bar, theme::cr(theme::RADIUS_LARGE), theme::BG_TOOLBAR);
    p.rect_stroke(bar.shrink(0.5), theme::cr(theme::RADIUS_LARGE), Stroke::new(1.0, theme::INNER_HIGHLIGHT), StrokeKind::Inside);
    ui.interact(bar, ui.id().with("board-session-block"), Sense::click_and_drag());
    widgets::record(ui, "board-session-bar", bar);

    let cy = bar.center().y;
    let mut x = bar.left() + PAD;
    let cell = |x: f32, w: f32| ERect::from_min_size(Pos2::new(x, cy - BTN / 2.0), EV::new(w, BTN));

    // ---- timer
    match &sess.timer {
        None => {
            let r = cell(x, 92.0);
            let resp = ui.interact(r, ui.id().with("board-timer-start"), Sense::click());
            widgets::button_bg(&p, r, &resp, false, theme::cr(theme::RADIUS_MEDIUM));
            widgets::paint_icon_in(&p, ERect::from_min_size(r.min, EV::splat(BTN)), Icon::Timer, 16.0, theme::ICON);
            p.text(Pos2::new(r.left() + BTN, cy), Align2::LEFT_CENTER, "Timer", theme::font_body_strong(), theme::TEXT);
            widgets::record(ui, "board-timer-start", r);
            if resp.on_hover_text("Start a timer").clicked() {
                start_timer(app, TIMER_DEFAULT_MINUTES);
            }
            x += 92.0;
        }
        Some(timer) => {
            let finished = timer.finished(t);
            let r = cell(x, BTN);
            widgets::paint_icon_in(&p, r, Icon::Timer, 16.0, if finished { theme::DANGER } else { theme::ICON });
            x += BTN;
            let clock = cell(x, 54.0);
            p.text(clock.center(), Align2::CENTER_CENTER, timer.clock(t), theme::font_body_strong(), if finished { theme::DANGER } else { theme::TEXT });
            widgets::record(ui, "board-timer-clock", clock);
            x += 54.0;
            let (icon, tip) = if timer.running() { (Icon::Pause, "Pause") } else { (Icon::Play, "Start") };
            if icon_btn(ui, cell(x, BTN), "board-timer-play", icon, timer.running(), tip) {
                toggle_timer(app);
            }
            x += BTN;
            let add = cell(x, BTN);
            if text_button(ui, add, "board-timer-add", "+1", false, "Add a minute") {
                add_timer_minute(app);
            }
            x += BTN;
            if icon_btn(ui, cell(x, BTN), "board-timer-reset", Icon::Reset, false, "Reset") {
                reset_timer(app);
            }
            x += BTN;
            if icon_btn(ui, cell(x, BTN), "board-timer-close", Icon::Close, false, "Close timer") {
                close_timer(app);
            }
            x += BTN;
        }
    }

    x += SEP / 2.0;
    widgets::vline(&p, x, bar.top() + 8.0, bar.bottom() - 8.0, theme::BORDER);
    x += SEP / 2.0;

    // ---- voting
    let vote_anchor;
    match &sess.vote {
        None => {
            let r = cell(x, 84.0);
            let resp = ui.interact(r, ui.id().with("board-vote-start"), Sense::click());
            widgets::button_bg(&p, r, &resp, app.board.vote_menu, theme::cr(theme::RADIUS_MEDIUM));
            widgets::paint_icon_in(&p, ERect::from_min_size(r.min, EV::splat(BTN)), Icon::Vote, 16.0, theme::ICON);
            p.text(Pos2::new(r.left() + BTN, cy), Align2::LEFT_CENTER, "Vote", theme::font_body_strong(), theme::TEXT);
            widgets::record(ui, "board-vote-start", r);
            if resp.on_hover_text("Start a voting session").clicked() {
                app.board.vote_menu = !app.board.vote_menu;
            }
            vote_anchor = r;
        }
        Some(v) if !v.ended => {
            let r = cell(x, BTN);
            widgets::paint_icon_in(&p, r, Icon::Vote, 16.0, VOTE_PURPLE);
            let label = cell(x + BTN, 70.0);
            p.text(label.center(), Align2::CENTER_CENTER, format!("{} of {} left", v.left(), v.votes_per_person), theme::font_body(), theme::TEXT);
            widgets::record(ui, "board-vote-left", label);
            let end = cell(x + BTN + 70.0, 48.0);
            if text_button(ui, end, "board-vote-end", "End", true, "End voting and show the results") {
                end_vote(app);
            }
            vote_anchor = cell(x, BTN + 70.0 + 48.0);
        }
        Some(v) => {
            let r = cell(x, BTN);
            widgets::paint_icon_in(&p, r, Icon::Vote, 16.0, VOTE_PURPLE);
            let res = cell(x + BTN, 66.0);
            let resp = ui.interact(res, ui.id().with("board-vote-results"), Sense::click());
            widgets::button_bg(&p, res, &resp, app.board.results_open, theme::cr(theme::RADIUS_MEDIUM));
            p.text(res.center(), Align2::CENTER_CENTER, format!("Results ({})", v.results(&app.doc).len()), theme::font_body(), theme::TEXT);
            widgets::record(ui, "board-vote-results", res);
            if resp.on_hover_text("Show or hide the results").clicked() {
                app.board.results_open = !app.board.results_open;
            }
            let clear = cell(x + BTN + 66.0, 58.0);
            if text_button(ui, clear, "board-vote-clear", "Clear", false, "Remove the votes") {
                clear_vote(app);
            }
            vote_anchor = cell(x, BTN + 66.0 + 58.0);
        }
    }

    if app.board.vote_menu && sess.vote.is_none() {
        setup_popup(app, ui, vote_anchor);
    }
    if app.board.results_open && sess.vote.as_ref().is_some_and(|v| v.ended) {
        results_panel(app, ui, vote_anchor);
    }
}

/// "Votes per person" stepper with a Start button, hanging below the Vote button.
fn setup_popup(app: &mut App, ui: &mut Ui, anchor: ERect) {
    let rect = ERect::from_min_size(Pos2::new(anchor.center().x - 104.0, anchor.bottom() + 12.0), EV::new(208.0, 116.0));
    let ctx = ui.ctx().clone();
    let mut start = false;
    let mut delta = 0i32;
    let area = egui::Area::new(egui::Id::new("board-vote-menu")).order(egui::Order::Foreground).fixed_pos(rect.min).show(&ctx, |ui| {
        ui.allocate_rect(rect, Sense::hover());
        let p = ui.painter().clone();
        widgets::menu_panel(&p, rect, theme::RADIUS_LARGE);
        p.text(Pos2::new(rect.left() + 12.0, rect.top() + 18.0), Align2::LEFT_CENTER, "Votes per person", theme::font_body_strong(), theme::TEXT);
        let row_y = rect.top() + 38.0;
        let minus = ERect::from_min_size(Pos2::new(rect.right() - 12.0 - 3.0 * BTN - 24.0, row_y), EV::splat(BTN));
        let plus = ERect::from_min_size(Pos2::new(minus.right() + 48.0, row_y), EV::splat(BTN));
        p.text(
            Pos2::new(minus.right() + 24.0, minus.center().y),
            Align2::CENTER_CENTER,
            app.board.votes_each.to_string(),
            theme::font_body_strong(),
            theme::TEXT,
        );
        if icon_btn(ui, minus, "board-vote-minus", Icon::Minus, false, "Fewer votes") {
            delta = -1;
        }
        if icon_btn(ui, plus, "board-vote-plus", Icon::Plus, false, "More votes") {
            delta = 1;
        }
        let go = ERect::from_min_size(Pos2::new(rect.left() + 12.0, rect.bottom() - 12.0 - BTN), EV::new(rect.width() - 24.0, BTN));
        start = text_button(ui, go, "board-vote-go", "Start voting", true, "");
    });
    if delta != 0 {
        app.board.votes_each = (app.board.votes_each as i32 + delta).clamp(1, VOTES_MAX as i32) as u32;
    }
    if start {
        start_vote(app);
        return;
    }
    let layer = area.response.layer_id;
    let (pressed, ptr, esc) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos(), i.key_pressed(egui::Key::Escape)));
    let other = |p: Pos2| ctx.layer_id_at(p).is_some_and(|l| l.order == egui::Order::Foreground && l != layer);
    if esc || (pressed && ptr.is_some_and(|p| !rect.contains(p) && !anchor.contains(p) && !other(p))) {
        app.board.vote_menu = false;
    }
}

/// The items by votes, most first; a click selects the item.
fn results_panel(app: &mut App, ui: &mut Ui, anchor: ERect) {
    let Some(v) = app.doc.board_session.vote.clone() else { return };
    let rows = v.results(&app.doc);
    let shown = rows.len().min(8);
    let row_h = 30.0;
    let h = 40.0 + if rows.is_empty() { 30.0 } else { shown as f32 * row_h } + 8.0;
    let rect = ERect::from_min_size(Pos2::new(anchor.center().x - 140.0, anchor.bottom() + 12.0), EV::new(280.0, h));
    let ctx = ui.ctx().clone();
    let mut picked = None;
    egui::Area::new(egui::Id::new("board-vote-results-panel")).order(egui::Order::Foreground).fixed_pos(rect.min).show(&ctx, |ui| {
        ui.allocate_rect(rect, Sense::hover());
        let p = ui.painter().clone();
        widgets::menu_panel(&p, rect, theme::RADIUS_LARGE);
        widgets::record(ui, "board-vote-panel", rect);
        let total: u32 = rows.iter().map(|r| r.1).sum();
        p.text(Pos2::new(rect.left() + 12.0, rect.top() + 20.0), Align2::LEFT_CENTER, "Results", theme::font_body_strong(), theme::TEXT);
        p.text(Pos2::new(rect.right() - 12.0, rect.top() + 20.0), Align2::RIGHT_CENTER, format!("{total} votes"), theme::font_body(), theme::TEXT_TERTIARY);
        if rows.is_empty() {
            p.text(Pos2::new(rect.left() + 12.0, rect.top() + 55.0), Align2::LEFT_CENTER, "No votes were cast", theme::font_body(), theme::TEXT_TERTIARY);
        }
        for (i, (id, n)) in rows.iter().take(8).enumerate() {
            let r = ERect::from_min_size(Pos2::new(rect.left() + 6.0, rect.top() + 40.0 + i as f32 * row_h), EV::new(rect.width() - 12.0, row_h));
            let resp = ui.interact(r, ui.id().with(("board-vote-row", i)), Sense::click());
            widgets::button_bg(&p, r, &resp, app.selection == vec![*id], theme::cr(theme::RADIUS_MEDIUM));
            p.text(Pos2::new(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, format!("{}", i + 1), theme::font_body(), theme::TEXT_TERTIARY);
            p.text(Pos2::new(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, item_label(&app.doc, *id), theme::font_body(), theme::TEXT);
            let dot = Pos2::new(r.right() - 20.0, r.center().y);
            p.circle_filled(dot, 10.0, VOTE_PURPLE);
            p.text(dot, Align2::CENTER_CENTER, n.to_string(), theme::font_body_strong(), Color32::WHITE);
            widgets::record(ui, &format!("board-vote-row-{i}"), r);
            if resp.clicked() {
                picked = Some(*id);
            }
        }
    });
    if let Some(id) = picked {
        app.select_only(id);
        app.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_come_from_text_then_name_and_are_short() {
        let mut d = Document::new("t");
        let page = d.add_page("p");
        let id = d.alloc_id();
        let mut n = Node::sticky(id, Rect::new(0.0, 0.0, 10.0, 10.0), "Ship it\nnow");
        n.name = "Sticky note".into();
        d.insert(page, n).unwrap();
        assert_eq!(item_label(&d, id), "Ship it now");
        let id2 = d.alloc_id();
        let mut n = Node::sticky(id2, Rect::new(0.0, 0.0, 10.0, 10.0), "");
        n.name = "Sticky note".into();
        d.insert(page, n).unwrap();
        assert_eq!(item_label(&d, id2), "Sticky note");
        let id3 = d.alloc_id();
        d.insert(page, Node::sticky(id3, Rect::new(0.0, 0.0, 10.0, 10.0), "x".repeat(60))).unwrap();
        assert!(item_label(&d, id3).ends_with("..."));
        assert!(votable(&d, id) && !votable(&d, page));
    }
}
