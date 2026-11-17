# md2docx.ps1 - convert a Markdown document from the current folder into DOCX
# with pandoc (https://pandoc.org - a universal document converter that turns
# Markdown, HTML, LaTeX, docx, ... into each other from the command line).
#
# Usage:
#   .\md2docx.ps1 <name> [-Force] [-ReferenceDoc <path>] [-Open]
#
# <name> is a file name (in the current folder by default), with or without
# the Markdown extension. Windows wildcards are supported:
#   *  any number of characters (including none)
#   ?  exactly one character
# The ".md" extension is appended automatically when the pattern does not
# already end with .md/.markdown, so all of these work:
#   .\md2docx.ps1 README
#   .\md2docx.ps1 "PLAN_*"
#   .\md2docx.ps1 "RELEASE_NOTES_v0.2?.md"
# If a literal file name contains '[' or ']', escape them for the wildcard
# engine: "notes`[1`].md".
#
# Every match is converted to <name>.docx next to the source file. Existing
# .docx files are skipped unless -Force is given. -ReferenceDoc points to a
# .docx whose styles are reused for the output (see pandoc --reference-doc);
# a default reference document can be produced with:
#   pandoc -o reference.docx --print-default-data-file reference.docx
#
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Exit codes: 0 = converted (or nothing to do), 1 = conversion errors,
#             2 = bad arguments / pandoc missing / no matching files.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0, HelpMessage = 'Markdown file name (extension optional, wildcards * and ? allowed)')]
    [string]$Name,

    [switch]$Force,

    [string]$ReferenceDoc = '',

    [switch]$Open
)

$ErrorActionPreference = 'Stop'

function Write-ErrorLine([string]$Message) {
    Write-Host "ERROR: $Message" -ForegroundColor Red
}

# --- 1. pandoc must be available -------------------------------------------
$pandocCmd = Get-Command -Name 'pandoc' -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $pandocCmd) {
    Write-ErrorLine 'pandoc not found in PATH.'
    Write-Host '  Install it: https://pandoc.org/installing.html' -ForegroundColor Yellow
    Write-Host '  Windows:   winget install --id JohnMacFarlane.Pandoc' -ForegroundColor Yellow
    exit 2
}
$pandocExe = $pandocCmd.Source

$Name = $Name.Trim()
if ([string]::IsNullOrWhiteSpace($Name)) {
    Write-ErrorLine 'empty file name. Usage: .\md2docx.ps1 <name-with-or-without-extension>'
    exit 2
}

# --- 2. optional reference document ----------------------------------------
$refDoc = ''
if ($ReferenceDoc) {
    if (-not (Test-Path -LiteralPath $ReferenceDoc -PathType Leaf)) {
        Write-ErrorLine "reference document not found: $ReferenceDoc"
        exit 2
    }
    $refDoc = (Resolve-Path -LiteralPath $ReferenceDoc).ProviderPath
}


# --- 3. resolve the wildcard pattern into Markdown files -------------------
# The leaf name may omit the extension: "<name>.md" and "<name>.markdown" are
# tried in turn, then the bare "<name>" (a name that already carries an
# extension). Every candidate is filtered to Markdown files, so a pattern like
# "PLAN_*" can never pick up .txt/.docx files.
$leaf = Split-Path -Leaf $Name
$hasMdExt = $leaf -match '(?i)\.(md|markdown)$'
$patterns = @()
if ($hasMdExt) {
    $patterns += $Name
}
else {
    $patterns += "$Name.md"
    $patterns += "$Name.markdown"
    $patterns += $Name
}

$files = @()
$tried = @()
foreach ($pattern in $patterns) {
    $tried += $pattern
    try {
        $found = @(Get-ChildItem -Path $pattern -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Extension -match '(?i)^\.(md|markdown)$' })
    }
    catch {
        $found = @()
    }
    if ($found.Count -gt 0) {
        $files = @($found | Sort-Object -Property FullName -Unique)
        break
    }
}

if ($files.Count -eq 0) {
    Write-ErrorLine ("no Markdown files match '{0}' (tried: {1}) in {2}" -f `
            $Name, ($tried -join ', '), (Get-Location).ProviderPath)
    exit 2
}

$pandocVersion = & $pandocExe --version | Select-Object -First 1
Write-Host ("using {0}" -f $pandocVersion)
Write-Host ("matched {0} file(s): {1}" -f $files.Count, (($files | ForEach-Object { $_.Name }) -join ', '))
Write-Host ''

# --- 4. convert -------------------------------------------------------------
$converted = 0
$skipped = 0
$failed = 0
$outputs = @()
$produced = @{}

foreach ($file in $files) {
    $outName = $file.BaseName + '.docx'
    $outPath = Join-Path $file.DirectoryName $outName
    $outKey = $outPath.ToLowerInvariant()

    if ($produced.ContainsKey($outKey)) {
        Write-Host ("SKIP  {0} -> {1} (same output as an earlier match)" -f $file.Name, $outName) -ForegroundColor Yellow
        $skipped++
        continue
    }
    $produced[$outKey] = $true

    if ((Test-Path -LiteralPath $outPath) -and -not $Force) {
        Write-Host ("SKIP  {0} -> {1} (already exists, use -Force to overwrite)" -f $file.Name, $outName) -ForegroundColor Yellow
        $skipped++
        continue
    }

    # Explicit --from/--to: the input is always Markdown even when the file is
    # named *.markdown, and the output format must not depend on the extension.
    $pandocArgs = @(
        $file.FullName
        '--from', 'markdown'
        '--to', 'docx'
        '--output', $outPath
    )
    if ($refDoc) {
        $pandocArgs += @('--reference-doc', $refDoc)
    }

    Write-Host ("CONV  {0} -> {1}" -f $file.Name, $outName)
    & $pandocExe @pandocArgs
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        Write-Host ("FAIL  {0}: pandoc exited with code {1}" -f $file.Name, $exitCode) -ForegroundColor Red
        $failed++
        continue
    }
    if (-not (Test-Path -LiteralPath $outPath -PathType Leaf)) {
        Write-Host ("FAIL  {0}: pandoc reported success but '{1}' is missing" -f $file.Name, $outPath) -ForegroundColor Red
        $failed++
        continue
    }

    $sizeKb = [math]::Round((Get-Item -LiteralPath $outPath).Length / 1KB, 1)
    Write-Host ("OK    {0} ({1} KB)" -f $outPath, $sizeKb) -ForegroundColor Green
    $converted++
    $outputs += $outPath
}

# --- 5. summary -------------------------------------------------------------
Write-Host ''
Write-Host ("converted: {0}  skipped: {1}  failed: {2}" -f $converted, $skipped, $failed)

if ($Open) {
    foreach ($outPath in $outputs) {
        Start-Process -FilePath $outPath
    }
}

if ($failed -gt 0) {
    exit 1
}
exit 0
