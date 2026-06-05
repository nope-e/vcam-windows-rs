[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [ValidateSet('Install', 'Uninstall', 'Register', 'Unregister', 'Create', 'Remove', 'Build')]
    [string]$Action = 'Install',

    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug',

    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$script:ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$script:RepoRoot = (Resolve-Path (Join-Path $script:ScriptDir '..')).Path
$script:ProfileDir = if ($Configuration -eq 'Release') { 'release' } else { 'debug' }
$script:TargetDir = Join-Path $script:RepoRoot "target\$script:ProfileDir"
$script:VcamCtlPath = Join-Path $script:TargetDir 'vcamctl.exe'
$script:BuildDllPath = Join-Path $script:TargetDir 'vcam_windows_rs.dll'
$script:InstallRoot = Join-Path $env:ProgramData 'vcam-windows-rs'
$script:InstallDir = Join-Path $script:InstallRoot $script:ProfileDir
$script:InstalledDllPath = Join-Path $script:InstallDir 'vcam_windows_rs.dll'

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Ensure-Cargo {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'cargo was not found. Install Rust and ensure cargo is on PATH.'
    }
}

function Ensure-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    $isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $isAdmin) {
        throw 'Administrator privileges are required for machine-wide COM registration.'
    }
}

function Stop-FrameServerServices {
    Ensure-Admin

    $serviceNames = @('FrameServer', 'FrameServerMonitor')
    foreach ($serviceName in $serviceNames) {
        $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
        if ($null -eq $service) {
            continue
        }

        if ($service.Status -ne [System.ServiceProcess.ServiceControllerStatus]::Running) {
            continue
        }

        if ($PSCmdlet.ShouldProcess($serviceName, 'Stop Frame Server related service')) {
            Write-Step "Stop service $serviceName"
            Stop-Service -Name $serviceName -Force -ErrorAction Stop
            $service.WaitForStatus(
                [System.ServiceProcess.ServiceControllerStatus]::Stopped,
                [TimeSpan]::FromSeconds(15)
            )
        }
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
    & $FilePath @Arguments
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        if ($IgnoreExitCode) {
            Write-Warning "$DisplayName failed with exit code $exitCode. Continuing."
            return
        }

        throw "$DisplayName failed with exit code $exitCode."
    }
}

function Build-Project {
    Ensure-Cargo

    $cargoArgs = @('build', '--bin', 'vcamctl')
    if ($Configuration -eq 'Release') {
        $cargoArgs += '--release'
    }

    if ($PSCmdlet.ShouldProcess($script:RepoRoot, "Build project ($Configuration)")) {
        Push-Location $script:RepoRoot
        try {
            Invoke-External -FilePath 'cargo' -Arguments $cargoArgs -DisplayName "Build vcamctl ($Configuration)"
        }
        finally {
            Pop-Location
        }
    }
}

function Build-IfNeeded {
    if (-not $SkipBuild) {
        Build-Project
    }
}

function Ensure-VcamCtl {
    if (-not (Test-Path $script:VcamCtlPath)) {
        throw "Missing $script:VcamCtlPath. Run -Action Build first or omit -SkipBuild."
    }
}

function Ensure-BuildDll {
    if (-not (Test-Path $script:BuildDllPath)) {
        throw "Missing $script:BuildDllPath. Run -Action Build first or omit -SkipBuild."
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

    if ($PSCmdlet.ShouldProcess($script:VcamCtlPath, $DisplayName)) {
        Push-Location $script:RepoRoot
        try {
            Invoke-External -FilePath $script:VcamCtlPath -Arguments $Arguments -DisplayName $DisplayName -IgnoreExitCode:$IgnoreFailure
        }
        finally {
            Pop-Location
        }
    }
}

function Install-Dll {
    Ensure-BuildDll
    Ensure-Admin

    if ($PSCmdlet.ShouldProcess($script:InstalledDllPath, 'Copy DLL into ProgramData')) {
        Write-Step 'Copy DLL into ProgramData'
        New-Item -Path $script:InstallDir -ItemType Directory -Force | Out-Null
        Copy-Item -Path $script:BuildDllPath -Destination $script:InstalledDllPath -Force
    }
}

function Remove-InstalledDll {
    if (-not (Test-Path $script:InstallDir)) {
        return
    }

    Ensure-Admin

    if ($PSCmdlet.ShouldProcess($script:InstallDir, 'Remove installed DLL directory')) {
        Write-Step 'Remove installed DLL directory'
        Remove-Item -LiteralPath $script:InstallDir -Recurse -Force

        if ((Test-Path $script:InstallRoot) -and -not (Get-ChildItem -LiteralPath $script:InstallRoot -Force | Select-Object -First 1)) {
            Remove-Item -LiteralPath $script:InstallRoot -Force
        }
    }
}

function Register-InstalledDll {
    Ensure-Admin
    if (-not (Test-Path $script:InstalledDllPath)) {
        throw "Missing installed DLL at $script:InstalledDllPath. Run Install or Register first."
    }

    Invoke-VcamCtl -Arguments @('register-com', '--scope', 'machine', '--dll-path', $script:InstalledDllPath) -DisplayName 'Register machine-wide COM server'
}

function Unregister-InstalledDll {
    Ensure-Admin
    Invoke-VcamCtl -Arguments @('unregister-com', '--scope', 'machine') -DisplayName 'Unregister machine-wide COM server' -IgnoreFailure
}

switch ($Action) {
    'Install' {
        Build-IfNeeded
        Install-Dll
        Register-InstalledDll
        Invoke-VcamCtl -Arguments @('create-camera') -DisplayName 'Create virtual camera'

        Write-Host ''
        Write-Host 'Install completed.' -ForegroundColor Green
        Write-Host 'Note: the prototype now uses System lifetime. Remove it explicitly with -Action Uninstall or vcamctl remove-camera.'
    }

    'Uninstall' {
        if (-not (Test-Path $script:VcamCtlPath) -and -not $SkipBuild) {
            Build-Project
        }

        Invoke-VcamCtl -Arguments @('remove-camera') -DisplayName 'Remove virtual camera' -IgnoreFailure
        Stop-FrameServerServices
        Unregister-InstalledDll
        Remove-InstalledDll

        Write-Host ''
        Write-Host 'Uninstall completed.' -ForegroundColor Green
    }

    'Register' {
        Build-IfNeeded
        Install-Dll
        Register-InstalledDll
    }

    'Unregister' {
        if (-not (Test-Path $script:VcamCtlPath) -and -not $SkipBuild) {
            Build-Project
        }
        Unregister-InstalledDll
    }

    'Create' {
        Build-IfNeeded
        Invoke-VcamCtl -Arguments @('create-camera') -DisplayName 'Create virtual camera'
    }

    'Remove' {
        if (-not (Test-Path $script:VcamCtlPath) -and -not $SkipBuild) {
            Build-Project
        }
        Invoke-VcamCtl -Arguments @('remove-camera') -DisplayName 'Remove virtual camera' -IgnoreFailure
    }

    'Build' {
        Build-Project
    }
}
