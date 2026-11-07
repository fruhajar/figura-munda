//! Figura Munda, the window.
//!
//! Two phases, in order: look at the files, then clean them. The first phase
//! writes nothing, and the second cannot be reached without passing through it.

mod app;
mod palette;
mod session;
mod work;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 740.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("Figura Munda"),
        ..Default::default()
    };
    eframe::run_native(
        "Figura Munda",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
