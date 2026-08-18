mod app;
mod startup;
mod updater;
mod widgets;

fn main() {
    let native_options = eframe::NativeOptions::default();
    let _ = eframe::run_native(
        "Dangerzone",
        native_options,
        Box::new(|cc| Ok(Box::new(app::DangerzoneApp::new(cc)))),
    );
}
