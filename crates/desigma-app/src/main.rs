mod app;
mod board;
mod canvas;
mod cli;
mod cityos_catalog;
mod cityos_jobs;
mod design_workspace;
mod clipboard;
mod comments;
mod comp_ops;
mod decks;
mod demo;
mod dev;
mod edit;
mod find;
mod fonts;
mod history;
mod home;
mod icons;
mod io;
mod layout;
mod libraries;
#[cfg(test)]
mod lint;
mod ops;
mod panels;
mod plugins;
mod present;
mod proto;
mod protovars;
mod richtext;
mod shell;
mod showcase;
mod styles;
mod textedit;
mod textmeasure;
mod theme;
mod util;
mod variables;
mod vecfixture;
mod vecops;
mod video;
mod widgets;

use std::sync::atomic::Ordering;

/// The app icon (256 px PNG embedded at build time) for the taskbar, Dock and alt-tab. Every window
/// desigma opens (including Present and "open in new window" processes) starts through `main`, so
/// they all get it.
fn window_icon() -> Option<egui::IconData> {
    const PNG: &[u8] = include_bytes!("../../../assets/icon/png/icon_256.png");
    let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

fn main() {
    let opts = match cli::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("desigma: {e}\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };
    if opts.help {
        println!("{}", cli::USAGE);
        return;
    }
    if let Some(directory)=&opts.cityos_verify_workspace {
        let browser=cityos_catalog::Browser::new(opts.cityos_repository.clone(),opts.cityos_reader.clone(),opts.cityos_node.clone(),opts.cityos_cache.clone(),opts.cityos_canonical_ref.clone());
        match design_workspace::verify_roundtrip(browser.workspace_config(),directory){Ok(report)=>println!("{report}"),Err(error)=>{eprintln!("Native workspace verification failed: {error}");std::process::exit(1);}}
        return;
    }
    if let Some(dir) = &opts.seed_library {
        match showcase::seed_library(dir) {
            Ok(()) => println!("desigma: wrote the showcase sample files to {}", dir.display()),
            Err(e) => {
                eprintln!("desigma: cannot seed {}: {e}", dir.display());
                std::process::exit(1);
            }
        }
        return;
    }
    let (w, h) = opts.size.unwrap_or((theme::DEFAULT_WINDOW.0 as u32, theme::DEFAULT_WINDOW.1 as u32));
    let screenshot_mode = opts.screenshot.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(theme::APP_TITLE)
        .with_inner_size([w as f32, h as f32])
        .with_min_inner_size([theme::MIN_WINDOW.0, theme::MIN_WINDOW.1]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    if screenshot_mode {
        viewport = viewport.with_min_inner_size([200.0, 200.0]).with_resizable(true);
    }
    let native = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(theme::APP_TITLE, native, Box::new(move |cc| Ok(Box::new(shell::Shell::new(cc, opts)))));
    if let Err(e) = result {
        eprintln!("desigma: {e}");
        std::process::exit(1);
    }
    if screenshot_mode && !shell::SHOT_OK.load(Ordering::SeqCst) {
        eprintln!("desigma: screenshot was not captured");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod icon_tests {
    #[test]
    fn embedded_window_icon_decodes() {
        let icon = super::window_icon().expect("icon decodes");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
