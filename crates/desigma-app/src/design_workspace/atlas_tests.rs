//! Exercises the real workspace show seam with egui/eframe's headless test frame.
//! Does not start a native app, browser, repository reader or owner operation.
use super::*;

fn document() -> Document {
    let digest=desigma_library::payload_digest(b"atlas-ui-fixture");
    let semantic=json!({"id":"atlas-ui-fixture","revision":"1","root":{"id":"material","props":{}}});
    let draft=json!({"schemaVersion":"cityos.design-exchange.v3","semanticContract":"fixture.component",
        "profileRef":"fixture","draftId":"atlas-ui-fixture","documentId":"atlas-ui-fixture","draftRevision":0,
        "base":{"kind":"canonical-git","repositoryId":"fixture","commit":"a".repeat(40),"sourcePath":"fixture.json","sourceDigest":digest},
        "semanticBase":semantic,"semanticDraft":semantic,"semanticDigest":"c".repeat(64),
        "dependencyPins":[{"path":"fixture.json","digest":digest}],"commandReceipts":[]});
    binding(draft,&json!({"schemaVersion":"cityos.desigma-canvas.v1","draftId":"atlas-ui-fixture",
        "draftRevision":0,"semanticDigest":"c".repeat(64),"nodes":[{"id":"native","sourceNodeId":"material",
        "definitionId":"fixture","parent":null,"slot":null,"x":0,"y":0,"width":100,"height":100,
        "label":"Atlas UI fixture","fill":"#ffffff"}]})).unwrap()
}

fn frame(workspace: &mut Workspace, app: &mut App, ctx: &egui::Context, width: f32, events: Vec<egui::Event>) -> egui::FullOutput {
    let native_frame=eframe::Frame::_new_kittest();
    ctx.run_ui(egui::RawInput {
        screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(width,1000.))),
        events,..Default::default()
    },|ui|workspace.show(app,ui,&native_frame))
}

fn rect(ctx: &egui::Context, id: &str) -> egui::Rect {
    ctx.data(|data|data.get_temp(egui::Id::new(("test-rect",id.to_owned())))).unwrap()
}

#[test]
fn atlas_menu_is_available_during_capture_and_recovery_without_leaving_or_editing_the_draft() {
    for width in [390.,1200.] {
        for (busy,recovery) in [(Some("Capture".to_owned()),false),(None,true),(Some("Capture".to_owned()),true)] {
            let ctx=egui::Context::default();ctx.all_styles_mut(|style|style.animation_time=0.);
            let mut app=App::new(document());app.home=None;
            assert!(app.lib.is_none()&&app.file_path.is_none());
            let mut workspace=Workspace {open:true,busy:busy.clone(),recovery_pending:recovery,..Default::default()};
            let doc=app.doc.to_json().unwrap();let history=serde_json::to_value(&app.hist).unwrap();
            let selection=app.selection.clone();let saved=app.autosave.saved_hash;let saves=app.autosave.saves;
            frame(&mut workspace,&mut app,&ctx,width,vec![]);
            frame(&mut workspace,&mut app,&ctx,width,vec![]);
            let menu=rect(&ctx,"catalog-atlas-menu");
            assert!(menu.left()>=0.&&menu.right()<=width&&menu.bottom()<=1000.);
            for pressed in [true,false] {
                frame(&mut workspace,&mut app,&ctx,width,vec![egui::Event::PointerMoved(menu.center()),egui::Event::PointerButton {
                    pos:menu.center(),button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE,
                }]);
            }
            frame(&mut workspace,&mut app,&ctx,width,vec![]);
            assert!(egui::Popup::is_any_open(&ctx),"Atlas must not inherit capture/recovery disablement");
            for page in ["overview","workbench","findings","reports","runs"] {
                let target=rect(&ctx,&format!("catalog-atlas-{page}"));
                assert!(target.width()>0.&&target.left()>=0.&&target.right()<=width);
            }
            assert!(workspace.open&&workspace.bridge.is_none()&&workspace.config.is_none());
            assert_eq!(workspace.busy,busy);assert_eq!(workspace.recovery_pending,recovery);
            assert_eq!(app.doc.to_json().unwrap(),doc);assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);
            assert_eq!(app.selection,selection);assert_eq!(app.autosave.saved_hash,saved);assert_eq!(app.autosave.saves,saves);
        }
    }
}
