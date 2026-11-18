//! End-to-end tests of the rv-report binary: the contract every Rakvor CLI must meet, on copies of the sample case
//! (examples/sample/case) and on copies tampered at test time (one run broken in one way each).

use assert_cmd::Command;
use predicates::prelude::*;
use rv_report_core::HashAlgo;
use serde_json::Value;
use std::path::{Path, PathBuf};

const TOOL: &str = rv_report_core::TOOL_NAME;

fn bin() -> Command {
    let mut c = Command::cargo_bin("rv-report").unwrap();
    c.env("RV_NO_TIMESTAMP", "1"); // offline by default; the trust test uses a local test TSA
    c
}

fn py() -> String {
    std::env::var("RV_TEST_PYTHON").unwrap_or_else(|_| "python3".into())
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The fake TSA process, killed when dropped, so a failing assertion cannot leave it running.
struct TsaServer(std::process::Child);

impl Drop for TsaServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts tests/fake_tsa.py (needs Python with pyhanko). Returns None to skip when unavailable.
fn test_pki(dir: &Path) -> Option<(TsaServer, String)> {
    let script = root().join("tests/fake_tsa.py");
    let ok = std::process::Command::new(py())
        .args(["-c", "import pyhanko"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skipping trust test: {} has no pyhanko", py());
        return None;
    }
    let mut server = TsaServer(
        std::process::Command::new(py())
            .arg(&script)
            .arg("serve")
            .arg(dir)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?,
    );
    let mut line = String::new();
    use std::io::BufRead;
    std::io::BufReader::new(server.0.stdout.take()?)
        .read_line(&mut line)
        .ok()?;
    Some((server, line.trim().to_string()))
}

fn has_python_module(m: &str) -> bool {
    std::process::Command::new(py())
        .args(["-c", &format!("import {m}")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn only_dir(p: &Path) -> PathBuf {
    std::fs::read_dir(p)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}

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

/// A fresh copy of examples/sample/case, so tests never write into the repository.
fn case_copy(dest: &Path) -> PathBuf {
    let case = dest.join("CASE");
    copy_dir(&root().join("examples/sample/case"), &case);
    case
}

/// The run folder of `tool` in the case (exactly one).
fn tool_run(case: &Path, tool: &str) -> PathBuf {
    only_dir(&case.join("runs").join(tool))
}

fn assemble(case: &Path) -> Command {
    let mut c = bin();
    c.args(["assemble", "--progress", "none", "--case-dir"])
        .arg(case);
    c
}

/// The run folder printed on the "logs:" line of the summary.
fn run_folder(stdout: &[u8]) -> PathBuf {
    let text = String::from_utf8_lossy(stdout);
    let logs = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("logs:"))
        .expect("logs line")
        .trim();
    PathBuf::from(logs).parent().unwrap().to_path_buf()
}

fn audit(run: &Path) -> Vec<Value> {
    std::fs::read_to_string(run.join("logs/audit.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Every file of the case outside rv-report's own run folders, with its content (the case log aside).
fn case_files(case: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut all = Vec::new();
    walk(case, &mut all);
    let own = case.join("runs").join(TOOL);
    all.into_iter()
        .filter(|p| !p.starts_with(&own) && !p.ends_with("case-log.jsonl"))
        .map(|p| {
            (
                p.strip_prefix(case).unwrap().to_path_buf(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect()
}

/// Every `reports/items/*/technical.json` of a run, with its findings.
fn technical_reports(run: &Path) -> Vec<(PathBuf, Value)> {
    let items = run.join("reports/items");
    let mut out: Vec<(PathBuf, Value)> = std::fs::read_dir(&items)
        .unwrap()
        .map(|e| e.unwrap().path().join("technical.json"))
        .filter(|p| p.is_file())
        .map(|p| {
            let v = json(&p);
            (p, v)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn minijinja_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
        .replace('/', "&#x2f;")
}

#[test]
fn help_has_every_required_section() {
    let out = bin().args(["assemble", "--help"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for section in [
        "DESCRIPTION:",
        "Options:",
        "EXAMPLES:",
        "EXIT CODES:",
        "FILES WRITTEN:",
        "FORENSIC NOTES:",
        "expert-report.json",
    ] {
        assert!(text.contains(section), "missing {}", section);
    }
    let flags = [
        "--case-dir",
        "--jurisdiction",
        "--lang",
        "--actor",
        "--examiner-id",
        "--mandate",
        "--include-tool",
        "--exclude-tool",
        "--final",
        "--hash",
        "--out",
        "--case",
        "--progress",
        "--tsa-url",
        "--no-timestamp",
        "--sign-cert",
        "--sign-pass-env",
    ];
    for flag in flags {
        assert!(text.contains(flag), "missing {}", flag);
    }
    assert!(text.matches("Default:").count() >= flags.len());
    assert!(text.matches("Example:").count() >= flags.len());
    for gone in ["--input", "--recursive", "--evidence "] {
        assert!(!text.contains(gone), "{} still documented", gone);
    }
    bin().args(["scan", "--help"]).assert().code(2);
}

#[test]
fn bad_options_are_usage_errors_and_write_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let case = case_copy(tmp.path());
    let not_utf8 = tmp.path().join("mandate.bin");
    std::fs::write(&not_utf8, [0xff, 0xfe, 0x00, 0x41]).unwrap();
    let big = tmp.path().join("big.txt");
    std::fs::write(&big, vec![b'a'; 70_000]).unwrap();
    for (args, msg) in [
        (vec!["--hash", "md5,sha1,sha256"], "at most 2".to_string()),
        (vec!["--hash", "crc32"], "unknown hash algorithm".into()),
        (vec!["--lang", "de"], "unsupported language".into()),
        (vec!["--jurisdiction", "us"], "invalid value".into()),
        (
            vec!["--include-tool", "pdfauth"],
            "a Rakvor tool name".into(),
        ),
        (vec!["--exclude-tool", "RV-X"], "a Rakvor tool name".into()),
        (vec!["--case", "OTHER-CASE"], "RV-REPORT-SAMPLE".into()),
        (
            vec!["--mandate", "missing-mandate.txt"],
            "mandate file".into(),
        ),
        (
            vec!["--mandate", not_utf8.to_str().unwrap()],
            "not UTF-8".into(),
        ),
        (vec!["--mandate", big.to_str().unwrap()], "at most".into()),
    ] {
        assemble(&case)
            .args(&args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(msg));
    }
    assert!(!case.join("runs").join(TOOL).exists());
    let empty = tmp.path().join("not-a-case");
    std::fs::create_dir_all(&empty).unwrap();
    assemble(&empty)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("is not a case folder"));
    assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);
    bin()
        .args(["assemble", "--progress", "none"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--case-dir"));
}

/// The end-to-end contract on the sample case: every run verified, every finding of every verified run quoted
/// verbatim, the JSON valid against its schema, the audit chain verifiable by the tool and by the independent
/// Python verifier, nothing outside the tool's own run folder changed.
#[test]
fn sample_case_assembles_a_report_that_quotes_every_finding_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let case = case_copy(tmp.path());
    let before = case_files(&case);
    let o = assemble(&case)
        .args([
            "--hash",
            "sha256,md5",
            "--lang",
            "all",
            "--actor",
            "Test Examiner",
        ])
        .output()
        .unwrap();
    // Only a low (item not examined) and info signals: exit 0.
    assert_eq!(o.status.code(), Some(0), "{:?}", o);
    let stdout = String::from_utf8_lossy(&o.stdout).to_string();
    assert!(stdout.contains("verified 4, excluded 0"), "{stdout}");
    assert!(
        stdout.contains("draft for the examiner's review"),
        "{stdout}"
    );
    let run = run_folder(&o.stdout);
    let rep = run.join("reports");
    for lang in ["en", "es", "fr"] {
        for ext in ["html", "pdf"] {
            assert!(rep.join(format!("expert-report.{lang}.{ext}")).is_file());
            assert!(rep.join(format!("statistics.{lang}.{ext}")).is_file());
        }
    }
    let report = json(&rep.join("expert-report.json"));
    assert_eq!(report["schema"], "rakvor.expert-report/1");
    assert_eq!(report["run"]["status"], "draft");
    assert_eq!(report["jurisdiction"]["code"], "co");
    assert!(
        report["jurisdiction"]["citations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] == "to_be_verified")
    );
    assert_eq!(report["totals"]["runs_verified"], 4);
    assert_eq!(report["totals"]["runs_excluded"], 0);

    // Every finding of every verified run's technical.json is in the report, identical (code, severity, params
    // and wording in every language), cited with the file's SHA-256; and the English HTML prints the wording.
    let html_en = std::fs::read_to_string(rep.join("expert-report.en.html")).unwrap();
    let entries: Vec<&Value> = report["results"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["entries"].as_array().unwrap().iter())
        .collect();
    let mut checked = 0;
    for tool in ["rv-hashinv", "rv-pdfauth", "rv-imgauth", "rv-timeline"] {
        let tr = tool_run(&case, tool);
        for (path, tech) in technical_reports(&tr) {
            let rel = path
                .strip_prefix(&case)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let entry = entries
                .iter()
                .find(|e| e["source"]["file"] == rel.as_str())
                .unwrap_or_else(|| panic!("{rel} not quoted"));
            let sha =
                rv_report_core::hash::hash_bytes(&std::fs::read(&path).unwrap(), HashAlgo::Sha256);
            assert_eq!(entry["source"]["sha256"], sha.as_str());
            let theirs = tech["findings"].as_array().unwrap();
            let ours = entry["findings"].as_array().unwrap();
            assert_eq!(theirs.len(), ours.len(), "{rel}");
            for (t, q) in theirs.iter().zip(ours) {
                assert_eq!(t["code"], q["code"]);
                assert_eq!(t["severity"], q["severity"]);
                assert_eq!(t["text"], q["text"], "{rel}: wording must be verbatim");
                let en = &t["text"]["en"];
                for k in ["title", "shows", "does_not_prove"] {
                    let s = en[k].as_str().unwrap();
                    assert!(
                        html_en.contains(&minijinja_escape(s)),
                        "{rel}: {k} not printed verbatim: {s}"
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked >= 10, "only {checked} findings checked");
    assert_eq!(report["totals"]["findings_quoted"], checked);
    // Wording only in English (rv-imgauth ran with --lang en): marked, not translated, and signalled.
    let html_es = std::fs::read_to_string(rep.join("expert-report.es.html")).unwrap();
    assert!(html_es.contains("[EN] "));
    let signals: Vec<String> = report["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["code"].as_str().unwrap().to_string())
        .collect();
    assert!(signals.contains(&"TRANSLATION_UNAVAILABLE".to_string()));
    // notes/acta.txt (E0003) was only registered: listed as not examined.
    assert_eq!(report["not_examined"][0]["evidence_id"], "E0003");
    assert!(signals.contains(&"ITEM_NOT_EXAMINED".to_string()));
    // Custody: the registration and every verified analysis run that compared the item with the registry.
    let e4 = report["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["evidence_id"] == "E0004")
        .unwrap();
    let actions: Vec<(&str, &str)> = e4["custody"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["action"].as_str().unwrap(), c["tool"].as_str().unwrap()))
        .collect();
    assert_eq!(
        actions,
        vec![("registered", "rv-hashinv"), ("examined", "rv-pdfauth")]
    );
    // The latest timeline run is summarised; limitations are quoted from the runs' printed reports.
    assert_eq!(report["timeline"]["events"], 30);
    let lims = report["limitations"]["tools"].as_array().unwrap();
    let pdf = lims.iter().find(|l| l["tool"] == "rv-pdfauth").unwrap();
    assert!(
        pdf["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["text"]["en"].is_string() && i["text"]["fr"].is_string())
    );
    // Draft on every page; legal references marked.
    for (lang, draft, mark) in [
        ("en", "DRAFT for the examiner", "[to be verified]"),
        ("es", "BORRADOR para revisión", "[a verificar]"),
        ("fr", "PROJET soumis", "[à vérifier]"),
    ] {
        let h = std::fs::read_to_string(rep.join(format!("expert-report.{lang}.html"))).unwrap();
        assert!(h.contains(draft) && h.contains(mark), "{lang}");
    }
    // Audit: start, case link, a verification per run, outputs recorded.
    let a = audit(&run);
    let acts: Vec<&str> = a.iter().map(|e| e["action"].as_str().unwrap()).collect();
    assert_eq!(&acts[..2], &["run_started", "case_linked"]);
    assert_eq!(acts.iter().filter(|x| **x == "run_verified").count(), 4);
    assert_eq!(acts.last(), Some(&"run_finished"));
    assert!(
        a.iter().any(
            |e| e["action"] == "output_written" && e["details"]["file"] == "expert-report.json"
        )
    );
    // Verification: the tool, the case, the independent verifier and the JSON schemas.
    bin()
        .arg("verify-log")
        .arg(run.join("logs"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains("truncation: the final entry"));
    bin().arg("verify-case").arg(&case).assert().code(0);
    let v = std::process::Command::new(py())
        .arg(root().join("tests/verify_chain.py"))
        .arg(run.join("logs"))
        .output()
        .unwrap();
    assert!(v.status.success(), "verify_chain.py: {:?}", v);
    if has_python_module("jsonschema") {
        let v = std::process::Command::new(py())
            .arg(root().join("tests/validate_reports.py"))
            .arg(&rep)
            .output()
            .unwrap();
        assert!(v.status.success(), "validate_reports.py: {:?}", v);
    }
    // Nothing outside the tool's own run folder changed (the case log only grew).
    assert_eq!(case_files(&case), before);
}

/// A run that fails verification is excluded with its reason and never quoted, in any language.
#[test]
fn tampered_runs_are_excluded_and_never_quoted() {
    type Tamper = fn(&Path) -> String;
    let digest: Tamper = |case| {
        // rv-imgauth: one finding's wording edited in a technical.json (its digest is in the audit log).
        let run = tool_run(case, "rv-imgauth");
        let (path, tech) = technical_reports(&run)
            .into_iter()
            .find(|(_, t)| !t["findings"].as_array().unwrap().is_empty())
            .unwrap();
        let shows = tech["findings"][0]["text"]["en"]["shows"]
            .as_str()
            .unwrap()
            .to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replacen("\"severity\": \"", "\"severity\":  \"", 1),
        )
        .unwrap();
        shows
    };
    let chain: Tamper = |case| {
        // rv-pdfauth: an audit entry edited without recomputing its hash.
        let run = tool_run(case, "rv-pdfauth");
        let (_, tech) = technical_reports(&run).into_iter().next().unwrap();
        let p = run.join("logs/audit.jsonl");
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(
            &p,
            text.replacen(
                "\"action\":\"item_analyzed\"",
                "\"action\":\"item_analysed\"",
                1,
            ),
        )
        .unwrap();
        tech["findings"][0]["text"]["en"]["shows"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let unfinished: Tamper = |case| {
        // rv-timeline: its run_finished (the case log's last entry) removed, so the run is not finished.
        let p = case.join("case-log.jsonl");
        let text = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.last().unwrap().contains("runs/rv-timeline/"));
        std::fs::write(&p, format!("{}\n", lines[..lines.len() - 1].join("\n"))).unwrap();
        String::new()
    };
    for (tool, reason, tamper) in [
        ("rv-imgauth", "digest_mismatch", digest),
        ("rv-pdfauth", "chain_broken", chain),
        ("rv-timeline", "not_finished", unfinished),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let case = case_copy(tmp.path());
        let marker = tamper(&case);
        let o = assemble(&case).args(["--lang", "all"]).output().unwrap();
        assert_eq!(o.status.code(), Some(3), "{tool}: {:?}", o);
        let text = String::from_utf8_lossy(&o.stdout).to_string();
        assert!(
            text.contains(&format!("excluded:  runs/{tool}/")) && text.contains(reason),
            "{tool}: {text}"
        );
        let run = run_folder(&o.stdout);
        let excluded: Vec<Value> = audit(&run)
            .into_iter()
            .filter(|e| e["action"] == "run_excluded")
            .collect();
        assert_eq!(excluded.len(), 1, "{tool}");
        assert_eq!(excluded[0]["details"]["tool"], tool);
        assert_eq!(excluded[0]["details"]["reason"], reason);
        let report = json(&run.join("reports/expert-report.json"));
        let annex = report["annexes"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["tool"] == tool)
            .unwrap();
        assert_eq!(annex["status"], "excluded");
        assert_eq!(annex["reason"], reason);
        assert_eq!(annex["findings_quoted"], 0);
        assert!(
            report["signals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["code"] == "RUN_EXCLUDED" && s["params"]["tool"] == tool)
        );
        // Never quoted: no entry of that tool, and its wording nowhere in the report.
        let quoted_from_tool = report["results"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["entries"].as_array().unwrap().iter())
            .any(|e| e["tool"] == tool);
        assert!(!quoted_from_tool, "{tool} quoted although excluded");
        if tool == "rv-timeline" {
            assert!(report["timeline"].is_null());
        }
        if !marker.is_empty() {
            for f in ["expert-report.json", "expert-report.en.html"] {
                let body = std::fs::read_to_string(run.join("reports").join(f)).unwrap();
                assert!(
                    !body.contains(&marker) && !body.contains(&minijinja_escape(&marker)),
                    "{tool}: excluded wording appears in {f}"
                );
            }
        }
        // The other runs stay verified and quoted.
        assert_eq!(report["totals"]["runs_verified"], 3, "{tool}");
    }
}

#[test]
fn mandate_examiner_final_and_french_structure() {
    let tmp = tempfile::tempdir().unwrap();
    let case = case_copy(tmp.path());
    let mandate = tmp.path().join("mandat.txt");
    std::fs::write(
        &mandate,
        "\u{feff}Mission : examiner les scellés E0001 à E0004.\r\nQuestion 1 : les fichiers ont-ils été modifiés ?\r\n",
    )
    .unwrap();
    let o = assemble(&case)
        .args([
            "--jurisdiction",
            "fr",
            "--lang",
            "fr",
            "--actor",
            "Expert Test",
            "--examiner-id",
            "LISTE-TEST-001",
            "--final",
            "--request",
        ])
        .arg(&mandate)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{:?}", o);
    let run = run_folder(&o.stdout);
    let report = json(&run.join("reports/expert-report.json"));
    assert_eq!(report["run"]["status"], "final");
    assert_eq!(report["jurisdiction"]["code"], "fr");
    assert_eq!(report["jurisdiction"]["case_jurisdiction"], "co");
    assert_eq!(report["jurisdiction"]["sections"][0], "mission");
    assert_eq!(
        report["identification"]["examiner"]["examiner_id"],
        "LISTE-TEST-001"
    );
    let m = &report["identification"]["mandate"];
    assert_eq!(
        m["text"],
        "Mission : examiner les scellés E0001 à E0004.\nQuestion 1 : les fichiers ont-ils été modifiés ?"
    );
    assert_eq!(
        m["sha256"],
        rv_report_core::hash::hash_bytes(&std::fs::read(&mandate).unwrap(), HashAlgo::Sha256)
            .as_str()
    );
    assert!(
        report["signals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["code"] == "JURISDICTION_DIFFERS_FROM_CASE")
    );
    let html = std::fs::read_to_string(run.join("reports/expert-report.fr.html")).unwrap();
    for s in [
        "Rapport d&#x27;expertise",
        "Version finale marquée par l&#x27;expert",
        "Code de procédure pénale, art. 166",
        "[à vérifier]",
        "Question 1 : les fichiers ont-ils été modifiés ?",
        "LISTE-TEST-001",
    ] {
        assert!(html.contains(s), "missing {s}");
    }
    let acts: Vec<String> = audit(&run)
        .iter()
        .map(|e| e["action"].as_str().unwrap().to_string())
        .collect();
    assert!(acts.contains(&"report_marked_final".to_string()));
    assert!(acts.contains(&"mandate_read".to_string()));
    // The mandate is in the register of files read.
    let csv = std::fs::read_to_string(run.join("logs/hashes.csv")).unwrap();
    assert!(csv.contains("mandat.txt"));
    // Custody forms follow the case (co): Spanish + English.
    let item = only_dir(&run.join("reports/items"));
    assert!(item.join("custody.es.pdf").is_file() && item.join("custody.en.pdf").is_file());
}

#[test]
fn tool_filters_skip_runs_and_widen_what_was_not_examined() {
    let tmp = tempfile::tempdir().unwrap();
    let case = case_copy(tmp.path());
    let o = assemble(&case)
        .args(["--include-tool", "rv-pdfauth,rv-hashinv"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{:?}", o);
    let report = json(&run_folder(&o.stdout).join("reports/expert-report.json"));
    for a in report["annexes"]["runs"].as_array().unwrap() {
        let tool = a["tool"].as_str().unwrap();
        let want = match tool {
            "rv-pdfauth" | "rv-hashinv" => ("verified", ""),
            "rv-report" => ("skipped", "self"),
            _ => ("skipped", "tool_filter"),
        };
        assert_eq!(
            (a["status"].as_str().unwrap(), a["reason"].as_str().unwrap()),
            want,
            "{tool}"
        );
    }
    let ids: Vec<&str> = report["not_examined"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["evidence_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["E0001", "E0002", "E0003"]);
    assert!(report["timeline"].is_null());
}

#[test]
fn a_broken_case_is_never_extended() {
    let tmp = tempfile::tempdir().unwrap();
    let case = case_copy(tmp.path());
    let log = case.join("case-log.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let broken = text.replacen("\"seq\":2", "\"seq\":7", 1);
    std::fs::write(&log, &broken).unwrap();
    assemble(&case).assert().code(3);
    assert!(!case.join("runs").join(TOOL).exists());
    assert_eq!(std::fs::read_to_string(&log).unwrap(), broken);
}

/// verify-log and verify-case re-hash every output the run's audit log records: an edited expert report and a
/// deleted statistics report are named, exit 3; restored, verify-log passes again.
#[test]
fn edited_or_deleted_outputs_fail_verify_log_and_verify_case() {
    let tmp = tempfile::tempdir().unwrap();
    let case = case_copy(tmp.path());
    let out = assemble(&case)
        .args(["--hash", "sha256,md5", "--lang", "en"])
        .assert()
        .code(0);
    let run = run_folder(&out.get_output().stdout);
    let logs = run.join("logs");
    bin().arg("verify-log").arg(&logs).assert().code(0).stdout(
        predicate::str::is_match(r"outputs: (\d+) verified of \d+ recorded \(md5, sha256\)")
            .unwrap(),
    );
    let html = run.join("reports/expert-report.en.html");
    let saved = std::fs::read(&html).unwrap();
    std::fs::write(&html, b"<p>edited</p>").unwrap();
    let stats = run.join("reports/statistics.json");
    std::fs::rename(&stats, tmp.path().join("moved.json")).unwrap();
    bin().arg("verify-log").arg(&logs).assert().code(3).stdout(
        predicate::str::contains("output reports/expert-report.en.html: md5 differs").and(
            predicate::str::contains("output reports/statistics.json: missing"),
        ),
    );
    bin().arg("verify-case").arg(&case).assert().code(3);
    std::fs::write(&html, saved).unwrap();
    std::fs::rename(tmp.path().join("moved.json"), &stats).unwrap();
    bin().arg("verify-log").arg(&logs).assert().code(0);
    // A second assembly skips the first one as its own (self) and still verifies every tool run.
    let o = assemble(&case).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{:?}", o);
    let report = json(&run_folder(&o.stdout).join("reports/expert-report.json"));
    let own: Vec<&Value> = report["annexes"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["tool"] == TOOL)
        .collect();
    assert_eq!(own.len(), 2);
    assert!(own.iter().all(|a| a["reason"] == "self"));
}

/// examples/sample/run.sh assembles the report on a copy of the sample case and checks it.
#[test]
#[cfg(unix)]
fn sample_script_assembles_the_report() {
    let script = root().join("examples/sample/run.sh");
    let exe = env!("CARGO_BIN_EXE_rv-report");
    let out = tempfile::tempdir().unwrap();
    let o = std::process::Command::new("sh")
        .arg(&script)
        .arg(exe)
        .env("RV_NO_TIMESTAMP", "1")
        .env("RV_SAMPLE_OUT", out.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}", text);
    assert!(
        text.contains("expert-report.json is listed in the audit log"),
        "{}",
        text
    );
}

#[test]
fn timestamp_of_a_missing_log_exits_3() {
    let d = tempfile::tempdir().unwrap();
    bin()
        .arg("timestamp")
        .arg(d.path())
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no audit log at"));
}

#[test]
fn self_test_passes() {
    bin()
        .arg("self-test")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("FAIL").not());
}

#[test]
fn usage_doc_matches_help() {
    let generated = bin().arg("docs-cli").output().unwrap().stdout;
    let committed = std::fs::read(root().join("docs/usage-cli.md")).unwrap_or_default();
    assert!(
        committed == generated,
        "docs/usage-cli.md is out of date: run `cargo run -p rv-report-cli -- docs-cli > docs/usage-cli.md`"
    );
}

#[test]
fn trust_timestamp_signature_and_custody_with_test_pki() {
    let pki = tempfile::tempdir().unwrap();
    let Some((tsa, url)) = test_pki(pki.path()) else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let case = case_copy(tmp.path());
    let o = bin()
        .env_remove("RV_NO_TIMESTAMP")
        .env("NO_PROXY", "127.0.0.1")
        .env("RV_SIGN_PASSWORD", "test")
        .args(["assemble", "--case-dir"])
        .arg(&case)
        .args([
            "--jurisdiction",
            "fr",
            "--tsa-url",
            &url,
            "--progress",
            "none",
            "--sign-cert",
        ])
        .arg(pki.path().join("examiner.p12"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{:?}", o);
    assert!(String::from_utf8_lossy(&o.stdout).contains("timestamp: ok"));
    let run = run_folder(&o.stdout);
    let logs = run.join("logs");
    bin()
        .arg("verify-log")
        .arg(&logs)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("TSA signature valid"))
        .stdout(predicate::str::contains("covers entry"));
    bin()
        .arg("verify-log")
        .arg(&logs)
        .arg("--tsa-ca")
        .arg(pki.path().join("ca.pem"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains("chain validated to"));
    // The case's own jurisdiction (co) applies to the custody forms.
    let item = only_dir(&run.join("reports/items"));
    for f in ["custody.json", "custody.es.pdf", "custody.en.pdf"] {
        assert!(item.join(f).is_file(), "{}", f);
    }
    // Every PDF, the expert report included, is signed; pyHanko validates them independently.
    let expert = std::fs::read(run.join("reports/expert-report.en.pdf")).unwrap();
    assert!(String::from_utf8_lossy(&expert).contains("/adbe.pkcs7.detached"));
    let v = std::process::Command::new(py())
        .arg(root().join("tests/validate_pdf_signature.py"))
        .arg(run.join("reports"))
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&v.stdout);
    assert!(v.status.success(), "pyHanko validation failed:\n{}", report);
    assert!(report.contains("expert-report.en.pdf"), "{}", report);
    let model = json(&run.join("reports/expert-report.json"));
    assert!(
        model["run"]["pdf_signed_by"]
            .as_str()
            .unwrap()
            .contains("Ana Ruiz")
    );
    // Tampering with the token is detected.
    let tsr = std::fs::read_dir(&logs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "tsr"))
        .unwrap();
    let mut b = std::fs::read(&tsr).unwrap();
    let last = b.len() - 1;
    b[last] ^= 0xff;
    std::fs::write(&tsr, &b).unwrap();
    bin()
        .arg("verify-log")
        .arg(&logs)
        .assert()
        .code(3)
        .stdout(predicate::str::contains("missing or altered"));
    // Offline run: pending, then added later with `timestamp`.
    let tmp2 = tempfile::tempdir().unwrap();
    let case2 = case_copy(tmp2.path());
    let o = bin()
        .env_remove("RV_NO_TIMESTAMP")
        .args(["assemble", "--case-dir"])
        .arg(&case2)
        .args(["--tsa-url", "http://127.0.0.1:9/tsr", "--progress", "none"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{:?}", o);
    assert!(String::from_utf8_lossy(&o.stdout).contains("timestamp: pending"));
    let logs2 = run_folder(&o.stdout).join("logs");
    bin()
        .env("NO_PROXY", "127.0.0.1")
        .args(["timestamp"])
        .arg(&logs2)
        .args(["--tsa-url", &url])
        .assert()
        .code(0);
    bin()
        .arg("verify-log")
        .arg(&logs2)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("covers entry"));
    drop(tsa);
}

/// A case folder written by the other profile (tests/golden) verifies here, and tampering with it is detected.
#[test]
fn golden_case_from_the_other_profile_verifies() {
    let golden = root().join("tests/golden");
    let mut archives: Vec<PathBuf> = std::fs::read_dir(&golden)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    archives.retain(|p| p.to_string_lossy().ends_with(".zip.b64"));
    archives.sort();
    for archive in archives {
        let tmp = tempfile::tempdir().unwrap();
        let unzip = std::process::Command::new(py())
            .arg(golden.join("unpack.py"))
            .arg(&archive)
            .arg(tmp.path())
            .status();
        if !unzip.is_ok_and(|s| s.success()) {
            eprintln!(
                "skipping golden case test: {} cannot unpack {}",
                py(),
                archive.display()
            );
            return;
        }
        let case = tmp.path().join("CASE-GOLDEN");
        bin().arg("verify-case").arg(&case).assert().code(0);
        let reg = case.join("evidence/registry.json");
        let text = std::fs::read_to_string(&reg).unwrap();
        std::fs::write(&reg, text.replacen("\"size\": ", "\"size\": 1", 1)).unwrap();
        bin()
            .arg("verify-case")
            .arg(&case)
            .assert()
            .code(3)
            .stdout(predicate::str::contains("registry.json"));
    }
}

/// CI and release build with the toolchain pinned in rust-toolchain.toml (never a floating `stable`).
#[test]
fn workflows_use_the_pinned_toolchain() {
    let pin = std::fs::read_to_string(root().join("rust-toolchain.toml")).unwrap();
    let channel = pin
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = "))
        .unwrap()
        .trim_matches('"');
    assert!(
        channel.split('.').count() == 3 && channel.split('.').all(|p| p.parse::<u32>().is_ok()),
        "rust-toolchain.toml must pin an exact x.y.z version, not {}",
        channel
    );
    for wf in ["ci.yml", "release.yml"] {
        let text = std::fs::read_to_string(root().join(".github/workflows").join(wf)).unwrap();
        assert!(!text.contains("rust-toolchain@stable"), "{}", wf);
        assert!(text.contains("rustup toolchain install"), "{}", wf);
    }
}

/// The installer ships the sample case folder (recursively) so `assemble --case-dir` works on a fresh install.
#[test]
fn installer_ships_the_sample_case() {
    let iss = std::fs::read_to_string(root().join("packaging/windows/installer.iss")).unwrap();
    let line = iss
        .lines()
        .find(|l| l.contains(r"examples\sample\case\*"))
        .expect("installer.iss installs examples/sample/case");
    assert!(line.contains("recursesubdirs"), "{line}");
    assert!(root().join("examples/sample/case/case.json").is_file());
}
