//! Offline example bundle. Icons and their attribution come from the editor's Lucide assets.

pub(super) fn file(path: &str) -> Result<Vec<u8>, String> {
    const MANIFEST: &[u8] = include_bytes!("../../assets/plugins/icon-browser/manifest.json");
    let bytes = match path {
        "manifest.json" => MANIFEST,
        "index.html" => include_bytes!("../../assets/plugins/icon-browser/index.html"),
        "main.js" => include_bytes!("../../assets/plugins/icon-browser/main.js"),
        "style.css" => include_bytes!("../../assets/plugins/icon-browser/style.css"),
        "LICENSE" => {
            return Ok(format!(
                "Icon Browser code:\n{}\nLucide icons:\n{}",
                include_str!("../../../../LICENSE-MIT"),
                include_str!("../../assets/icons/lucide/LICENSE")
            )
            .into_bytes());
        }
        "icons.json" => {
            let icons = serde_json::json!({
                "search": include_str!("../../assets/icons/lucide/search.svg"),
                "book": include_str!("../../assets/icons/lucide/book.svg"),
                "clock": include_str!("../../assets/icons/lucide/clock.svg"),
                "download": include_str!("../../assets/icons/lucide/download.svg"),
                "check": include_str!("../../assets/icons/lucide/check.svg"),
                "code-xml": include_str!("../../assets/icons/lucide/code-xml.svg"),
                "component": include_str!("../../assets/icons/lucide/component.svg"),
                "diamond": include_str!("../../assets/icons/lucide/diamond.svg"),
                "arrow-right": include_str!("../../assets/icons/lucide/arrow-right.svg"),
                "circle-help": include_str!("../../assets/icons/lucide/circle-help.svg"),
                "dices": include_str!("../../assets/icons/lucide/dices.svg"),
                "contrast": include_str!("../../assets/icons/lucide/contrast.svg")
            });
            return serde_json::to_vec(&icons).map_err(|e| e.to_string());
        }
        _ => return Err("Unknown included plugin asset.".into()),
    };
    Ok(bytes.to_vec())
}
