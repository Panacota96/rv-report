# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

## [Unreleased]

### Added
- `rv-report assemble`: one expert report (dictamen pericial, Colombia; rapport d'expertise, France) from the
  verified runs of a case folder, as JSON (`rakvor.expert-report/1`) + HTML + signed PDF in EN/ES/FR: identification,
  mandate, evidence and custody, method per tool, results quoted verbatim, RV Timeline summary, limitations, what
  was not examined, annexes, declaration template; draft until `--final`; every legal reference "to be verified".
- Verify before quote: runs that fail verification are excluded with their reason and never quoted.
- GUI: case folder picker, read-only section preview, generation, audited `ui_*` actions.
- Sample case written by the released Rakvor tools; end-to-end and tampering tests.
- Project scaffolded with rakvor-tool-forge (rust profile) on 2026-11-11.
