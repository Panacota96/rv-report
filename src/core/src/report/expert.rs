//! The expert report (dictamen pericial / rapport d'expertise) in HTML and PDF, one file per language, rendered
//! from the [`ExpertReport`] model only. Both formats use the same view model ([`expert_view`]), so they never
//! disagree. Quoted wording is printed exactly as the tool wrote it; RV Report's own wording comes from
//! `i18n/*.json` (`expert`, `tools`, `exclusion`, `skip`, `findings`, `limitations`).

use super::pdf::{self, Block};
use crate::i18n::{Catalog, catalog, fill};
use crate::model::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const CSS: &str = include_str!("../../../../templates/report.css");
const LOGO_SVG: &str = include_str!("../../../../templates/rv-mark.svg");
const TEMPLATE: &str = include_str!("../../../../templates/expert-report.html");

/// The model as written to `reports/expert-report.json` (pretty, keys sorted).
pub fn expert_json(r: &ExpertReport) -> Vec<u8> {
    super::pretty(&serde_json::to_value(r).unwrap_or(Value::Null))
}

fn x(c: &Catalog, key: &str) -> String {
    c.text(&format!("expert.{key}"))
}

fn xf(c: &Catalog, key: &str, params: &[(&str, String)]) -> String {
    let p: BTreeMap<String, String> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    fill(&x(c, key), &p)
}

fn kv(rows: Vec<(String, String)>) -> Value {
    json!({"kind": "kv", "rows": rows.into_iter().map(|(k, v)| vec![k, v]).collect::<Vec<_>>()})
}
fn para(t: String) -> Value {
    json!({"kind": "para", "text": t})
}
fn note(t: String) -> Value {
    json!({"kind": "note", "text": t})
}
fn sub(t: String) -> Value {
    json!({"kind": "sub", "text": t})
}
fn bullets(items: Vec<String>) -> Value {
    json!({"kind": "bullets", "items": items})
}
fn boxed(
    badge: String,
    sev: &str,
    title: String,
    code: &str,
    rows: Vec<(String, String)>,
) -> Value {
    json!({"kind": "box", "badge": badge, "sev": sev, "title": title, "code": code,
           "rows": rows.into_iter().map(|(k, v)| vec![k, v]).collect::<Vec<_>>()})
}

fn or_dash(s: &str) -> String {
    if s.is_empty() {
        "—".into()
    } else {
        s.to_string()
    }
}

fn yes_no_match(c: &Catalog, m: Option<bool>) -> String {
    match m {
        Some(true) => x(c, "labels.match_yes"),
        Some(false) => x(c, "labels.match_no"),
        None => x(c, "labels.match_unknown"),
    }
}

/// The quoted wording in `lang`, else in the first language the tool wrote (en, es, fr), with that language.
pub fn text_for<'a>(f: &'a QuotedFinding, lang: &str) -> Option<(&'a FindingText, String)> {
    if let Some(t) = f.text.get(lang) {
        return Some((t, lang.to_string()));
    }
    crate::i18n::LANGS
        .iter()
        .find_map(|l| f.text.get(*l).map(|t| (t, l.to_string())))
        .or_else(|| f.text.iter().next().map(|(l, t)| (t, l.clone())))
}

fn lang_mark(used: &str, lang: &str) -> String {
    if used == lang {
        String::new()
    } else {
        format!("[{}] ", used.to_uppercase())
    }
}

fn signal_box(c: &Catalog, s: &ReportSignal) -> Value {
    let (title, shows, dnp) = c.finding(&s.code, &s.params);
    boxed(
        c.severity(s.severity.key()),
        s.severity.key(),
        title,
        &s.code,
        vec![
            (x(c, "labels.shows"), shows),
            (x(c, "labels.does_not_prove"), dnp),
        ],
    )
}

fn basis(c: &Catalog, r: &ExpertReport, section: &str) -> Vec<String> {
    let code = r.jurisdiction.code.as_str();
    crate::assemble::section_citations(code, section)
        .iter()
        .filter_map(|k| r.jurisdiction.citations.iter().find(|ci| ci.key == *k))
        .map(|ci| {
            format!(
                "{} — {} {}",
                ci.reference,
                x(c, &format!("citations.{}", ci.key)),
                x(c, "to_verify")
            )
        })
        .collect()
}

fn identification_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let id = &r.identification;
    let mut rows = vec![
        (x(c, "labels.case"), id.case_id.clone()),
        (x(c, "labels.case_created"), or_dash(&id.case_created_utc)),
        (x(c, "labels.case_json_sha256"), id.case_json_sha256.clone()),
        (x(c, "labels.examiner"), id.examiner.name.clone()),
        (
            x(c, "labels.examiner_id"),
            id.examiner
                .examiner_id
                .clone()
                .unwrap_or_else(|| x(c, "labels.not_given")),
        ),
        (x(c, "labels.report_run"), or_dash(&r.run.run_id)),
        (x(c, "labels.generated"), r.run.generated_utc.clone()),
        (
            x(c, "labels.status"),
            x(c, &format!("labels.status_{}", r.run.status)),
        ),
        (
            x(c, "labels.structure"),
            x(c, &format!("labels.structure_{}", r.jurisdiction.code)),
        ),
        (x(c, "labels.languages"), r.run.langs.join(", ")),
    ];
    if !r.run.include_tools.is_empty() || !r.run.exclude_tools.is_empty() {
        rows.push((
            x(c, "labels.tool_filter"),
            format!(
                "+[{}] −[{}]",
                r.run.include_tools.join(", "),
                r.run.exclude_tools.join(", ")
            ),
        ));
    }
    let mut b = vec![kv(rows)];
    if r.jurisdiction.code != r.jurisdiction.case_jurisdiction {
        b.push(note(xf(
            c,
            "jurisdiction_differs",
            &[
                ("requested", r.jurisdiction.code.clone()),
                ("case", r.jurisdiction.case_jurisdiction.clone()),
            ],
        )));
    }
    let v = &id.case_verification;
    b.push(sub(x(c, "labels.case_verification")));
    b.push(para(x(c, "case_verification_text")));
    b.push(kv(vec![
        (
            x(c, "labels.case_log_entries"),
            v.case_log_entries.to_string(),
        ),
        (x(c, "labels.case_log_head"), v.case_log_head.clone()),
        (x(c, "labels.registry_items"), v.registry_items.to_string()),
        (x(c, "labels.registry_sha256"), or_dash(&v.registry_sha256)),
    ]));
    b
}

fn object_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    match &r.identification.mandate {
        None => vec![note(x(c, "no_mandate"))],
        Some(m) => {
            let mut b = vec![
                para(x(c, "mandate_quoted")),
                kv(vec![
                    (x(c, "labels.file"), m.file_name.clone()),
                    (x(c, "labels.sha256"), m.sha256.clone()),
                    (x(c, "labels.bytes"), m.bytes.to_string()),
                ]),
            ];
            b.extend(
                m.text
                    .split('\n')
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| json!({"kind": "quote", "text": l})),
            );
            b
        }
    }
}

fn evidence_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let mut b = vec![para(x(c, "intro.evidence"))];
    if r.evidence.is_empty() {
        b.push(para(x(c, "labels.none")));
    }
    for e in &r.evidence {
        let mut rows = vec![
            (x(c, "labels.path"), e.path.clone()),
            (
                x(c, "labels.size"),
                format!("{} {}", e.size, x(c, "labels.bytes_unit")),
            ),
        ];
        for (a, h) in &e.hashes {
            rows.push((a.to_uppercase(), h.clone()));
        }
        rows.push((x(c, "labels.registered"), e.registered_utc.clone()));
        rows.push((
            x(c, "labels.registered_by"),
            format!(
                "{} {} · {}",
                e.registered_by.tool, e.registered_by.version, e.registered_by.run_id
            ),
        ));
        b.push(boxed(
            e.evidence_id.clone(),
            "info",
            e.file_name.clone(),
            "",
            rows,
        ));
        b.push(sub(xf(c, "custody_of", &[("id", e.evidence_id.clone())])));
        b.push(bullets(
            e.custody
                .iter()
                .map(|k| {
                    let m = if k.action == "examined" {
                        format!(
                            " · {}: {}",
                            x(c, "labels.registry"),
                            yes_no_match(c, k.registry_match)
                        )
                    } else {
                        String::new()
                    };
                    format!(
                        "#{} · {} · {} · {} {} · {} · {}{}",
                        k.case_log_seq,
                        k.ts_utc,
                        x(c, &format!("labels.custody_{}", k.action)),
                        k.tool,
                        k.tool_version,
                        k.run_id,
                        or_dash(&k.actor),
                        m
                    )
                })
                .collect(),
        ));
    }
    b.push(note(x(c, "custody_note")));
    b
}

fn method_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let mut b = vec![
        para(x(c, "intro.method")),
        bullets(vec![
            x(c, "method_steps.verify_case"),
            x(c, "method_steps.verify_runs"),
            x(c, "method_steps.quote"),
            x(c, "method_steps.not_examined"),
        ]),
    ];
    b.push(sub(x(c, "labels.tools_used")));
    b.push(para(x(c, "tools_note")));
    for m in &r.method.tools {
        let (what, checks) = if m.known {
            (
                c.text(&format!("tools.{}.role", m.tool)),
                c.text(&format!("tools.{}.checks", m.tool)),
            )
        } else {
            (x(c, "unknown_tool"), "—".into())
        };
        b.push(boxed(
            or_dash(&m.role),
            "info",
            format!("{} {}", m.tool, m.versions.join(", ")),
            "",
            vec![
                (x(c, "labels.what_it_does"), what),
                (x(c, "labels.what_it_checks"), checks),
                (x(c, "labels.runs"), m.runs.join(", ")),
                (
                    x(c, "labels.algorithms"),
                    or_dash(&m.algorithms.join(" + ")),
                ),
                (x(c, "labels.languages"), or_dash(&m.langs.join(", "))),
                (x(c, "labels.frontends"), or_dash(&m.frontends.join(", "))),
                (x(c, "labels.items_examined"), m.items_examined.to_string()),
                (
                    x(c, "labels.findings_quoted"),
                    m.findings_quoted.to_string(),
                ),
            ],
        ));
    }
    if r.method.tools.is_empty() {
        b.push(para(x(c, "no_verified_runs")));
    }
    b.push(sub(x(c, "labels.this_tool")));
    b.push(kv(vec![
        (
            x(c, "labels.tool"),
            format!("{} {}", r.tool.name, r.tool.version),
        ),
        (x(c, "labels.report_run"), or_dash(&r.run.run_id)),
    ]));
    b
}

fn results_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let lang = c.lang;
    let mut b = vec![para(x(c, "intro.results"))];
    for it in &r.results {
        let name = it.path.rsplit(['/', '\\']).next().unwrap_or(&it.path);
        b.push(sub(format!("{} — {}", or_dash(&it.evidence_id), name)));
        for s in r.signals.iter().filter(|s| {
            s.evidence_id.as_deref() == Some(it.evidence_id.as_str()) && !it.evidence_id.is_empty()
        }) {
            b.push(signal_box(c, s));
        }
        if it.entries.is_empty() {
            b.push(para(x(c, "no_results")));
        }
        for e in &it.entries {
            let registry = match e.registry_match {
                Some(_) => format!(
                    " · {}: {}",
                    x(c, "labels.registry"),
                    yes_no_match(c, e.registry_match)
                ),
                None => String::new(),
            };
            b.push(para(format!(
                "{} {} · {} · {}: {}{}",
                e.tool,
                e.tool_version,
                e.run_id,
                x(c, "labels.item_status"),
                or_dash(&e.item_status),
                registry
            )));
            b.push(json!({"kind": "source", "text": format!("{}: {} (SHA-256 {})", x(c, "labels.source"), e.source.file, e.source.sha256)}));
            if e.findings.is_empty() {
                b.push(para(x(c, "no_findings")));
            }
            for f in &e.findings {
                let (title, rows) = match text_for(f, lang) {
                    Some((t, used)) => (
                        format!("{}{}", lang_mark(&used, lang), t.title),
                        vec![
                            (x(c, "labels.shows"), t.shows.clone()),
                            (x(c, "labels.does_not_prove"), t.does_not_prove.clone()),
                            (x(c, "labels.pointer"), f.pointer.clone()),
                        ],
                    ),
                    None => (
                        f.code.clone(),
                        vec![
                            (x(c, "labels.shows"), x(c, "no_wording")),
                            (x(c, "labels.pointer"), f.pointer.clone()),
                        ],
                    ),
                };
                b.push(boxed(
                    c.severity(f.severity.key()),
                    f.severity.key(),
                    title,
                    &f.code,
                    rows,
                ));
            }
        }
    }
    b.push(note(x(c, "quote_note")));
    b
}

fn timeline_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let mut b = vec![para(x(c, "intro.timeline"))];
    match &r.timeline {
        None => b.push(note(x(c, "no_timeline"))),
        Some(t) => {
            let sev = ["high", "medium", "low", "info"]
                .iter()
                .map(|k| {
                    format!(
                        "{} {}",
                        c.severity(k),
                        t.signals_by_severity.get(*k).copied().unwrap_or(0)
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let mut rows = vec![
                (
                    x(c, "labels.timeline_run"),
                    format!("rv-timeline {} · {}", t.tool_version, t.run_id),
                ),
                (x(c, "labels.started"), or_dash(&t.started_utc)),
                (x(c, "labels.finished"), or_dash(&t.finished_utc)),
                (x(c, "labels.events"), t.events.to_string()),
                (
                    x(c, "labels.timeline_runs"),
                    xf(
                        c,
                        "timeline_runs",
                        &[
                            ("verified", t.runs_verified.to_string()),
                            ("excluded", t.runs_excluded.to_string()),
                        ],
                    ),
                ),
                (x(c, "labels.zone_unknown"), t.zone_unknown.to_string()),
                (x(c, "labels.display_zone"), t.display_zone.clone()),
                (x(c, "labels.signals"), sev),
            ];
            if !t.signals_by_code.is_empty() {
                rows.push((
                    x(c, "labels.signal_codes"),
                    t.signals_by_code
                        .iter()
                        .map(|(k, v)| format!("{k} ×{v}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            rows.push((
                x(c, "labels.source"),
                format!("{} (SHA-256 {})", t.source.file, t.source.sha256),
            ));
            if let Some(f) = &t.timeline_file {
                rows.push((
                    x(c, "labels.timeline_file"),
                    format!("{} (SHA-256 {})", f.file, f.sha256),
                ));
            }
            b.push(kv(rows));
            b.push(para(x(c, "timeline_signals_note")));
        }
    }
    for s in r.signals.iter().filter(|s| s.code.starts_with("TIMELINE_")) {
        b.push(signal_box(c, s));
    }
    b
}

fn limitations_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let lang = c.lang;
    let mut b = vec![para(x(c, "intro.limitations"))];
    for l in &r.limitations.tools {
        b.push(sub(format!(
            "{} {} ({}: {})",
            l.tool,
            l.tool_version,
            x(c, "labels.runs"),
            l.runs.join(", ")
        )));
        b.push(bullets(
            l.items
                .iter()
                .map(|q| {
                    if let Some(t) = q.text.get(lang) {
                        t.clone()
                    } else if let Some((ul, t)) = crate::i18n::LANGS
                        .iter()
                        .find_map(|ll| q.text.get(*ll).map(|t| (ll.to_string(), t)))
                    {
                        format!("{}{}", lang_mark(&ul, lang), t)
                    } else {
                        format!("{} ({})", q.code, x(c, "no_printed_limitation"))
                    }
                })
                .collect(),
        ));
    }
    b.push(sub(format!("{} {}", r.tool.name, r.tool.version)));
    b.push(bullets(
        r.limitations.own.iter().map(|l| c.limitation(l)).collect(),
    ));
    b
}

fn not_examined_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let mut b = vec![para(x(c, "intro.not_examined"))];
    if r.not_examined.is_empty() {
        b.push(para(x(c, "all_examined")));
    } else {
        b.push(bullets(
            r.not_examined
                .iter()
                .map(|n| {
                    format!(
                        "{} · {} · {}",
                        n.evidence_id,
                        n.path,
                        x(c, &format!("not_examined_reason.{}", n.reason))
                    )
                })
                .collect(),
        ));
    }
    b.push(note(x(c, "not_examined_note")));
    b
}

fn annex_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let t = &r.totals;
    let mut b = vec![
        para(x(c, "intro.annexes")),
        kv(vec![
            (x(c, "labels.runs_found"), t.runs_found.to_string()),
            (x(c, "labels.runs_verified"), t.runs_verified.to_string()),
            (x(c, "labels.runs_excluded"), t.runs_excluded.to_string()),
            (x(c, "labels.runs_skipped"), t.runs_skipped.to_string()),
            (
                x(c, "labels.findings_quoted"),
                t.findings_quoted.to_string(),
            ),
        ]),
    ];
    for a in &r.annexes.runs {
        let sev = match a.status.as_str() {
            "excluded" => "high",
            "skipped" => "low",
            _ => "info",
        };
        let mut rows = vec![(x(c, "labels.run_dir"), a.run_dir.clone())];
        if !a.reason.is_empty() {
            let group = if a.status == "excluded" {
                "exclusion"
            } else {
                "skip"
            };
            rows.push((
                x(c, "labels.reason"),
                format!(
                    "{}{}",
                    c.text(&format!("{group}.{}", a.reason)),
                    if a.detail.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", a.detail)
                    }
                ),
            ));
        }
        rows.push((x(c, "labels.started"), or_dash(&a.started_utc)));
        rows.push((x(c, "labels.finished"), or_dash(&a.finished_utc)));
        if a.status != "excluded" && a.audit_entries > 0 {
            rows.push((
                x(c, "labels.audit_head"),
                format!("#{} {}", a.audit_entries, a.audit_head),
            ));
            rows.push((
                x(c, "labels.outputs_verified"),
                xf(
                    c,
                    "outputs_of",
                    &[
                        ("verified", a.outputs_verified.to_string()),
                        ("recorded", a.outputs_recorded.to_string()),
                    ],
                ),
            ));
        }
        rows.push((
            x(c, "labels.timestamp"),
            x(c, &format!("labels.timestamp_{}", a.timestamp_status)),
        ));
        for line in &a.timestamps {
            rows.push((String::new(), line.clone()));
        }
        rows.push((
            x(c, "labels.findings_quoted"),
            a.findings_quoted.to_string(),
        ));
        b.push(boxed(
            x(c, &format!("labels.run_{}", a.status)),
            sev,
            format!("{} {} · {}", a.tool, a.tool_version, a.run_id),
            "",
            rows,
        ));
    }
    b.push(sub(x(c, "labels.report_signals")));
    let run_level: Vec<&ReportSignal> = r
        .signals
        .iter()
        .filter(|s| s.evidence_id.is_none() && !s.code.starts_with("TIMELINE_"))
        .collect();
    if run_level.is_empty() {
        b.push(para(x(c, "labels.none")));
    }
    for s in run_level {
        b.push(signal_box(c, s));
    }
    b
}

fn declaration_blocks(c: &Catalog, r: &ExpertReport) -> Vec<Value> {
    let code = &r.jurisdiction.code;
    let mut b = vec![
        para(x(c, &format!("declaration_{code}"))),
        bullets(vec![
            x(c, &format!("examiner_adds_{code}.qualifications")),
            x(c, &format!("examiner_adds_{code}.conclusions")),
            x(c, &format!("examiner_adds_{code}.statement")),
        ]),
    ];
    b.push(if r.run.status == "final" {
        note(x(c, "final_note"))
    } else {
        note(x(c, "draft_note"))
    });
    b.push(json!({"kind": "sign", "labels": [
        format!("{}: {}", x(c, "labels.examiner"), r.identification.examiner.name),
        x(c, "labels.date_place"),
        x(c, "labels.signature"),
    ]}));
    b
}

/// The view model both formats render (see the module documentation).
pub fn expert_view(r: &ExpertReport, lang: &str) -> Value {
    let c = catalog(lang);
    let code = r.jurisdiction.code.as_str();
    let mut sections = Vec::new();
    for key in &r.jurisdiction.sections {
        let blocks: Vec<Value> = match key.as_str() {
            "identification" => identification_blocks(c, r),
            "mission" => {
                let mut b = identification_blocks(c, r);
                b.push(sub(x(c, "labels.mission")));
                b.extend(object_blocks(c, r));
                b
            }
            "object" => object_blocks(c, r),
            "evidence" => evidence_blocks(c, r),
            "method" => method_blocks(c, r),
            "results" => results_blocks(c, r),
            "timeline" => timeline_blocks(c, r),
            "limitations" => limitations_blocks(c, r),
            "not_examined" => not_examined_blocks(c, r),
            "annexes" => annex_blocks(c, r),
            "declaration" => declaration_blocks(c, r),
            _ => vec![],
        };
        sections.push(json!({
            "key": key,
            "title": x(c, &format!("sections_{code}.{key}")),
            "basis": basis(c, r, key),
            "blocks": blocks,
        }));
    }
    let draft = r.run.status != "final";
    json!({
        "lang": c.lang,
        "css": CSS,
        "logo_svg": LOGO_SVG,
        "brand": c.brand().clone(),
        "title": x(c, &format!("title_{code}")),
        "subtitle": xf(c, "subtitle", &[("case", r.identification.case_id.clone()), ("run", or_dash(&r.run.run_id)), ("date", r.run.generated_utc.clone())]),
        "banner": if draft { x(c, "draft_banner") } else { x(c, "final_banner") },
        "draft": draft,
        "legal_basis": x(c, "labels.legal_basis"),
        "disclaimer": x(c, "disclaimer"),
        "sections": sections,
        "footer": format!("{} {} {} · {}", c.label("generated_by"), r.tool.name, r.tool.version, or_dash(&r.run.run_id)),
        "signature": match &r.run.pdf_signed_by {
            Some(s) => format!("{} {} ({} {})", c.label("signed_by"), s, c.label("certificate_sha256"), r.run.pdf_signer_certificate_sha256.clone().unwrap_or_default()),
            None => c.label("not_signed"),
        },
    })
}

pub fn expert_html(r: &ExpertReport, lang: &str) -> Result<String, String> {
    let mut env = minijinja::Environment::new();
    env.add_template("expert-report.html", TEMPLATE)
        .map_err(|e| format!("template expert-report.html: {e}"))?;
    env.get_template("expert-report.html")
        .and_then(|t| t.render(minijinja::value::Serde(&expert_view(r, lang))))
        .map_err(|e| format!("template expert-report.html: {e}"))
}

fn sv(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn rows_of(v: &Value) -> Vec<(String, String)> {
    v["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            (
                r[0].as_str().unwrap_or_default().to_string(),
                r[1].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The PDF blocks of the view (same order and content as the HTML).
pub fn expert_blocks(view: &Value) -> Vec<Block> {
    let mut b = vec![
        Block::Title(sv(view, "title")),
        Block::Subtitle(sv(view, "subtitle")),
        Block::Status(sv(view, "banner")),
        Block::Note(sv(view, "disclaimer")),
    ];
    for s in view["sections"].as_array().into_iter().flatten() {
        b.push(Block::Heading(sv(s, "title")));
        for line in s["basis"].as_array().into_iter().flatten() {
            b.push(Block::Mono(format!(
                "{}: {}",
                sv(view, "legal_basis"),
                line.as_str().unwrap_or_default()
            )));
        }
        for blk in s["blocks"].as_array().into_iter().flatten() {
            match blk["kind"].as_str().unwrap_or_default() {
                "para" => b.push(Block::Para(sv(blk, "text"))),
                "note" => b.push(Block::Note(sv(blk, "text"))),
                "sub" => {
                    b.push(Block::Space(4.0));
                    b.push(Block::Note(sv(blk, "text")));
                }
                "quote" => b.push(Block::Bullet(sv(blk, "text"))),
                "source" => b.push(Block::Mono(sv(blk, "text"))),
                "kv" => b.push(Block::Kv(rows_of(blk))),
                "bullets" => {
                    for i in blk["items"].as_array().into_iter().flatten() {
                        b.push(Block::Bullet(i.as_str().unwrap_or_default().to_string()));
                    }
                }
                "box" => {
                    let code = sv(blk, "code");
                    b.push(Block::Finding {
                        badge: sv(blk, "badge"),
                        title: if code.is_empty() {
                            sv(blk, "title")
                        } else {
                            format!("{} ({})", sv(blk, "title"), code)
                        },
                        rows: rows_of(blk),
                    })
                }
                "sign" => b.push(Block::SignLines(
                    blk["labels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|l| l.as_str().unwrap_or_default().to_string())
                        .collect(),
                )),
                _ => {}
            }
        }
    }
    b.push(Block::Space(6.0));
    b.push(Block::Para(sv(view, "signature")));
    b
}

/// The PDF of one language, signed when `signer` is given.
pub fn expert_pdf(
    r: &ExpertReport,
    lang: &str,
    signer: Option<&crate::trust::PdfSigner>,
) -> Result<Vec<u8>, String> {
    let view = expert_view(r, lang);
    let c = catalog(lang);
    let blocks = expert_blocks(&view);
    let title = sv(&view, "title");
    let footer = format!("{} · {}", sv(&view, "footer"), sv(&view, "banner"));
    let producer = format!("{} {}", r.tool.name, r.tool.version);
    let (page, of) = (c.label("page"), c.label("of"));
    let (descriptor, tagline) = (
        c.brand()["descriptor"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        c.brand()["tagline"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    let meta = pdf::Meta {
        title: &title,
        footer: &footer,
        page_word: &page,
        of_word: &of,
        producer: &producer,
        descriptor: &descriptor,
        tagline: &tagline,
    };
    match signer {
        Some(s) => {
            let f = |data: &[u8]| s.sign_detached(data);
            pdf::render(&blocks, &meta, Some(&f))
        }
        None => pdf::render(&blocks, &meta, None),
    }
}
