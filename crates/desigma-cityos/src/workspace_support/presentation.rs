use super::super::workspace::Workspace;
use crate::editor::EditorHost;
use serde_json::{json,Value};
use std::time::Duration;
pub fn key(draft: &Value, presentation: &Value) -> Value {
    json!({"draftId":draft["draftId"],"draftRevision":draft["draftRevision"],
        "semanticDigest":draft["semanticDigest"],"presentation":presentation})
}

pub fn scene_matches(projection: &Value, sent: Option<&Value>) -> bool {
    let Some(sent) = sent else { return false; };
    let scene = &projection["scene"];
    scene.is_object() && sent["themeId"] == ""
        && ["locale", "theme", "viewport"].iter().all(|field| scene[*field] == sent[*field])
}

pub fn scene_resource(resource: &Value) -> bool {
    matches!(resource["fixture"].as_str(), Some("cityos-accordion" | "cityos-web-frame"))
        || resource["fixture"] == "cityos-lower-level"
            && resource["nativeSceneAdapter"] == "cityos.living-canvas-lower-level.native-scene.v1"
}

impl<H: EditorHost> Workspace<H> {
    pub fn presentation(&self) -> Value {
        // Named repository themes are qualified for the actual renderer only.
        // Keep the native default-theme projection distinct and explicitly labelled.
        json!({"locale":self.locale,"theme":self.theme,"themeId":"",
            "viewport":{"width":self.viewport.0,"height":self.viewport.1}})
    }
    pub fn native_theme_notice(&self) -> Option<&'static str> {
        (self.has_native_scene() && !self.theme_id.is_empty()).then_some(
            "Native canvas uses the renderer default theme. The selected theme applies to the actual web preview.")
    }
    pub fn has_native_scene(&self) -> bool {
        !self.is_specimen() && self.resource.as_ref().is_some_and(scene_resource)
    }
    pub fn native_presentation_current(&self, app: &H) -> bool {
        self.active_draft(app).is_some_and(|draft| self.canvas_presentation.as_ref() == Some(&key(&draft, &self.presentation())))
    }
    pub fn project_response_current(&self, app: &H, value: &Value, sent: Option<&Value>) -> bool {
        sent.is_some_and(|sent| *sent == self.presentation())
            && self.active_draft(app).as_ref() == Some(&value["draft"])
    }
    pub fn refresh_native_presentation(&mut self, app: &H) -> bool {
        if !self.has_native_scene() || self.busy.is_some() || self.recovery_pending
            || !self.pending.is_some_and(|at| at.elapsed() >= Duration::from_millis(250)) { return false; }
        let Some(draft) = self.active_draft(app) else { return false; };
        let key = key(&draft, &self.presentation());
        if self.projection_attempted.as_ref() == Some(&key) { return false; }
        self.send(json!({"op":"Project","draft":draft}));
        self.busy.as_deref() == Some("Project")
    }
}

