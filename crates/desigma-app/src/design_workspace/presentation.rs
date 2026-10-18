//! Presentation can replace derived canvas objects without editing a bound draft.
use super::Workspace;
#[cfg(test)]
use super::draft;
use crate::app::App;
use desigma_doc::Document;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Default)]
pub struct AutoFit {
    draft_id: String,
    bounds: [f32; 6],
    view: [f32; 3],
}

pub(super) fn fit_canvas(state: &mut Option<AutoFit>, app: &mut App) {
    let Some(binding)=app.doc.cityos.as_ref() else { return; };
    let Some(bounds)=app.page_bounds() else { return; };
    let size=app.canvas_rect.size();
    if !size.is_finite() || size.x<=0. || size.y<=0. {return;}
    let geometry=[bounds.x,bounds.y,bounds.w,bounds.h,size.x,size.y];
    let view=[app.view.zoom,app.view.offset.x,app.view.offset.y];
    let initial=!app.first_fit_done || state.as_ref().is_none_or(|last|last.draft_id!=binding.draft_id);
    // Follow automatic fitting only until the user pans or zooms. Opening from
    // a large preview and panel reflow can resize the derived scene afterwards.
    if initial || state.as_ref().is_some_and(|last|last.view==view && last.bounds!=geometry) {
        let draft_id=binding.draft_id.clone();
        app.view=crate::app::View::fit(bounds,size);
        app.first_fit_done=true;
        app.canvas.tiles.invalidate_all();
        *state=Some(AutoFit {draft_id,bounds:geometry,view:[app.view.zoom,app.view.offset.x,app.view.offset.y]});
    }
}

pub use desigma_cityos::workspace_support::presentation::{key,scene_matches,scene_resource};
pub(super) fn install(app: &mut App, projection: Document) -> Result<(), String> {
    let mut current = serde_json::to_value(&app.doc).map_err(|e| e.to_string())?;
    let derived = serde_json::to_value(&projection).map_err(|e| e.to_string())?;
    let mut binding = current["cityos"].clone();
    let mut proposed = derived["cityos"].clone();
    for value in [&mut binding, &mut proposed] {
        value.as_object_mut().ok_or("Presentation requires a bound document")?.remove("occurrences");
    }
    if binding != proposed { return Err("Obsolete presentation cannot replace the current semantic draft".into()); }
    // Keep document-owned annotations, variables, styles and metadata. Only the
    // derived node tree and its occurrence map belong to this projection.
    for field in ["nodes", "pages", "next_id"] { current[field] = derived[field].clone(); }
    current["cityos"]["occurrences"] = derived["cityos"]["occurrences"].clone();
    let document: Document = serde_json::from_value(current).map_err(|e| e.to_string())?;
    document.to_json().map_err(|e| e.to_string())?;
    let selected: Vec<_> = app.selection.iter().filter_map(|id| app.doc.cityos.as_ref()?.occurrences.get(&id.0.to_string()))
        .map(|occurrence| (occurrence.source_node_id.clone(), occurrence.view_id.clone())).collect();
    let page = if document.pages.contains(&app.page) { app.page } else { *document.pages.first().ok_or("Presentation has no canvas page")? };
    app.doc = document;
    app.page = page;
    app.selection = app.doc.cityos.as_ref().into_iter().flat_map(|b| b.occurrences.iter())
        .filter(|(_, o)| selected.iter().any(|(source, view)| source == &o.source_node_id && view == &o.view_id))
        .filter_map(|(id, _)| id.parse().ok().map(desigma_doc::NodeId)).collect();
    // No changed(), history operation, saved-hash reset or tile revision bump.
    app.canvas.tiles.invalidate_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use desigma_doc::{CityosBase, CityosBinding, CityosOccurrence, Command, Node, Rect};
    use std::collections::BTreeMap;

    #[test]
    fn standalone_native_scenes_require_the_explicit_adapter_without_upgrading_saved_protected_profiles() {
        let mut resource = json!({"fixture":"cityos-lower-level","actualWebPreview":true});
        assert!(!scene_resource(&resource));
        resource["nativeSceneAdapter"] = json!("cityos.living-canvas-lower-level.native-scene.v1");
        assert!(scene_resource(&resource));
        resource["nativeSceneAdapter"] = json!("unqualified");
        assert!(!scene_resource(&resource));
        resource["fixture"] = json!("foreign");
        resource["nativeSceneAdapter"] = json!("cityos.living-canvas-lower-level.native-scene.v1");
        assert!(!scene_resource(&resource));
    }

    fn document() -> Document {
        let mut doc = Document::new("Saved Accordion");
        let page = doc.add_page("Bound canvas");
        let node = doc.alloc_id();
        doc.insert(page, Node::frame(node, Rect::new(0., 0., 560., 720.))).unwrap();
        let base = json!({"root":{"id":"accordion","props":{"allowMultiple":false}}});
        doc.cityos = Some(CityosBinding { schema_version:"cityos.design-exchange.v3".into(), document_id:"document".into(), draft_id:"draft".into(),
            base:CityosBase::CanonicalGit{repository_id:"repo".into(),commit:"a".repeat(40)},semantic_base:base.clone(),dependency_pins:vec![],
            occurrences:BTreeMap::from([(node.0.to_string(),CityosOccurrence {definition_id:"Accordion".into(),source_node_id:"accordion".into(),
                instance_id:None,occurrence_id:"draft:accordion".into(),view_id:"native-canvas/root".into()})]),exchange_v2:None,
            exchange_v3:Some(json!({"schemaVersion":"cityos.design-exchange.v3","documentId":"document","draftId":"draft",
                "base":{"kind":"canonical-git","repositoryId":"repo","commit":"a".repeat(40)},"semanticBase":base,"semanticDraft":base,
                "draftRevision":0,"semanticDigest":"base","semanticContract":"component","commandReceipts":[],"dependencyPins":[]})) });
        doc
    }
    fn reply(draft: Value, presentation: &Value) -> Value {
        json!({"draft":draft,"resource":{"fixture":"cityos-accordion","actualWebPreview":true},
            "projection":{"schemaVersion":"cityos.desigma-canvas.v1","draftId":draft["draftId"],"draftRevision":draft["draftRevision"],"semanticDigest":draft["semanticDigest"],
                "nodes":[{"id":"draft:accordion","sourceNodeId":"accordion","definitionId":"Accordion","parent":null,"protected":true}],
                "scene":{"schemaVersion":"cityos.desigma-native-scene.v1","adapter":"cityos.accordion.visual.v1",
                    "locale":presentation["locale"],"direction":if presentation["locale"]=="ar" {"rtl"} else {"ltr"},
                    "theme":presentation["theme"],"viewport":presentation["viewport"],"nodes":[{"id":"root","occurrenceId":"draft:accordion","parent":null,
                        "kind":"frame","x":0,"y":0,"width":560,"height":720,"label":"Accordion","fill":[1,1,1,1]}]}}})
    }
    #[test]
    fn reprojecting_preserves_unsaved_semantics_history_metadata_and_dirty_retry() {
        let mut app = App::new(document()); app.home = None;
        app.autosave.saved_hash = crate::home::doc_hash(&app.doc);
        let mut next = app.doc.clone();
        next.cityos.as_mut().unwrap().exchange_mut().unwrap()["semanticDraft"]["root"]["props"]["allowMultiple"] = json!(true);
        app.hist.exec(&mut app.doc, Command::CityosInstall {document:Box::new(next)}).unwrap();
        app.doc.name = "Unsaved name".into();
        app.autosave.confirmed = true; app.autosave.changed_at = Some(42.); app.autosave.failures = 2; app.autosave.retry_at = Some(50.);
        let saved = app.autosave.saved_hash; let hash = crate::home::doc_hash(&app.doc);
        let history = serde_json::to_value(&app.hist).unwrap(); let semantic = draft(&app.doc).unwrap().clone(); let rev = app.canvas.tiles.rev();
        let mut projection = app.doc.clone(); projection.name = "Renderer title must not replace mine".into();
        projection.nodes.values_mut().for_each(|node| node.transform.tx += 20.);
        install(&mut app, projection).unwrap();
        assert_eq!(app.doc.name,"Unsaved name"); assert_eq!(draft(&app.doc),Some(&semantic));
        assert_eq!(crate::home::doc_hash(&app.doc),hash); assert_ne!(hash,saved);
        assert_eq!(serde_json::to_value(&app.hist).unwrap(),history); assert_eq!(app.canvas.tiles.rev(),rev);
        assert_eq!(app.autosave.saved_hash,saved); assert_eq!(app.autosave.changed_at,Some(42.));
        assert!(app.autosave.confirmed); assert_eq!(app.autosave.failures,2); assert_eq!(app.autosave.retry_at,Some(50.));
    }
    #[test]
    fn saved_bound_presentation_is_clean_but_semantics_dependencies_and_native_local_copies_are_dirty() {
        let original = document(); let baseline = crate::home::doc_hash(&original);
        let mut projected = original.clone(); projected.nodes.values_mut().for_each(|node|node.transform.tx+=50.);
        projected.cityos.as_mut().unwrap().occurrences.values_mut().for_each(|o|o.view_id.push_str("/new"));
        assert_eq!(crate::home::doc_hash(&projected),baseline);
        let mut authored = projected.clone(); authored.cityos.as_mut().unwrap().exchange_mut().unwrap()["draftRevision"] = json!(1);
        assert_ne!(crate::home::doc_hash(&authored),baseline);
        let mut dependency = projected.clone(); dependency.cityos.as_mut().unwrap().dependency_pins.push("retained".into());
        assert_ne!(crate::home::doc_hash(&dependency),baseline);
        let mut metadata = projected.clone(); metadata.name.push_str(" changed"); assert_ne!(crate::home::doc_hash(&metadata),baseline);
        let mut local = original.clone(); local.cityos = None;
        let hash = crate::home::doc_hash(&local); local.nodes.values_mut().for_each(|node|node.transform.tx+=20.);
        assert_ne!(crate::home::doc_hash(&local),hash);
    }
    #[test]
    fn delayed_projection_is_fenced_by_full_draft_and_presentation_without_fencing_a_semantic_command() {
        let app = App::new(document()); let mut workspace = Workspace::default();
        let sent = workspace.presentation(); let reply = json!({"draft":draft(&app.doc)});
        assert!(workspace.project_response_current(&app,&reply,Some(&sent)));
        workspace.locale = "en".into();
        assert!(!workspace.project_response_current(&app,&reply,Some(&sent)));
        let mut stale = reply.clone(); stale["draft"]["draftRevision"] = json!(1);
        assert!(!workspace.project_response_current(&app,&stale,Some(&workspace.presentation())));
        let mut changed = document(); changed.cityos.as_mut().unwrap().exchange_mut().unwrap()["draftRevision"] = json!(2);
        let mut app = app; let before = app.doc.to_json().unwrap();
        assert!(install(&mut app,changed).is_err()); assert_eq!(app.doc.to_json().unwrap(),before);
        // Command replies still use the existing revision+1 history boundary;
        // their older presentation is recorded and refreshed after installation.
        workspace.canvas_presentation = Some(key(draft(&app.doc).unwrap(),&sent));
        assert!(!workspace.native_presentation_current(&app));
    }
    #[test]
    fn native_scenes_must_match_the_requested_presentation_and_cannot_substitute_inspection_cards() {
        let workspace = Workspace::default(); let sent = workspace.presentation();
        let mut projection = json!({"scene":{"locale":"ar","theme":"light","viewport":{"width":560,"height":720}}});
        assert!(scene_matches(&projection,Some(&sent)));
        for field in ["locale","theme","viewport"] {
            let mut wrong=projection.clone(); wrong["scene"][field]=Value::Null;
            assert!(!scene_matches(&wrong,Some(&sent)),"{field}");
        }
        projection.as_object_mut().unwrap().remove("scene");
        assert!(!scene_matches(&projection,Some(&sent)));
        assert!(!scene_matches(&projection,None));
    }
    #[test]
    fn named_renderer_themes_never_masquerade_as_native_paints_or_block_semantic_commands() {
        let mut workspace=Workspace::default();
        workspace.resource=Some(json!({"fixture":"cityos-web-frame","actualWebPreview":true}));
        let native=workspace.presentation();let rendered=workspace.scenario();
        workspace.theme_id="pinned-repository-theme".into();
        assert_eq!(workspace.presentation(),native);assert_ne!(workspace.scenario(),rendered);
        assert_eq!(workspace.scenario()["themeId"],"pinned-repository-theme");
        assert!(workspace.native_theme_notice().unwrap().contains("actual web preview"));
        let (bridge,requests,_responses)=super::super::bridge::Bridge::test_transport();
        workspace.bridge=Some(bridge);
        workspace.send(json!({"op":"Command","draft":draft(&document())}));
        let request=requests.recv().unwrap();
        assert_eq!(request["presentation"]["themeId"],"");assert_eq!(workspace.theme_id,"pinned-repository-theme");
        workspace.theme_id.clear();assert!(workspace.native_theme_notice().is_none());
    }
    #[test]
    fn a_locale_change_during_a_command_keeps_the_edit_then_reprojects_without_a_history_entry() {
        let root=std::env::temp_dir().join(format!("desigma-presentation-{}",uuid::Uuid::new_v4()));
        let mut app=App::new(document()); app.lib=Some(desigma_library::Library::open(&root).unwrap()); app.home=None;
        let (bridge,requests,responses)=super::super::bridge::Bridge::test_transport();
        let mut workspace=Workspace::default(); workspace.bridge=Some(bridge); workspace.validated=true; workspace.themes_loaded=true;
        workspace.resource=Some(json!({"fixture":"cityos-accordion","actualWebPreview":true}));
        workspace.send(json!({"op":"Command","draft":draft(&app.doc),"command":{
            "draftId":"draft","expectedRevision":0,"operationId":"locale-race-edit",
            "operation":"set-property","nodeId":"accordion","property":"allowMultiple","value":true}}));
        let command=requests.recv().unwrap(); assert_eq!(command["presentation"]["locale"],"ar");
        workspace.locale="en".into();
        let mut edited=draft(&app.doc).unwrap().clone(); edited["draftRevision"]=json!(1); edited["semanticDigest"]=json!("changed");
        edited["semanticDraft"]["root"]["props"]["allowMultiple"]=json!(true);
        edited["commandReceipts"]=json!([{"operationId":command["command"]["operationId"],"requestDigest":"a".repeat(64),"revision":1}]);
        responses.send(Ok(reply(edited,&command["presentation"]))).unwrap(); workspace.poll(&mut app);
        assert_eq!(draft(&app.doc).unwrap()["draftRevision"],1);
        assert_eq!(draft(&app.doc).unwrap()["semanticDraft"]["root"]["props"]["allowMultiple"],true);
        assert!(app.hist.can_undo()); assert!(!workspace.native_presentation_current(&app));
        let history=serde_json::to_value(&app.hist).unwrap(); let hash=crate::home::doc_hash(&app.doc);
        workspace.pending=Some(std::time::Instant::now()-Duration::from_millis(251)); workspace.poll(&mut app);
        let project=requests.recv().unwrap(); assert_eq!(project["op"],"Project"); assert_eq!(project["presentation"]["locale"],"en");
        responses.send(Ok(reply(project["draft"].clone(),&project["presentation"]))).unwrap(); workspace.poll(&mut app);
        assert!(workspace.native_presentation_current(&app)); assert_eq!(serde_json::to_value(&app.hist).unwrap(),history);
        assert_eq!(crate::home::doc_hash(&app.doc),hash); assert_eq!(draft(&app.doc).unwrap()["draftRevision"],1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn opening_from_large_preview_refits_the_native_scene_but_manual_zoom_survives_later_projections() {
        let mut app=App::new(document()); app.canvas_rect=egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(580.,780.));
        let root=app.doc.pages[0];let object=app.doc.get(root).unwrap().children[0];
        app.doc.get_mut(object).unwrap().size=desigma_doc::Vec2::new(1381.,676.);
        let hash=crate::home::doc_hash(&app.doc);let mut auto=None;
        fit_canvas(&mut auto,&mut app);let large=app.view.zoom;
        app.doc.get_mut(object).unwrap().size=desigma_doc::Vec2::new(539.,608.);
        fit_canvas(&mut auto,&mut app);
        assert!(app.view.zoom>large*1.5,"A wide temporary preview must not leave a miniature native canvas after opening");
        app.view.zoom=0.75;app.view.offset=desigma_doc::Vec2::new(10.,20.);
        app.doc.get_mut(object).unwrap().size=desigma_doc::Vec2::new(960.,720.);
        fit_canvas(&mut auto,&mut app);
        assert_eq!(app.view.zoom,0.75);assert_eq!(app.view.offset,desigma_doc::Vec2::new(10.,20.));
        assert_eq!(crate::home::doc_hash(&app.doc),hash);assert!(!app.hist.can_undo());
        assert!(app.autosave.changed_at.is_none(),"Fitting a derived view must not schedule a semantic save");
    }
}
