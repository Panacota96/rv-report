//! The window. Drawing only: rules live in `state.rs`, work lives in the core (on a worker thread).
//! Rakvor dark theme; interface language EN/ES/FR switchable at any time (texts from i18n "ui").

use crate::state::{GuiState, LANG_CHOICES, Phase};
use eframe::egui;
use rv_report_core::{Event, HashAlgo, RunSummary, TOOL_VERSION};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

// Rakvor palette
const NIGHT: egui::Color32 = egui::Color32::from_rgb(0x0D, 0x11, 0x17);
const NIGHT2: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x1F, 0x26);
const BLUE: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x5C, 0xFF);
const VIOLET: egui::Color32 = egui::Color32::from_rgb(0x6B, 0x00, 0xFF);
const CYAN: egui::Color32 = egui::Color32::from_rgb(0x00, 0xE5, 0xFF);
const MIST: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xE6, 0xE6);
const AMBER: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xB0, 0x40);
const UI_LANGS: [(&str, &str); 3] = [("en", "English"), ("es", "Español"), ("fr", "Français")];

enum Msg {
    Event(Event),
    Done(Result<RunSummary, String>),
}

pub struct ReportApp {
    pub state: GuiState,
    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
    pub notice: Option<String>,
    themed: bool,
    /// Text fields being typed; applied (and audited) when the field loses focus.
    pub case_entry: String,
    mandate_entry: String,
    include_entry: String,
    exclude_entry: String,
    examiner_entry: String,
    examiner_id_entry: String,
}

/// Applies the Rakvor dark theme.
pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = NIGHT;
    v.window_fill = NIGHT2;
    v.extreme_bg_color = NIGHT2;
    v.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0x2B, 0x32, 0x40));
    v.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(0x2B, 0x32, 0x40));
    v.faint_bg_color = NIGHT2;
    v.override_text_color = Some(MIST);
    v.selection.bg_fill = BLUE;
    v.hyperlink_color = CYAN;
    v.widgets.inactive.bg_fill = NIGHT2;
    v.widgets.inactive.weak_bg_fill = NIGHT2;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CYAN);
    v.widgets.active.bg_fill = BLUE;
    ctx.set_visuals(v);
}

impl ReportApp {
    pub fn new() -> Self {
        let state = GuiState::default();
        let examiner_entry = state.actor.clone();
        ReportApp {
            state,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: None,
            notice: None,
            themed: false,
            case_entry: String::new(),
            mandate_entry: String::new(),
            include_entry: String::new(),
            exclude_entry: String::new(),
            examiner_entry,
            examiner_id_entry: String::new(),
        }
    }

    /// Starts from the executable's arguments: `--case-dir DIR` (RV Hub), or a folder passed (or dropped on the
    /// .exe) that is a case folder.
    pub fn from_launch(args: crate::launch::LaunchArgs) -> Self {
        let mut app = Self::new();
        let dir = args.case_dir.or_else(|| {
            args.inputs
                .into_iter()
                .find(|p| p.join("case.json").is_file())
        });
        if let Some(dir) = dir {
            app.state.set_case_dir_from_launcher(&dir);
            app.case_entry = dir.display().to_string();
            // A verified case opens on its section preview (read-only; audited as ui_preview_built).
            if app.state.case_summary.is_some() {
                app.state.build_preview();
            }
        }
        for n in &args.notes {
            app.state.note_launch_problem(n);
        }
        if !args.notes.is_empty() {
            app.notice = Some(args.notes.join("\n"));
        }
        app
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (cfg, opts) = match self.state.to_config() {
            Ok(c) => c,
            Err(e) => {
                self.notice = Some(e);
                return;
            }
        };
        self.notice = None;
        self.state.start();
        self.cancel.store(false, Ordering::SeqCst);
        self.started = Some(Instant::now());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let cancel = self.cancel.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let r2 = repaint.clone();
            let mut sink = move |e: &Event| {
                let _ = tx2.send(Msg::Event(e.clone()));
                r2.request_repaint();
            };
            let result = rv_report_core::run::run_assemble(&cfg, &opts, &mut sink, &cancel);
            let _ = tx.send(Msg::Done(result));
            repaint.request_repaint();
        });
    }

    fn drain(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut finished = false;
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Event(e) => {
                    if let Event::Timestamp { status, reason, .. } = &e
                        && status != "ok"
                    {
                        self.state
                            .warnings
                            .push(format!("timestamp: {}", reason.as_deref().unwrap_or("-")));
                    }
                    self.state.apply(&e)
                }
                Msg::Done(Ok(_)) => finished = true,
                Msg::Done(Err(e)) => {
                    self.state.fail(e);
                    finished = true;
                }
            }
        }
        if finished {
            self.rx = None;
            self.started = None;
            self.state.refresh_case();
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
            ui.painter().rect(
                rect,
                8.0,
                NIGHT2,
                egui::Stroke::new(2.0, BLUE),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center() - egui::vec2(0.0, 3.0),
                egui::Align2::CENTER_CENTER,
                "RV",
                egui::FontId::proportional(16.0),
                MIST,
            );
            ui.painter().line_segment(
                [
                    rect.center() + egui::vec2(-8.0, 10.0),
                    rect.center() + egui::vec2(8.0, 10.0),
                ],
                egui::Stroke::new(2.0, CYAN),
            );
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("R A K V O R  ·  RV Report")
                        .strong()
                        .size(16.0),
                );
                let desc = rv_report_core::i18n::catalog(&self.state.ui_lang).brand()["descriptor"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                ui.label(egui::RichText::new(desc).color(CYAN).size(10.0));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let current = UI_LANGS
                    .iter()
                    .find(|(k, _)| *k == self.state.ui_lang)
                    .map(|(_, v)| *v)
                    .unwrap_or("English");
                let mut chosen = None;
                egui::ComboBox::from_id_salt("ui-lang")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (k, v) in UI_LANGS {
                            if ui.selectable_label(self.state.ui_lang == k, v).clicked() {
                                chosen = Some(k);
                            }
                        }
                    });
                if let Some(k) = chosen {
                    self.state.set_ui_lang(k);
                }
                ui.label(self.state.tr("ui_lang"));
                ui.weak(format!("v{}", TOOL_VERSION));
            });
        });
        ui.weak(self.state.tr("audit_note"));
        let (r, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
        let third = r.width() / 3.0;
        for (i, c) in [BLUE, VIOLET, CYAN].iter().enumerate() {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    r.min + egui::vec2(third * i as f32, 0.0),
                    egui::vec2(third, 3.0),
                ),
                0.0,
                *c,
            );
        }
    }

    fn section_title(ui: &mut egui::Ui, text: String) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(text).color(CYAN).strong().size(15.0));
    }

    fn case_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("case_section"));
        ui.label(self.state.tr("case_help"));
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.case_entry)
                        .hint_text(self.state.tr("case_dir_hint"))
                        .desired_width(460.0),
                );
                if r.lost_focus() {
                    self.state.set_case_dir(&self.case_entry.clone());
                }
                #[cfg(windows)]
                if ui.button(self.state.tr("browse")).clicked()
                    && let Some(d) = rfd::FileDialog::new().pick_folder()
                {
                    self.case_entry = d.display().to_string();
                    self.state.set_case_dir(&self.case_entry.clone());
                }
            });
        });
        // Drag and drop of a case folder onto the window.
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| p.is_dir())
                .collect()
        });
        if enabled && let Some(d) = dropped.into_iter().next() {
            self.case_entry = d.display().to_string();
            self.state.set_case_dir(&self.case_entry.clone());
        }
        if let Some(s) = &self.state.case_summary {
            ui.label(
                egui::RichText::new(format!(
                    "{} {} · {} {} · {} {}",
                    self.state.tr("case"),
                    s.case_id,
                    s.items,
                    self.state.tr("items"),
                    self.state.tr("jurisdiction_case"),
                    s.jurisdiction
                ))
                .color(CYAN),
            );
            ui.weak(format!(
                "{}: {}",
                self.state.tr("out_dir"),
                self.state.out_dir_display()
            ));
        } else if let Some(e) = &self.state.case_error {
            ui.colored_label(egui::Color32::from_rgb(0xFF, 0x6B, 0x6B), e);
        }
    }

    fn report_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("report_section"));
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("report-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(self.state.tr("structure"));
                    let label = |j: &str| match j {
                        "co" => self.state.tr("jur_co"),
                        "fr" => self.state.tr("jur_fr"),
                        _ => self.state.tr("jur_case"),
                    };
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt("jur")
                        .selected_text(label(&self.state.jurisdiction))
                        .show_ui(ui, |ui| {
                            for j in ["", "co", "fr"] {
                                if ui
                                    .selectable_label(self.state.jurisdiction == j, label(j))
                                    .clicked()
                                {
                                    chosen = Some(j);
                                }
                            }
                        });
                    if let Some(j) = chosen {
                        self.state.set_jurisdiction(j);
                    }
                    ui.end_row();
                    ui.label(self.state.tr("report_lang"));
                    let mut lang = None;
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(
                            LANG_CHOICES
                                .iter()
                                .find(|(k, _)| *k == self.state.lang)
                                .map(|(_, v)| *v)
                                .unwrap_or("English"),
                        )
                        .show_ui(ui, |ui| {
                            for (k, v) in LANG_CHOICES {
                                if ui.selectable_label(self.state.lang == k, v).clicked() {
                                    lang = Some(k);
                                }
                            }
                        });
                    if let Some(l) = lang {
                        self.state.set_lang(l);
                    }
                    ui.end_row();
                    ui.label(self.state.tr("examiner"));
                    let a = ui.add(
                        egui::TextEdit::singleline(&mut self.examiner_entry).desired_width(260.0),
                    );
                    ui.end_row();
                    ui.label(self.state.tr("examiner_id"));
                    let b = ui.add(
                        egui::TextEdit::singleline(&mut self.examiner_id_entry)
                            .hint_text(self.state.tr("optional"))
                            .desired_width(260.0),
                    );
                    if a.lost_focus() || b.lost_focus() {
                        self.state.set_examiner(
                            &self.examiner_entry.clone(),
                            &self.examiner_id_entry.clone(),
                        );
                    }
                    ui.end_row();
                    ui.label(self.state.tr("mandate"));
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.mandate_entry)
                                .hint_text(self.state.tr("mandate_hint"))
                                .desired_width(320.0),
                        );
                        // Only the Windows build has the file dialog; elsewhere `pick` stays None.
                        #[cfg_attr(not(windows), allow(unused_mut))]
                        let mut pick: Option<String> = None;
                        #[cfg(windows)]
                        if ui.button(self.state.tr("browse")).clicked()
                            && let Some(f) = rfd::FileDialog::new()
                                .add_filter("Text", &["txt"])
                                .pick_file()
                        {
                            pick = Some(f.display().to_string());
                        }
                        if let Some(p) = pick {
                            self.mandate_entry = p;
                        }
                        if (r.lost_focus()
                            || self.mandate_entry != self.state.mandate && !r.has_focus())
                            && let Err(e) = self.state.set_mandate(&self.mandate_entry.clone())
                        {
                            self.notice = Some(e);
                            self.mandate_entry = self.state.mandate.clone();
                        }
                    });
                    ui.end_row();
                    ui.label(self.state.tr("include_tools"));
                    let i = ui.add(
                        egui::TextEdit::singleline(&mut self.include_entry)
                            .hint_text("rv-pdfauth, rv-imgauth")
                            .desired_width(320.0),
                    );
                    ui.end_row();
                    ui.label(self.state.tr("exclude_tools"));
                    let x = ui.add(
                        egui::TextEdit::singleline(&mut self.exclude_entry).desired_width(320.0),
                    );
                    if (i.lost_focus() || x.lost_focus())
                        && let Err(e) = self.state.set_tool_filters(
                            &self.include_entry.clone(),
                            &self.exclude_entry.clone(),
                        )
                    {
                        self.notice = Some(e);
                    }
                    ui.end_row();
                    ui.label(self.state.tr("hashes"));
                    ui.horizontal(|ui| {
                        for a in HashAlgo::ALL {
                            let mut on = self.state.selected.contains(&a);
                            let r = ui.add_enabled(
                                self.state.can_toggle(a),
                                egui::Checkbox::new(&mut on, a.name()),
                            );
                            if r.changed()
                                && let Err(e) = self.state.toggle_algo(a)
                            {
                                self.notice = Some(e);
                            }
                        }
                    });
                    ui.end_row();
                    ui.label(self.state.tr("final"));
                    let mut fin = self.state.final_report;
                    if ui.checkbox(&mut fin, self.state.tr("final_help")).changed() {
                        self.state.set_final(fin);
                    }
                    ui.end_row();
                });
        });
    }

    fn trust_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("trust"));
        let (l_tsa, l_cert, l_pass) = (
            self.state.tr("tsa"),
            self.state.tr("sign_cert"),
            self.state.tr("sign_pass"),
        );
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("trust-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label(l_tsa);
                    ui.horizontal(|ui| {
                        let mut on = self.state.timestamp;
                        let mut url = self.state.tsa_url.clone();
                        let c = ui.checkbox(&mut on, "");
                        let t = ui.add_enabled(
                            on,
                            egui::TextEdit::singleline(&mut url).desired_width(280.0),
                        );
                        if c.changed() || t.lost_focus() {
                            self.state.set_timestamp(on, &url);
                        } else {
                            self.state.tsa_url = url;
                        }
                    });
                    ui.end_row();
                    ui.label(l_cert);
                    ui.horizontal(|ui| {
                        let mut path = self.state.sign_cert.clone();
                        let r = ui.add(egui::TextEdit::singleline(&mut path).desired_width(280.0));
                        if r.lost_focus() || r.changed() {
                            self.state.set_sign_cert(&path);
                        }
                        #[cfg(windows)]
                        if ui.button(self.state.tr("browse")).clicked()
                            && let Some(f) = rfd::FileDialog::new()
                                .add_filter("PKCS#12", &["p12", "pfx"])
                                .pick_file()
                        {
                            self.state.set_sign_cert(&f.display().to_string());
                        }
                    });
                    ui.end_row();
                    ui.label(l_pass);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.sign_password)
                            .password(true)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                });
        });
    }

    fn preview_section(&mut self, ui: &mut egui::Ui, enabled: bool) {
        Self::section_title(ui, self.state.tr("preview_section"));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    enabled && self.state.can_start(),
                    egui::Button::new(self.state.tr("preview")),
                )
                .clicked()
            {
                self.state.build_preview();
            }
            ui.weak(self.state.tr("preview_help"));
        });
        if let Some(e) = &self.state.preview_error {
            ui.colored_label(egui::Color32::from_rgb(0xFF, 0x6B, 0x6B), e);
        }
        let secs = self.state.preview_sections();
        if secs.is_empty() {
            return;
        }
        if let Some(r) = &self.state.preview {
            let t = &r.totals;
            ui.label(
                egui::RichText::new(format!(
                    "{} {} · {} {} · {} {} · {} {}",
                    t.runs_verified,
                    self.state.tr("runs_verified"),
                    t.runs_excluded,
                    self.state.tr("runs_excluded"),
                    t.findings_quoted,
                    self.state.tr("findings_quoted"),
                    t.items_not_examined,
                    self.state.tr("not_examined"),
                ))
                .color(if t.runs_excluded > 0 { AMBER } else { CYAN }),
            );
        }
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(250.0);
                for (i, s) in secs.iter().enumerate() {
                    if ui
                        .selectable_label(self.state.selected_section == i, &s.title)
                        .clicked()
                    {
                        self.state.selected_section = i;
                    }
                }
            });
            ui.separator();
            let i = self.state.selected_section.min(secs.len() - 1);
            egui::ScrollArea::vertical()
                .id_salt("preview-text")
                .max_height(260.0)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&secs[i].title).strong());
                        for l in &secs[i].lines {
                            ui.add(egui::Label::new(l).wrap());
                        }
                    });
                });
        });
    }

    fn run_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if self.state.is_running() {
                if ui.button(self.state.tr("cancel")).clicked() {
                    self.cancel.store(true, Ordering::SeqCst);
                    self.notice = Some(self.state.tr("cancelling"));
                }
            } else {
                let start = egui::Button::new(
                    egui::RichText::new(self.state.tr("start"))
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(BLUE);
                if ui.add_enabled(self.state.can_start(), start).clicked() {
                    let ctx = ui.ctx().clone();
                    self.start(&ctx);
                }
            }
            if let Some(n) = &self.notice {
                ui.colored_label(AMBER, n);
            }
        });
        ui.add_space(6.0);
        ui.add(
            egui::ProgressBar::new(self.state.fraction())
                .text(self.state.progress_text())
                .fill(BLUE)
                .animate(self.state.is_running()),
        );
        let live = self.started.map(|s| s.elapsed().as_millis() as u64);
        ui.label(self.state.timing_text(live));
        if !self.state.current_item.is_empty() {
            ui.weak(format!(
                "{}: {}",
                self.state.tr("now"),
                self.state.current_item
            ));
        }
        match &self.state.phase {
            Phase::Failed(e) => {
                ui.colored_label(
                    egui::Color32::from_rgb(0xFF, 0x6B, 0x6B),
                    format!("{}: {}", self.state.tr("failed"), e),
                );
            }
            Phase::Finished | Phase::Cancelled => {
                if let Some(o) = self.state.outcome.clone() {
                    ui.separator();
                    let title = if self.state.phase == Phase::Cancelled {
                        self.state.tr("cancelled")
                    } else {
                        self.state.tr("done")
                    };
                    ui.label(egui::RichText::new(title).color(CYAN).strong());
                    ui.label(format!(
                        "{} {} \u{00B7} {}: {} / {} / {} / {}",
                        o.items,
                        self.state.tr("items"),
                        self.state.tr("signals"),
                        o.high,
                        o.medium,
                        o.low,
                        o.info
                    ));
                    ui.horizontal(|ui| {
                        if ui.button(self.state.tr("open_report")).clicked() {
                            open_path(&o.report);
                        }
                        if ui.button(self.state.tr("open_logs")).clicked() {
                            open_path(&o.log_dir);
                        }
                    });
                }
            }
            _ => {}
        }
        if !self.state.warnings.is_empty() {
            egui::CollapsingHeader::new(format!(
                "{} ({})",
                self.state.tr("warnings"),
                self.state.warnings.len()
            ))
            .default_open(true)
            .show(ui, |ui| {
                for w in &self.state.warnings {
                    ui.label(w);
                }
            });
        }
    }
}

impl Default for ReportApp {
    fn default() -> Self {
        Self::new()
    }
}

fn open_path(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

impl eframe::App for ReportApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.themed {
            apply_theme(ui.ctx());
            self.themed = true;
        }
        self.drain();
        if self.state.is_running() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.header(ui);
                let enabled = !self.state.is_running();
                self.case_section(ui, enabled);
                self.report_section(ui, enabled);
                self.trust_section(ui, enabled);
                self.preview_section(ui, enabled);
                self.run_section(ui);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::parse_launch_args;

    fn launch(args: &[&str]) -> ReportApp {
        ReportApp::from_launch(parse_launch_args(args.iter().map(std::ffi::OsString::from)))
    }

    fn actions(app: &ReportApp) -> Vec<&str> {
        app.state
            .ui_actions
            .iter()
            .map(|(a, _)| a.as_str())
            .collect()
    }

    #[test]
    fn launcher_case_dir_is_preset_and_audited() {
        let app = launch(&["--case-dir", "X"]);
        assert_eq!(app.state.case_dir, "X");
        assert_eq!(app.case_entry, "X");
        assert_eq!(
            actions(&app),
            ["ui_case_dir_from_launcher", "ui_case_dir_changed"]
        );
        let d = &app.state.ui_actions[0].1;
        assert_eq!(d["path"], "X");
        assert!(d["at_utc"].as_str().is_some_and(|t| t.ends_with('Z')));
        assert!(app.notice.is_none());
    }

    #[test]
    fn a_case_folder_passed_to_the_exe_is_opened() {
        let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample/case");
        let app = launch(&[case.to_str().unwrap()]);
        assert!(app.state.case_summary.is_some());
        assert!(!app.state.preview_sections().is_empty());
        // A file that is not a case folder is ignored.
        let app = launch(&["a.pdf"]);
        assert!(app.state.case_dir.is_empty() && actions(&app).is_empty());
    }

    #[test]
    fn case_dir_without_a_value_is_noted_and_ignored() {
        let app = launch(&["--case-dir"]);
        assert!(app.state.case_dir.is_empty());
        assert_eq!(actions(&app), ["ui_launch_argument_ignored"]);
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|n| n.contains("--case-dir"))
        );
    }
}
