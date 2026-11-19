//! Run discovery, verification and verified reading — adapted from RV Timeline (`readers/mod.rs`), the suite's
//! reference for "verify before read".
//!
//! ```text
//! case.json + case-log.jsonl ──► discover_runs ──► RunRef × n (announced runs, then unannounced folders)
//! RunRef ──► verify_run:  run_dir safe & inside the case ─► audit chain ─► run_finished (audit, not cancelled)
//!                         ─► case log: announced, completed, audit head anchored, nothing but timestamps after
//!                         ─► version agrees ─► verify-log (tokens, truncation, EVERY recorded output re-hashed)
//! verified RunRef ──► RunData::json / text (only files the audit log records, digest re-checked when read)
//! ```
//!
//! Exclusion reasons: `unreadable`, `chain_broken`, `not_finished`, `digest_mismatch`, `timestamp_invalid`
//! (i18n `exclusion.<reason>`). A run that fails is never read by the assembler.

#![forbid(unsafe_code)]

use crate::audit::{GENESIS, entry_hash};
use crate::hash::{HashAlgo, hash_bytes};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// The case-folder schema this version reads.
pub const CASE_SCHEMA: &str = "rakvor.case/1";

/// Error for a run that cannot be used: `reason` is an i18n key (`exclusion.<reason>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub reason: String,
    pub detail: String,
}

pub(crate) fn err(reason: &str, detail: impl Into<String>) -> ReadError {
    ReadError {
        reason: reason.into(),
        detail: detail.into(),
    }
}

/// One tool run found in the case folder.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunRef {
    pub tool: String,
    pub tool_version: String,
    pub run_id: String,
    /// `runs/<tool>/<run-id>`, relative to the case folder.
    pub run_dir: String,
    /// Role recorded in the case log's `run_started` (`intake`, `analysis`; empty when not announced).
    pub role: String,
    /// Case-log `seq` and `ts_utc` of its `run_started` / `run_finished` (0 / empty when absent).
    pub started_seq: u64,
    pub started_utc: String,
    pub finished_seq: u64,
    pub finished_utc: String,
    pub actor: String,
    /// `timestamp_status` the case log recorded at `run_finished`.
    pub timestamp_status: String,
}

/// What verification found about one run.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Verified {
    pub audit_entries: u64,
    pub audit_head: String,
    pub outputs_recorded: u64,
    pub outputs_verified: u64,
    /// verify-log's lines, one per timestamp token.
    pub timestamps: Vec<String>,
}

fn read_json_file(p: &Path) -> Result<Value, String> {
    let t = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&t).map_err(|e| format!("{}: not valid JSON ({e})", p.display()))
}

/// A run folder path from the case log, accepted only when it is relative, below `runs/` and free of `..`.
pub fn safe_run_dir(rel: &str) -> bool {
    let p = Path::new(rel);
    rel.starts_with("runs/")
        && !rel.contains('\\')
        && p.components().all(|c| matches!(c, Component::Normal(_)))
        && p.components().count() == 3
}

/// An output path recorded in an audit log, accepted only when it is relative, uses forward slashes, has no `..`,
/// root, drive or empty component, and no `:` (drive letters, NTFS alternate streams).
pub fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains(['\\', ':'])
        && rel
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// `path` resolved (symlinks followed) when it stays inside `root` (already canonical).
fn contained(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if real.starts_with(root) {
        Ok(real)
    } else {
        Err(format!(
            "{}: resolves outside the case folder (symlink or junction), not read",
            path.display()
        ))
    }
}

/// Entries of a JSON Lines file with their 1-based line numbers.
pub type Entries = Vec<(usize, Map<String, Value>)>;

/// JSON Lines with their 1-based line numbers. Blank lines are skipped; a line that is not a JSON object is an error.
pub fn read_jsonl(p: &Path) -> Result<(Entries, Vec<u8>), String> {
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("{}: not UTF-8", p.display()))?;
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(l) {
            Ok(Value::Object(m)) => out.push((i + 1, m)),
            _ => return Err(format!("line {} is not a JSON object", i + 1)),
        }
    }
    Ok((out, bytes))
}

/// Hash chain of an audit.jsonl or case-log.jsonl (`seq` 1, 2, …; `prev_entry_hash`; `entry_hash` over the
/// canonical JSON without `entry_hash`).
pub fn check_chain(entries: &[(usize, Map<String, Value>)]) -> Result<(), String> {
    let mut prev = GENESIS.to_string();
    for (n, (line, e)) in entries.iter().enumerate() {
        if e.get("seq").and_then(Value::as_u64) != Some(n as u64 + 1) {
            return Err(format!("line {line}: seq is not {}", n + 1));
        }
        if e.get("prev_entry_hash").and_then(Value::as_str) != Some(prev.as_str()) {
            return Err(format!(
                "line {line}: prev_entry_hash does not match the entry before"
            ));
        }
        let h = e
            .get("entry_hash")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if entry_hash(e) != h {
            return Err(format!(
                "line {line}: entry_hash does not match its content"
            ));
        }
        prev = h.to_string();
    }
    Ok(())
}

/// A field of an entry, `Null` when absent (indexing a `Map` with `[]` panics on a missing key).
pub(crate) fn at<'a>(e: &'a Map<String, Value>, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    e.get(key).unwrap_or(&NULL)
}

pub(crate) fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn action(e: &Map<String, Value>) -> &str {
    e.get("action").and_then(Value::as_str).unwrap_or_default()
}

/// Lists every run recorded in the case folder: runs announced by `run_started` in case-log.jsonl (in log order),
/// then run folders under runs/<tool>/<run>/ the log does not announce. Err when it is not a case folder.
pub fn discover_runs(case_dir: &Path) -> Result<Vec<RunRef>, ReadError> {
    let case = read_json_file(&case_dir.join("case.json")).map_err(|e| err("unreadable", e))?;
    if case["schema"] != CASE_SCHEMA {
        return Err(err(
            "unreadable",
            format!("case.json: schema {} is not {CASE_SCHEMA}", case["schema"]),
        ));
    }
    let mut runs: Vec<RunRef> = Vec::new();
    if let Ok((log, _)) = read_jsonl(&case_dir.join("case-log.jsonl")) {
        for (_, e) in &log {
            let d = at(e, "details");
            let run_dir = s(&d["run_dir"]).to_string();
            match action(e) {
                "run_started" => {
                    if runs.iter().any(|r| r.run_dir == run_dir) {
                        continue;
                    }
                    runs.push(RunRef {
                        tool: s(&d["tool"]).to_string(),
                        tool_version: s(&d["version"]).to_string(),
                        run_id: s(at(e, "run_id")).to_string(),
                        run_dir,
                        role: s(&d["role"]).to_string(),
                        started_seq: at(e, "seq").as_u64().unwrap_or(0),
                        started_utc: s(at(e, "ts_utc")).to_string(),
                        actor: s(at(e, "actor")).to_string(),
                        ..Default::default()
                    });
                }
                "run_finished" => {
                    if let Some(r) = runs.iter_mut().find(|r| r.run_dir == run_dir) {
                        r.finished_seq = at(e, "seq").as_u64().unwrap_or(0);
                        r.finished_utc = s(at(e, "ts_utc")).to_string();
                        r.timestamp_status = s(&d["timestamp_status"]).to_string();
                    }
                }
                _ => {}
            }
        }
    }
    let mut extra = Vec::new();
    for t in std::fs::read_dir(case_dir.join("runs"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if !t.path().is_dir() {
            continue;
        }
        let tool = t.file_name().to_string_lossy().into_owned();
        for r in std::fs::read_dir(t.path()).into_iter().flatten().flatten() {
            if !r.path().is_dir() {
                continue;
            }
            let run_id = r.file_name().to_string_lossy().into_owned();
            let run_dir = format!("runs/{tool}/{run_id}");
            if runs.iter().any(|x| x.run_dir == run_dir) {
                continue;
            }
            let version = read_jsonl(&r.path().join("logs/audit.jsonl"))
                .ok()
                .and_then(|(l, _)| {
                    l.into_iter()
                        .find(|(_, e)| action(e) == "run_started")
                        .map(|(_, e)| s(&at(&e, "details")["version"]).to_string())
                })
                .unwrap_or_default();
            extra.push(RunRef {
                tool: tool.clone(),
                tool_version: version,
                run_id,
                run_dir,
                ..Default::default()
            });
        }
    }
    extra.sort_by(|a, b| a.run_dir.cmp(&b.run_dir));
    runs.extend(extra);
    Ok(runs)
}

/// Verifies one run (see the module documentation). Ok = the run may be read.
pub fn verify_run(case_dir: &Path, run: &RunRef) -> Result<Verified, ReadError> {
    let data = RunData::load(case_dir, run)?;
    let fin = data
        .audit
        .iter()
        .rev()
        .find(|(_, e)| action(e) == "run_finished")
        .ok_or_else(|| err("not_finished", "logs/audit.jsonl has no run_finished entry"))?;
    if at(&fin.1, "details")["cancelled"] == Value::Bool(true) {
        return Err(err("not_finished", "the run was cancelled"));
    }
    // The case log: intact, announces the run as completed, and holds the run's final audit head.
    let (log, _) = read_jsonl(&case_dir.join("case-log.jsonl"))
        .map_err(|e| err("unreadable", format!("case-log.jsonl: {e}")))?;
    check_chain(&log).map_err(|e| err("chain_broken", format!("case-log.jsonl {e}")))?;
    let announced = log
        .iter()
        .any(|(_, e)| action(e) == "run_started" && s(&at(e, "details")["run_dir"]) == run.run_dir);
    if !announced {
        return Err(err(
            "not_finished",
            "the case log does not announce this run folder (run_started missing)",
        ));
    }
    let finished = log
        .iter()
        .rev()
        .find(|(_, e)| {
            action(e) == "run_finished" && s(&at(e, "details")["run_dir"]) == run.run_dir
        })
        .ok_or_else(|| {
            err(
                "not_finished",
                "the case log has no run_finished for this run",
            )
        })?;
    let d = at(&finished.1, "details");
    let exit_failed = d["exit_code"].as_i64().is_some_and(|c| c != 0);
    if s(&d["status"]) != "completed" || exit_failed {
        return Err(err(
            "not_finished",
            format!("case log status: {}", d["status"].as_str().unwrap_or("?")),
        ));
    }
    let n = d["audit_entries"].as_u64().unwrap_or(0) as usize;
    let head = s(&d["audit_head"]).to_string();
    match data.audit.get(n.wrapping_sub(1)) {
        Some((_, e)) if n > 0 && e.get("entry_hash").and_then(Value::as_str) == Some(&head) => {}
        _ => {
            return Err(err(
                "chain_broken",
                format!(
                    "the audit head recorded in the case log is not entry #{n} of logs/audit.jsonl"
                ),
            ));
        }
    }
    if let Some((line, e)) = data.audit[n..]
        .iter()
        .find(|(_, e)| !action(e).starts_with("timestamp_"))
    {
        return Err(err(
            "chain_broken",
            format!(
                "logs/audit.jsonl line {line}: {} after the recorded end of the run",
                action(e)
            ),
        ));
    }
    let started = data.audit.iter().find(|(_, e)| action(e) == "run_started");
    if let Some((_, e)) = started
        && !run.tool_version.is_empty()
        && s(&at(e, "details")["version"]) != run.tool_version
    {
        return Err(err(
            "unreadable",
            format!(
                "case log says version {}, the run's audit log says {}",
                run.tool_version,
                s(&at(e, "details")["version"])
            ),
        ));
    }
    // The suite's full verify-log: chain, timestamp tokens (imprint + TSA signature), truncation against the case
    // log, and every recorded output re-hashed (shared outputs.rs).
    let audit_path = data.dir.join("logs").join("audit.jsonl");
    let v = crate::verify::verify_log(&audit_path).map_err(|e| err("unreadable", e))?;
    let outputs = v.outputs.clone().unwrap_or_default();
    if !outputs.problems.is_empty() {
        return Err(err(
            "digest_mismatch",
            outputs
                .problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    if !v.problems.is_empty() {
        let joined = v.problems.join("; ");
        let reason = if joined.contains("timestamp") || joined.contains("token") {
            "timestamp_invalid"
        } else {
            "chain_broken"
        };
        return Err(err(reason, joined));
    }
    Ok(Verified {
        audit_entries: n as u64,
        audit_head: head,
        outputs_recorded: outputs.recorded as u64,
        outputs_verified: outputs.verified as u64,
        timestamps: v.timestamps,
    })
}

/// An evidence item of a run, from its audit log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunItem {
    pub index: u64,
    pub path: String,
    pub evidence_id: String,
    /// `evidence_verified_against_case.match` (None when the run did not compare).
    pub registry_match: Option<bool>,
}

/// A run loaded for reading: its audit entries, the digests the audit log recorded per output, its items.
pub struct RunData {
    pub run: RunRef,
    pub dir: PathBuf,
    /// The run folder with symlinks resolved; every file read must stay below it.
    pub root: PathBuf,
    pub audit: Entries,
    pub audit_sha256: String,
    /// Output path relative to the run folder → {algorithm: hex}.
    pub digests: BTreeMap<String, BTreeMap<String, String>>,
    pub items: BTreeMap<u64, RunItem>,
}

fn digest_map(v: &Value) -> BTreeMap<String, String> {
    let o = v
        .get("hashes")
        .or_else(|| v.get("digest"))
        .filter(|x| x.is_object())
        .unwrap_or(v);
    o.as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| HashAlgo::parse(k).is_some())
                .filter_map(|(k, x)| x.as_str().map(|h| (k.clone(), h.to_ascii_lowercase())))
                .collect()
        })
        .unwrap_or_default()
}

/// The output files an audit entry records, with their digests (paths relative to the run folder).
fn recorded_outputs(e: &Map<String, Value>) -> Vec<(String, BTreeMap<String, String>)> {
    let d = e.get("details").cloned().unwrap_or(Value::Null);
    let item = e.get("item").and_then(Value::as_str).unwrap_or_default();
    let mut out = Vec::new();
    let mut files = |base: &str, files: &Value| {
        for (name, h) in files.as_object().into_iter().flatten() {
            out.push((format!("{base}{name}"), digest_map(h)));
        }
    };
    match action(e) {
        "item_reports_written" => files(&format!("reports/{}/", s(&d["folder"])), &d["files"]),
        "statistics_report_written" => files("reports/", &d["files"]),
        "timeline_written" => files(&format!("{}/", s(&d["folder"])), &d["files"]),
        "output_written" if !s(&d["file"]).is_empty() => {
            out.push((format!("reports/{}", s(&d["file"])), digest_map(&d)))
        }
        "environment_recorded" => out.push(("logs/environment.json".into(), digest_map(&d))),
        "hash_register_written" => out.push(("logs/hashes.csv".into(), digest_map(&d))),
        "trace_written" | "rsmf_written" if !item.is_empty() => {
            out.push((format!("rsmf/{item}"), digest_map(&d)))
        }
        _ => {}
    }
    out.retain(|(p, h)| !h.is_empty() && safe_rel(p));
    out
}

impl RunData {
    /// Reads the run's audit log and checks its chain (`unreadable`, `chain_broken`).
    pub fn load(case_dir: &Path, run: &RunRef) -> Result<RunData, ReadError> {
        if !safe_run_dir(&run.run_dir) {
            return Err(err(
                "unreadable",
                format!("run folder {:?} is not runs/<tool>/<run>", run.run_dir),
            ));
        }
        let dir = case_dir.join(&run.run_dir);
        let case_root = std::fs::canonicalize(case_dir)
            .map_err(|e| err("unreadable", format!("{}: {e}", case_dir.display())))?;
        let root = contained(&case_root, &dir).map_err(|e| err("unreadable", e))?;
        contained(&root, &dir.join("logs/audit.jsonl")).map_err(|e| err("unreadable", e))?;
        let (audit, bytes) = read_jsonl(&dir.join("logs/audit.jsonl")).map_err(|e| {
            if dir.join("logs/audit.jsonl").is_file() {
                err("chain_broken", format!("logs/audit.jsonl {e}"))
            } else {
                err("unreadable", e)
            }
        })?;
        if audit.is_empty() {
            return Err(err("unreadable", "logs/audit.jsonl is empty"));
        }
        check_chain(&audit).map_err(|e| err("chain_broken", format!("logs/audit.jsonl {e}")))?;
        let mut digests = BTreeMap::new();
        let mut items: BTreeMap<u64, RunItem> = BTreeMap::new();
        for (_, e) in &audit {
            for (p, h) in recorded_outputs(e) {
                digests.insert(p, h);
            }
            let d = at(e, "details");
            let Some(index) = d["index"].as_u64() else {
                continue;
            };
            let item = e.get("item").and_then(Value::as_str).unwrap_or_default();
            match action(e) {
                "item_started" | "item_hashed" => {
                    let it = items.entry(index).or_default();
                    it.index = index;
                    if it.path.is_empty() {
                        it.path = item.to_string();
                    }
                }
                "evidence_registered" => {
                    let it = items.entry(index).or_default();
                    it.index = index;
                    it.evidence_id = s(&d["evidence_id"]).to_string();
                }
                "evidence_verified_against_case" => {
                    let it = items.entry(index).or_default();
                    it.index = index;
                    it.evidence_id = s(&d["evidence_id"]).to_string();
                    it.registry_match = d["match"].as_bool();
                }
                _ => {}
            }
        }
        Ok(RunData {
            run: run.clone(),
            dir,
            root,
            audit,
            audit_sha256: hash_bytes(&bytes, HashAlgo::Sha256),
            digests,
            items,
        })
    }

    /// Checks one recorded output against the audit log; returns its bytes and SHA-256 (`unreadable` when it is
    /// missing or was never recorded, `digest_mismatch` when it changed).
    pub fn check_file(&self, rel: &str) -> Result<(Vec<u8>, String), ReadError> {
        let rec = self.digests.get(rel).ok_or_else(|| {
            err(
                "unreadable",
                format!("{rel}: not recorded in logs/audit.jsonl"),
            )
        })?;
        if !safe_rel(rel) {
            return Err(err(
                "unreadable",
                format!("{rel}: not a path inside the run folder"),
            ));
        }
        let path = contained(&self.root, &self.dir.join(rel)).map_err(|_| {
            err(
                "unreadable",
                format!("{rel}: resolves outside the run folder (symlink), not read"),
            )
        })?;
        let bytes = std::fs::read(path).map_err(|e| err("unreadable", format!("{rel}: {e}")))?;
        let mut checked = false;
        for (algo, want) in rec {
            if let Some(a) = HashAlgo::parse(algo) {
                if hash_bytes(&bytes, a) != *want {
                    return Err(err(
                        "digest_mismatch",
                        format!("{rel}: {algo} differs from logs/audit.jsonl"),
                    ));
                }
                checked = true;
            }
        }
        if !checked {
            return Err(err(
                "unreadable",
                format!("{rel}: no usable digest recorded"),
            ));
        }
        let sha = rec
            .get("sha256")
            .cloned()
            .unwrap_or_else(|| hash_bytes(&bytes, HashAlgo::Sha256));
        Ok((bytes, sha))
    }

    /// A recorded JSON output, digest checked. `Ok(None)` when the run did not record that file.
    pub fn json(&self, rel: &str) -> Result<Option<(Value, String)>, ReadError> {
        if !self.digests.contains_key(rel) {
            return Ok(None);
        }
        let (bytes, sha) = self.check_file(rel)?;
        let v = serde_json::from_slice(&bytes)
            .map_err(|e| err("unreadable", format!("{rel}: not valid JSON ({e})")))?;
        Ok(Some((v, sha)))
    }

    /// A recorded text output (UTF-8), digest checked. `Ok(None)` when not recorded.
    pub fn text(&self, rel: &str) -> Result<Option<(String, String)>, ReadError> {
        if !self.digests.contains_key(rel) {
            return Ok(None);
        }
        let (bytes, sha) = self.check_file(rel)?;
        let t =
            String::from_utf8(bytes).map_err(|_| err("unreadable", format!("{rel}: not UTF-8")))?;
        Ok(Some((t, sha)))
    }

    /// Recorded outputs whose path ends with `suffix`, in path order.
    pub fn outputs_ending(&self, suffix: &str) -> Vec<String> {
        self.digests
            .keys()
            .filter(|k| k.ends_with(suffix))
            .cloned()
            .collect()
    }

    /// The run's `run_started` details (algorithms, langs, frontend, version…).
    pub fn started_details(&self) -> Value {
        self.audit
            .iter()
            .find(|(_, e)| action(e) == "run_started")
            .map(|(_, e)| at(e, "details").clone())
            .unwrap_or(Value::Null)
    }

    /// `ts_utc` of the run's own `run_started` / `run_finished` audit entries.
    pub fn audit_times(&self) -> (String, String) {
        let ts = |a: &str| {
            self.audit
                .iter()
                .rev()
                .find(|(_, e)| action(e) == a)
                .map(|(_, e)| s(at(e, "ts_utc")).to_string())
                .unwrap_or_default()
        };
        (ts("run_started"), ts("run_finished"))
    }
}

/// Index of an item report folder name `NNNNN-name`: the digits before the first `-` (at least five).
pub fn folder_index(folder: &str) -> Option<u64> {
    let (digits, _) = folder.split_once('-')?;
    if digits.len() < 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_index_reads_every_digit() {
        assert_eq!(folder_index("00003-photo.jpg"), Some(3));
        assert_eq!(folder_index("100000-x"), Some(100_000));
        assert_eq!(folder_index("0003-x"), None);
        assert_eq!(folder_index("photo.jpg"), None);
    }

    #[test]
    fn run_dirs_must_stay_in_runs() {
        assert!(safe_run_dir("runs/rv-imgauth/20261110T100000Z-ab12cd"));
        for bad in [
            "runs/../x/y",
            "/runs/a/b",
            "runs/a",
            "runs/a/b/c",
            "C:/runs/a/b",
            "runs\\a\\b",
            "logs/a/b",
        ] {
            assert!(!safe_run_dir(bad), "{bad}");
        }
    }

    #[test]
    fn recorded_outputs_must_stay_in_the_run_folder() {
        assert!(safe_rel("reports/items/00001-a.jpg/technical.json"));
        for bad in [
            "",
            "../x.json",
            "reports/../../x.json",
            "reports//x.json",
            "/etc/passwd",
            "C:/Windows/win.ini",
            "reports/x.json:stream",
        ] {
            assert!(!safe_rel(bad), "{bad}");
        }
        let e = Map::from_iter([
            ("action".to_string(), Value::from("output_written")),
            (
                "details".to_string(),
                serde_json::json!({"file": "..\\..\\outside\\x.json", "sha256": "00"}),
            ),
        ]);
        assert!(recorded_outputs(&e).is_empty());
    }

    #[test]
    fn chain_check_catches_an_edit() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = crate::audit::AuditLog::create(dir.path(), "RUN-1", "t").unwrap();
        l.record("a", None, serde_json::json!({"i": 1})).unwrap();
        l.record("b", None, serde_json::json!({"i": 2})).unwrap();
        let p = dir.path().join("audit.jsonl");
        let (e, _) = read_jsonl(&p).unwrap();
        assert!(check_chain(&e).is_ok());
        let t = std::fs::read_to_string(&p)
            .unwrap()
            .replace("\"i\":2", "\"i\":3");
        std::fs::write(&p, t).unwrap();
        let (e, _) = read_jsonl(&p).unwrap();
        assert!(check_chain(&e).unwrap_err().contains("entry_hash"));
    }
}
