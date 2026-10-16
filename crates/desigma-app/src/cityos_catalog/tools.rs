//! Private Atlas tools open through the existing native browser launcher.
//! Only these fixed routes leave Desigma; catalog data and credentials do not.
use crate::theme;
use egui::{RichText, Ui};

const ATLAS_SITE: &str = "https://cityos-atlas.qahtani1979.chatgpt.site/atlas-v4.1/";
const PRIVATE_ACCESS: &str = "Private Site · sign in with your approved account in your browser.";
const BROWSER_DISABLED: &str = "Browser opening is disabled in this build. Copy a link and open it in your browser.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    Overview,
    Graph,
    Findings,
    Reports,
    Runs,
}

const DESTINATIONS: [Destination; 5] = [Destination::Overview, Destination::Graph, Destination::Findings, Destination::Reports, Destination::Runs];

impl Destination {
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Graph => "Architecture graph",
            Self::Findings => "Findings",
            Self::Reports => "Reports & Codex exports",
            Self::Runs => "Analysis runs",
        }
    }

    fn page(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Graph => "workbench",
            Self::Findings => "findings",
            Self::Reports => "reports",
            Self::Runs => "runs",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Overview => "Review the latest accepted analysis and its coverage.",
            Self::Graph => "Trace systems, services, packages and dependencies.",
            Self::Findings => "Inspect issues, retained evidence and source locations.",
            Self::Reports => "Prepare scoped reports and download Codex Markdown for fixes.",
            Self::Runs => "Inspect analysis history, source revisions and execution status.",
        }
    }

    fn url(self) -> String {
        // Atlas investigation.js parseRoute/serializeRoute: v3 with an empty
        // state follows its latest accepted analysis. Graph is `workbench`.
        format!("{ATLAS_SITE}#v=3&page={}&state=%7B%7D", self.page())
    }
}

fn atlas_url_allowed(url: &str) -> bool {
    // An exact route allowlist also rejects credentials, alternate ports,
    // arbitrary hashes, redirect queries and deceptive host suffixes.
    DESTINATIONS.iter().any(|destination| destination.url() == url)
}

fn open_atlas_url(url: &str) -> Result<(), String> {
    if !atlas_url_allowed(url) {
        return Err("Blocked Atlas link. Use one of the registered Atlas tools.".into());
    }
    #[cfg(feature = "open-url")]
    {
        crate::home::open_link(url)
    }
    #[cfg(not(feature = "open-url"))]
    {
        Err(BROWSER_DISABLED.into())
    }
}

struct LaunchFailure {
    destination: Destination,
    message: String,
}

#[derive(Default)]
pub(super) struct AtlasTools {
    failure: Option<LaunchFailure>,
}

impl AtlasTools {
    fn open(&mut self, destination: Destination) {
        self.failure = open_atlas_url(&destination.url()).err().map(|message| LaunchFailure { destination, message });
        // A successful OS handoff says nothing about remote authentication,
        // availability or analysis qualification. The browser owns those checks.
    }

    pub(super) fn menu(&mut self, ui: &mut Ui) {
        let menu = ui.menu_button("Atlas ↗", |ui| {
            ui.set_max_width(320.0);
            ui.strong("CityOS Atlas");
            ui.label(RichText::new("Investigate architecture · prepare fixes").small().color(theme::TEXT_SECONDARY));
            ui.separator();
            if !cfg!(feature = "open-url") {
                ui.label(RichText::new(BROWSER_DISABLED).small().color(theme::TEXT_SECONDARY));
            }
            for destination in DESTINATIONS {
                ui.push_id(destination.page(), |ui| {
                    ui.horizontal(|ui| {
                        let response = ui.add_enabled(cfg!(feature = "open-url"), egui::Button::new(format!("{} ↗", destination.label())));
                        crate::widgets::record(ui, &format!("catalog-atlas-{}", destination.page()), response.rect);
                        if response.on_hover_text(destination.hint()).on_disabled_hover_text(BROWSER_DISABLED).clicked() {
                            self.open(destination);
                            ui.close();
                        }
                        if !cfg!(feature = "open-url") {
                            let copy = ui.small_button("Copy link");
                            crate::widgets::record(ui, &format!("catalog-atlas-copy-{}", destination.page()), copy.rect);
                            if copy.clicked() {
                                ui.ctx().copy_text(destination.url());
                            }
                        }
                    });
                });
            }
            ui.separator();
            ui.label(RichText::new(PRIVATE_ACCESS).small().color(theme::TEXT_SECONDARY));
            ui.collapsing("Prepare fixes for Codex", |ui| {
                ui.label("In Atlas, choose a findings scope, open Reports and export Codex Markdown. Load the downloaded .codex.md file into Codex.");
                ui.label("For large reports, use scripts/atlas-codex-pack.mjs in the Atlas repository to create dependency-aware batches.");
                ui.label("These links open the latest accepted analysis. Check its source revision against your design source, then pin the intended run before preparing fixes.");
            });
        });
        crate::widgets::record(ui, "catalog-atlas-menu", menu.response.rect);
        menu.response.on_hover_text("Open private Atlas tools in your default browser");
    }

    pub(super) fn feedback(&mut self, ui: &mut Ui) {
        let Some(failure) = &self.failure else { return; };
        let destination = failure.destination;
        let message = failure.message.clone();
        let mut retry = false;
        let mut dismiss = false;
        egui::Frame::new().inner_margin(egui::Margin::symmetric(theme::CATALOG_PAD, 6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("Atlas could not open: {message}")).color(theme::DANGER));
                retry = ui.add_enabled(cfg!(feature = "open-url"), egui::Button::new("Retry"))
                    .on_disabled_hover_text(BROWSER_DISABLED).clicked();
                let copy = ui.button("Copy Atlas link");
                crate::widgets::record(ui, "catalog-atlas-copy-failure", copy.rect);
                if copy.clicked() { ui.ctx().copy_text(destination.url()); }
                dismiss = ui.button("Dismiss").clicked();
            });
        });
        if dismiss { self.failure = None; }
        else if retry { self.open(destination); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_destinations_use_supported_v3_routes_without_credentials_or_catalog_data() {
        let expected = ["overview", "workbench", "findings", "reports", "runs"];
        for (destination, page) in DESTINATIONS.into_iter().zip(expected) {
            let raw = destination.url();
            assert!(atlas_url_allowed(&raw));
            let url = url::Url::parse(&raw).unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("cityos-atlas.qahtani1979.chatgpt.site"));
            assert_eq!(url.path(), "/atlas-v4.1/");
            assert_eq!(url.username(), "");
            assert_eq!(url.password(), None);
            assert_eq!(url.query(), None);
            let params: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(url.fragment().unwrap().as_bytes()).into_owned().collect();
            assert_eq!(params.len(), 3);
            assert_eq!(params["v"], "3");
            assert_eq!(params["page"], page);
            assert_eq!(serde_json::from_str::<serde_json::Value>(&params["state"]).unwrap(), serde_json::json!({}));
        }
    }

    #[test]
    fn atlas_allowlist_blocks_deceptive_urls_and_arbitrary_routes_before_os_handoff() {
        let allowed = Destination::Overview.url();
        let invalid = [
            allowed.replacen("https:", "http:", 1),
            allowed.replacen("https://", "https://owner@", 1),
            allowed.replacen(".site/", ".site.evil.example/", 1),
            allowed.replacen(".site/", ".site@evil.example/", 1),
            allowed.replacen(".site/", ".site:8443/", 1),
            allowed.replacen("/atlas-v4.1/", "/atlas-v4.1/../admin/", 1),
            allowed.replacen("#", "?redirect=https://evil.example#", 1),
            allowed.replace("page=overview", "page=graph"),
            allowed.replace("state=%7B%7D", "state=%7B%22runId%22%3A%22invented%22%7D"),
            format!("{allowed}&token=private"),
            format!(" {allowed}"),
            format!("{allowed}\n"),
            "javascript:alert(1)".into(),
            "file:///C:/Windows/System32/cmd.exe".into(),
            "//cityos-atlas.qahtani1979.chatgpt.site/atlas-v4.1/".into(),
        ];
        for url in invalid {
            assert!(!atlas_url_allowed(&url), "Unexpected allowlisted URL: {url}");
            assert!(open_atlas_url(&url).unwrap_err().starts_with("Blocked Atlas link."));
        }
    }

    fn painted_text(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(text) => { out.push_str(&text.galley.job.text); out.push('\n'); }
            egui::epaint::Shape::Vec(shapes) => { for shape in shapes { painted_text(shape, out); } }
            _ => {}
        }
    }

    fn frame(tools: &mut AtlasTools, ctx: &egui::Context, width: f32, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run_ui(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 720.0))),
            events, ..Default::default()
        }, |ui| { tools.menu(ui); tools.feedback(ui); })
    }

    fn click(tools: &mut AtlasTools, ctx: &egui::Context, width: f32, id: &str) -> egui::FullOutput {
        let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", id.to_string())))).unwrap();
        let pos = rect.center();
        let mut output = None;
        for pressed in [true, false] {
            output = Some(frame(tools, ctx, width, vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            }]));
        }
        output.unwrap()
    }

    #[test]
    fn atlas_menu_is_compact_and_exposes_all_five_tools_only_on_request() {
        for width in [390.0, 1200.0] {
            let mut tools = AtlasTools::default();
            let ctx = egui::Context::default();
            ctx.all_styles_mut(|style| style.animation_time = 0.0);
            frame(&mut tools, &ctx, width, vec![]);
            let closed = frame(&mut tools, &ctx, width, vec![]);
            let mut text = String::new();
            for shape in &closed.shapes { painted_text(&shape.shape, &mut text); }
            assert!(text.contains("Atlas ↗"));
            assert!(!text.contains("CityOS Atlas") && !text.contains("Codex Markdown"));
            click(&mut tools, &ctx, width, "catalog-atlas-menu");
            frame(&mut tools, &ctx, width, vec![]);
            let open = frame(&mut tools, &ctx, width, vec![]);
            text.clear();
            for shape in &open.shapes { painted_text(&shape.shape, &mut text); }
            assert!(text.contains("CityOS Atlas"));
            assert!(text.contains(PRIVATE_ACCESS));
            assert!(text.contains("Prepare fixes for Codex"));
            assert!(!text.contains("downloaded .codex.md"), "The workflow guide starts collapsed");
            for destination in DESTINATIONS {
                assert!(text.contains(destination.label()));
                let id = format!("catalog-atlas-{}", destination.page());
                let rect: egui::Rect = ctx.data(|data| data.get_temp(egui::Id::new(("test-rect", id)))).unwrap();
                assert!(rect.left() >= 0.0 && rect.right() <= width && rect.bottom() <= 720.0, "Tool outside viewport: {rect:?}");
            }
        }
    }

    #[test]
    fn browser_failure_offers_the_exact_destination_for_manual_recovery() {
        let ctx = egui::Context::default();
        let mut tools = AtlasTools { failure: Some(LaunchFailure { destination: Destination::Reports, message: "No default browser is configured".into() }) };
        frame(&mut tools, &ctx, 390.0, vec![]);
        let output = frame(&mut tools, &ctx, 390.0, vec![]);
        let mut text = String::new();
        for shape in &output.shapes { painted_text(&shape.shape, &mut text); }
        assert!(text.contains("No default browser is configured"));
        assert!(text.contains("Copy Atlas link") && text.contains("Retry") && text.contains("Dismiss"));
        let copied = click(&mut tools, &ctx, 390.0, "catalog-atlas-copy-failure");
        assert!(copied.platform_output.commands.iter().any(|command| matches!(command,
            egui::OutputCommand::CopyText(text) if text == &Destination::Reports.url())));
        assert!(!text.contains("connected") && !text.contains("authenticated"));
    }

    #[cfg(not(feature = "open-url"))]
    #[test]
    fn build_without_browser_support_offers_real_copy_links_and_never_claims_launch() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut tools = AtlasTools::default();
        assert_eq!(open_atlas_url(&Destination::Reports.url()), Err(BROWSER_DISABLED.into()));
        frame(&mut tools, &ctx, 390.0, vec![]);
        frame(&mut tools, &ctx, 390.0, vec![]);
        click(&mut tools, &ctx, 390.0, "catalog-atlas-menu");
        frame(&mut tools, &ctx, 390.0, vec![]);
        let output = frame(&mut tools, &ctx, 390.0, vec![]);
        let mut text = String::new();
        for shape in &output.shapes { painted_text(&shape.shape, &mut text); }
        assert!(text.contains(BROWSER_DISABLED));
        let copied = click(&mut tools, &ctx, 390.0, "catalog-atlas-copy-reports");
        assert!(copied.platform_output.commands.iter().any(|command| matches!(command,
            egui::OutputCommand::CopyText(text) if text == &Destination::Reports.url())));
        assert!(tools.failure.is_none());
    }
}
