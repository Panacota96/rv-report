//! rv-report command-line interface. All work happens in `rv_report_core`; this file only parses arguments,
//! renders events (progress bar or JSON Lines) and maps results to exit codes.

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rv_report_core::model::AssembleOptions;
use rv_report_core::trust::{self, DEFAULT_TSA};
use rv_report_core::{
    CASE_ROLE, Event, HashAlgo, RunConfig, TOOL_NAME, TOOL_VERSION, assemble, audit, case, hash,
    i18n, run, verify,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const EXIT_OK: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_FINDINGS: u8 = 3;
const EXIT_CANCELLED: u8 = 130;

const ABOUT: &str = "Assemble one expert report (dictamen pericial for Colombia, rapport d'expertise for France) from the verified runs of the Rakvor tools in a case folder: method, results quoted verbatim, limitations, custody and annexes filled in from the audit logs, a draft for the examiner until --final.";

const LONG_ABOUT: &str = "\
DESCRIPTION:
  rv-report reads a Rakvor case folder (the folder that holds case.json) and assembles ONE expert
  report from every tool that ran on the case: a dictamen pericial structured for Colombia
  (Ley 906 de 2004) or a rapport d'expertise structured for France (CPP arts. 156-169). It never
  opens the evidence itself. For the case it:
    1. verifies the case log, case.json and the evidence registry (a broken case is refused,
       exit 3, and nothing is written to it);
    2. verifies every tool run before using it: audit chain intact, run finished and anchored in
       the case log, timestamp tokens, and every output re-hashed against the digest its audit
       log recorded. A run that fails is excluded with the reason and NEVER quoted;
    3. quotes each verified run's findings verbatim from its own technical reports (code,
       severity, title, what it shows, what it does not prove) and its printed limitations,
       and embeds the latest verified RV Timeline summary;
    4. fills in identification (case, examiner, mandate), evidence received with its custody
       rows from the case log, method per tool, results per evidence item, timeline,
       limitations, what was not examined, annexes (every run with its audit head, timestamp
       status and outputs verified) and the examiner's declaration template;
    5. writes reports/expert-report.json and expert-report.<LANG>.html/.pdf (PDF signed with
       --sign-cert), plus the suite's statistics and per-item reports.
  The report says \"draft for the examiner's review\" on every page until the examiner gives
  --final (audited). The run is written inside the case, under <CASE>/runs/rv-report/<RUN-ID>/,
  and recorded in the case log. Every action is appended to a hash-chained log that
  `rv-report verify-log` can check later; `rv-report verify-case <CASE>` checks the whole case.";

const ASSEMBLE_AFTER_HELP: &str = "\
EXAMPLES:
  Draft report of a case, structure and language of the case (Colombia -> use Spanish):
    rv-report assemble --case-dir E:\\CASE-014 --lang es
  French structure, French and English, with the examiner's id and the mandate text:
    rv-report assemble --case-dir E:\\CASE-014 --jurisdiction fr --lang fr,en \\
      --actor \"Ana Ruiz\" --examiner-id \"CA Paris 2026-117\" --mandate E:\\CASE-014\\mandat.txt
  Only the PDF and image analyses, two hashes, machine-readable progress:
    rv-report assemble --case-dir E:\\CASE-014 --include-tool rv-pdfauth,rv-imgauth \\
      --hash sha256,md5 --progress jsonl
  After review, the final version signed with the examiner's certificate:
    rv-report assemble --case-dir E:\\CASE-014 --lang es --final --sign-cert C:\\certs\\perito.p12
  Then verify the run's audit log and the whole case:
    rv-report verify-log E:\\CASE-014\\runs\\rv-report\\<RUN-ID>\\logs
    rv-report verify-case E:\\CASE-014

EXIT CODES:
  0    Report assembled; every run used verified and no medium- or high-severity signal.
  1    Runtime error (case log locked by another tool, output not writable, signing
       certificate unreadable or wrong password, ...). See the audit log.
  2    Usage error (unknown option, more than two hash algorithms, unsupported language,
       unknown tool in --include-tool/--exclude-tool, mandate file missing, not UTF-8 or larger
       than 64 KiB, a folder that is not a case folder, --case of another case).
  3    Report assembled with at least one run excluded (not verified: chain broken, digest
       mismatch, not finished, timestamp invalid, unreadable) or a medium/high signal (e.g. a
       tool examined a copy that differs from the registry); or the case folder fails
       verification (nothing is written to it).
  130  Cancelled with Ctrl+C. The cancellation is written to the audit log.

FILES WRITTEN:
  Inside the case folder, <RUN> = <CASE>/runs/rv-report/<RUN-ID>:
  <RUN>/logs/audit.jsonl              Hash-chained log of every action: runs skipped, verified,
                                      excluded (with the reason) and quoted (with each source's
                                      SHA-256), the mandate's digest, report_marked_final, outputs.
  <RUN>/logs/audit.log                The same actions, human-readable.
  <RUN>/logs/environment.json         Tool and dependency versions, OS, host, user, command line.
  <RUN>/logs/hashes.csv               Every file whose content entered the report (case.json,
                                      registry, each verified run's audit log and reports read,
                                      the mandate), with the run's hash algorithms.
  <RUN>/logs/timestamp-<SEQ>.tsr|.json  RFC 3161 token for the audit chain head.
  <RUN>/reports/expert-report.json    The report model (schema rakvor.expert-report/1).
  <RUN>/reports/expert-report.<LANG>.html|.pdf  The report in each language (PDF signed with
                                      --sign-cert). Digests in the audit log (output_written).
  <RUN>/reports/statistics.json       Signals, items and run summary (+ .<LANG>.html and .pdf).
  <RUN>/reports/items/<NNNNN>-<NAME>/technical|summary|custody.*  Per registered item: RV
                                      Report's signals, the quoted results, the custody form.
  <CASE>/case-log.jsonl               run_started and run_finished of this run are appended.
  Nothing else in the case folder changes: the other tools' runs and the evidence are only read.

FORENSIC NOTES:
  - Verification comes before reading: a run whose audit chain is broken, that did not finish,
    whose timestamp token fails or whose output differs from the digest in its own audit log is
    excluded and never quoted. The exclusion is in the audit log, the annexes and a RUN_EXCLUDED
    signal.
  - Findings are quoted verbatim from each tool's own report, with the file, its SHA-256 and the
    JSON pointer. RV Report never rewords or translates them: when a tool wrote no text in the
    report language, the text is shown in its own language, marked [EN]/[ES]/[FR].
  - Results are signals, not verdicts: every finding says what it does not prove, and an item
    without findings is not proven authentic. Items no analysis tool examined are listed.
  - RV Report writes no conclusion: the examiner reviews the draft, writes the opinion and
    signs. --final only marks the version (audited); the signature is --sign-cert's.
  - Every legal reference in the report is marked \"to be verified\"; the structure is a draft
    for counsel's review (docs/admissibility.md).
  - Each audit entry stores the SHA-256 of the previous one; the run's audit head is recorded in
    the case log. The chain head is timestamped by an RFC 3161 authority; offline, the run
    finishes with `timestamp_pending`: run `rv-report timestamp <LOG>` later.
  - With --sign-cert every PDF is signed with the examiner's certificate; the password is never
    logged.";

#[derive(Parser)]
#[command(name = TOOL_NAME, version = TOOL_VERSION, about = ABOUT, long_about = LONG_ABOUT, max_term_width = 100,
          after_long_help = "Run `rv-report <COMMAND> --help` for the options, examples and exit codes of each command.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Assemble the expert report of a case folder from the verified runs of the other Rakvor tools.
    #[command(long_about = LONG_ABOUT, after_long_help = ASSEMBLE_AFTER_HELP)]
    Assemble(Box<AssembleArgs>),
    /// Verify an audit log (audit.jsonl): edited, deleted or re-ordered entries, and truncation.
    #[command(after_long_help = "\
CHECKS:
  - every entry links to the previous one (hash chain): an edited, deleted or re-ordered entry
    breaks it;
  - every timestamp token the log records (timestamp_obtained) is still next to it with its
    metadata (timestamp-<SEQ>.tsr with the recorded SHA-256, timestamp-<SEQ>.json agreeing): a
    missing file is a problem, never a skipped check. Every token covers the hash of the entry
    it names;
  - the TSA's CMS signature of every token: signer certificate in the token, ESS
    signing-certificate binding, message digest over TSTInfo, RSA signature, and an extended key
    usage that is critical and only timeStamping (RFC 3161). With --tsa-ca, also the TSA
    certificate path up to that CA bundle at the token's genTime; without it the output says
    \"chain not checked\". Revocation is never checked;
  - truncation: a chain alone cannot show that entries were removed from its end. The log must
    still contain the entry covered by its RFC 3161 timestamp (covers_seq) and, for a run inside
    a case folder (<CASE>/runs/<TOOL>/<RUN-ID>/logs), end with the audit head that the case log
    recorded at run_finished (only timestamp_obtained may follow). With neither record the
    output says \"truncation not detectable without a timestamp or case record\";
  - recorded outputs: every file the log says the run wrote (expert report, item reports,
    statistics, hashes.csv, environment.json) is re-hashed with each algorithm recorded and
    compared; the latest entry for a file wins. A missing, edited or replaced file is a
    problem; a recorded path that is absolute, contains .. or resolves outside the run folder
    is rejected and not read. Output: \"outputs: N verified of M recorded (algorithms)\".

EXAMPLES:
  rv-report verify-log rakvor-out/logs/20261019T101500Z-a1b2c3/audit.jsonl
  rv-report verify-log E:\\CASE-014\\runs\\rv-report\\20261019T101500Z-a1b2c3\\logs
  rv-report verify-log E:\\CASE-014\\runs\\rv-report\\20261019T101500Z-a1b2c3\\logs --tsa-ca roots.pem

EXIT CODES:
  0  The chain is intact and agrees with its timestamps and case record (if any), and every
     output file it records still has the recorded digests.
  1  The log or the --tsa-ca file cannot be read.
  3  The chain is broken, a timestamp does not match or does not verify, the log is shorter
     than its timestamp or case record says (truncated), or a recorded output file was edited,
     deleted or replaced after the run.")]
    VerifyLog {
        /// Path to audit.jsonl, or to the run's log folder that contains it.
        ///
        /// Example: E:\CASE-014\runs\rv-report\20261019T101500Z-a1b2c3\logs
        log: PathBuf,
        /// CA bundle (PEM with one or more certificates, or one DER certificate) that the TSA
        /// certificate of every timestamp token must chain to, at the token's genTime.
        ///
        /// Default: none (the TSA signature is checked, the chain is not).
        /// Example: --tsa-ca C:\certs\tsa-roots.pem
        #[arg(long, value_name = "PEM", verbatim_doc_comment)]
        tsa_ca: Option<PathBuf>,
    },
    /// Check every hash algorithm against known-answer test vectors.
    #[command(after_long_help = "\
EXAMPLES:
  rv-report self-test

EXIT CODES:
  0  All algorithms give the published answer for the input \"abc\".
  1  At least one algorithm failed: do not use this build for evidence.")]
    SelfTest,
    /// Add an RFC 3161 timestamp to a finished run (e.g. after working offline).
    #[command(after_long_help = "\
EXAMPLES:
  rv-report timestamp E:\\CASE-014\\runs\\rv-report\\20261019T101500Z-a1b2c3\\logs
  rv-report timestamp E:\\CASE-014\\runs\\rv-report\\20261019T101500Z-a1b2c3\\logs --tsa-url https://freetsa.org/tsr

EXIT CODES:
  0  Timestamp obtained and logged.
  1  The TSA could not be reached (nothing changed; try again).
  3  The log is missing or broken: no timestamp is added.")]
    Timestamp {
        /// The run's log folder (or its audit.jsonl).
        ///
        /// Example: E:\CASE-014\runs\rv-report\20261019T101500Z-a1b2c3\logs
        log: PathBuf,
        /// RFC 3161 time-stamping authority.
        /// Default: http://timestamp.digicert.com
        /// Example: --tsa-url https://freetsa.org/tsr
        #[arg(long, value_name = "URL", default_value = DEFAULT_TSA, verbatim_doc_comment, hide_default_value = true)]
        tsa_url: String,
        /// Name written in the new audit entry.
        /// Default: the operating-system user name
        /// Example: --actor "Ana Ruiz"
        #[arg(short, long, value_name = "NAME", verbatim_doc_comment)]
        actor: Option<String>,
    },
    /// Verify a whole case folder: case log, evidence registry and every run's audit log.
    #[command(after_long_help = "\
CHECKS:
  - case-log.jsonl is an intact hash chain starting with case_created; case.json is unchanged;
  - evidence/registry.json has the SHA-256 recorded at the last registry change;
  - every run announced in the case log has an intact audit.jsonl (and valid timestamp
    tokens: imprint and TSA signature), its case_linked entry points to its run_started entry,
    and the audit head recorded at run_finished is its final entry (only timestamp_obtained may
    follow);
  - every output file a run's audit log records is re-hashed and compared with the recorded
    digests (edited, deleted or replaced: a problem);
  - with --rehash: every registered item still reachable at its registered path is read again
    and its digests are compared with the registry (the latest registration of a path).

EXAMPLES:
  rv-report verify-case E:\\CASE-014
  rv-report verify-case E:\\CASE-014 --rehash

EXIT CODES:
  0  The case folder is intact (warnings, e.g. a run still in progress or an item not
     reachable for --rehash, are printed).
  1  The folder is not a case folder (no case.json).
  3  At least one check failed: the case was edited, truncated or re-ordered, a run's
     recorded output was edited, deleted or replaced, or (--rehash) an evidence file differs
     from the registry.")]
    VerifyCase {
        /// The case folder (the one that holds case.json).
        ///
        /// Example: E:\CASE-014
        case: PathBuf,
        /// Also re-hash every registered item still reachable and compare it with the registry.
        /// Default: off (only the registry's digest is checked)
        /// Example: --rehash
        #[arg(long, verbatim_doc_comment)]
        rehash: bool,
    },
    /// Print the full help of every command as Markdown (used to generate docs/usage-cli.md).
    #[command(hide = true)]
    DocsCli,
    /// Describe this tool as JSON (schema rakvor.describe/1) for RV Hub and scripts.
    #[command(hide = true, after_long_help = DESCRIBE_AFTER_HELP)]
    Describe {
        /// Print JSON: one pretty-printed object with sorted keys.
        /// Default: on (JSON is the only format)
        /// Example: --json
        #[arg(long, verbatim_doc_comment)]
        json: bool,
    },
}

const DESCRIBE_AFTER_HELP: &str = "\
DESCRIPTION:
  Prints what this tool is, what it accepts and how to launch it, validated by
  schemas/describe.schema.json. RV Hub reads it to list the tool and start its GUI on a case
  folder (rv-report-gui.exe --case-dir <CASE>). rv-report's input is a case folder, not
  evidence files: the files it reads are the case's JSON documents and JSON Lines logs and the
  other tools' JSON and HTML reports, so it is never suggested for an evidence file.
  Fields: schema, tool, title, version, role (analysis), main_verb (assemble), accepts (json,
  jsonl; no signature), case_formats, log_formats, cli_exe, gui_exe, exit_codes. Nothing is
  read or written; no audit log is created.

EXAMPLES:
  rv-report describe --json

EXIT CODES:
  0  The description was printed.";

#[derive(Clone, Copy, ValueEnum)]
enum ProgressMode {
    /// Progress bar with elapsed time and the runs done on stderr.
    Bar,
    /// One JSON event per line on stdout (for scripts and other tools).
    Jsonl,
    /// No progress output; only the final summary.
    None,
}

#[derive(Args)]
struct AssembleArgs {
    /// The case folder to read (the one that holds case.json), written by the other Rakvor tools.
    /// The report run is written inside it, under <DIR>/runs/rv-report/<RUN-ID>/.
    /// Default: none (required)
    /// Example: --case-dir E:\CASE-014
    #[arg(long, value_name = "DIR", verbatim_doc_comment)]
    case_dir: PathBuf,

    /// Structure and legal references of the report: co (dictamen pericial, Colombia) or fr
    /// (rapport d'expertise, France). The per-item custody forms always follow the case's own
    /// jurisdiction (case.json); a different value here is recorded and noted in the report.
    /// Default: the case's jurisdiction (case.json)
    /// Example: --jurisdiction fr
    #[arg(short, long, value_name = "CC", value_parser = ["co", "fr"], verbatim_doc_comment, hide_possible_values = true)]
    jurisdiction: Option<String>,

    /// Report language(s): en, es, fr (comma-separated), or all. A filing needs the language of
    /// the court (es for Colombia, fr for France). JSON is language-neutral.
    /// Default: en
    /// Example: --lang es   or   --lang all
    #[arg(
        short,
        long,
        value_name = "LANG",
        default_value = "en",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    lang: String,

    /// Name of the examiner (the expert who reviews and signs), written in every audit entry and
    /// in the report (alias: --examiner).
    /// Default: the operating-system user name
    /// Example: --actor "Ana Ruiz"
    #[arg(
        short,
        long,
        alias = "examiner",
        value_name = "NAME",
        verbatim_doc_comment
    )]
    actor: Option<String>,

    /// The expert's identification as it must appear in the report (professional card, expert
    /// list registration, ...). Personal data: printed in the report and the audit log.
    /// Default: none (the report says it was not given)
    /// Example: --examiner-id "TP 12345-CO"
    #[arg(long, value_name = "TEXT", verbatim_doc_comment)]
    examiner_id: Option<String>,

    /// UTF-8 text file with the object of the expertise (the order, request or questions), quoted
    /// verbatim in the report; its SHA-256 is audited. At most 64 KiB (alias: --request).
    /// Default: none (the report asks the examiner to state the object)
    /// Example: --mandate E:\CASE-014\mandato.txt
    #[arg(long, alias = "request", value_name = "FILE", verbatim_doc_comment)]
    mandate: Option<PathBuf>,

    /// Only the runs of these tools, comma-separated or repeated (e.g. rv-pdfauth, rv-imgauth,
    /// rv-timeline). Other runs are skipped and listed in the annexes.
    /// Default: every tool
    /// Example: --include-tool rv-pdfauth,rv-imgauth
    #[arg(
        long = "include-tool",
        value_name = "TOOL",
        value_delimiter = ',',
        verbatim_doc_comment
    )]
    include_tools: Vec<String>,

    /// Leave out the runs of these tools (comma-separated or repeated); listed as skipped.
    /// Default: none
    /// Example: --exclude-tool rv-rsmf
    #[arg(
        long = "exclude-tool",
        value_name = "TOOL",
        value_delimiter = ',',
        verbatim_doc_comment
    )]
    exclude_tools: Vec<String>,

    /// Mark the report as the examiner's final version (audited as report_marked_final). Without
    /// it every page says "draft for the examiner's review". It does not sign: use --sign-cert.
    /// Default: off (draft)
    /// Example: --final
    #[arg(long = "final", verbatim_doc_comment)]
    final_report: bool,

    /// Hash algorithm(s) for the files this run reads and writes (hashes.csv, reports, logs): one
    /// or two of md5, sha1, sha256, sha512, sha3-256, blake3. More than two is refused (exit 2).
    /// Default: sha256
    /// Example: --hash sha256,md5
    #[arg(
        short = 'H',
        long = "hash",
        value_name = "ALGO",
        value_delimiter = ',',
        default_value = "sha256",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    hashes: Vec<String>,

    /// Not used by assemble: every output goes inside the case folder (<CASE>/runs/rv-report/).
    /// Accepted so that scripts shared with the other Rakvor tools keep working.
    /// Default: ./rakvor-out (ignored)
    /// Example: --out E:\case-014\rv-report
    #[arg(
        short,
        long,
        value_name = "DIR",
        default_value = "rakvor-out",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    out: PathBuf,

    /// Case identifier: when given, it must match the case folder's case.json (exit 2 otherwise).
    /// Default: none (the case id is read from case.json)
    /// Example: --case CASE-2026-014
    #[arg(short, long = "case", value_name = "ID", verbatim_doc_comment)]
    case_id: Option<String>,

    /// How progress is shown: bar, jsonl or none.
    /// Default: bar (hidden automatically when stderr is not a terminal)
    /// Example: --progress jsonl
    #[arg(
        short,
        long,
        value_enum,
        value_name = "MODE",
        default_value = "bar",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    progress: ProgressMode,

    /// RFC 3161 time-stamping authority for the audit chain head.
    /// Default: http://timestamp.digicert.com
    /// Example: --tsa-url https://freetsa.org/tsr
    #[arg(long, value_name = "URL", default_value = DEFAULT_TSA, verbatim_doc_comment, hide_default_value = true)]
    tsa_url: String,

    /// Do not request a trusted timestamp (offline labs). Also set by RV_NO_TIMESTAMP=1.
    /// Default: off (a timestamp is requested)
    /// Example: --no-timestamp
    #[arg(long, env = "RV_NO_TIMESTAMP", value_parser = clap::builder::FalseyValueParser::new(), verbatim_doc_comment, hide_env = true)]
    no_timestamp: bool,

    /// Examiner's certificate (.p12/.pfx, RSA) used to sign every PDF (the expert report and the
    /// item reports). The password is read from the environment variable named by
    /// --sign-pass-env, or asked for interactively.
    /// Default: none (PDFs are not signed)
    /// Example: --sign-cert C:\certs\ana-ruiz.p12
    #[arg(long, value_name = "P12", verbatim_doc_comment)]
    sign_cert: Option<PathBuf>,

    /// Environment variable holding the certificate password.
    /// Default: RV_SIGN_PASSWORD
    /// Example: --sign-pass-env CERT_PASS
    #[arg(
        long,
        value_name = "VAR",
        default_value = "RV_SIGN_PASSWORD",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    sign_pass_env: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    ExitCode::from(match cli.command {
        Command::Assemble(a) => assemble_cmd(*a),
        Command::VerifyLog { log, tsa_ca } => verify_log(log, tsa_ca),
        Command::VerifyCase { case, rehash } => verify_case(case, rehash),
        Command::SelfTest => self_test(),
        Command::Timestamp {
            log,
            tsa_url,
            actor,
        } => timestamp(log, tsa_url, actor),
        Command::DocsCli => {
            print!("{}", docs_cli());
            EXIT_OK
        }
        Command::Describe { json: _ } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&describe()).expect("describe is valid JSON")
            );
            EXIT_OK
        }
    })
}

fn usage_error(msg: &str) -> u8 {
    eprintln!(
        "error: {}\n\nFor more information, try 'rv-report assemble --help'.",
        msg
    );
    EXIT_USAGE
}

/// Tool names --include-tool / --exclude-tool accept: rv-<lowercase letters, digits, dashes>.
fn tool_list(raw: &[String], flag: &str) -> Result<Vec<String>, u8> {
    let mut out: Vec<String> = Vec::new();
    for t in raw.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        let ok = t.starts_with("rv-")
            && t.len() > 3
            && t[3..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !ok {
            return Err(usage_error(&format!(
                "{} '{}': a Rakvor tool name such as rv-pdfauth (known: {})",
                flag,
                t,
                assemble::KNOWN_TOOLS.join(", ")
            )));
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    Ok(out)
}

/// Checks every option and the case folder before anything is written: usage problems give exit 2,
/// a case that fails verification exit 3.
fn options(a: &AssembleArgs) -> Result<(Vec<HashAlgo>, Vec<String>, AssembleOptions), u8> {
    let mut algos = Vec::new();
    for h in &a.hashes {
        match HashAlgo::parse(h) {
            Some(x) => algos.push(x),
            None => {
                return Err(usage_error(&format!(
                    "unknown hash algorithm '{}': use md5, sha1, sha256, sha512, sha3-256 or blake3",
                    h
                )));
            }
        }
    }
    hash::validate_selection(&algos).map_err(|e| usage_error(&e))?;
    let langs = i18n::parse_langs(&a.lang).map_err(|e| usage_error(&e))?;
    let include_tools = tool_list(&a.include_tools, "--include-tool")?;
    let exclude_tools = tool_list(&a.exclude_tools, "--exclude-tool")?;
    if let Some(m) = &a.mandate {
        assemble::read_mandate(m).map_err(|e| usage_error(&e))?;
    }
    let dir = &a.case_dir;
    if !dir.join(case::CASE_FILE).is_file() {
        return Err(usage_error(&format!(
            "{} is not a case folder (no {}): rv-report reads a case written by the other Rakvor tools and never creates one",
            dir.display(),
            case::CASE_FILE
        )));
    }
    // Case problems found before the run map to their own exit codes (2 another case, 3 broken case).
    if let Err(e) = case::preflight(dir, a.case_id.as_deref(), &[]) {
        return Err(match e.exit_code() {
            EXIT_USAGE => usage_error(&e.message),
            code => {
                eprintln!("error: {}", e.message);
                code
            }
        });
    }
    let opts = AssembleOptions {
        case_dir: dir.clone(),
        jurisdiction: a.jurisdiction.clone(),
        examiner_id: a.examiner_id.clone(),
        mandate: a.mandate.clone(),
        include_tools,
        exclude_tools,
        final_report: a.final_report,
    };
    Ok((algos, langs, opts))
}

/// What the run's own audit log says about it (read back for the summary, so the summary never says more
/// than the log).
#[derive(Default)]
struct AuditFacts {
    runs_verified: u64,
    skipped: u64,
    findings_quoted: u64,
    excluded: Vec<String>,
    status: String,
}

fn audit_facts(log_dir: &Path) -> AuditFacts {
    let mut f = AuditFacts::default();
    let text = std::fs::read_to_string(log_dir.join("audit.jsonl")).unwrap_or_default();
    for line in text.lines() {
        let Ok(e) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let d = &e["details"];
        match e["action"].as_str().unwrap_or_default() {
            "run_excluded" => f.excluded.push(format!(
                "{} ({}{}{})",
                d["run_dir"].as_str().unwrap_or("-"),
                d["reason"].as_str().unwrap_or("-"),
                if d["detail"].as_str().unwrap_or("").is_empty() {
                    ""
                } else {
                    ": "
                },
                d["detail"].as_str().unwrap_or("")
            )),
            "run_finished" => {
                f.runs_verified = d["runs_verified"].as_u64().unwrap_or(0);
                f.skipped = d["runs_skipped"].as_u64().unwrap_or(0);
                f.findings_quoted = d["findings_quoted"].as_u64().unwrap_or(0);
                f.status = d["status"].as_str().unwrap_or_default().to_string();
            }
            _ => {}
        }
    }
    f
}

fn assemble_cmd(a: AssembleArgs) -> u8 {
    let (algos, langs, opts) = match options(&a) {
        Ok(x) => x,
        Err(code) => return code,
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let _ = ctrlc::set_handler(move || c.store(true, Ordering::SeqCst));

    let cfg = RunConfig {
        inputs: vec![],
        input_list: None,
        recursive: false,
        algorithms: algos,
        out_dir: a.out.clone(),
        langs,
        actor: a
            .actor
            .clone()
            .unwrap_or_else(rv_report_core::env::os_user),
        case_id: a.case_id.clone(),
        frontend: "cli".into(),
        preamble: vec![],
        command_line: std::env::args().collect(),
        tool_name: TOOL_NAME.into(),
        tool_version: TOOL_VERSION.into(),
        jurisdiction: a.jurisdiction.clone().unwrap_or_else(|| "co".into()),
        tsa_url: if a.no_timestamp {
            None
        } else {
            Some(a.tsa_url.clone())
        },
        sign_password: a
            .sign_cert
            .as_ref()
            .and_then(|_| password(&a.sign_pass_env)),
        sign_cert: a.sign_cert.clone(),
        case_dir: Some(a.case_dir.clone()),
        evidence_ids: vec![],
        case_role: CASE_ROLE,
    };

    let bar = match a.progress {
        ProgressMode::Bar => {
            let b = ProgressBar::with_draw_target(Some(0), ProgressDrawTarget::stderr_with_hz(8));
            b.set_style(
                ProgressStyle::with_template(
                    "{spinner} [{elapsed_precise}] {wide_bar} {pos}/{len} runs {msg}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            Some(b)
        }
        _ => None,
    };
    let jsonl = matches!(a.progress, ProgressMode::Jsonl);
    // A closed stdout (e.g. `| head`) must never abort a forensic run half-way: stop printing, keep working.
    let mut stdout_open = true;
    let mut sink = |e: &Event| {
        if jsonl && stdout_open {
            let mut out = std::io::stdout().lock();
            stdout_open = writeln!(out, "{}", serde_json::to_string(e).unwrap_or_default())
                .and_then(|_| out.flush())
                .is_ok();
        }
        if let Some(b) = &bar {
            match e {
                Event::RunStarted { total_items, .. } => b.set_length(*total_items),
                Event::Progress { done, .. } => b.set_position(*done),
                Event::ItemFinished { path, status, .. } if status == "excluded" => b.println(
                    format!("warning: run excluded (not verified, not quoted): {}", path),
                ),
                Event::Error { path, message } => b.println(format!(
                    "warning: {}: {}",
                    path.as_deref().unwrap_or("-"),
                    message
                )),
                Event::Cancelled { .. } => b.println("cancelled"),
                Event::Timestamp { status, reason, .. } if status != "ok" => b.println(format!(
                    "warning: timestamp pending ({}); run `rv-report timestamp` later",
                    reason.as_deref().unwrap_or("-")
                )),
                _ => {}
            }
        }
    };
    let result = run::run_assemble(&cfg, &opts, &mut sink, &cancel);
    if let Some(b) = &bar {
        b.finish_and_clear();
    }
    let s = match result {
        Err(e) => {
            eprintln!("error: {}", e);
            return EXIT_ERROR;
        }
        Ok(s) => s,
    };
    let facts = audit_facts(&s.log_dir);
    let to_review = !facts.excluded.is_empty() || s.high > 0 || s.medium > 0;
    if jsonl {
        // stdout carries only JSON events; exclusions are repeated on stderr for the operator.
        for x in &facts.excluded {
            eprintln!("warning: run excluded (not verified, not quoted): {}", x);
        }
    } else {
        let mut out = format!(
            "{status}  run {id}\n  case:      {case}\n  report:    {report} ({state})\n  runs:      verified {verified}, excluded {excluded}, skipped {skipped}\n  quoted:    {quoted} findings  items: {items}  elapsed: {elapsed}\n  signals:   high {h}, medium {m}, low {l}, info {i}\n  timestamp: {ts}\n  logs:      {logs}",
            status = if s.cancelled {
                "CANCELLED"
            } else if to_review {
                "DONE (to review: excluded runs or medium/high signals)"
            } else {
                "DONE"
            },
            id = s.run_id,
            case = s
                .case
                .as_ref()
                .map(|c| c.case_id.clone())
                .unwrap_or_default(),
            report = s
                .report_dir
                .join(format!("expert-report.{}.html", cfg.langs[0]))
                .display(),
            state = if facts.status == "final" {
                "final"
            } else {
                "draft for the examiner's review"
            },
            verified = facts.runs_verified,
            excluded = facts.excluded.len(),
            skipped = facts.skipped,
            quoted = facts.findings_quoted,
            items = s.items,
            elapsed = rv_report_core::time::human_duration(s.elapsed_ms),
            h = s.high,
            m = s.medium,
            l = s.low,
            i = s.info,
            ts = format!(
                "{} {}",
                s.timestamp["status"].as_str().unwrap_or("off"),
                s.timestamp["gen_time"].as_str().unwrap_or("")
            )
            .trim(),
            logs = s.log_dir.display()
        );
        for x in &facts.excluded {
            out.push_str(&format!("\n  excluded:  {}", x));
        }
        let _ = writeln!(std::io::stdout(), "{}", out);
    }
    if s.cancelled {
        EXIT_CANCELLED
    } else if to_review {
        EXIT_FINDINGS
    } else {
        EXIT_OK
    }
}

fn password(env: &str) -> Option<String> {
    if let Ok(p) = std::env::var(env) {
        return Some(p);
    }
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        // No echo: the password never appears on screen or in a terminal recording.
        return rpassword::prompt_password("Certificate password: ").ok();
    }
    None
}

fn timestamp(log: PathBuf, tsa_url: String, actor: Option<String>) -> u8 {
    let dir = if log.is_file() {
        log.parent().map(|p| p.to_path_buf()).unwrap_or_default()
    } else {
        log
    };
    if !dir.join("audit.jsonl").is_file() {
        eprintln!(
            "error: no audit log at {}: nothing to timestamp",
            dir.join("audit.jsonl").display()
        );
        return EXIT_FINDINGS;
    }
    let mut audit_log = match audit::AuditLog::resume(
        &dir,
        &actor.unwrap_or_else(rv_report_core::env::os_user),
    ) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {}", e);
            return EXIT_FINDINGS;
        }
    };
    match trust::timestamp_head(&dir, audit_log.entries(), audit_log.head(), &tsa_url) {
        Ok(ts) => {
            if let Err(e) = audit_log.record("timestamp_obtained", Some(&ts.file), ts.to_json()) {
                eprintln!("error: {}", e);
                return EXIT_ERROR;
            }
            println!(
                "OK  timestamp {} by {} covers entry #{}",
                ts.gen_time, ts.tsa, ts.covers_seq
            );
            EXIT_OK
        }
        Err(e) => {
            eprintln!("error: {}", e);
            EXIT_ERROR
        }
    }
}

fn verify_log(log: PathBuf, tsa_ca: Option<PathBuf>) -> u8 {
    let path = if log.is_dir() {
        log.join("audit.jsonl")
    } else {
        log
    };
    let anchors = match tsa_ca.as_deref().map(trust::load_ca_bundle).transpose() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {}", e);
            return EXIT_ERROR;
        }
    };
    let v = match verify::verify_log_with(&path, anchors.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {}", e);
            return EXIT_ERROR;
        }
    };
    if v.problems.is_empty() {
        println!(
            "OK  {} entries, chain intact: {}",
            v.entries,
            path.display()
        );
        for t in &v.timestamps {
            println!("    {}", t);
        }
        println!("    {}", v.truncation);
    } else {
        println!("BROKEN  {}", path.display());
        for p in &v.problems {
            println!("  - {}", p);
        }
    }
    if let Some(o) = &v.outputs {
        println!("    {}", o.summary());
        for s in o.skipped_lines() {
            println!("    {}", s);
        }
    }
    for w in &v.warnings {
        println!("  warning: {}", w);
    }
    if v.problems.is_empty() {
        EXIT_OK
    } else {
        EXIT_FINDINGS
    }
}

fn verify_case(dir: PathBuf, rehash: bool) -> u8 {
    if !dir.join(case::CASE_FILE).is_file() {
        eprintln!(
            "error: {} is not a case folder (no {})",
            dir.display(),
            case::CASE_FILE
        );
        return EXIT_ERROR;
    }
    let mut v = case::verify_case(&dir);
    let rehashed = rehash.then(|| case::rehash_registry(&dir, &mut v));
    let ok = v.problems.is_empty();
    if ok {
        println!(
            "OK  case {}: {} case-log entries, {} registered items, {} runs verified: {}",
            v.case_id,
            v.entries,
            v.registry_items,
            v.runs,
            dir.display()
        );
    } else {
        println!("BROKEN  case {}: {}", v.case_id, dir.display());
        for p in &v.problems {
            println!("  - {}", p);
        }
    }
    println!(
        "    outputs: {} verified of {} recorded by the runs' audit logs",
        v.outputs_verified, v.outputs_recorded
    );
    if let Some(r) = &rehashed {
        println!(
            "    re-hashed {} of {} registered items: {} match the registry",
            r.checked, r.registered, r.matched
        );
    }
    for w in &v.warnings {
        println!("  warning: {}", w);
    }
    if ok { EXIT_OK } else { EXIT_FINDINGS }
}

fn self_test() -> u8 {
    let failed = hash::self_test();
    for (algo, _) in hash::KNOWN_ANSWERS {
        println!(
            "{:<9} {}",
            algo.name(),
            if failed.contains(&algo) {
                "FAIL"
            } else {
                "PASS"
            }
        );
    }
    if failed.is_empty() {
        EXIT_OK
    } else {
        EXIT_ERROR
    }
}

/// The tool's self-description (`describe --json`, schema rakvor.describe/1; see schemas/describe.schema.json).
/// serde_json sorts the keys (no preserve_order feature), so the output is stable.
fn describe() -> serde_json::Value {
    serde_json::json!({
        "schema": "rakvor.describe/1",
        "tool": TOOL_NAME,
        "title": "RV Report",
        "version": TOOL_VERSION,
        "role": CASE_ROLE.name(),
        "main_verb": "assemble",
        // The input is a case folder: the files read directly are the case's JSON documents and JSON Lines logs
        // and the other tools' reports. No evidence signature: never suggested for an evidence file.
        "accepts": {"extensions": ["json", "jsonl"], "signatures": []},
        "case_formats": [case::SCHEMA_CASE],
        "log_formats": ["rakvor.audit/1"],
        "cli_exe": format!("{}.exe", TOOL_NAME),
        "gui_exe": format!("{}-gui.exe", TOOL_NAME),
        "exit_codes": {
            "0": "Expert report assembled; every run used verified and no medium- or high-severity signal.",
            "1": "Runtime error (case log locked, output not writable, signing certificate, ...).",
            "2": "Usage error (unknown option, bad value, mandate file, not a case folder, case folder of another case).",
            "3": "Report assembled with an excluded run or a medium/high signal to review, or the case folder fails verification.",
            "130": "Cancelled with Ctrl+C; the cancellation is in the audit log."
        }
    })
}

/// Markdown of every command's long help, so docs/usage-cli.md can never drift from the binary.
fn docs_cli() -> String {
    let mut root = Cli::command().term_width(100);
    root.build();
    let mut out = String::from(
        "# Command-line reference\n\n<!-- Generated by `rv-report docs-cli`. Do not edit by hand: CI fails if this file differs. -->\n\n",
    );
    out.push_str("```mermaid\nflowchart LR\n  A[rv-report] --> B[assemble]\n  A --> C[verify-log]\n  A --> D[self-test]\n  A --> T[timestamp]\n  A --> V[verify-case]\n  K[(CASE folder<br/>runs of the other tools)] -- verified, quoted --> B\n  M[/mandate.txt/] --> B\n  B --> E[(runs/rv-report/RUN-ID/logs)]\n  B --> F[(runs/rv-report/RUN-ID/reports<br/>expert-report.LANG.html/pdf)]\n  V --> K\n  T --> E\n  C --> E\n```\n\n");
    out.push_str(&format!(
        "## rv-report\n\n```text\n{}\n```\n\n",
        root.render_long_help().to_string().trim_end()
    ));
    let names: Vec<String> = root
        .get_subcommands()
        // `describe` is hidden from the command list but documented (RV Hub reads it).
        .filter(|c| (!c.is_hide_set() || c.get_name() == "describe") && c.get_name() != "help")
        .map(|c| c.get_name().to_string())
        .collect();
    for name in names {
        let sub = root.find_subcommand_mut(&name).expect("subcommand");
        let mut sub = sub.clone().bin_name(format!("rv-report {}", name));
        out.push_str(&format!(
            "## rv-report {}\n\n```text\n{}\n```\n\n",
            name,
            sub.render_long_help().to_string().trim_end()
        ));
    }
    out
}
