//! Builds the [`ExpertReport`] model from a verified case folder. Read-only: it never writes, so the GUI can use it
//! for the section preview and the pipeline (`run::run_assemble`) for the report itself.
//!
//! ```text
//! Case (verified) ──► discover_runs ──► skip? (self, RV Hub, --include/--exclude-tool)
//!                                   └─► verify_run ──► excluded (reason)          ──► annex + RUN_EXCLUDED
//!                                                   └─► verified ──► latest RV Timeline kept, older: superseded
//!   verified ──► technical.json findings (verbatim) ──► results per item, custody "examined" rows
//!            ──► statistics.json + statistics.<lang>.html ──► limitations      ──► method per tool
//! registry ──► evidence + custody "registered" rows (case log) ──► not examined ──► own signals ──► totals
//! ```
//!
//! Every file read is a recorded output of a verified run, checked against its recorded digest when read
//! (`sources::RunData`), plus case.json, the registry and the case log. The paths read are returned for
//! `hashes.csv`.

#![forbid(unsafe_code)]

use crate::case::Case;
use crate::hash::{HashAlgo, hash_bytes};
use crate::model::*;
use crate::sources::{self, ReadError, RunData, RunRef, Verified, at, s};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Tools RV Report has a description of (i18n `tools.<name>.role` / `.checks`).
pub const KNOWN_TOOLS: [&str; 9] = [
    "rv-avauth",
    "rv-certcheck",
    "rv-hashinv",
    "rv-imgauth",
    "rv-mailauth",
    "rv-pdfauth",
    "rv-rsmf",
    "rv-timeline",
    "rv-triage",
];
/// RV Hub's runs verify the case; they hold no findings about the evidence (skipped, reason `verification`).
pub const VERIFICATION_TOOLS: [&str; 1] = ["rv-hub"];
/// Tools whose runs do not examine an item's content (for "what was not examined").
pub const NOT_EXAMINING_TOOLS: [&str; 3] = ["rv-timeline", "rv-report", "rv-hub"];
pub const TIMELINE_TOOL: &str = "rv-timeline";

/// RV Report's own limitations, printed in every report.
pub const OWN_LIMITATIONS: [&str; 7] = [
    "REPORT_DERIVED_CONTENT",
    "REPORT_QUOTES_ONLY_VERIFIED_RUNS",
    "REPORT_NO_CONCLUSION",
    "REPORT_LEGAL_STRUCTURE_UNVERIFIED",
    "REPORT_NO_TRANSLATION",
    "CLEAN_RESULT_IS_NOT_AUTHENTICITY",
    "TIMESTAMP_PROVES_EXISTENCE_ONLY",
];

/// Sections of each structure, in order (i18n `expert.sections_<code>.<key>`).
pub fn sections(code: &str) -> Vec<String> {
    let keys: &[&str] = if code == "fr" {
        &[
            "mission",
            "evidence",
            "method",
            "results",
            "timeline",
            "limitations",
            "not_examined",
            "annexes",
            "declaration",
        ]
    } else {
        &[
            "identification",
            "object",
            "evidence",
            "method",
            "results",
            "timeline",
            "limitations",
            "not_examined",
            "annexes",
            "declaration",
        ]
    };
    keys.iter().map(|k| k.to_string()).collect()
}

/// Legal references of each structure (all "to be verified").
pub fn citations(code: &str) -> Vec<Citation> {
    let list: &[(&str, &str)] = if code == "fr" {
        &[
            ("fr_cpp_156", "Code de procédure pénale, art. 156"),
            ("fr_cpp_157", "Code de procédure pénale, arts. 157 et 160"),
            ("fr_cpp_158", "Code de procédure pénale, art. 158"),
            ("fr_cpp_163", "Code de procédure pénale, art. 163"),
            ("fr_cpp_166", "Code de procédure pénale, art. 166"),
            ("fr_cpp_167", "Code de procédure pénale, art. 167"),
        ]
    } else {
        &[
            ("co_906_405", "Ley 906 de 2004, arts. 405–423"),
            ("co_906_413", "Ley 906 de 2004, art. 413"),
            ("co_906_414", "Ley 906 de 2004, art. 414"),
            ("co_906_415", "Ley 906 de 2004, art. 415"),
            ("co_906_417", "Ley 906 de 2004, art. 417"),
            ("co_906_254", "Ley 906 de 2004, arts. 254–266"),
            ("co_cgp_226", "Ley 1564 de 2012 (CGP), art. 226"),
        ]
    };
    list.iter()
        .map(|(k, r)| Citation {
            key: k.to_string(),
            reference: r.to_string(),
            status: "to_be_verified".into(),
        })
        .collect()
}

/// Citation keys printed under each section heading.
pub fn section_citations(code: &str, section: &str) -> &'static [&'static str] {
    match (code, section) {
        ("fr", "mission") => &["fr_cpp_156", "fr_cpp_158", "fr_cpp_157"],
        ("fr", "evidence") => &["fr_cpp_163"],
        ("fr", "method") | ("fr", "results") => &["fr_cpp_166"],
        ("fr", "declaration") => &["fr_cpp_166", "fr_cpp_167"],
        ("co", "identification") => &["co_906_405", "co_906_413"],
        ("co", "object") | ("co", "results") => &["co_906_415"],
        ("co", "evidence") => &["co_906_254"],
        ("co", "method") | ("co", "limitations") => &["co_906_417"],
        ("co", "annexes") => &["co_906_414"],
        ("co", "declaration") => &["co_906_415", "co_cgp_226"],
        _ => &[],
    }
}

/// Who assembles, for what (the parts of the model that do not come from the case).
#[derive(Clone, Debug, Default)]
pub struct AssembleCtx {
    pub tool_name: String,
    pub tool_version: String,
    /// This run's id (its own folder is skipped as `self`); empty for a preview.
    pub run_id: String,
    pub actor: String,
    pub langs: Vec<String>,
    pub generated_utc: String,
    pub pdf_signed_by: Option<String>,
    pub pdf_signer_certificate_sha256: Option<String>,
}

/// One step of the assembly for the audit log (action, item, details), in the order it happened.
pub type Note = (String, Option<String>, Value);

/// The model plus what the pipeline records about how it was built.
#[derive(Clone, Debug, Default)]
pub struct Assembly {
    pub report: ExpertReport,
    /// Absolute paths of the files read (case.json, registry, each verified run's audit log and outputs, mandate).
    pub files_read: Vec<PathBuf>,
    /// Audit notes in order (run_skipped, run_verified, run_excluded, run_quoted, …).
    pub notes: Vec<Note>,
}

/// Reads the mandate file: UTF-8 text (BOM ignored), at most [`MAX_MANDATE_BYTES`].
pub fn read_mandate(p: &Path) -> Result<Mandate, String> {
    let meta =
        std::fs::metadata(p).map_err(|e| format!("mandate file {}: {}", p.display(), e))?;
    if !meta.is_file() {
        return Err(format!("mandate file {} is not a file", p.display()));
    }
    if meta.len() > MAX_MANDATE_BYTES {
        return Err(format!(
            "mandate file {} is {} bytes; at most {} are accepted (quote the object of the expertise, not the whole file)",
            p.display(),
            meta.len(),
            MAX_MANDATE_BYTES
        ));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("mandate file {}: {}", p.display(), e))?;
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = std::str::from_utf8(body)
        .map_err(|_| format!("mandate file {} is not UTF-8 text", p.display()))?
        .replace("\r\n", "\n");
    if text.trim().is_empty() {
        return Err(format!("mandate file {} is empty", p.display()));
    }
    Ok(Mandate {
        file_name: p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        sha256: hash_bytes(&bytes, HashAlgo::Sha256),
        bytes: bytes.len() as u64,
        text: text.trim_end().to_string(),
    })
}

/// Whether a run of `tool` is left out by the filters.
pub fn filtered_out(tool: &str, include: &[String], exclude: &[String]) -> bool {
    (!include.is_empty() && !include.iter().any(|t| t == tool)) || exclude.iter().any(|t| t == tool)
}

/// The printed limitation sentences of a statistics.<lang>.html: the `<li>` items of the list right after the
/// localized heading (`Limitations`, `Limitaciones`, `Limites`). None when the heading is not found.
pub fn printed_limitations(html: &str) -> Option<Vec<String>> {
    let start = ["<h2>Limitations</h2>", "<h2>Limitaciones</h2>", "<h2>Limites</h2>"]
        .iter()
        .find_map(|h| html.find(h).map(|i| i + h.len()))?;
    let rest = &html[start..];
    let ul = rest.trim_start();
    if !ul.starts_with("<ul>") {
        return None;
    }
    let end = ul.find("</ul>")?;
    let body = &ul[4..end];
    let mut out = Vec::new();
    for part in body.split("<li>").skip(1) {
        let text = part.split("</li>").next().unwrap_or_default();
        out.push(html_unescape(text));
    }
    Some(out)
}

/// Unescapes the entities the suite's HTML templates write (minijinja autoescape).
pub fn html_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&#x2f;", "/")
        .replace("&#x2F;", "/")
        .replace("&amp;", "&")
}

fn string_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, x)| {
                    (
                        k.clone(),
                        x.as_str().map(String::from).unwrap_or_else(|| x.to_string()),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Findings of one `technical.json`, verbatim.
pub fn quote_findings(tech: &Value) -> Vec<QuotedFinding> {
    tech["findings"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(n, f)| QuotedFinding {
                    code: f["code"].as_str().unwrap_or_default().to_string(),
                    severity: Severity::parse(f["severity"].as_str().unwrap_or_default())
                        .unwrap_or_default(),
                    params: string_map(&f["params"]),
                    text: f["text"]
                        .as_object()
                        .map(|m| {
                            m.iter()
                                .map(|(l, t)| {
                                    (
                                        l.clone(),
                                        FindingText {
                                            title: t["title"].as_str().unwrap_or_default().into(),
                                            shows: t["shows"].as_str().unwrap_or_default().into(),
                                            does_not_prove: t["does_not_prove"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .into(),
                                        },
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    pointer: format!("/findings/{n}"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn ts(s: &str) -> Option<jiff::Timestamp> {
    s.parse::<jiff::Timestamp>().ok()
}

/// What one verified run contributed.
struct RunRead {
    run: RunRef,
    verified: Verified,
    started_details: Value,
    /// (evidence id, path, quoted run)
    quoted: Vec<(String, String, QuotedRun)>,
    limitation_codes: Vec<String>,
    limitation_texts: BTreeMap<String, Vec<String>>,
    /// evidence id → registry match, for every item the run examined (from its audit log).
    examined: BTreeMap<String, Option<bool>>,
    /// languages the run's finding texts are in.
    langs: BTreeSet<String>,
    statistics: Option<(Value, String)>,
    timeline_jsonl_sha: Option<String>,
    files: Vec<PathBuf>,
}

fn read_verified(case_dir: &Path, run: &RunRef, verified: Verified) -> Result<RunRead, ReadError> {
    let data = RunData::load(case_dir, run)?;
    let abs = |rel: &str| data.dir.join(rel);
    let mut files = vec![data.dir.join("logs").join("audit.jsonl")];
    let mut quoted = Vec::new();
    let mut langs = BTreeSet::new();
    let mut examined: BTreeMap<String, Option<bool>> = BTreeMap::new();
    for it in data.items.values() {
        if !it.evidence_id.is_empty() {
            let e = examined.entry(it.evidence_id.clone()).or_insert(None);
            if it.registry_match.is_some() {
                *e = it.registry_match;
            }
        }
    }
    for rel in data.outputs_ending("/technical.json") {
        if !rel.starts_with("reports/items/") {
            continue;
        }
        let Some((tech, sha)) = data.json(&rel)? else {
            continue;
        };
        files.push(abs(&rel));
        if !tech["schema"]
            .as_str()
            .unwrap_or_default()
            .starts_with("rakvor.item-technical/")
        {
            continue;
        }
        let item = &tech["item"];
        let index = item["index"].as_u64();
        let mut evidence_id = item["evidence_id"].as_str().unwrap_or_default().to_string();
        let run_item = index.and_then(|i| data.items.get(&i));
        if evidence_id.is_empty()
            && let Some(ri) = run_item
        {
            evidence_id = ri.evidence_id.clone();
        }
        let registry_match = run_item.and_then(|i| i.registry_match);
        let findings = quote_findings(&tech);
        for f in &findings {
            langs.extend(f.text.keys().cloned());
        }
        quoted.push((
            evidence_id.clone(),
            item["path"].as_str().unwrap_or_default().to_string(),
            QuotedRun {
                tool: run.tool.clone(),
                tool_version: run.tool_version.clone(),
                run_id: run.run_id.clone(),
                item_status: item["status"].as_str().unwrap_or_default().to_string(),
                registry_match,
                source: SourceRef {
                    file: format!("{}/{}", run.run_dir, rel),
                    sha256: sha,
                },
                findings,
            },
        ));
    }
    quoted.sort_by(|a, b| a.2.source.file.cmp(&b.2.source.file));
    let statistics = data.json("reports/statistics.json")?;
    let mut limitation_codes = Vec::new();
    let mut limitation_texts = BTreeMap::new();
    if let Some((st, _)) = &statistics {
        files.push(abs("reports/statistics.json"));
        limitation_codes = str_list(&st["limitations"]);
        for l in crate::i18n::LANGS {
            let rel = format!("reports/statistics.{l}.html");
            if let Some((html, _)) = data.text(&rel)? {
                files.push(abs(&rel));
                if let Some(t) = printed_limitations(&html) {
                    limitation_texts.insert(l.to_string(), t);
                }
            }
        }
    }
    let timeline_jsonl_sha = data
        .digests
        .get("reports/timeline.jsonl")
        .and_then(|d| d.get("sha256").cloned());
    Ok(RunRead {
        run: run.clone(),
        verified,
        started_details: data.started_details(),
        quoted,
        limitation_codes,
        limitation_texts,
        examined,
        langs,
        statistics,
        timeline_jsonl_sha,
        files,
    })
}

/// Assembles the model. `case` must already be open (verified). `on_run` is called once per run discovered with
/// (index from 1, total, run_dir, status) so front-ends can show progress.
pub fn assemble(
    case: &Case,
    opts: &AssembleOptions,
    ctx: &AssembleCtx,
    on_run: &mut dyn FnMut(u64, u64, &str, &str),
) -> Result<Assembly, String> {
    let case_dir = case.dir.clone();
    let mut notes: Vec<Note> = Vec::new();
    let mut files_read: Vec<PathBuf> = Vec::new();
    let base = std::path::absolute(&case_dir).unwrap_or_else(|_| case_dir.clone());
    files_read.push(base.join(crate::case::CASE_FILE));
    if base.join(crate::case::REGISTRY_JSON).is_file() {
        files_read.push(base.join("evidence").join("registry.json"));
    }

    // Case-level facts (the case is already verified by Case::open).
    let case_json_bytes = std::fs::read(case_dir.join(crate::case::CASE_FILE))
        .map_err(|e| format!("case.json: {e}"))?;
    let (log, _) = sources::read_jsonl(&case_dir.join(crate::case::CASE_LOG))?;
    let case_log_head = log
        .last()
        .map(|(_, e)| s(at(e, "entry_hash")).to_string())
        .unwrap_or_default();
    let code = opts
        .jurisdiction
        .clone()
        .unwrap_or_else(|| case.jurisdiction.clone());
    let code = if code == "fr" { "fr" } else { "co" }.to_string();

    let mandate = match &opts.mandate {
        Some(p) => {
            let m = read_mandate(p)?;
            files_read.push(std::path::absolute(p).unwrap_or_else(|_| p.clone()));
            notes.push((
                "mandate_read".into(),
                Some(p.display().to_string()),
                json!({"file_name": m.file_name, "bytes": m.bytes, "sha256": m.sha256}),
            ));
            Some(m)
        }
        None => None,
    };

    // 1. Discover, skip, verify.
    let mut discovered =
        sources::discover_runs(&case_dir).map_err(|e| format!("{}: {}", e.reason, e.detail))?;
    discovered.sort_by(|a, b| (&a.tool, &a.run_id).cmp(&(&b.tool, &b.run_id)));
    let total = discovered.len() as u64;
    let mut annex: Vec<RunAnnex> = Vec::new();
    let mut reads: Vec<RunRead> = Vec::new();
    let mut signals: Vec<ReportSignal> = Vec::new();
    for (i, r) in discovered.into_iter().enumerate() {
        let skip = if r.tool == ctx.tool_name {
            Some("self")
        } else if VERIFICATION_TOOLS.contains(&r.tool.as_str()) {
            Some("verification")
        } else if filtered_out(&r.tool, &opts.include_tools, &opts.exclude_tools) {
            Some("tool_filter")
        } else {
            None
        };
        let mut a = RunAnnex {
            tool: r.tool.clone(),
            tool_version: r.tool_version.clone(),
            run_id: r.run_id.clone(),
            run_dir: r.run_dir.clone(),
            started_utc: r.started_utc.clone(),
            finished_utc: r.finished_utc.clone(),
            timestamp_status: if r.timestamp_status.is_empty() {
                "unknown".into()
            } else {
                r.timestamp_status.clone()
            },
            ..Default::default()
        };
        let run_json = json!({"tool": r.tool, "version": r.tool_version, "run_id": r.run_id, "run_dir": r.run_dir});
        if let Some(reason) = skip {
            a.status = "skipped".into();
            a.reason = reason.into();
            let mut d = run_json;
            d["reason"] = json!(reason);
            notes.push(("run_skipped".into(), Some(r.run_dir.clone()), d));
            on_run(i as u64 + 1, total, &r.run_dir, "skipped");
            annex.push(a);
            continue;
        }
        let outcome = sources::verify_run(&case_dir, &r)
            .and_then(|v| read_verified(&case_dir, &r, v));
        match outcome {
            Ok(rr) => {
                a.status = "verified".into();
                a.audit_entries = rr.verified.audit_entries;
                a.audit_head = rr.verified.audit_head.clone();
                a.outputs_recorded = rr.verified.outputs_recorded;
                a.outputs_verified = rr.verified.outputs_verified;
                a.timestamps = rr.verified.timestamps.clone();
                if !a.timestamps.is_empty() {
                    a.timestamp_status = "ok".into();
                }
                let mut d = run_json;
                d["audit_entries"] = json!(a.audit_entries);
                d["audit_head"] = json!(a.audit_head);
                d["outputs_verified"] = json!(a.outputs_verified);
                d["outputs_recorded"] = json!(a.outputs_recorded);
                notes.push(("run_verified".into(), Some(r.run_dir.clone()), d));
                on_run(i as u64 + 1, total, &r.run_dir, "verified");
                reads.push(rr);
            }
            Err(e) => {
                a.status = "excluded".into();
                a.reason = e.reason.clone();
                a.detail = e.detail.clone();
                let mut d = run_json;
                d["reason"] = json!(e.reason);
                d["detail"] = json!(e.detail);
                notes.push(("run_excluded".into(), Some(r.run_dir.clone()), d));
                on_run(i as u64 + 1, total, &r.run_dir, "excluded");
                signals.push(ReportSignal {
                    code: "RUN_EXCLUDED".into(),
                    severity: Severity::Medium,
                    params: BTreeMap::from([
                        ("tool".to_string(), r.tool.clone()),
                        ("run_id".to_string(), r.run_id.clone()),
                        ("reason".to_string(), e.reason.clone()),
                    ]),
                    evidence_id: None,
                    run_id: Some(r.run_id.clone()),
                });
            }
        }
        annex.push(a);
    }

    // 2. Only the latest verified RV Timeline run is used.
    let latest_timeline = reads
        .iter()
        .filter(|r| r.run.tool == TIMELINE_TOOL)
        .map(|r| r.run.run_id.clone())
        .max();
    reads.retain(|r| {
        let superseded =
            r.run.tool == TIMELINE_TOOL && Some(&r.run.run_id) != latest_timeline.as_ref();
        if superseded {
            if let Some(a) = annex.iter_mut().find(|a| a.run_dir == r.run.run_dir) {
                a.status = "skipped".into();
                a.reason = "superseded".into();
                a.detail = latest_timeline.clone().unwrap_or_default();
            }
            notes.push((
                "run_skipped".into(),
                Some(r.run.run_dir.clone()),
                json!({"tool": r.run.tool, "version": r.run.tool_version, "run_id": r.run.run_id,
                       "run_dir": r.run.run_dir, "reason": "superseded", "by": latest_timeline}),
            ));
        }
        !superseded
    });
    for r in &reads {
        files_read.extend(r.files.iter().cloned());
    }

    // 3. Evidence and custody.
    let registered_rows = registration_rows(&log);
    let mut evidence: Vec<EvidenceItem> = Vec::new();
    for it in &case.items {
        let mut custody: Vec<CustodyEvent> = registered_rows
            .get(&it.evidence_id)
            .cloned()
            .into_iter()
            .collect();
        for r in &reads {
            let Some(m) = r.examined.get(&it.evidence_id) else {
                continue;
            };
            if custody
                .iter()
                .any(|c| c.action == "registered" && c.run_id == r.run.run_id)
            {
                continue;
            }
            custody.push(CustodyEvent {
                action: "examined".into(),
                case_log_seq: r.run.started_seq,
                ts_utc: r.run.started_utc.clone(),
                actor: r.run.actor.clone(),
                tool: r.run.tool.clone(),
                tool_version: r.run.tool_version.clone(),
                run_id: r.run.run_id.clone(),
                registry_match: *m,
            });
        }
        custody.sort_by_key(|c| c.case_log_seq);
        evidence.push(EvidenceItem {
            evidence_id: it.evidence_id.clone(),
            path: it.path.clone(),
            file_name: file_name(&it.path),
            size: it.size,
            hashes: it.hashes.clone(),
            registered_utc: it.registered_utc.clone(),
            registered_by: RunPointer {
                tool: it.registered_by.tool.clone(),
                version: it.registered_by.version.clone(),
                run_id: it.registered_by.run_id.clone(),
            },
            custody,
        });
    }

    // 4. Results per item: registry order, then items a run named outside the registry.
    let mut results: Vec<ItemResults> = case
        .items
        .iter()
        .map(|it| ItemResults {
            evidence_id: it.evidence_id.clone(),
            path: it.path.clone(),
            entries: vec![],
        })
        .collect();
    for r in &reads {
        for (id, path, q) in &r.quoted {
            let slot = results.iter_mut().find(|x| {
                (!id.is_empty() && x.evidence_id == *id)
                    || (id.is_empty() && x.evidence_id.is_empty() && x.path == *path)
            });
            match slot {
                Some(x) => x.entries.push(q.clone()),
                None => results.push(ItemResults {
                    evidence_id: id.clone(),
                    path: path.clone(),
                    entries: vec![q.clone()],
                }),
            }
            if q.registry_match == Some(false) {
                signals.push(ReportSignal {
                    code: "REGISTRY_MISMATCH_REPORTED".into(),
                    severity: Severity::High,
                    params: BTreeMap::from([
                        ("tool".to_string(), q.tool.clone()),
                        ("run_id".to_string(), q.run_id.clone()),
                        ("evidence_id".to_string(), id.clone()),
                    ]),
                    evidence_id: Some(id.clone()).filter(|x| !x.is_empty()),
                    run_id: Some(q.run_id.clone()),
                });
            }
        }
        notes.push((
            "run_quoted".into(),
            Some(r.run.run_dir.clone()),
            json!({"tool": r.run.tool, "run_id": r.run.run_id, "items": r.quoted.len(),
                   "findings": r.quoted.iter().map(|q| q.2.findings.len()).sum::<usize>(),
                   "sources": r.quoted.iter().map(|q| json!({"file": q.2.source.file, "sha256": q.2.source.sha256})).collect::<Vec<_>>()}),
        ));
    }

    // 5. Method per tool.
    let mut tools: BTreeMap<String, ToolMethod> = BTreeMap::new();
    for r in &reads {
        let m = tools.entry(r.run.tool.clone()).or_insert_with(|| ToolMethod {
            tool: r.run.tool.clone(),
            known: KNOWN_TOOLS.contains(&r.run.tool.as_str()),
            role: r.run.role.clone(),
            ..Default::default()
        });
        let add = |v: &mut Vec<String>, x: String| {
            if !x.is_empty() && !v.contains(&x) {
                v.push(x);
            }
        };
        add(&mut m.versions, r.run.tool_version.clone());
        add(&mut m.runs, r.run.run_id.clone());
        for a in str_list(&r.started_details["algorithms"]) {
            add(&mut m.algorithms, a);
        }
        for l in str_list(&r.started_details["langs"]) {
            add(&mut m.langs, l);
        }
        add(
            &mut m.frontends,
            r.started_details["frontend"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        m.findings_quoted += r
            .quoted
            .iter()
            .map(|q| q.2.findings.len() as u64)
            .sum::<u64>();
        if m.role.is_empty() {
            m.role = r.run.role.clone();
        }
    }
    for (tool, m) in tools.iter_mut() {
        let ids: BTreeSet<&String> = reads
            .iter()
            .filter(|r| &r.run.tool == tool)
            .flat_map(|r| r.examined.keys())
            .collect();
        m.items_examined = ids.len() as u64;
        if !m.known {
            signals.push(ReportSignal {
                code: "UNKNOWN_TOOL".into(),
                severity: Severity::Info,
                params: BTreeMap::from([("tool".to_string(), tool.clone())]),
                evidence_id: None,
                run_id: None,
            });
        }
    }

    // 6. Limitations, one block per (tool, version, codes).
    let mut lims: Vec<ToolLimitations> = Vec::new();
    for r in &reads {
        if r.limitation_codes.is_empty() {
            continue;
        }
        let items: Vec<QuotedLimitation> = r
            .limitation_codes
            .iter()
            .enumerate()
            .map(|(n, code)| QuotedLimitation {
                code: code.clone(),
                text: r
                    .limitation_texts
                    .iter()
                    .filter(|(_, t)| t.len() == r.limitation_codes.len())
                    .map(|(l, t)| (l.clone(), t[n].clone()))
                    .collect(),
            })
            .collect();
        match lims
            .iter_mut()
            .find(|l| l.tool == r.run.tool && l.tool_version == r.run.tool_version && l.items == items)
        {
            Some(l) => l.runs.push(r.run.run_id.clone()),
            None => lims.push(ToolLimitations {
                tool: r.run.tool.clone(),
                tool_version: r.run.tool_version.clone(),
                runs: vec![r.run.run_id.clone()],
                items,
            }),
        }
    }

    // 7. Missing languages (once per run and report language).
    for r in &reads {
        let has_text = !r.langs.is_empty()
            || lims
                .iter()
                .any(|l| l.runs.contains(&r.run.run_id) && l.items.iter().any(|i| !i.text.is_empty()));
        if !has_text {
            continue;
        }
        let mut available: BTreeSet<String> = r.langs.clone();
        available.extend(r.limitation_texts.keys().cloned());
        for l in &ctx.langs {
            if !available.contains(l) {
                signals.push(ReportSignal {
                    code: "TRANSLATION_UNAVAILABLE".into(),
                    severity: Severity::Info,
                    params: BTreeMap::from([
                        ("tool".to_string(), r.run.tool.clone()),
                        ("run_id".to_string(), r.run.run_id.clone()),
                        ("lang".to_string(), l.clone()),
                        (
                            "available".to_string(),
                            available.iter().cloned().collect::<Vec<_>>().join(", "),
                        ),
                    ]),
                    evidence_id: None,
                    run_id: Some(r.run.run_id.clone()),
                });
            }
        }
    }

    // 8. Timeline.
    let timeline = reads
        .iter()
        .find(|r| r.run.tool == TIMELINE_TOOL)
        .map(|r| timeline_summary(r));
    match &timeline {
        None => signals.push(ReportSignal {
            code: "TIMELINE_ABSENT".into(),
            severity: Severity::Info,
            ..Default::default()
        }),
        Some(t) => {
            let started = ts(&t.started_utc);
            let late: Vec<&RunRead> = reads
                .iter()
                .filter(|r| r.run.tool != TIMELINE_TOOL)
                .filter(|r| match (started, ts(&r.run.finished_utc)) {
                    (Some(a), Some(b)) => b > a,
                    _ => false,
                })
                .collect();
            for r in late {
                signals.push(ReportSignal {
                    code: "TIMELINE_PREDATES_RUNS".into(),
                    severity: Severity::Low,
                    params: BTreeMap::from([
                        ("timeline_run".to_string(), t.run_id.clone()),
                        ("tool".to_string(), r.run.tool.clone()),
                        ("run_id".to_string(), r.run.run_id.clone()),
                    ]),
                    evidence_id: None,
                    run_id: Some(r.run.run_id.clone()),
                });
            }
        }
    }

    // 9. What was not examined.
    let mut not_examined = Vec::new();
    for it in &case.items {
        let examined = reads.iter().any(|r| {
            r.run.role != "intake"
                && !NOT_EXAMINING_TOOLS.contains(&r.run.tool.as_str())
                && r.examined.contains_key(&it.evidence_id)
        });
        if !examined {
            not_examined.push(NotExamined {
                evidence_id: it.evidence_id.clone(),
                path: it.path.clone(),
                reason: "no_analysis_run".into(),
            });
            signals.push(ReportSignal {
                code: "ITEM_NOT_EXAMINED".into(),
                severity: Severity::Low,
                params: BTreeMap::from([
                    ("evidence_id".to_string(), it.evidence_id.clone()),
                    ("file".to_string(), file_name(&it.path)),
                ]),
                evidence_id: Some(it.evidence_id.clone()),
                run_id: None,
            });
        }
    }
    if code != case.jurisdiction {
        signals.push(ReportSignal {
            code: "JURISDICTION_DIFFERS_FROM_CASE".into(),
            severity: Severity::Info,
            params: BTreeMap::from([
                ("requested".to_string(), code.clone()),
                ("case".to_string(), case.jurisdiction.clone()),
            ]),
            evidence_id: None,
            run_id: None,
        });
    }

    // 10. Totals.
    let mut quoted_by_severity: BTreeMap<String, u64> = ["high", "medium", "low", "info"]
        .iter()
        .map(|k| (k.to_string(), 0))
        .collect();
    let mut findings_quoted = 0;
    for r in &results {
        for e in &r.entries {
            for f in &e.findings {
                findings_quoted += 1;
                *quoted_by_severity
                    .entry(f.severity.key().to_string())
                    .or_default() += 1;
            }
        }
    }
    for a in annex.iter_mut() {
        a.findings_quoted = results
            .iter()
            .flat_map(|r| r.entries.iter())
            .filter(|e| e.run_id == a.run_id && e.tool == a.tool)
            .map(|e| e.findings.len() as u64)
            .sum();
    }
    let count = |st: &str| annex.iter().filter(|a| a.status == st).count() as u64;
    let totals = Totals {
        evidence_items: case.items.len() as u64,
        runs_found: annex.len() as u64,
        runs_verified: count("verified"),
        runs_excluded: count("excluded"),
        runs_skipped: count("skipped"),
        findings_quoted,
        quoted_by_severity,
        items_not_examined: not_examined.len() as u64,
    };

    let registry_sha256 = crate::case::registry_digest(&case_dir).unwrap_or_default();
    let report = ExpertReport {
        schema: SCHEMA_EXPERT_REPORT.into(),
        tool: ToolRef {
            name: ctx.tool_name.clone(),
            version: ctx.tool_version.clone(),
        },
        run: ReportRun {
            run_id: ctx.run_id.clone(),
            generated_utc: ctx.generated_utc.clone(),
            actor: ctx.actor.clone(),
            langs: ctx.langs.clone(),
            status: if opts.final_report { "final" } else { "draft" }.into(),
            include_tools: opts.include_tools.clone(),
            exclude_tools: opts.exclude_tools.clone(),
            pdf_signed_by: ctx.pdf_signed_by.clone(),
            pdf_signer_certificate_sha256: ctx.pdf_signer_certificate_sha256.clone(),
        },
        jurisdiction: JurisdictionInfo {
            code: code.clone(),
            case_jurisdiction: case.jurisdiction.clone(),
            sections: sections(&code),
            citations: citations(&code),
        },
        identification: Identification {
            case_id: case.case_id.clone(),
            case_created_utc: case.created_utc.clone(),
            case_json_sha256: hash_bytes(&case_json_bytes, HashAlgo::Sha256),
            examiner: Examiner {
                name: ctx.actor.clone(),
                examiner_id: opts
                    .examiner_id
                    .clone()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            },
            mandate,
            case_verification: CaseCheck {
                case_log_entries: log.len() as u64,
                case_log_head,
                registry_items: case.items.len() as u64,
                registry_sha256,
            },
        },
        evidence,
        method: Method {
            tools: tools.into_values().collect(),
        },
        results,
        timeline,
        limitations: Limitations {
            tools: lims,
            own: OWN_LIMITATIONS.iter().map(|s| s.to_string()).collect(),
        },
        annexes: Annexes { runs: annex },
        not_examined,
        signals,
        totals,
    };
    Ok(Assembly {
        report,
        files_read,
        notes,
    })
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// `registered` custody rows from the case log's `evidence_registered` entries (first registration of each id).
fn registration_rows(log: &sources::Entries) -> BTreeMap<String, CustodyEvent> {
    let mut started: BTreeMap<String, (String, String, String)> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (_, e) in log {
        let d = at(e, "details");
        let run_id = s(at(e, "run_id")).to_string();
        match s(at(e, "action")) {
            "run_started" => {
                started.insert(
                    run_id,
                    (
                        s(&d["tool"]).to_string(),
                        s(&d["version"]).to_string(),
                        s(at(e, "actor")).to_string(),
                    ),
                );
            }
            "evidence_registered" => {
                let (tool, version, _) = started.get(&run_id).cloned().unwrap_or_else(|| {
                    (
                        s(&d["tool"]).to_string(),
                        s(&d["version"]).to_string(),
                        String::new(),
                    )
                });
                for id in str_list(&d["added"]) {
                    out.entry(id).or_insert_with(|| CustodyEvent {
                        action: "registered".into(),
                        case_log_seq: at(e, "seq").as_u64().unwrap_or(0),
                        ts_utc: s(at(e, "ts_utc")).to_string(),
                        actor: s(at(e, "actor")).to_string(),
                        tool: tool.clone(),
                        tool_version: version.clone(),
                        run_id: run_id.clone(),
                        registry_match: None,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

fn timeline_summary(r: &RunRead) -> TimelineSummary {
    let (st, sha) = r
        .statistics
        .clone()
        .unwrap_or((Value::Null, String::new()));
    let t = &st["timeline"];
    let mut by_code: BTreeMap<String, u64> = BTreeMap::new();
    for sgl in t["signals"].as_array().into_iter().flatten() {
        *by_code
            .entry(sgl["code"].as_str().unwrap_or_default().to_string())
            .or_default() += 1;
    }
    let by_sev: BTreeMap<String, u64> = t["signals_by_severity"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0)))
                .collect()
        })
        .unwrap_or_default();
    let file = t["timeline_file"].as_str().unwrap_or_default();
    TimelineSummary {
        run_id: r.run.run_id.clone(),
        tool_version: r.run.tool_version.clone(),
        started_utc: r.run.started_utc.clone(),
        finished_utc: r.run.finished_utc.clone(),
        events: t["events"].as_u64().unwrap_or(0),
        runs_verified: t["runs_verified"].as_u64().unwrap_or(0),
        runs_excluded: t["runs_excluded"].as_u64().unwrap_or(0),
        zone_unknown: t["events_zone_unknown"].as_u64().unwrap_or(0),
        display_zone: t["display_zone"].as_str().unwrap_or("UTC").to_string(),
        signals_by_severity: by_sev,
        signals_by_code: by_code,
        source: SourceRef {
            file: format!("{}/reports/statistics.json", r.run.run_dir),
            sha256: sha,
        },
        timeline_file: (!file.is_empty())
            .then(|| {
                r.timeline_jsonl_sha.clone().map(|sha| SourceRef {
                    file: format!("{}/reports/{}", r.run.run_dir, file),
                    sha256: sha,
                })
            })
            .flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_limitations_are_paired_from_the_html() {
        let html = "<h2>Findings</h2><ul><li>x</li></ul>\n<h2>Limitaciones</h2>\n<ul><li>Una &amp; dos</li><li>It&#x27;s &lt;b&gt;</li></ul><h2>Sello</h2>";
        assert_eq!(
            printed_limitations(html).unwrap(),
            vec!["Una & dos".to_string(), "It's <b>".to_string()]
        );
        assert!(printed_limitations("<h2>Other</h2><ul><li>a</li></ul>").is_none());
    }

    #[test]
    fn findings_are_quoted_verbatim() {
        let tech = json!({"findings": [{"code": "X_Y", "severity": "medium", "params": {"a": "1", "n": 2},
            "text": {"en": {"title": "T", "shows": "S “q”", "does_not_prove": "D"}}}]});
        let q = quote_findings(&tech);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].severity, Severity::Medium);
        assert_eq!(q[0].params["n"], "2");
        assert_eq!(q[0].text["en"].shows, "S “q”");
        assert_eq!(q[0].pointer, "/findings/0");
    }

    #[test]
    fn filters_and_structures() {
        assert!(filtered_out("rv-imgauth", &["rv-pdfauth".into()], &[]));
        assert!(!filtered_out("rv-pdfauth", &["rv-pdfauth".into()], &[]));
        assert!(filtered_out("rv-pdfauth", &[], &["rv-pdfauth".into()]));
        assert_eq!(sections("co")[0], "identification");
        assert_eq!(sections("fr")[0], "mission");
        for code in ["co", "fr"] {
            let keys: Vec<String> = citations(code).into_iter().map(|c| c.key).collect();
            for sec in sections(code) {
                for k in section_citations(code, &sec) {
                    assert!(keys.contains(&k.to_string()), "{code} {sec} {k}");
                }
            }
            assert!(citations(code).iter().all(|c| c.status == "to_be_verified"));
        }
    }

    #[test]
    fn mandate_rules() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.txt");
        std::fs::write(&p, b"\xEF\xBB\xBFObject: examine E0001.\r\nSecond line.\r\n").unwrap();
        let m = read_mandate(&p).unwrap();
        assert_eq!(m.text, "Object: examine E0001.\nSecond line.");
        assert_eq!(m.sha256.len(), 64);
        std::fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_mandate(&p).unwrap_err().contains("UTF-8"));
        std::fs::write(&p, vec![b'a'; (MAX_MANDATE_BYTES + 1) as usize]).unwrap();
        assert!(read_mandate(&p).unwrap_err().contains("at most"));
        std::fs::write(&p, b"  \n").unwrap();
        assert!(read_mandate(&p).unwrap_err().contains("empty"));
    }
}
