use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use eframe::egui::{self, Color32};
use rfd::FileDialog;

use dz_core::document::Document;
use dz_core::logic::{DangerzoneCore, IsolationProvider as CoreIsolationProvider};
use dz_core::settings::{read_settings, write_settings, Settings};
use dz_core::updater::{ReleaseReport, UpdateChecker};
use dz_core::util;
use dz_runtime::base::IsolationProvider as RuntimeIsolationProvider;
use dz_runtime::container::Container;
use dz_runtime::dummy::Dummy;
use dz_runtime::qubes::Qubes;
use dz_update::updater::Updater;

use crate::startup::run_startup_tasks;
use crate::updater::open_release_page;
use crate::widgets::{document_list_widget, drag_drop_widget, log_window};

#[derive(Debug)]
pub enum GuiEvent {
    ProgressLine(String),
    StartupLine(String),
    ConversionCompleted,
    StartupCompleted(Result<(), String>),
}

enum GuiProvider {
    Dummy(Dummy),
    Qubes(Qubes),
    Container(Container),
}

impl GuiProvider {
    fn new() -> Self {
        if dz_runtime::qubes::is_qubes_native_conversion() {
            GuiProvider::Qubes(Qubes::new(false))
        } else if util::is_dev() {
            match Dummy::new() {
                Ok(dummy) => GuiProvider::Dummy(dummy),
                Err(_) => GuiProvider::Container(Container::new(false)),
            }
        } else {
            GuiProvider::Container(Container::new(false))
        }
    }
}

impl CoreIsolationProvider for GuiProvider {
    fn convert(
        &self,
        document: &mut Document,
        ocr_lang: Option<&str>,
        stdout_callback: Option<&(dyn Fn(&str) + Sync)>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut progress = |_: bool, message: &str, _: f64| {
            if let Some(callback) = stdout_callback {
                callback(message);
            }
        };

        match self {
            GuiProvider::Dummy(provider) => {
                provider.convert(document, ocr_lang, &mut progress);
            }
            GuiProvider::Qubes(provider) => {
                provider.convert(document, ocr_lang, &mut progress);
            }
            GuiProvider::Container(provider) => {
                provider.convert(document, ocr_lang, &mut progress);
            }
        }

        Ok(())
    }

    fn get_max_parallel_conversions(&self) -> usize {
        match self {
            GuiProvider::Dummy(provider) => provider.get_max_parallel_conversions(),
            GuiProvider::Qubes(provider) => provider.get_max_parallel_conversions(),
            GuiProvider::Container(provider) => provider.get_max_parallel_conversions(),
        }
    }
}

pub struct DangerzoneApp {
    core: Option<Arc<Mutex<DangerzoneCore<GuiProvider>>>>,
    settings: Settings,
    selected_document: Option<usize>,
    messages: Vec<String>,
    startup_messages: Vec<String>,
    progress_rx: mpsc::Receiver<GuiEvent>,
    progress_tx: mpsc::Sender<GuiEvent>,
    startup_running: bool,
    conversion_running: bool,
    update_report: Option<ReleaseReport>,
    startup_error: Option<String>,
    ocr_languages: Vec<String>,
}

impl DangerzoneApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = mpsc::channel();
        let settings = read_settings().clone();
        let provider = GuiProvider::new();
        let core = DangerzoneCore::new(provider)
            .ok()
            .map(|core| Arc::new(Mutex::new(core)));
        let ocr_languages = core
            .as_ref()
            .and_then(|core| core.lock().ok())
            .map(|core| core.ocr_languages().values().cloned().collect())
            .unwrap_or_default();

        Self {
            core,
            settings,
            selected_document: None,
            messages: Vec::new(),
            startup_messages: Vec::new(),
            progress_rx: rx,
            progress_tx: tx,
            startup_running: false,
            conversion_running: false,
            update_report: None,
            startup_error: None,
            ocr_languages,
        }
    }

    fn add_documents(&mut self, paths: Vec<PathBuf>) {
        if self.core.is_none() {
            self.messages
                .push("Unable to add documents because GUI failed to initialize.".into());
            return;
        }

        let mut core = match self.core.as_ref().unwrap().lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.messages
                    .push("Could not lock the document core.".into());
                return;
            }
        };

        for path in paths {
            let path_str = path.to_string_lossy().to_string();
            match core.add_document_from_filename(&path_str, None, self.settings.archive()) {
                Ok(()) => {}
                Err(error) => self
                    .messages
                    .push(format!("Failed to add '{}': {}", path_str, error)),
            }
        }

        if let Some(output_dir) = self.settings.output_dir() {
            for doc in core.documents_mut() {
                let _ = doc.set_output_dir(output_dir);
            }
        }
    }

    fn clear_documents(&mut self) {
        if let Some(core) = &self.core {
            if let Ok(mut guard) = core.lock() {
                guard.clear_documents();
            }
        }
    }

    fn save_settings(&mut self) {
        let mut guard = write_settings();
        *guard = self.settings.clone();
        let _ = guard.save();
    }

    fn start_conversion(&mut self) {
        if self.conversion_running || self.core.is_none() {
            return;
        }

        self.save_settings();
        self.startup_error = None;
        self.startup_messages.clear();
        self.messages.clear();
        self.conversion_running = true;
        self.startup_running = true;

        let core = self.core.as_ref().unwrap().clone();
        let tx = self.progress_tx.clone();
        let ocr_lang = if self.settings.ocr() {
            Some(self.settings.ocr_language().to_string())
        } else {
            None
        };

        thread::spawn(move || {
            let _ = tx.send(GuiEvent::StartupLine("Beginning startup tasks...".into()));
            let startup_result = run_startup_tasks(|message| {
                let _ = tx.send(GuiEvent::StartupLine(message));
            });
            let _ = tx.send(GuiEvent::StartupCompleted(
                startup_result.map_err(|e| e.to_string()),
            ));
            let _ = tx.send(GuiEvent::StartupLine("Starting conversion...".into()));

            if let Ok(mut core) = core.lock() {
                core.convert_documents(
                    ocr_lang.as_deref(),
                    Some(&|line| {
                        let _ = tx.send(GuiEvent::ProgressLine(line.to_string()));
                    }),
                );
            }
            let _ = tx.send(GuiEvent::ConversionCompleted);
        });
    }

    fn check_for_updates(&mut self) {
        let mut settings_guard = write_settings();

        let updater = Updater;
        self.update_report = match updater.should_check_for_updates(&mut settings_guard) {
            Ok(true) => match updater.check_for_updates(&mut settings_guard) {
                Ok(Some(report)) => Some(report),
                Ok(None) => None,
                Err(error_report) => {
                    self.messages
                        .push(format!("Update check failed: {}", error_report.error));
                    None
                }
            },
            Ok(false) => None,
            Err(_) => None,
        };
    }

    fn handle_events(&mut self) {
        while let Ok(event) = self.progress_rx.try_recv() {
            match event {
                GuiEvent::ProgressLine(line) => self.messages.push(line),
                GuiEvent::StartupLine(line) => self.startup_messages.push(line),
                GuiEvent::StartupCompleted(result) => {
                    self.startup_running = false;
                    if let Err(error) = result {
                        self.startup_error = Some(error);
                    }
                }
                GuiEvent::ConversionCompleted => {
                    self.conversion_running = false;
                }
            }
        }
    }
}

impl eframe::App for DangerzoneApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_events();
        ctx.request_repaint();

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.heading("Dangerzone GUI");
        });

        egui::SidePanel::left("documents_panel")
            .resizable(true)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label("Drag and drop files here");
                });
                ui.add_space(8.0);

                let response = drag_drop_widget(ui);
                if response.clicked() {
                    if let Some(paths) = FileDialog::new()
                        .add_filter(
                            "Documents",
                            &["pdf", "png", "jpg", "jpeg", "tif", "tiff", "doc", "docx"],
                        )
                        .pick_files()
                    {
                        self.add_documents(paths);
                    }
                }

                for file in ctx.input(|i| i.raw.dropped_files.clone()) {
                    if let Some(path) = file.path {
                        self.add_documents(vec![path.clone()]);
                    }
                }

                ui.add_space(8.0);
                if let Some(core) = &self.core {
                    if let Ok(core) = core.lock() {
                        document_list_widget(ui, core.documents(), &mut self.selected_document);
                    } else {
                        ui.label("Unable to lock document list");
                    }
                } else {
                    ui.label("Dangerzone core failed to initialize.");
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Clear list").clicked() {
                        self.clear_documents();
                    }
                    if ui.button("Convert").clicked() {
                        self.start_conversion();
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.heading("Conversion settings");
                ui.checkbox(&mut self.settings.archive_mut(), "Archive unsafe originals");
                ui.checkbox(&mut self.settings.ocr_mut(), "Enable OCR");
                ui.horizontal(|ui| {
                    ui.label("OCR language:");
                    egui::ComboBox::from_label("")
                        .selected_text(self.settings.ocr_language())
                        .show_ui(ui, |ui| {
                            for language in &self.ocr_languages {
                                ui.selectable_value(
                                    self.settings.ocr_language_mut(),
                                    language.clone(),
                                    language,
                                );
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("Output folder:");
                    if ui.button("Browse").clicked() {
                        if let Some(path) = FileDialog::new().pick_folder() {
                            *self.settings.output_dir_mut() =
                                Some(path.to_string_lossy().to_string());
                        }
                    }
                    if let Some(output_dir) = self.settings.output_dir() {
                        ui.label(output_dir);
                    }
                });

                if ui.button("Save settings").clicked() {
                    self.save_settings();
                    self.messages.push("Saved settings".into());
                }

                ui.separator();
                ui.heading("Startup log");
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for message in &self.startup_messages {
                            ui.label(message);
                        }
                    });

                if let Some(error) = &self.startup_error {
                    ui.colored_label(Color32::RED, error);
                }

                ui.separator();
                ui.heading("Progress log");
                log_window(ui, &self.messages);

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Check for updates").clicked() {
                        self.check_for_updates();
                    }
                });

                if let Some(report) = &self.update_report {
                    ui.heading("Update status");
                    if let Some(version) = &report.version {
                        ui.label(format!("New release available: {version}"));
                        if ui.button("Open release page").clicked() {
                            open_release_page();
                        }
                    }
                    if report.container_image_bump {
                        ui.label("A new sandbox image is available.");
                    }
                }

                ui.add_space(8.0);
                if self.conversion_running {
                    ui.colored_label(Color32::GREEN, "Conversion in progress...");
                } else {
                    ui.label("Conversion idle.");
                }
            });
        });
    }
}
