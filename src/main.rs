mod app;
mod fling;
mod input;
mod layout;
mod render;
mod search;
mod selection;
mod ui;

use std::{path::PathBuf, sync::Arc};

use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("PAPERLIGHT_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Paperlight")
        .with_app_id("paperlight")
        .with_inner_size([900.0, 1100.0])
        .with_min_inner_size([420.0, 320.0])
        .with_drag_and_drop(true);
    match eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        Ok(icon) => viewport = viewport.with_icon(Arc::new(icon)),
        Err(e) => tracing::warn!("bad icon: {e}"),
    }
    // glow is the only renderer compiled in.
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let code = match eframe::run_native(
        "paperlight",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, path)))),
    ) {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!("{e}");
            1
        }
    };
    // Exit before winit tears down the display the pinch thread borrows.
    std::process::exit(code);
}
