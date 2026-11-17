#!/usr/bin/env sh
# Assembles the expert report of the sample case and checks the result. Usage: examples/sample/run.sh [path-to-rv-report]
# Works on a copy of examples/sample/case in examples/sample/out (or $RV_SAMPLE_OUT), so the committed case is
# never changed. Fails (non-zero exit) if the self-test fails, if `assemble` does not end with exit code 0, if
# reports/expert-report.json is missing or not listed with its SHA-256 in the run's audit log, or if verify-log or
# verify-case fails.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-rv-report}
out=${RV_SAMPLE_OUT:-"$here/out"}
rm -rf "$out/case"
mkdir -p "$out"
cp -R "$here/case" "$out/case"
"$bin" self-test
# Expected exit code 0: every run verifies; the signals are low (an item no analysis tool examined) and info.
code=0
"$bin" assemble --case-dir "$out/case" --hash sha256,md5 --lang all --actor "Sample" --progress none || code=$?
if [ "$code" -ne 0 ]; then
  echo "FAIL: assemble exited with $code, expected 0"
  exit 1
fi
run=$(ls "$out/case/runs/rv-report" | tail -n 1)
dir="$out/case/runs/rv-report/$run"
if [ ! -s "$dir/reports/expert-report.json" ]; then
  echo "FAIL: $dir/reports/expert-report.json is missing or empty"
  exit 1
fi
want=$(sha256sum "$dir/reports/expert-report.json" | cut -d' ' -f1)
if ! grep '"action":"output_written"' "$dir/logs/audit.jsonl" | grep '"file":"expert-report.json"' | grep -q "\"sha256\":\"$want\""; then
  echo "FAIL: expert-report.json (SHA-256 $want) is not listed in the audit log"
  exit 1
fi
echo "expert-report.json is listed in the audit log (SHA-256 $want)"
"$bin" verify-log "$dir/logs"
"$bin" verify-case "$out/case"
echo "Report: $dir/reports/expert-report.en.html"
