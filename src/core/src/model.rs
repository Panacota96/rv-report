//! Data model of the expert report — FROZEN for v0.1 (`rakvor.expert-report/1`, `schemas/expert-report.schema.json`).
//!
//! Field names here are the contract between the assembler (`assemble.rs`), the renderers (`report/expert.rs`:
//! HTML and PDF in every language, from this model only), the JSON written to `reports/expert-report.json`, the
//! GUI preview and the tests. Later work may add fields (`#[serde(default)]`), never rename or remove one.
//!
//! ```text
//! case folder ──► Identification (case.json, case log, examiner, mandate)
//!             ──► EvidenceItem × n (registry + custody rows from the case log and the verified runs)
//!   each run ──► RunAnnex (verified | excluded | skipped, audit head, timestamp, outputs verified)
//!   verified ──► ToolMethod (per tool) + ItemResults (findings quoted verbatim from technical.json)
//!            ──► ToolLimitations (statistics.json codes + printed wording) + TimelineSummary (latest RV Timeline)
//! registry items no analysis run examined ──► NotExamined
//! ```
//!
//! The JSON is language-neutral: quoted wording keeps every language the tool wrote (`text.<lang>`), RV Report's
//! own wording (section titles, signals, its limitations) is rendered from `i18n/*.json` by code. Integers and
//! strings only (no floats), keys sorted when written (serde_json maps).

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const SCHEMA_EXPERT_REPORT: &str = "rakvor.expert-report/1";

/// Severity of a quoted finding or of one of RV Report's own signals (the suite's four levels).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Info,
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn key(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Severity> {
        match s {
            "info" => Some(Severity::Info),
            "low" => Some(Severity::Low),
            "medium" => Some(Severity::Medium),
            "high" => Some(Severity::High),
            _ => None,
        }
    }
}

/// The whole report. One per `assemble` run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExpertReport {
    /// [`SCHEMA_EXPERT_REPORT`]
    pub schema: String,
    pub tool: ToolRef,
    pub run: ReportRun,
    pub jurisdiction: JurisdictionInfo,
    pub identification: Identification,
    /// Every registered evidence item, in registry order.
    pub evidence: Vec<EvidenceItem>,
    pub method: Method,
    /// One entry per registered item (and per item a verified run named outside the registry).
    pub results: Vec<ItemResults>,
    /// The latest verified RV Timeline run's summary; None when the case has none.
    pub timeline: Option<TimelineSummary>,
    pub limitations: Limitations,
    pub annexes: Annexes,
    /// Registered items no verified run of an analysis tool examined.
    pub not_examined: Vec<NotExamined>,
    /// RV Report's own signals about the record (never about the evidence's authenticity).
    pub signals: Vec<ReportSignal>,
    pub totals: Totals,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolRef {
    pub name: String,
    pub version: String,
}

/// This assembly: who, when, how, and whether the examiner marked it final.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReportRun {
    pub run_id: String,
    /// RFC 3339 UTC, the time the model was assembled.
    pub generated_utc: String,
    pub actor: String,
    pub langs: Vec<String>,
    /// `draft` until the examiner passes `--final`, then `final`.
    pub status: String,
    /// `--include-tool` (empty = every tool) and `--exclude-tool`.
    #[serde(default)]
    pub include_tools: Vec<String>,
    #[serde(default)]
    pub exclude_tools: Vec<String>,
    /// Subject and SHA-256 fingerprint of the certificate that signs the PDFs, if any.
    #[serde(default)]
    pub pdf_signed_by: Option<String>,
    #[serde(default)]
    pub pdf_signer_certificate_sha256: Option<String>,
}

/// Which legal frame the report follows. Every citation is printed "to be verified".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JurisdictionInfo {
    /// `co` (dictamen pericial) or `fr` (rapport d'expertise): the report's structure.
    pub code: String,
    /// The case's own jurisdiction (case.json); custody forms follow it.
    pub case_jurisdiction: String,
    /// The structure's sections, in order (i18n keys `expert.sections.<key>`).
    pub sections: Vec<String>,
    pub citations: Vec<Citation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Citation {
    /// i18n key `expert.citations.<key>` (the reference and what it asks for).
    pub key: String,
    /// The legal reference as printed (`Ley 906 de 2004, art. 415`).
    pub reference: String,
    /// Always `to_be_verified` in v0.1: no citation has been checked by counsel for this report.
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Identification {
    pub case_id: String,
    pub case_created_utc: String,
    pub case_json_sha256: String,
    pub examiner: Examiner,
    pub mandate: Option<Mandate>,
    pub case_verification: CaseCheck,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Examiner {
    pub name: String,
    /// `--examiner-id` (professional card, list registration…), as the examiner gave it.
    #[serde(default)]
    pub examiner_id: Option<String>,
}

/// The object of the expertise, quoted verbatim from `--mandate FILE`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Mandate {
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
    pub text: String,
}

/// What was verified about the case folder before anything was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CaseCheck {
    /// Case-log entries verified (before this run appended to it).
    pub case_log_entries: u64,
    /// `entry_hash` of the last case-log entry verified.
    pub case_log_head: String,
    pub registry_items: u64,
    pub registry_sha256: String,
}

/// One registered evidence item, as received.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EvidenceItem {
    pub evidence_id: String,
    pub path: String,
    pub file_name: String,
    pub size: u64,
    /// algorithm → hex, as in the registry.
    pub hashes: BTreeMap<String, String>,
    pub registered_utc: String,
    pub registered_by: RunPointer,
    /// Custody rows in case-log order: registration, then every verified run that examined the item.
    pub custody: Vec<CustodyEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunPointer {
    pub tool: String,
    pub version: String,
    pub run_id: String,
}

/// One custody row: what happened to the item, when, by whom, with which tool run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CustodyEvent {
    /// `registered` (intake), `examined` (a verified run compared it with the registry and analysed it).
    pub action: String,
    /// Case-log entry (`seq`) that records it: `evidence_registered`, or the run's `run_started`.
    pub case_log_seq: u64,
    pub ts_utc: String,
    pub actor: String,
    pub tool: String,
    pub tool_version: String,
    pub run_id: String,
    /// For `examined`: whether the run's copy matched the registry (`evidence_verified_against_case`), None when
    /// the run did not record the comparison.
    #[serde(default)]
    pub registry_match: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Method {
    /// One entry per tool whose verified runs are quoted, in tool-name order.
    pub tools: Vec<ToolMethod>,
}

/// How one tool took part: from its runs' audit logs and the case log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolMethod {
    pub tool: String,
    /// RV Report has a description of this tool (`i18n` → `tools.<tool>`); false = unknown tool, said so.
    pub known: bool,
    /// Role the case log recorded (`intake`, `analysis`, …).
    pub role: String,
    pub versions: Vec<String>,
    pub runs: Vec<String>,
    /// Hash algorithms the runs used (run_started), report languages, front-ends.
    pub algorithms: Vec<String>,
    pub langs: Vec<String>,
    pub frontends: Vec<String>,
    /// Distinct evidence ids the runs examined.
    pub items_examined: u64,
    pub findings_quoted: u64,
}

/// All quoted results about one evidence item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ItemResults {
    /// Empty when a run named an item outside the registry.
    pub evidence_id: String,
    pub path: String,
    pub entries: Vec<QuotedRun>,
}

/// One verified run's report about one item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct QuotedRun {
    pub tool: String,
    pub tool_version: String,
    pub run_id: String,
    /// The tool's own item status (`ok`, `attention`, `problem`, …).
    pub item_status: String,
    #[serde(default)]
    pub registry_match: Option<bool>,
    pub source: SourceRef,
    pub findings: Vec<QuotedFinding>,
}

/// A run output a quotation comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SourceRef {
    /// Relative to the case folder, forward slashes.
    pub file: String,
    /// SHA-256 as recorded in the run's audit log (checked before reading).
    pub sha256: String,
}

/// A finding exactly as the tool wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct QuotedFinding {
    pub code: String,
    pub severity: Severity,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// language → the tool's own title / shows / does_not_prove.
    pub text: BTreeMap<String, FindingText>,
    /// JSON pointer inside the source file (`/findings/3`).
    pub pointer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FindingText {
    pub title: String,
    pub shows: String,
    pub does_not_prove: String,
}

/// The latest verified RV Timeline run, summarised from its own statistics.json.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TimelineSummary {
    pub run_id: String,
    pub tool_version: String,
    pub started_utc: String,
    pub finished_utc: String,
    pub events: u64,
    pub runs_verified: u64,
    pub runs_excluded: u64,
    pub zone_unknown: u64,
    pub display_zone: String,
    /// severity → count of the timeline's signals.
    pub signals_by_severity: BTreeMap<String, u64>,
    /// code → count.
    pub signals_by_code: BTreeMap<String, u64>,
    pub source: SourceRef,
    /// `timeline.jsonl` of that run (file + recorded SHA-256), when it wrote one.
    #[serde(default)]
    pub timeline_file: Option<SourceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Limitations {
    pub tools: Vec<ToolLimitations>,
    /// RV Report's own limitation codes (`REPORT_*`, i18n `limitations`).
    pub own: Vec<String>,
}

/// One tool version's limitations as its runs printed them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolLimitations {
    pub tool: String,
    pub tool_version: String,
    pub runs: Vec<String>,
    pub items: Vec<QuotedLimitation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct QuotedLimitation {
    pub code: String,
    /// language → the sentence the tool printed (empty map when no printed wording could be paired).
    pub text: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Annexes {
    /// Every run found in the case, in (tool, run id) order.
    pub runs: Vec<RunAnnex>,
}

/// One tool run and what RV Report did with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunAnnex {
    pub tool: String,
    pub tool_version: String,
    pub run_id: String,
    /// `runs/<tool>/<run-id>`
    pub run_dir: String,
    /// `verified`, `excluded` or `skipped`.
    pub status: String,
    /// Excluded: `unreadable`, `chain_broken`, `not_finished`, `digest_mismatch`, `timestamp_invalid`;
    /// skipped: `self`, `verification`, `tool_filter`, `superseded`. Empty when verified.
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub detail: String,
    pub started_utc: String,
    pub finished_utc: String,
    pub audit_entries: u64,
    pub audit_head: String,
    /// `ok` (a token verified), `pending`, `off`, or `unknown` (not verified).
    pub timestamp_status: String,
    /// One line per verified token (gen time, TSA, covered entry).
    #[serde(default)]
    pub timestamps: Vec<String>,
    pub outputs_recorded: u64,
    pub outputs_verified: u64,
    pub findings_quoted: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NotExamined {
    pub evidence_id: String,
    pub path: String,
    /// `no_analysis_run` (only the intake saw it).
    pub reason: String,
}

/// One of RV Report's own signals (i18n `findings.<code>`: title, shows, does_not_prove).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReportSignal {
    pub code: String,
    pub severity: Severity,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub evidence_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Totals {
    pub evidence_items: u64,
    pub runs_found: u64,
    pub runs_verified: u64,
    pub runs_excluded: u64,
    pub runs_skipped: u64,
    pub findings_quoted: u64,
    /// severity → quoted findings.
    pub quoted_by_severity: BTreeMap<String, u64>,
    pub items_not_examined: u64,
}

/// Options of an assembly; the CLI flags and GUI settings map 1:1 onto these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AssembleOptions {
    /// `--case-dir` (required).
    pub case_dir: PathBuf,
    /// `--jurisdiction`: None = the case's own.
    #[serde(default)]
    pub jurisdiction: Option<String>,
    /// `--examiner-id`.
    #[serde(default)]
    pub examiner_id: Option<String>,
    /// `--mandate` / `--request`: UTF-8 text file with the object of the expertise.
    #[serde(default)]
    pub mandate: Option<PathBuf>,
    /// `--include-tool`: only these tools' runs (empty = all).
    #[serde(default)]
    pub include_tools: Vec<String>,
    /// `--exclude-tool`.
    #[serde(default)]
    pub exclude_tools: Vec<String>,
    /// `--final`: the examiner marks the report final (audited).
    #[serde(default)]
    pub final_report: bool,
}

/// Largest mandate file accepted (bytes).
pub const MAX_MANDATE_BYTES: u64 = 64 * 1024;
