#!/usr/bin/env bash
# Builds examples/sample/case/: a small Rakvor case folder written by released Rakvor tools on the synthetic
# evidence of examples/sample/evidence (4 files), so `rv-report assemble` has real, audited runs to assemble.
#
#   1. rv-hashinv scan   intake: registers the 4 files (E0001..E0004, sorted by path)
#   2. rv-pdfauth analyze  pdf/            reports in en, es and fr
#   3. rv-imgauth analyze  images/         reports in en only (the expert report then shows the English wording,
#                                          marked, in its es/fr versions: TRANSLATION_UNAVAILABLE)
#   4. rv-timeline build   the case timeline (RV Report embeds its summary)
#   notes/acta.txt is registered but examined by no analysis tool: it is listed under "what was not examined".
#
# Usage: tests/make_sample_case.sh [RAKVOR_ROOT]
#   RAKVOR_ROOT  folder holding rv-hashinv, rv-pdfauth and rv-imgauth, each with a release build in
#                <repo>/target/release (default: the parent of this repository).
# Environment:
#   RV_TIMELINE_EXE  rv-timeline executable (default: RAKVOR_ROOT/rv-timeline/target/release/rv-timeline[.exe])
#   RV_SAMPLE_DRIVE  Windows only: free drive letter mapped with `subst` onto the work folder, so the paths the
#                    tools record are neutral (R:\evidence\...) instead of the user's profile. Default: R:
#   RV_SAMPLE_ROOT   Linux/macOS: work folder (its absolute path is recorded by the tools). Default: /tmp/rv-sample
#
# Nothing here is personal: neutral host and user names, synthetic evidence, no timestamp, no signing certificate.
# Not deterministic: run ids and run times (the tools' clock). The tests never compare them.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
rakvor=$(cd "${1:-$here/..}" && pwd)
sample="$here/examples/sample"
tools="rv-hashinv rv-pdfauth rv-imgauth"

case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) windows=1; exe=.exe ;; *) windows=0; exe= ;; esac
timeline=${RV_TIMELINE_EXE:-$rakvor/rv-timeline/target/release/rv-timeline$exe}
[ -x "$timeline" ] || { echo "missing $timeline (set RV_TIMELINE_EXE)" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/rv-report-sample.XXXXXX")
if [ "$windows" = 1 ]; then
  drive=${RV_SAMPLE_DRIVE:-R:}
  if [ -e "$drive/" ]; then echo "drive $drive is in use; set RV_SAMPLE_DRIVE to a free letter" >&2; exit 1; fi
  subst "$drive" "$(cygpath -w "$work")"
  trap 'cd /; MSYS_NO_PATHCONV=1 subst "$drive" /d >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
  root="$drive"
  w() { printf '%s\%s' "$drive" "$(printf '%s' "$1" | tr '/' '\')"; }
else
  root=${RV_SAMPLE_ROOT:-/tmp/rv-sample}
  rm -rf "$root"; mkdir -p "$root"
  trap 'rm -rf "$work" "$root"' EXIT
  w() { printf '%s/%s' "$root" "$1"; }
fi
mkdir -p "$root/bin"
for t in $tools; do
  src="$rakvor/$t/target/release/$t$exe"
  [ -x "$src" ] || { echo "missing $src" >&2; exit 1; }
  cp "$src" "$root/bin/"
done
cp "$timeline" "$root/bin/rv-timeline$exe"
bin() { printf '%s/bin/%s%s' "$root" "$1" "$exe"; }

# Evidence: copied byte for byte; one fixed modification time.
cp -R "$sample/evidence" "$root/evidence"
find "$root/evidence" -type f -exec touch -d '2026-11-12T06:00:00Z' {} +

export COMPUTERNAME=RV-SAMPLE-HOST HOSTNAME=RV-SAMPLE-HOST USERNAME=sample-examiner USER=sample-examiner
export RV_NO_TIMESTAMP=1
mkdir -p "$root/profile/local" "$root/profile/roaming" "$root/profile/tmp"
export LOCALAPPDATA="$(w profile/local)" APPDATA="$(w profile/roaming)" USERPROFILE="$(w profile)" \
  XDG_DATA_HOME="$(w profile/local)" XDG_CONFIG_HOME="$(w profile/roaming)"
tmp_tools="$(w profile/tmp)"
cd "$root/"
case_dir=$(w case)
common=(--case-dir "$case_dir" --hash sha256,md5 --actor "Sample Examiner" --no-timestamp --progress none)

run() {
  local t=$1 ok=$2 rc=0; shift 2
  echo "== $t $1"
  TEMP="$tmp_tools" TMP="$tmp_tools" "$(bin "$t")" "$@" || rc=$?
  case " $ok " in *" $rc "*) ;; *) echo "$t exited $rc (expected one of: $ok)" >&2; exit 1 ;; esac
}

run rv-hashinv "0 3" scan -i "$(w evidence)" -r --case RV-REPORT-SAMPLE --jurisdiction co --lang en "${common[@]}"
run rv-pdfauth "0 3" analyze -i "$(w evidence/pdf)" --lang all "${common[@]}"
run rv-imgauth "0 3" analyze -i "$(w evidence/images)" --maps none --lang en "${common[@]}"
run rv-timeline "0 3" build --lang en "${common[@]}"

{
  printf '{\n  "description": "Executables that wrote examples/sample/case (released builds; SHA-256 of each executable).",\n  "tools": {\n'
  first=1
  for t in $tools rv-timeline; do
    v=$("$(bin "$t")" --version | tr -d '\r'); v=${v#"$t "}
    s=$(sha256sum "$(bin "$t")" | cut -d' ' -f1)
    [ $first = 1 ] || printf ',\n'; first=0
    printf '    "%s": {"version": "%s", "exe_sha256": "%s"}' "$t" "$v" "$s"
  done
  printf '\n  }\n}\n'
} > "$work/tool-versions.json"

"$(bin rv-timeline)" verify-case "$case_dir" > "$work/verify.txt" || { cat "$work/verify.txt"; echo "verify-case failed" >&2; exit 1; }
cat "$work/verify.txt"
rm -rf "$sample/case"
cp -R "$root/case" "$sample/case"
cp "$work/tool-versions.json" "$sample/tool-versions.json"
echo "sample case written to $sample/case ($(du -sh "$sample/case" | cut -f1))"
