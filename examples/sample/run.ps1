# Assembles the expert report of the sample case on Windows and checks the result.
# Works on a copy of examples\sample\case in examples\sample\out (or $env:RV_SAMPLE_OUT), so the committed case is
# never changed. Fails (non-zero exit) if the self-test fails, if `assemble` does not end with exit code 0, if
# reports\expert-report.json is missing or not listed with its SHA-256 in the run's audit log, or if verify-log or
# verify-case fails.
# Usage: .\examples\sample\run.ps1 [-Bin C:\path\to\rv-report.exe]
param([string]$Bin = "rv-report")
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$out = if ($env:RV_SAMPLE_OUT) { $env:RV_SAMPLE_OUT } else { Join-Path $here "out" }
$case = Join-Path $out "case"
if (Test-Path $case) { Remove-Item -Recurse -Force $case }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item -Recurse (Join-Path $here "case") $case
& $Bin self-test; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
# Expected exit code 0: every run verifies; the signals are low (an item no analysis tool examined) and info.
& $Bin assemble --case-dir $case --hash sha256,md5 --lang all --actor "Sample" --progress none
if ($LASTEXITCODE -ne 0) { Write-Host "FAIL: assemble exited with $LASTEXITCODE, expected 0"; exit 1 }
$run = Get-ChildItem (Join-Path $case "runs\rv-report") | Sort-Object Name | Select-Object -Last 1
$report = Join-Path $run.FullName "reports\expert-report.json"
if (-not (Test-Path $report) -or (Get-Item $report).Length -eq 0) {
  Write-Host "FAIL: $report is missing or empty"; exit 1
}
$want = (Get-FileHash -Algorithm SHA256 $report).Hash.ToLowerInvariant()
$listed = Select-String -Path (Join-Path $run.FullName "logs\audit.jsonl") -SimpleMatch '"file":"expert-report.json"' |
  Where-Object { $_.Line.Contains('"action":"output_written"') -and $_.Line.Contains("""sha256"":""$want""") }
if (-not $listed) { Write-Host "FAIL: expert-report.json (SHA-256 $want) is not listed in the audit log"; exit 1 }
Write-Host "expert-report.json is listed in the audit log (SHA-256 $want)"
& $Bin verify-log (Join-Path $run.FullName "logs"); if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $Bin verify-case $case; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "Report: $($run.FullName)\reports\expert-report.en.html"
