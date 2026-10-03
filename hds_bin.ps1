# Shared binary resolver for the launcher / installer scripts.
#
# A packaged release ships the Rust binaries in bin\ (that is where setup.ps1 and the
# release staging put them). A source checkout has no bin\ at all - cargo leaves the
# binaries in target\<profile>\. Without this helper run_ui.ps1 (the desktop shortcut)
# and the autostart installers failed with "bin\hds.exe not found" when run straight
# from the repository. Resolving the location here makes both layouts work:
#   bin\                         -> packaged release / install_app_version (authoritative)
#   newest of target\release\ or
#   target\debug\                -> source checkout (a stale profile must not shadow a
#                                   freshly built one, so the newest wins)
#
# Usage:
#   . (Join-Path $PSScriptRoot 'hds_bin.ps1')
#   $hdsExe = Get-HdsBinPath -Root $root -Name "hds.exe"
#
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).

function Get-HdsBinPath {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Name
    )
    # Packaged release / install_app_version layout: bin\ is authoritative.
    $bin = Join-Path $Root "bin\$Name"
    if (Test-Path -LiteralPath $bin) { return $bin }

    # Source checkout: cargo keeps binaries in target\<profile>\. A stale profile is
    # common (e.g. an old release build while the tree was rebuilt in debug), so pick
    # the MOST RECENT one instead of blindly preferring release - otherwise the
    # launcher may pick a binary that predates a newly added subcommand.
    $dev = @()
    foreach ($c in @((Join-Path $Root "target\release\$Name"),
                     (Join-Path $Root "target\debug\$Name"))) {
        if (Test-Path -LiteralPath $c) { $dev += (Get-Item -LiteralPath $c) }
    }
    if ($dev.Count -eq 0) { return $null }
    return ($dev | Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
}

# Human-readable list of the locations Get-HdsBinPath searches - for error messages.
function Get-HdsBinHint {
    param([Parameter(Mandatory = $true)][string]$Name)
    return "bin\$Name, target\release\$Name, target\debug\$Name (newest dev build wins)"
}
