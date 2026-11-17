//! Everything the window shows and every rule it enforces, without any drawing code.
//! Kept separate from `app.rs` so it can be unit-tested headless (CI has no display).
//!
//! ```text
//! settings (each change = ui_* entry with at_utc) ──► to_config ──► RunConfig + AssembleOptions ──► run_assemble
//! "Preview" ──► preview_assembly (read-only: verifies the case and every run, writes nothing) ──► sections
//!            (rendered from the same view model as the HTML/PDF report) + ui_preview_built in the next audit log
//! ```

use rv_report_core::model::{AssembleOptions, ExpertReport};
use rv_report_core::{
    CASE_ROLE, Event, HashAlgo, RunConfig, TOOL_NAME, TOOL_VERSION, assemble, case, hash, i18n,
    report::expert::expert_view, time::human_duration,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Running,
    Finished,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub items: u64,
    pub high: u64,
    pub medium: u64,
    pub low: u64,
    pub info: u64,
    /// expert-report.<lang>.html
    pub report: PathBuf,
    pub log_dir: PathBuf,
}

/// What the case folder holds, read (and verified) when it is chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseSummary {
    pub case_id: String,
    pub items: usize,
    pub jurisdiction: String,
}

/// One section of the preview: its title and its text lines (from the report's own view model).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewSection {
    pub key: String,
    pub title: String,
    pub lines: Vec<String>,
}

pub const LANG_CHOICES: [(&str, &str); 4] = [
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("all", "EN + ES + FR"),
];

#[derive(Clone, Debug)]
pub struct GuiState {
    pub case_dir: String,
    pub case_summary: Option<CaseSummary>,
    pub case_error: Option<String>,
    /// "" = the case's own jurisdiction, else co / fr.
    pub jurisdiction: String,
    pub lang: String,
    pub actor: String,
    pub examiner_id: String,
    pub mandate: String,
    /// Comma-separated tool names.
    pub include_tools: String,
    pub exclude_tools: String,
    pub final_report: bool,
    pub selected: Vec<HashAlgo>,
    pub phase: Phase,
    pub progress: Progress,
    pub current_item: String,
    pub warnings: Vec<String>,
    pub outcome: Option<Outcome>,
    /// User actions recorded before the run; handed to the core as audit preamble entries.
    pub ui_actions: Vec<(String, Value)>,
    /// Interface language (en/es/fr), independent from the report language.
    pub ui_lang: String,
    pub timestamp: bool,
    pub tsa_url: String,
    pub sign_cert: String,
    /// Kept in memory only; never logged.
    pub sign_password: String,
    pub preview: Option<ExpertReport>,
    pub preview_error: Option<String>,
    pub selected_section: usize,
}

impl Default for GuiState {
    fn default() -> Self {
        GuiState {
            case_dir: String::new(),
            case_summary: None,
            case_error: None,
            jurisdiction: String::new(),
            lang: "en".into(),
            actor: rv_report_core::env::os_user(),
            examiner_id: String::new(),
            mandate: String::new(),
            include_tools: String::new(),
            exclude_tools: String::new(),
            final_report: false,
            selected: vec![HashAlgo::Sha256],
            phase: Phase::Idle,
            progress: Progress::default(),
            current_item: String::new(),
            warnings: vec![],
            outcome: None,
            ui_actions: vec![],
            ui_lang: "en".into(),
            timestamp: true,
            tsa_url: rv_report_core::trust::DEFAULT_TSA.into(),
            sign_cert: String::new(),
            sign_password: String::new(),
            preview: None,
            preview_error: None,
            selected_section: 0,
        }
    }
}

fn tools_of(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn valid_tool(t: &str) -> bool {
    t.starts_with("rv-")
        && t.len() > 3
        && t[3..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl GuiState {
    /// Records a user action with the time it happened (written to the audit log when the run starts).
    fn note(&mut self, action: &str, mut details: Value) {
        if let Some(o) = details.as_object_mut() {
            o.insert("at_utc".into(), json!(rv_report_core::time::utc_now()));
        }
        self.ui_actions.push((action.to_string(), details));
    }

    pub fn is_running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// GUI text in the interface language.
    pub fn tr(&self, key: &str) -> String {
        i18n::catalog(&self.ui_lang).ui(key)
    }

    // ---------------------------------------------------------------- settings

    /// The case folder; it is opened (and verified) at once to show its id. A new folder drops the preview.
    pub fn set_case_dir(&mut self, path: &str) {
        let path = path.trim().trim_matches('"').to_string();
        if self.case_dir == path {
            return;
        }
        self.case_dir = path.clone();
        self.note(
            "ui_case_dir_changed",
            json!({"path": if path.is_empty() { Value::Null } else { json!(path) }}),
        );
        self.preview = None;
        self.preview_error = None;
        self.refresh_case();
    }

    /// Re-reads the case folder's id and registry size.
    pub fn refresh_case(&mut self) {
        self.case_summary = None;
        self.case_error = None;
        if self.case_dir.is_empty() {
            return;
        }
        match case::Case::open(Path::new(&self.case_dir)) {
            Ok(c) => {
                self.case_summary = Some(CaseSummary {
                    case_id: c.case_id,
                    items: c.items.len(),
                    jurisdiction: c.jurisdiction,
                })
            }
            Err(e) => self.case_error = Some(e.message),
        }
    }

    /// The case folder given by RV Hub (`--case-dir`), recorded as `ui_case_dir_from_launcher`.
    pub fn set_case_dir_from_launcher(&mut self, path: &Path) {
        let shown = path.display().to_string();
        self.note("ui_case_dir_from_launcher", json!({"path": shown}));
        self.set_case_dir(&shown);
    }

    /// A launch argument that was ignored, recorded in the audit log.
    pub fn note_launch_problem(&mut self, problem: &str) {
        self.note("ui_launch_argument_ignored", json!({"problem": problem}));
    }

    /// "" (the case's own), "co" or "fr".
    pub fn set_jurisdiction(&mut self, j: &str) {
        let j = if j == "co" || j == "fr" { j } else { "" };
        if self.jurisdiction != j {
            self.jurisdiction = j.to_string();
            self.note(
                "ui_jurisdiction_changed",
                json!({"jurisdiction": if j.is_empty() { Value::Null } else { json!(j) }}),
            );
            self.preview = None;
        }
    }

    pub fn set_lang(&mut self, l: &str) {
        if self.lang != l {
            self.lang = l.to_string();
            self.note("ui_language_changed", json!({"lang": l}));
        }
    }

    pub fn set_ui_lang(&mut self, l: &str) {
        if self.ui_lang != l && ["en", "es", "fr"].contains(&l) {
            self.ui_lang = l.to_string();
            self.note("ui_interface_language_changed", json!({"ui_lang": l}));
        }
    }

    pub fn set_examiner(&mut self, name: &str, id: &str) {
        let (name, id) = (name.trim().to_string(), id.trim().to_string());
        if self.actor != name || self.examiner_id != id {
            self.actor = name.clone();
            self.examiner_id = id.clone();
            self.note(
                "ui_examiner_changed",
                json!({"actor": name, "examiner_id": if id.is_empty() { Value::Null } else { json!(id) }}),
            );
        }
    }

    /// The mandate file: checked at once (UTF-8, size); its name and SHA-256 are audited, never its text.
    pub fn set_mandate(&mut self, path: &str) -> Result<(), String> {
        let path = path.trim().trim_matches('"').to_string();
        if self.mandate == path {
            return Ok(());
        }
        let digest = if path.is_empty() {
            Value::Null
        } else {
            json!(assemble::read_mandate(Path::new(&path))?.sha256)
        };
        self.mandate = path.clone();
        self.note(
            "ui_mandate_changed",
            json!({"path": if path.is_empty() { Value::Null } else { json!(path) }, "sha256": digest}),
        );
        self.preview = None;
        Ok(())
    }

    /// Tool filters (comma-separated). Names must look like Rakvor tools (`rv-…`).
    pub fn set_tool_filters(&mut self, include: &str, exclude: &str) -> Result<(), String> {
        let (inc, exc) = (tools_of(include), tools_of(exclude));
        if let Some(bad) = inc.iter().chain(exc.iter()).find(|t| !valid_tool(t)) {
            return Err(format!("{}: {}", self.tr("tool_invalid"), bad));
        }
        if inc.join(",") != self.include_tools || exc.join(",") != self.exclude_tools {
            self.include_tools = inc.join(",");
            self.exclude_tools = exc.join(",");
            self.note(
                "ui_tool_filter_changed",
                json!({"include_tools": inc, "exclude_tools": exc}),
            );
            self.preview = None;
        }
        Ok(())
    }

    pub fn set_final(&mut self, on: bool) {
        if self.final_report != on {
            self.final_report = on;
            self.note("ui_final_changed", json!({"final": on}));
        }
    }

    pub fn can_toggle(&self, a: HashAlgo) -> bool {
        if self.selected.contains(&a) {
            self.selected.len() > 1
        } else {
            self.selected.len() < 2
        }
    }

    /// Ticks or unticks an algorithm: 1 or 2 stay selected.
    pub fn toggle_algo(&mut self, a: HashAlgo) -> Result<(), String> {
        if !self.can_toggle(a) {
            return Err(if self.selected.contains(&a) {
                self.tr("hash_tip_one")
            } else {
                self.tr("hash_tip_two")
            });
        }
        if let Some(i) = self.selected.iter().position(|x| *x == a) {
            self.selected.remove(i);
        } else {
            self.selected.push(a);
        }
        self.note(
            "ui_hash_selection_changed",
            json!({"algorithms": self.selected}),
        );
        Ok(())
    }

    pub fn set_timestamp(&mut self, on: bool, url: &str) {
        let url = url.trim().to_string();
        if self.timestamp != on || self.tsa_url != url {
            self.timestamp = on;
            self.tsa_url = url.clone();
            self.note(
                "ui_timestamp_changed",
                json!({"enabled": on, "tsa_url": url}),
            );
        }
    }

    pub fn set_sign_cert(&mut self, path: &str) {
        let path = path.trim().trim_matches('"').to_string();
        if self.sign_cert != path {
            self.sign_cert = path.clone();
            self.note(
                "ui_signing_certificate_changed",
                json!({"path": if path.is_empty() { Value::Null } else { json!(path) }}),
            );
        }
    }

    /// What the output field shows: the run is written inside the case folder.
    pub fn out_dir_display(&self) -> String {
        PathBuf::from(self.case_dir.trim())
            .join("runs")
            .join(TOOL_NAME)
            .join("<RUN-ID>")
            .display()
            .to_string()
    }

    pub fn can_start(&self) -> bool {
        !self.is_running() && self.case_summary.is_some()
    }

    fn options(&self) -> AssembleOptions {
        AssembleOptions {
            case_dir: PathBuf::from(self.case_dir.trim()),
            jurisdiction: Some(self.jurisdiction.clone()).filter(|j| !j.is_empty()),
            examiner_id: Some(self.examiner_id.clone()).filter(|s| !s.is_empty()),
            mandate: Some(PathBuf::from(&self.mandate)).filter(|_| !self.mandate.is_empty()),
            include_tools: tools_of(&self.include_tools),
            exclude_tools: tools_of(&self.exclude_tools),
            final_report: self.final_report,
        }
    }

    /// Builds the run configuration and options; the recorded UI actions become audit preamble entries.
    pub fn to_config(&self) -> Result<(RunConfig, AssembleOptions), String> {
        if self.case_dir.trim().is_empty() {
            return Err(self.tr("case_dir_needed"));
        }
        case::preflight(Path::new(self.case_dir.trim()), None, &[]).map_err(|e| e.message)?;
        hash::validate_selection(&self.selected)?;
        let langs = i18n::parse_langs(&self.lang)?;
        if !self.sign_cert.is_empty() && !Path::new(&self.sign_cert).is_file() {
            return Err(format!("{}: {}", self.tr("cert_missing"), self.sign_cert));
        }
        let opts = self.options();
        if let Some(m) = &opts.mandate {
            assemble::read_mandate(m)?;
        }
        let mut preamble = self.ui_actions.clone();
        preamble.push(("ui_start_clicked".into(), json!({"algorithms": self.selected, "lang": self.lang,
            "case_dir": self.case_dir.trim(), "jurisdiction": opts.jurisdiction, "examiner_id": opts.examiner_id,
            "mandate": self.mandate, "include_tools": opts.include_tools, "exclude_tools": opts.exclude_tools,
            "final": self.final_report, "timestamp": self.timestamp, "pdf_signing": !self.sign_cert.is_empty(),
            "at_utc": rv_report_core::time::utc_now()})));
        let settings = vec![
            format!("{} (gui)", TOOL_NAME),
            format!("case_dir={}", self.case_dir.trim()),
            format!(
                "hash={}",
                self.selected
                    .iter()
                    .map(|a| a.name())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            format!("lang={}", self.lang),
            format!(
                "jurisdiction={}",
                if self.jurisdiction.is_empty() { "case" } else { self.jurisdiction.as_str() }
            ),
            format!("mandate={}", if self.mandate.is_empty() { "none" } else { self.mandate.as_str() }),
            format!("include_tools={}", self.include_tools),
            format!("exclude_tools={}", self.exclude_tools),
            format!("final={}", self.final_report),
            format!("tsa={}", if self.timestamp { self.tsa_url.as_str() } else { "off" }),
            format!(
                "sign_cert={}",
                if self.sign_cert.is_empty() { "none" } else { self.sign_cert.as_str() }
            ),
        ];
        let cfg = RunConfig {
            inputs: vec![],
            input_list: None,
            recursive: false,
            algorithms: self.selected.clone(),
            out_dir: PathBuf::from("rakvor-out"),
            langs,
            actor: if self.actor.trim().is_empty() {
                rv_report_core::env::os_user()
            } else {
                self.actor.trim().to_string()
            },
            case_id: None,
            frontend: "gui".into(),
            preamble,
            command_line: settings,
            tool_name: TOOL_NAME.into(),
            tool_version: TOOL_VERSION.into(),
            jurisdiction: if self.jurisdiction.is_empty() { "co".into() } else { self.jurisdiction.clone() },
            tsa_url: self.timestamp.then(|| self.tsa_url.clone()),
            sign_cert: Some(PathBuf::from(&self.sign_cert)).filter(|_| !self.sign_cert.is_empty()),
            sign_password: Some(self.sign_password.clone()).filter(|p| !p.is_empty()),
            case_dir: Some(PathBuf::from(self.case_dir.trim())),
            evidence_ids: vec![],
            case_role: CASE_ROLE,
        };
        Ok((cfg, opts))
    }

    // ---------------------------------------------------------------- preview

    /// Assembles the model read-only (nothing written) and keeps it for the section preview; the preview's
    /// counts are recorded as `ui_preview_built` in the next run's audit log.
    pub fn build_preview(&mut self) {
        self.preview = None;
        self.preview_error = None;
        let (cfg, opts) = match self.to_config() {
            Ok(x) => x,
            Err(e) => {
                self.preview_error = Some(e);
                return;
            }
        };
        match rv_report_core::run::preview_assembly(&cfg, &opts) {
            Ok(r) => {
                let t = &r.totals;
                self.note(
                    "ui_preview_built",
                    json!({"structure": r.jurisdiction.code, "runs_verified": t.runs_verified,
                           "runs_excluded": t.runs_excluded, "runs_skipped": t.runs_skipped,
                           "findings_quoted": t.findings_quoted, "items_not_examined": t.items_not_examined}),
                );
                self.preview = Some(r);
                self.selected_section = 0;
            }
            Err(e) => self.preview_error = Some(e),
        }
    }

    /// Sections of the preview in the first report language, from the same view model as the HTML/PDF.
    pub fn preview_sections(&self) -> Vec<PreviewSection> {
        let Some(r) = &self.preview else {
            return vec![];
        };
        let lang = i18n::parse_langs(&self.lang)
            .ok()
            .and_then(|l| l.into_iter().next())
            .unwrap_or_else(|| "en".into());
        let view = expert_view(r, &lang);
        let mut out = Vec::new();
        for s in view["sections"].as_array().into_iter().flatten() {
            let mut lines: Vec<String> = s["basis"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| b.as_str().map(|b| format!("§ {b}")))
                .collect();
            for b in s["blocks"].as_array().into_iter().flatten() {
                let text = |k: &str| b[k].as_str().unwrap_or_default().to_string();
                match b["kind"].as_str().unwrap_or_default() {
                    "para" | "note" | "source" => lines.push(text("text")),
                    "sub" => lines.push(format!("— {} —", text("text"))),
                    "quote" => lines.push(format!("  \u{201C}{}\u{201D}", text("text"))),
                    "bullets" => lines.extend(
                        b["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|i| format!("  • {}", i.as_str().unwrap_or_default())),
                    ),
                    "kv" | "box" => {
                        if b["kind"] == "box" {
                            lines.push(format!("[{}] {} {}", text("badge"), text("title"), text("code")));
                        }
                        for r in b["rows"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "    {}: {}",
                                r[0].as_str().unwrap_or_default(),
                                r[1].as_str().unwrap_or_default()
                            ));
                        }
                    }
                    "sign" => lines.push("______________________".into()),
                    _ => {}
                }
            }
            out.push(PreviewSection {
                key: s["key"].as_str().unwrap_or_default().to_string(),
                title: s["title"].as_str().unwrap_or_default().to_string(),
                lines,
            });
        }
        out
    }

    // ---------------------------------------------------------------- run

    pub fn start(&mut self) {
        self.phase = Phase::Running;
        self.progress = Progress::default();
        self.current_item.clear();
        self.warnings.clear();
        self.outcome = None;
        // Recorded actions now belong to this run's audit log.
        self.ui_actions.clear();
    }

    pub fn apply(&mut self, e: &Event) {
        match e {
            Event::RunStarted { total_items, .. } => self.progress.total = *total_items,
            Event::ItemStarted { path, .. } => self.current_item = path.clone(),
            Event::ItemFinished { path, status, .. } if status == "excluded" => {
                self.warnings.push(format!("{}: {}", self.tr("run_excluded"), path))
            }
            Event::Progress {
                done,
                total,
                elapsed_ms,
                ..
            } => {
                self.progress = Progress {
                    done: *done,
                    total: *total,
                    elapsed_ms: *elapsed_ms,
                };
            }
            Event::Error { path, message } => self
                .warnings
                .push(format!("{}: {}", path.as_deref().unwrap_or("-"), message)),
            Event::RunFinished {
                items,
                high,
                medium,
                low,
                info,
                report_dir,
                log_dir,
                cancelled,
                elapsed_ms,
                ..
            } => {
                self.progress.elapsed_ms = *elapsed_ms;
                let lang = if self.lang == "all" { "en".to_string() } else { self.lang.clone() };
                self.outcome = Some(Outcome {
                    items: *items,
                    high: *high,
                    medium: *medium,
                    low: *low,
                    info: *info,
                    report: PathBuf::from(report_dir).join(format!("expert-report.{}.html", lang)),
                    log_dir: PathBuf::from(log_dir),
                });
                self.phase = if *cancelled { Phase::Cancelled } else { Phase::Finished };
                self.current_item.clear();
            }
            _ => {}
        }
    }

    pub fn fail(&mut self, msg: String) {
        self.phase = Phase::Failed(msg);
    }

    /// 0.0..=1.0, by runs examined.
    pub fn fraction(&self) -> f32 {
        let p = &self.progress;
        if p.total > 0 {
            (p.done as f64 / p.total as f64).clamp(0.0, 1.0) as f32
        } else if matches!(self.phase, Phase::Finished) {
            1.0
        } else {
            0.0
        }
    }

    /// `live_elapsed_ms` is measured by the window clock so the timer keeps moving between events.
    pub fn timing_text(&self, live_elapsed_ms: Option<u64>) -> String {
        let elapsed = live_elapsed_ms
            .unwrap_or(self.progress.elapsed_ms)
            .max(self.progress.elapsed_ms);
        format!(
            "{}: {}   \u{00B7}   {}: \u{2014}",
            self.tr("elapsed"),
            human_duration(elapsed),
            self.tr("remaining"),
        )
    }

    pub fn progress_text(&self) -> String {
        format!(
            "{} / {} {}  \u{00B7}  {:.0}%",
            self.progress.done,
            self.progress.total,
            self.tr("runs"),
            self.fraction() * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy_dir(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }

    fn sample_case(dest: &Path) -> PathBuf {
        let case = dest.join("CASE");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample/case"),
            &case,
        );
        case
    }

    fn actions(s: &GuiState) -> Vec<&str> {
        s.ui_actions.iter().map(|(a, _)| a.as_str()).collect()
    }

    #[test]
    fn at_most_two_and_at_least_one_algorithm() {
        let mut s = GuiState::default();
        assert!(s.toggle_algo(HashAlgo::Sha256).is_err());
        s.toggle_algo(HashAlgo::Md5).unwrap();
        assert!(!s.can_toggle(HashAlgo::Blake3));
        assert!(s.toggle_algo(HashAlgo::Blake3).is_err());
        assert_eq!(s.selected.len(), 2);
    }

    #[test]
    fn start_needs_a_verified_case_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = GuiState::default();
        assert!(!s.can_start());
        assert!(s.to_config().is_err());
        s.set_case_dir(tmp.path().to_str().unwrap());
        assert!(s.case_error.is_some() && !s.can_start());
        let case = sample_case(tmp.path());
        s.set_case_dir(case.to_str().unwrap());
        let sum = s.case_summary.clone().unwrap();
        assert_eq!((sum.case_id.as_str(), sum.items, sum.jurisdiction.as_str()), ("RV-REPORT-SAMPLE", 4, "co"));
        assert!(s.can_start());
        assert!(s.out_dir_display().contains("rv-report"));
    }

    #[test]
    fn settings_changes_are_audited_with_their_time() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = GuiState::default();
        s.set_jurisdiction("fr");
        s.set_lang("es");
        s.set_ui_lang("fr");
        s.set_examiner("Ana", "TP-1");
        s.set_final(true);
        s.set_timestamp(false, "http://x");
        s.set_sign_cert("C:/certs/a.p12");
        s.set_tool_filters("rv-pdfauth, rv-pdfauth", "").unwrap();
        assert!(s.set_tool_filters("pdfauth", "").is_err());
        let m = tmp.path().join("m.txt");
        std::fs::write(&m, "Objeto").unwrap();
        s.set_mandate(m.to_str().unwrap()).unwrap();
        let bad = tmp.path().join("bad.txt");
        std::fs::write(&bad, [0xff, 0xfe]).unwrap();
        assert!(s.set_mandate(bad.to_str().unwrap()).is_err());
        s.toggle_algo(HashAlgo::Md5).unwrap();
        assert_eq!(
            actions(&s),
            vec![
                "ui_jurisdiction_changed",
                "ui_language_changed",
                "ui_interface_language_changed",
                "ui_examiner_changed",
                "ui_final_changed",
                "ui_timestamp_changed",
                "ui_signing_certificate_changed",
                "ui_tool_filter_changed",
                "ui_mandate_changed",
                "ui_hash_selection_changed",
            ]
        );
        assert!(s.ui_actions.iter().all(|(_, d)| d["at_utc"].is_string()));
        assert_eq!(s.include_tools, "rv-pdfauth");
        // The mandate's digest is audited, never its text.
        let (_, d) = &s.ui_actions[8];
        assert_eq!(d["sha256"].as_str().unwrap().len(), 64);
        assert!(!d.to_string().contains("Objeto"));
        // Interface language switches the labels.
        assert_ne!(s.tr("start"), GuiState::default().tr("start"));
    }

    #[test]
    fn preview_shows_the_sections_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let case = sample_case(tmp.path());
        let log_before = std::fs::read(case.join("case-log.jsonl")).unwrap();
        let mut s = GuiState::default();
        s.set_case_dir(case.to_str().unwrap());
        s.build_preview();
        assert!(s.preview_error.is_none(), "{:?}", s.preview_error);
        let secs = s.preview_sections();
        assert_eq!(secs.len(), 10);
        assert_eq!(secs[0].key, "identification");
        assert!(secs.iter().any(|x| x.key == "results" && x.lines.iter().any(|l| l.contains("rv-pdfauth"))));
        assert!(secs.iter().any(|x| x.key == "not_examined" && x.lines.iter().any(|l| l.contains("E0003"))));
        assert!(actions(&s).contains(&"ui_preview_built"));
        assert_eq!(std::fs::read(case.join("case-log.jsonl")).unwrap(), log_before);
        assert!(!case.join("runs").join(TOOL_NAME).exists());
        s.set_jurisdiction("fr");
        assert!(s.preview.is_none());
    }

    #[test]
    fn events_drive_progress_and_outcome() {
        let mut s = GuiState::default();
        s.start();
        s.apply(&Event::Progress {
            done: 2,
            total: 4,
            bytes_done: 0,
            bytes_total: 0,
            elapsed_ms: 10,
            eta_ms: None,
        });
        assert_eq!(s.fraction(), 0.5);
        s.apply(&Event::ItemFinished {
            index: 3,
            path: "runs/rv-imgauth/X".into(),
            status: "excluded".into(),
            findings: vec![],
        });
        assert_eq!(s.warnings.len(), 1);
        s.apply(&Event::RunFinished {
            items: 4,
            bytes: 0,
            errors: 0,
            elapsed_ms: 20,
            high: 0,
            medium: 1,
            low: 0,
            info: 2,
            report_dir: "R".into(),
            log_dir: "L".into(),
            cancelled: false,
        });
        assert_eq!(s.phase, Phase::Finished);
        assert!(s.outcome.unwrap().report.ends_with("expert-report.en.html"));
    }

    /// The real core from GUI settings: the run's audit log holds the ui_* actions, the preview and the start.
    #[test]
    fn a_real_run_from_gui_settings_is_audited() {
        let tmp = tempfile::tempdir().unwrap();
        let case = sample_case(tmp.path());
        let mut s = GuiState::default();
        s.set_case_dir(case.to_str().unwrap());
        s.set_timestamp(false, rv_report_core::trust::DEFAULT_TSA);
        s.set_lang("es");
        s.build_preview();
        let (cfg, opts) = s.to_config().unwrap();
        s.start();
        let summary = rv_report_core::run::run_assemble(&cfg, &opts, &mut |e| s.apply(e), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(s.phase, Phase::Finished);
        let audit = std::fs::read_to_string(summary.log_dir.join("audit.jsonl")).unwrap();
        let acts: Vec<String> = audit
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["action"].as_str().unwrap().to_string())
            .collect();
        for a in ["ui_case_dir_changed", "ui_timestamp_changed", "ui_language_changed", "ui_preview_built", "ui_start_clicked"] {
            assert!(acts.iter().any(|x| x == a), "{a} missing");
        }
        assert_eq!(&acts[..2], &["run_started".to_string(), "case_linked".to_string()]);
        assert!(audit.contains("\"frontend\":\"gui\""));
        assert!(s.outcome.unwrap().report.is_file());
    }
}
