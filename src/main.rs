mod app;
mod fling;
mod input;
mod layout;
mod render;
mod search;
mod selection;
mod system_theme;
mod ui;

use std::{path::PathBuf, sync::Arc};

use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    prelude::*,
};

fn main() {
    // `PAPERLIGHT_LOG=debug` or per target, e.g. `paperlight=debug,warn`.
    let filter = std::env::var("PAPERLIGHT_LOG")
        .ok()
        .and_then(|s| s.parse::<Targets>().ok())
        .unwrap_or_else(|| Targets::new().with_default(LevelFilter::WARN));
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().log_internal_errors(true))
        .with(filter)
        .init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Paperlight")
        .with_app_id("paperlight")
        .with_inner_size([900.0, 1100.0])
        // Narrowest width at which the title bar's controls leave room for the title.
        .with_min_inner_size([640.0, 320.0])
        .with_drag_and_drop(true)
        // The title bar draws its own controls (`ui::chrome`), macOS style.
        .with_decorations(false);
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
