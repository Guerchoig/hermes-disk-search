# sidecar_common.ps1 - shared helpers for the Python sidecar installers (P5).
#
# Dot-source it from an installer script:
#   . (Join-Path $PSScriptRoot "sidecar_common.ps1")
#
# Provides: Get-SidecarRoot / Get-PortablePython / Ensure-PortablePython /
# New-SidecarVenv / Invoke-SidecarPip / Uninstall-SidecarPip /
# Install-SidecarRequirements /
# Test-SidecarImports / Copy-SidecarTree / Get-GpuVendor / Test-IntegratedGpu /
# Get-TranscribeProfile / Get-SidecarVenvName / Set-ConfigPath /
# Set-ConfigPathNested.
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).

function Get-SidecarRoot {
    param([string]$ScriptRoot)
    return (Resolve-Path (Join-Path $ScriptRoot "..")).Path
}

function Get-PortablePython {
    param([Parameter(Mandatory = $true)][string]$SidecarDir)
    $base = Join-Path $SidecarDir "python"
    if (-not (Test-Path $base)) { return $null }
    $exe = Get-ChildItem -Path $base -Recurse -Filter python.exe -ErrorAction SilentlyContinue |
        Sort-Object { $_.FullName.Length } | Select-Object -First 1
    if ($exe) { return $exe.FullName }
    return $null
}

function Ensure-PortablePython {
    # Bundled CPython from python-build-standalone via uv.
    #
    # For a DISTRIBUTION tree (OutDir != repo\sidecar) build_sidecar.ps1 owns
    # sidecar\python: it also copies the extractor and installs its
    # requirements. For the REPO tree build_sidecar.ps1 cannot run (it copies
    # sidecar\hds_extract\worker.py onto itself), so the interpreter and the
    # extractor dependencies are installed here - otherwise the extraction
    # worker would pick up a bare python without fitz/pymorphy3.
    param(
        [Parameter(Mandatory = $true)][string]$SidecarDir,
        [string]$PythonVersion = "3.12",
        [switch]$Force
    )
    $repoSidecar = Join-Path (Get-SidecarRoot -ScriptRoot $PSScriptRoot) "sidecar"
    $inPlace = $false
    try { $inPlace = ((Resolve-Path $SidecarDir).Path -eq (Resolve-Path $repoSidecar).Path) } catch { $inPlace = $false }
    $pyDir = Join-Path $SidecarDir "python"
    $py = Get-PortablePython -SidecarDir $SidecarDir
    if ($py -and -not $Force -and -not $inPlace) { return $py }
    if ($py -and -not $Force -and $inPlace -and (Test-Path (Join-Path $pyDir ".hds-deps"))) { return $py }

    if (-not $inPlace) {
        $builder = Join-Path (Split-Path -Parent $PSScriptRoot) "installers\build_sidecar.ps1"
        if (-not (Test-Path $builder)) { throw "build_sidecar.ps1 not found: $builder" }
        Write-Host "[..] portable CPython is missing - running build_sidecar.ps1"
        $a = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $builder,
            "-OutDir", $SidecarDir, "-PythonVersion", $PythonVersion)
        if ($Force) { $a += "-Force" }
        & powershell @a
        if ($LASTEXITCODE -ne 0) { throw "build_sidecar.ps1 failed (exit $LASTEXITCODE)" }
        $py = Get-PortablePython -SidecarDir $SidecarDir
        if (-not $py) { throw "python.exe not found under $SidecarDir\python after build_sidecar.ps1" }
        return $py
    }

    $uv = Get-Command uv -ErrorAction SilentlyContinue
    if (-not $uv) { throw "uv not found in PATH (https://docs.astral.sh/uv/) - needed to install the bundled CPython" }
    if (-not $py) {
        Write-Host "[..] uv python install $PythonVersion --install-dir $pyDir"
        & $uv.Source python install $PythonVersion --install-dir $pyDir
        if ($LASTEXITCODE -ne 0) { throw "uv python install failed" }
        New-Item -ItemType File -Force -Path (Join-Path $pyDir ".hds-python") | Out-Null
        $py = Get-PortablePython -SidecarDir $SidecarDir
        if (-not $py) { throw "python.exe not found under $pyDir" }
    }
    $lock = Join-Path $repoSidecar "hds_extract\requirements.lock"
    if ((Test-Path $lock) -and ($Force -or -not (Test-Path (Join-Path $pyDir ".hds-deps")))) {
        Write-Host "[..] extraction worker dependencies into $py"
        & $uv.Source pip install --python $py --break-system-packages -r $lock
        if ($LASTEXITCODE -ne 0) { throw "uv pip install (hds_extract) failed" }
        New-Item -ItemType File -Force -Path (Join-Path $pyDir ".hds-deps") | Out-Null
    }
    return $py
}

function New-SidecarVenv {
    param(
        [Parameter(Mandatory = $true)][string]$SidecarDir,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$PortablePython,
        [switch]$Force
    )
    $venv = Join-Path $SidecarDir $Name
    $py = Join-Path $venv "Scripts\python.exe"
    if ($Force -and (Test-Path $venv)) {
        Write-Host "[..] removing $venv (-Force)"
        Remove-Item -LiteralPath $venv -Recurse -Force
    }
    if (-not (Test-Path (Join-Path $venv "pyvenv.cfg"))) {
        New-Item -ItemType Directory -Force -Path $venv | Out-Null
        Write-Host "[..] venv $Name from $PortablePython"
        & $PortablePython -m venv $venv
        if ($LASTEXITCODE -ne 0) { throw "python -m venv failed (exit $LASTEXITCODE)" }
    }
    if (-not (Test-Path $py)) { throw "venv python not found: $py" }
    return $py
}

function Invoke-SidecarPip {
    param(
        [Parameter(Mandatory = $true)][string]$VenvPython,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [string[]]$IndexArguments = @()
    )
    $uv = Get-Command uv -ErrorAction SilentlyContinue
    if ($uv) {
        $a = @("pip", "install", "--python", $VenvPython) + $IndexArguments + $Arguments
        Write-Host ("[..] uv " + ($a -join " "))
        & $uv.Source @a
    } else {
        $a = @("-m", "pip", "install", "--disable-pip-version-check") + $IndexArguments + $Arguments
        Write-Host ("[..] pip " + ($a -join " "))
        & $VenvPython @a
    }
    if ($LASTEXITCODE -ne 0) { throw ("pip install failed (exit {0}): {1}" -f $LASTEXITCODE, ($Arguments -join " ")) }
}

function Uninstall-SidecarPip {
    # pip/uv uninstall for a profile switch (e.g. the CPU onnxruntime wheel that
    # `pip install onnxruntime-gpu` leaves behind: same module, other dist name).
    # Returns the exit code; "not installed" is not treated as an error.
    param(
        [Parameter(Mandatory = $true)][string]$VenvPython,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )
    $uv = Get-Command uv -ErrorAction SilentlyContinue
    if ($uv) {
        $a = @("pip", "uninstall", "--python", $VenvPython) + $Arguments
        Write-Host ("[..] uv " + ($a -join " "))
        & $uv.Source @a
    } else {
        $a = @("-m", "pip", "uninstall", "--disable-pip-version-check") + $Arguments
        Write-Host ("[..] pip " + ($a -join " "))
        & $VenvPython @a
    }
    return $LASTEXITCODE
}

function Install-SidecarRequirements {
    param(
        [Parameter(Mandatory = $true)][string]$VenvPython,
        [Parameter(Mandatory = $true)][string]$RequirementsPath,
        [string[]]$IndexArguments = @()
    )
    if (-not (Test-Path $RequirementsPath)) { throw "requirements file not found: $RequirementsPath" }
    Invoke-SidecarPip -VenvPython $VenvPython -IndexArguments $IndexArguments -Arguments @("-r", $RequirementsPath)
}

function Test-SidecarImports {
    param(
        [Parameter(Mandatory = $true)][string]$VenvPython,
        [Parameter(Mandatory = $true)][string]$Code
    )
    $tmp = Join-Path $env:TEMP ("hds-sidecar-selftest-" + [Guid]::NewGuid().ToString("N") + ".py")
    try {
        Set-Content -LiteralPath $tmp -Encoding ASCII -Value $Code
        & $VenvPython $tmp
        if ($LASTEXITCODE -ne 0) { throw "sidecar self-test failed (exit $LASTEXITCODE)" }
    } finally {
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
}

function Get-GpuVendor {
    # Returns nvidia | amd | intel | none. Discrete NVIDIA wins over an
    # integrated AMD/Intel adapter (the usual Ryzen + RTX combination).
    $names = @()
    try {
        $names = @(Get-CimInstance Win32_VideoController -ErrorAction Stop |
            ForEach-Object { $_.Name })
    } catch {
        try {
            $names = @(Get-WmiObject Win32_VideoController -ErrorAction Stop |
                ForEach-Object { $_.Name })
        } catch { $names = @() }
    }
    $joined = ($names -join " | ")
    if ($joined -match "NVIDIA") { return "nvidia" }
    if ($joined -match "AMD|Radeon|Advanced Micro Devices") { return "amd" }
    if ($joined -match "Intel") { return "intel" }
    return "none"
}

function Test-IntegratedGpu {
    param([string]$Names = "")
    if (-not $Names) {
        try {
            $Names = (@(Get-CimInstance Win32_VideoController -ErrorAction Stop |
                ForEach-Object { $_.Name }) -join " | ")
        } catch { $Names = "" }
    }
    # "AMD Radeon(TM) Graphics" (Ryzen APU), "Intel(R) UHD/Iris(R) Graphics".
    # Discrete cards carry a model number (RX 6700, Arc A770, ...).
    if ($Names -match "Radeon\(TM\) Graphics|Radeon Vega|Intel\(R\) UHD|Intel\(R\) Iris|Intel\(R\) Graphics") {
        return $true
    }
    return $false
}

function Get-TranscribeProfile {
    # cuda (NVIDIA), dml (discrete AMD/Intel), cpu (no GPU or integrated AMD/
    # Intel - P0 measured DML 2.2-2.5x SLOWER than CPU on the Ryzen iGPU).
    # -Preferred cuda|cpu|dml overrides the detection (installer parameter).
    param([switch]$ForceDml, [string]$Preferred = "auto")
    $wanted = ($Preferred + "").Trim().ToLowerInvariant()
    if ($wanted -in @("cuda", "cpu", "dml")) { return $wanted }
    $vendor = Get-GpuVendor
    switch ($vendor) {
        "nvidia" { return "cuda" }
        "amd" {
            if (-not $ForceDml -and (Test-IntegratedGpu)) { return "cpu" }
            return "dml"
        }
        "intel" {
            if (-not $ForceDml -and (Test-IntegratedGpu)) { return "cpu" }
            return "dml"
        }
        default { return "cpu" }
    }
}

function Get-SidecarVenvName {
    param([string]$Profile = "cuda")
    if (($Profile + "").ToLowerInvariant() -eq "dml") { return "venv-transcribe-dml" }
    return "venv-transcribe"
}

function Copy-SidecarTree {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    foreach ($pkg in @("hds_sidecar", "hds_transcribe")) {
        $src = Join-Path $Source $pkg
        if (-not (Test-Path $src)) { throw "package not found: $src" }
        $dst = Join-Path $Destination $pkg
        New-Item -ItemType Directory -Force -Path $dst | Out-Null
        Copy-Item (Join-Path $src "*") $dst -Recurse -Force
        Get-ChildItem -Path $dst -Recurse -Directory -Filter "__pycache__" -ErrorAction SilentlyContinue |
            Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    }
    foreach ($f in @("README.md")) {
        $src = Join-Path $Source $f
        if (Test-Path $src) { Copy-Item $src (Join-Path $Destination $f) -Force }
    }
}

function Set-ConfigPath {
    # Replace one "  key: value" line inside a top-level [Section] of config.yaml.
    # Everything else (comments, order, other sections) is preserved.
    param(
        [string]$Path,
        [string]$Section,
        [string]$Key,
        [string]$Value
    )
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $enc = New-Object System.Text.UTF8Encoding($false)
    $lines = @([System.IO.File]::ReadAllText($Path, $enc) -split "`n")
    $inSection = $false
    $keyPattern = "^  " + [regex]::Escape($Key) + ":"
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match "^([A-Za-z_][\w.-]*):") {
            $inSection = ($Matches[1] -eq $Section)
            continue
        }
        if ($inSection -and $line -match $keyPattern) {
            $lines[$i] = "  ${Key}: $Value"
            [System.IO.File]::WriteAllText($Path, ($lines -join "`n"), $enc)
            return $true
        }
    }
    return $false
}

function Set-ConfigPathNested {
    # Same for "Section -> Sub -> Key" (indent 2 and 4 spaces), e.g.
    # transcribe.asr.backend. Missing blocks are NOT created.
    param(
        [string]$Path,
        [string]$Section,
        [string]$Sub,
        [string]$Key,
        [string]$Value
    )
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $enc = New-Object System.Text.UTF8Encoding($false)
    $lines = @([System.IO.File]::ReadAllText($Path, $enc) -split "`n")
    $inSection = $false
    $inSub = $false
    $subPattern = "^  " + [regex]::Escape($Sub) + ":"
    $keyPattern = "^    " + [regex]::Escape($Key) + ":"
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match "^([A-Za-z_][\w.-]*):") {
            $inSection = ($Matches[1] -eq $Section)
            $inSub = $false
            continue
        }
        if (-not $inSection) { continue }
        if ($line -match $subPattern) {
            $inSub = $true
            continue
        }
        if ($inSub -and $line -match "^  [A-Za-z_]") {
            $inSub = $false
            continue
        }
        if ($inSub -and $line -match $keyPattern) {
            $lines[$i] = "    ${Key}: $Value"
            [System.IO.File]::WriteAllText($Path, ($lines -join "`n"), $enc)
            return $true
        }
    }
    return $false
}
