[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug',

    [switch]$SkipBuild,

    [switch]$KeepRegistration,

    [string]$OutputDir
)

$ErrorActionPreference = 'Stop'

$script:ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$script:RepoRoot = (Resolve-Path (Join-Path $script:ScriptDir '..')).Path
$script:ProfileDir = if ($Configuration -eq 'Release') { 'release' } else { 'debug' }
$script:TargetDir = Join-Path $script:RepoRoot "target\$script:ProfileDir"
$script:VcamCtlPath = Join-Path $script:TargetDir 'vcamctl.exe'
$script:DumpDir = if ($OutputDir) {
    [System.IO.Path]::GetFullPath($OutputDir)
} else {
    Join-Path $script:TargetDir 'com-frame-test'
}

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Ensure-Cargo {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'cargo was not found. Install Rust and ensure cargo is on PATH.'
    }
}

function Ensure-VcamCtl {
    if (-not (Test-Path $script:VcamCtlPath)) {
        throw "Missing $script:VcamCtlPath. Build the project first or omit -SkipBuild."
    }
}

function Invoke-External {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,

        [Parameter(Mandatory = $true)]
        [string]$DisplayName,

        [switch]$IgnoreExitCode
    )

    Write-Step $DisplayName
    & $FilePath @Arguments | ForEach-Object { Write-Host $_ }
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        if ($IgnoreExitCode) {
            Write-Warning "$DisplayName failed with exit code $exitCode. Continuing."
            return $exitCode
        }

        throw "$DisplayName failed with exit code $exitCode."
    }

    return $exitCode
}

function Build-Project {
    Ensure-Cargo

    $cargoArgs = @('build', '--workspace')
    if ($Configuration -eq 'Release') {
        $cargoArgs += '--release'
    }

    Push-Location $script:RepoRoot
    try {
        Invoke-External -FilePath 'cargo' -Arguments $cargoArgs -DisplayName "Build workspace ($Configuration)"
    }
    finally {
        Pop-Location
    }
}

function Invoke-VcamCtl {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments,

        [Parameter(Mandatory = $true)]
        [string]$DisplayName,

        [switch]$IgnoreFailure
    )

    Ensure-VcamCtl

    Push-Location $script:RepoRoot
    try {
        Invoke-External -FilePath $script:VcamCtlPath -Arguments $Arguments -DisplayName $DisplayName -IgnoreExitCode:$IgnoreFailure
    }
    finally {
        Pop-Location
    }
}

if (-not $SkipBuild) {
    Build-Project
}

Ensure-VcamCtl
New-Item -Path $script:DumpDir -ItemType Directory -Force | Out-Null

$rgb32Path = Join-Path $script:DumpDir 'com-rgb32.bmp'
$nv12Path = Join-Path $script:DumpDir 'com-nv12.bmp'

[void](Invoke-VcamCtl -Arguments @('register-com', '--scope', 'user') -DisplayName 'Register current-user COM server')

try {
    $rgb32Exit = Invoke-VcamCtl -Arguments @('dump-com-frame', $rgb32Path, '--subtype', 'rgb32') -DisplayName 'Pull RGB32 frame through COM server' -IgnoreFailure
    $nv12Exit = Invoke-VcamCtl -Arguments @('dump-com-frame', $nv12Path, '--subtype', 'nv12') -DisplayName 'Pull NV12 frame through COM server' -IgnoreFailure

    Write-Host ''
    Write-Host "RGB32 exit code: $rgb32Exit"
    Write-Host "NV12  exit code: $nv12Exit"

    if ($rgb32Exit -eq 0 -and $nv12Exit -eq 0) {
        Write-Host "COM frame test passed. Dumps written to $script:DumpDir" -ForegroundColor Green
    }
    else {
        throw "COM frame test failed. Dumps directory: $script:DumpDir"
    }
}
finally {
    if (-not $KeepRegistration) {
        [void](Invoke-VcamCtl -Arguments @('unregister-com', '--scope', 'user') -DisplayName 'Unregister current-user COM server' -IgnoreFailure)
    }
}
