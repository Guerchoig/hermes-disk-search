# Run a one-shot incremental indexing pass (Rust build).
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
$root = $PSScriptRoot
Set-Location $root
$hdsExe = Join-Path $root "bin\hds.exe"
if (-not (Test-Path $hdsExe)) {
    Write-Host "== bin\hds.exe not found. Run setup.cmd first ==" -ForegroundColor Yellow
    Read-Host "Press Enter to exit"
    exit 1
}
Write-Host "== hermes-disk-search: indexing (incremental) ==" -ForegroundColor Cyan
& $hdsExe index
Write-Host ""
Read-Host "Done. Press Enter to exit"
