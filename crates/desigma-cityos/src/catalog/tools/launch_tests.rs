//! Local lifecycle fixtures only. Never opens a browser or claims remote auth.
use super::*;
use std::process::{Child, Command, Stdio};

static SERIAL: Mutex<()> = Mutex::new(());
const CHILD_TEST: &str = "catalog::tools::launch_tests::launcher_child_fixture";

// Test-binary fixture, absent from production. The native child is owned/reaped
// through the same helper wait/cancellation path; no external browser is used.
#[test]
fn launcher_child_fixture() {
    match std::env::var("DESIGMA_ATLAS_TEST_CHILD").as_deref() {
        Ok("hold") => {
            std::thread::sleep(Duration::from_secs(60));
            std::process::exit(0);
        }
        Ok("fail") => std::process::exit(7),
        Ok("exit") => std::process::exit(0),
        _ => {}
    }
}

fn child(kind: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--test-threads=1"])
        .env("DESIGMA_ATLAS_TEST_CHILD", kind)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn frame(ctx: &egui::Context, workspace: bool, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600., 900.))), events, ..Default::default() }, |ui| {
        if workspace {
            super::super::Browser::atlas_workspace_menu(ui, super::super::test_support::services());
            super::super::Browser::atlas_workspace_feedback(ui, super::super::test_support::services());
        } else {
            let mut tools = AtlasTools::default();
            tools.menu(ui, super::super::test_support::services());
            tools.feedback(ui, super::super::test_support::services());
        }
    })
}

fn text(output: &egui::FullOutput) -> String {
    fn append(shape: &egui::epaint::Shape, result: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                result.push_str(&text.galley.job.text);
                result.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    append(shape, result);
                }
            }
            _ => {}
        }
    }
    let mut result = String::new();
    for shape in &output.shapes {
        append(&shape.shape, &mut result);
    }
    result
}

fn click(ctx: &egui::Context, workspace: bool, id: &str) -> egui::FullOutput {
    let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", id.to_owned())))).unwrap();
    let mut result = None;
    for pressed in [true, false] {
        result = Some(frame(
            ctx,
            workspace,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton { pos: rect.center(), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
            ],
        ));
    }
    result.unwrap()
}

fn settle(ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = session(ctx);
        let mut state = state.lock().unwrap();
        state.poll(ctx);
        if state.pending.is_none() {
            return;
        }
        assert!(Instant::now() < deadline, "local helper did not finish");
        drop(state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn permit_released() {
    let deadline = Instant::now() + Duration::from_secs(5);
    while HANDOFF_WORKER.load(Ordering::Acquire) {
        assert!(Instant::now() < deadline, "worker permit leaked");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn real_helper_exit_failure_timeout_and_cancel_are_reaped() {
    let _serial = SERIAL.lock().unwrap();
    for (kind, timeout, cancel, expected) in [
        ("exit", Duration::from_secs(5), false, "ok"),
        ("fail", Duration::from_secs(5), false, "exit 7"),
        ("hold", Duration::from_millis(100), false, "timed out"),
        ("hold", Duration::from_secs(5), true, "canceled"),
    ] {
        let mut helper = child(kind);
        let flag = AtomicBool::new(cancel);
        let outcome = wait_for_helper(&mut helper, &flag, Instant::now() + timeout);
        if expected == "ok" {
            assert!(outcome.is_ok());
        } else {
            assert!(outcome.unwrap_err().contains(expected));
        }
        assert!(helper.try_wait().unwrap().is_some(), "owned native helper must be reaped before result publication");
    }
}

#[test]
fn single_worker_ignores_concurrent_clicks_and_ui_frames_do_not_wait() {
    let _serial = SERIAL.lock().unwrap();
    permit_released();
    let ctx = egui::Context::default();
    let state = session(&ctx);
    let (ready_send, ready) = mpsc::sync_channel(1);
    let pending = spawn_handoff(Destination::Reports, move |cancelled, deadline| {
        ready_send.send(()).unwrap();
        loop {
            handoff_active(cancelled, deadline)?;
            std::thread::park_timeout(HANDOFF_POLL);
        }
    })
    .unwrap();
    state.lock().unwrap().pending = Some(pending);
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    state.lock().unwrap().open(Destination::Findings);
    assert_eq!(state.lock().unwrap().pending.as_ref().unwrap().destination, Destination::Reports);
    assert!(spawn_handoff(Destination::Overview, |_, _| panic!("second worker must not start")).is_err());
    let before = Instant::now();
    let output = frame(&ctx, true, vec![]);
    assert!(before.elapsed() < Duration::from_secs(3), "UI waited for the unfinished background task");
    assert!(state.lock().unwrap().pending.is_some());
    assert!(text(&output).contains("Sending to your browser"));
    assert!(output.viewport_output.values().any(|viewport| viewport.repaint_delay <= UI_POLL));
    state.lock().unwrap().pending.as_ref().unwrap().cancel();
    settle(&ctx);
    permit_released();
    assert!(state.lock().unwrap().failure.as_ref().unwrap().message.contains("canceled"));
}

#[test]
fn catalog_and_canvas_share_failure_identity_copy_and_manual_recovery() {
    let ctx = egui::Context::default();
    let original = session(&ctx);
    original.lock().unwrap().failure = Some(LaunchFailure { destination: Destination::Reports, message: "Local launcher failed".into() });
    for workspace in [false, true, false] {
        frame(&ctx, workspace, vec![]);
        let output = frame(&ctx, workspace, vec![]);
        assert!(Arc::ptr_eq(&original, &session(&ctx)));
        assert!(text(&output).contains("Local launcher failed"));
        let output = click(&ctx, workspace, "catalog-atlas-copy-failure");
        assert!(output.platform_output.commands.iter().any(|command| matches!(command,
            egui::OutputCommand::CopyText(value) if value==&Destination::Reports.url())));
    }
}

#[test]
fn successful_local_helper_exit_only_reports_handoff_never_authenticated_access() {
    let _serial = SERIAL.lock().unwrap();
    permit_released();
    let ctx = egui::Context::default();
    let pending = spawn_handoff(Destination::Overview, |cancelled, deadline| {
        let mut helper = child("exit");
        wait_for_helper(&mut helper, cancelled, deadline)
    })
    .unwrap();
    session(&ctx).lock().unwrap().pending = Some(pending);
    settle(&ctx);
    permit_released();
    frame(&ctx, true, vec![]);
    let output = frame(&ctx, true, vec![]);
    let text = text(&output);
    assert!(text.contains("Browser handoff finished") && text.contains("access has not been verified"));
    assert!(!text.contains("Connected") && !text.contains("authenticated"));
}

#[test]
fn worker_failure_publishes_exact_destination_after_native_helper_reap() {
    let _serial = SERIAL.lock().unwrap();
    permit_released();
    let ctx = egui::Context::default();
    let pending = spawn_handoff(Destination::Findings, |cancelled, deadline| {
        let mut helper = child("fail");
        let result = wait_for_helper(&mut helper, cancelled, deadline);
        assert!(helper.try_wait().unwrap().is_some());
        result
    })
    .unwrap();
    session(&ctx).lock().unwrap().pending = Some(pending);
    settle(&ctx);
    permit_released();
    let state = session(&ctx);
    let state = state.lock().unwrap();
    assert_eq!(state.failure.as_ref().unwrap().destination, Destination::Findings);
    assert!(state.failure.as_ref().unwrap().message.contains("exit 7"));
}

#[test]
fn context_disposal_cancels_and_reaps_helper_without_retaining_context_or_joining_ui() {
    let _serial = SERIAL.lock().unwrap();
    permit_released();
    let ctx = egui::Context::default();
    let state = session(&ctx);
    let weak = Arc::downgrade(&state);
    let (ready_send, ready) = mpsc::sync_channel(1);
    let (ended_send, ended) = mpsc::sync_channel(1);
    let pending = spawn_handoff(Destination::Reports, move |cancelled, deadline| {
        let mut helper = child("hold");
        ready_send.send(()).unwrap();
        let result = wait_for_helper(&mut helper, cancelled, deadline);
        ended_send.send(helper.try_wait().unwrap().is_some()).unwrap();
        result
    })
    .unwrap();
    state.lock().unwrap().pending = Some(pending);
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(state);
    let before = Instant::now();
    drop(ctx);
    assert!(before.elapsed() < Duration::from_secs(3), "context disposal joined an OS worker");
    assert!(weak.upgrade().is_none(), "worker must not own the egui session/context");
    assert!(ended.recv_timeout(Duration::from_secs(5)).unwrap());
    permit_released();
}

#[test]
fn disconnected_result_is_failure_and_does_not_leave_stale_pending_state() {
    let ctx = egui::Context::default();
    let (send, result) = mpsc::sync_channel(1);
    drop(send);
    session(&ctx).lock().unwrap().pending =
        Some(PendingLaunch { destination: Destination::Graph, cancelled: Arc::new(AtomicBool::new(false)), result, worker: std::thread::current() });
    session(&ctx).lock().unwrap().poll(&ctx);
    let state = session(&ctx);
    let state = state.lock().unwrap();
    assert!(state.pending.is_none() && state.dispatched.is_none());
    assert_eq!(state.failure.as_ref().unwrap().destination, Destination::Graph);
    assert!(state.failure.as_ref().unwrap().message.contains("stopped"));
}

#[test]
fn expanded_shared_guide_routes_small_and_large_reports_and_retains_source_pin() {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|style| style.animation_time = 0.);
    frame(&ctx, true, vec![]);
    frame(&ctx, true, vec![]);
    click(&ctx, true, "catalog-atlas-menu");
    frame(&ctx, true, vec![]);
    frame(&ctx, true, vec![]);
    click(&ctx, true, "catalog-atlas-guide");
    frame(&ctx, true, vec![]);
    let output = frame(&ctx, true, vec![]);
    let text = text(&output);
    // Rendered guide only; this does not prove a browser save or local packaging.
    for expected in [
        "For small reports in Atlas Reports, use a completed report's Download Codex batches action.",
        "The ZIP contains README.md",
        "25 tasks",
        "For large reports, choose Save full report, then use scripts/atlas-codex-pack.mjs",
        "local batches from the saved report",
        "one downloaded .codex.md batch",
        "serially",
        "exact source pin",
        PRIVATE_ACCESS,
    ] {
        assert!(text.contains(expected), "shared guide omitted {expected}");
    }
    assert!(!text.contains("Optional CLI fallback"), "local packaging is the large-report workflow");
}
