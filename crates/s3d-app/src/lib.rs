//! Section3D desktop application.

use eframe::egui;
pub mod app;
pub mod beds;
pub mod doc;
pub mod jobs;
pub mod logo;
pub mod panels;
pub mod parts;
pub mod project;
pub mod recent;
pub mod theme;
pub mod tooltips;
pub mod view;
pub mod viewport;
pub mod widgets;

/// The egui context, for waking the UI from background threads.
pub static CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

pub fn run() -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).ok();
    let mut viewport = egui::ViewportBuilder::default().with_title("Section3D").with_inner_size([1500.0, 920.0]).with_min_inner_size([980.0, 640.0]).with_drag_and_drop(true);
    if let Some(i) = icon {
        viewport = viewport.with_icon(i);
    }
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, ..Default::default() };
    eframe::run_native("Section3D", options, Box::new(|cc| Ok(Box::new(app::S3dApp::new(cc)))))
}
