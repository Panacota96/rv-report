# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

## [0.3.1](https://github.com/Panacota96/rv-report/compare/v0.3.0...v0.3.1) (2026-11-24)


### Performance Improvements

* **verify:** stream the audit log instead of holding every entry ([#10](https://github.com/Panacota96/rv-report/issues/10)) ([914ec6d](https://github.com/Panacota96/rv-report/commit/914ec6d452ba2706ee19a32c45b28ce8b0a3605c))

## [0.3.0](https://github.com/Panacota96/rv-report/compare/v0.2.0...v0.3.0) (2026-11-21)


### Features

* **update:** check for a newer release (update-check, start-up banner) ([#8](https://github.com/Panacota96/rv-report/issues/8)) ([d8a97c3](https://github.com/Panacota96/rv-report/commit/d8a97c3793a1d07379807ab35e34472dc03377df))

## [0.2.0](https://github.com/Panacota96/rv-report/compare/v0.1.0...v0.2.0) (2026-11-19)


### Features

* RV Report v0.1 — expert report assembled from a verified case ([#7](https://github.com/Panacota96/rv-report/issues/7)) ([616ad02](https://github.com/Panacota96/rv-report/commit/616ad025a1e8c3560a158550057cf5c01e11bf8c))


### Bug Fixes

* **core:** late audit entries and redirected run folders cannot hide an edited output ([b5e2e73](https://github.com/Panacota96/rv-report/commit/b5e2e73ac6b365d230e160dc5c64caeef55f25fb))
* **core:** re-hash the outputs a run's audit log records in verify-log and verify-case ([7a7a848](https://github.com/Panacota96/rv-report/commit/7a7a848ef86fc46b3c842f428ca26dae7cbcc6f5))
* verify-log and verify-case re-hash the outputs the audit log records ([c841655](https://github.com/Panacota96/rv-report/commit/c84165570b110a95a561711b84d7d28124e6e098))

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
