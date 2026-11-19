# Architecture

One core library that never prints; the CLI and the GUI are thin front-ends that build a `RunConfig` +
`AssembleOptions`, call the core and render its events.

```mermaid
flowchart LR
    CLI["src/cli<br/>assemble, verify-*"] --> RUN
    GUI["src/gui<br/>state.rs + app.rs"] --> RUN
    GUI -. preview .-> PRE["run::preview_assembly<br/>(read-only)"]
    RUN["run::run_assemble"] --> CASE["case.rs<br/>open, verify, lock, append"]
    RUN --> ASM["assemble.rs"]
    PRE --> ASM
    ASM --> SRC["sources.rs<br/>discover, verify, read"]
    SRC --> VER["verify.rs + outputs.rs + trust.rs<br/>(shared)"]
    ASM --> MOD[["model.rs<br/>ExpertReport"]]
    RUN --> EXP["report/expert.rs<br/>HTML + PDF"]
    RUN --> REP["report/mod.rs<br/>statistics + items"]
    RUN --> AUD["audit.rs"]
```

## Modules

| File | Role | Shared with the suite |
| --- | --- | --- |
| `src/core/src/model.rs` | frozen data model of the report (`rakvor.expert-report/1`) | — |
| `src/core/src/sources.rs` | run discovery, verification, verified reads (adapted from RV Timeline) | pattern |
| `src/core/src/assemble.rs` | builds the model: skip/verify/quote, custody, method, limitations, timeline, not examined, signals | — |
| `src/core/src/report/expert.rs` | view model → HTML (minijinja) and PDF (built-in writer), per language | — |
| `src/core/src/run.rs` | `run_assemble` (pipeline, audit, case link), `preview_assembly`; template `run` kept for its tests | partly |
| `src/core/src/{audit,case,verify,i18n,events,env}.rs` | audit chain, case folder, verify-log, wording, events | yes |
| `src/core/src/{trust,outputs,hash,time}.rs`, `report/pdf.rs` | timestamps and signing, output re-hash, hashing, time, PDF writer | **byte-identical** across repos |
| `src/gui/src/state.rs` | every GUI rule, audited `ui_*` actions, preview sections | — |

## One run, step by step

```mermaid
sequenceDiagram
    autonumber
    participant F as front-end
    participant R as run_assemble
    participant C as case log
    participant A as assemble
    participant L as audit.jsonl
    F->>R: RunConfig + AssembleOptions
    R->>R: mandate readable? jurisdiction valid?
    R->>C: Case::open (verify) then run_started (under the lock)
    R->>L: run_started, case_linked, ui_*, environment_recorded, signing, report_marked_final
    R->>A: assemble (read-only)
    A-->>R: model + notes + files read
    R->>L: run_skipped / run_verified / run_excluded / run_quoted, report_assembled
    R->>L: hash_register_written, output_written x (1 + 2 x langs)
    R->>L: item_assembled, item_reports_written per item
    R->>L: timestamp_obtained | timestamp_pending, statistics_report_written, run_finished
    R->>C: run_finished (audit head read back from disk)
```

## Run states (as the GUI sees them)

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Idle: case chosen (verified), preview built
    Idle --> Running: Generate
    Running --> Finished
    Running --> Cancelled
    Running --> Failed
    Finished --> Idle: new settings
```

## Concurrency and cancellation

The GUI runs the core on a `std::thread` and receives events through an `mpsc` channel. The assembly is short (it
reads records, not evidence); Ctrl+C or Cancel is checked after the assembly: the run then records `run_cancelled`,
still writes the (partial) reports with the cancellation noted in the statistics report, and ends with
`run_finished {cancelled: true}` (exit 130).

## Why Rust

Same profile as the other Rakvor tools for one shared, audited core ([ADR 0001](adr/0001-language.md)).
