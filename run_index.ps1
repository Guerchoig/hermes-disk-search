# Run a one-shot incremental indexing pass (Rust build).
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
$root = $PSScriptRoot
Set-Location $root
. (Join-Path $root 'hds_bin.ps1')
# bin\ (packaged release) or target\{release,debug}\ (source checkout).
$hdsExe = Get-HdsBinPath -Root $root -Name "hds.exe"
if (-not $hdsExe) {
    Write-Host "== hds.exe not found. Looked in: $(Get-HdsBinHint 'hds.exe') ==" -ForegroundColor Yellow
    Write-Host "   Run setup.cmd (release) or build it: cargo build --release -p hds-cli" -ForegroundColor Yellow
    Read-Host "Press Enter to exit"
    exit 1
}
Write-Host "== hermes-disk-search: indexing (incremental) ==" -ForegroundColor Cyan
& $hdsExe index
Write-Host ""
Read-Host "Done. Press Enter to exit"
