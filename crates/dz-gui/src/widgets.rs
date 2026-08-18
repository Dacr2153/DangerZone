use dz_core::document::{Document, DocumentState};
use eframe::egui::{self, Color32, RichText};

pub fn drag_drop_widget(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 120.0),
        egui::Sense::click(),
    );
    let visuals = ui.style().interact(&response);
    ui.painter().rect_filled(rect, 4.0, visuals.bg_fill);
    ui.painter()
        .rect_stroke(rect, 4.0, (1.0, visuals.bg_stroke.color));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Drop files here or click to choose",
        egui::FontId::proportional(16.0),
        visuals.fg_stroke.color,
    );
    response
}

pub fn document_list_widget(
    ui: &mut egui::Ui,
    documents: &[Document],
    selected: &mut Option<usize>,
) {
    ui.vertical(|ui| {
        ui.heading("Document queue");
        ui.separator();
        for (index, document) in documents.iter().enumerate() {
            let input_path = document
                .input_filename()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| "<unknown>".into());
            let label = format!("{} — {}", input_path, status_text(document.state()));
            ui.selectable_value(selected, Some(index), label);
        }
    });
}

fn status_text(state: DocumentState) -> &'static str {
    match state {
        DocumentState::Unconverted => "Unconverted",
        DocumentState::Converting => "Converting",
        DocumentState::Safe => "Safe",
        DocumentState::Failed => "Failed",
    }
}

pub fn log_window(ui: &mut egui::Ui, messages: &[String]) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for message in messages.iter().rev() {
                    let color = if message.contains("failed") || message.contains("Failed") {
                        Color32::RED
                    } else {
                        Color32::WHITE
                    };
                    ui.label(RichText::new(message).color(color));
                    ui.separator();
                }
            });
    });
}
