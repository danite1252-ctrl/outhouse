# Sets up this Windows machine as the GitHub Actions runner for the Outhouse
# Windows installer job (Desktop app (Windows installer) in ci.yml). Safe to
# re-run. Run in an elevated (Run as administrator) Windows PowerShell:
#
#   powershell -ExecutionPolicy Bypass -File .\setup-windows-runner.ps1
#   ... register the runner (the script prints the command) ...
#   powershell -ExecutionPolicy Bypass -File .\setup-windows-runner.ps1 -AfterConfig
#
# Installs or checks: Git (its bash runs the workflow's bash steps), PowerShell 7
# (pwsh), Node.js, Visual Studio Build Tools with the C++ workload, Rust (rustup,
# MSVC, machine-wide in C:\rust so the runner's service account can use it), and
# downloads the runner to C:\actions-runner-outhouse.
#
# The build needs no admin rights (Tauri's NSIS installer and the push-to-talk
# hook build as a normal user), so the runner service should run as a normal
# account: the default NETWORK SERVICE, or a dedicated local user
# (-ServiceUser outhouse-ci creates one).
param(
    [string]$RunnerDir = "C:\actions-runner-outhouse",
    [string]$RustDir = "C:\rust",
    [string]$ServiceUser = "",
    [string]$RunnerName = "outhouse-windows",
    [switch]$AfterConfig,
    [switch]$DefenderExclusion
)
$ErrorActionPreference = "Stop"
$Repo = "https://github.com/danite1252-ctrl/outhouse"
$GitBin = "C:\Program Files\Git\bin"

function Say($m) { Write-Host ""; Write-Host "== $m" -ForegroundColor Yellow }
function Have($cmd) { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }
function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")
}
function Winget-Install($id, $extra) {
    if (-not (Have winget)) { throw "winget isn't available. Install 'App Installer' from the Microsoft Store, or install $id by hand." }
    $wingetArgs = @("install", "--id", $id, "-e", "--silent", "--accept-package-agreements", "--accept-source-agreements") + $extra
    & winget @wingetArgs
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) { throw "winget couldn't install $id (exit $LASTEXITCODE)" }
    Refresh-Path
}

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw "Run this in PowerShell opened with 'Run as administrator'." }
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Account = if ($ServiceUser) { "$env:COMPUTERNAME\$ServiceUser" } else { "NT AUTHORITY\NETWORK SERVICE" }

if ($AfterConfig) {
    # The runner service reads PATH from .path (written by config.cmd). Put Git's bash
    # first: with WSL installed, C:\Windows\System32\bash.exe (WSL) would otherwise run
    # the workflow's `shell: bash` steps inside Linux.
    $pathFile = Join-Path $RunnerDir ".path"
    if (-not (Test-Path (Join-Path $RunnerDir ".runner"))) { throw "The runner isn't registered yet: run config.cmd first." }
    # The Windows runner doesn't always write .path; it reads it when present,
    # so start from the machine PATH if it's missing.
    $p = if (Test-Path $pathFile) { (Get-Content $pathFile -Raw).Trim() } else { [Environment]::GetEnvironmentVariable("Path", "Machine") }
    $parts = @($GitBin) + ($p -split ";" | Where-Object { $_ -and ($_ -ne $GitBin) })
    Set-Content -Path $pathFile -Value ($parts -join ";") -NoNewline
    Write-Host "Git's bash is first in $pathFile"
    $svc = Get-Service -Name "actions.runner.*" -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*$RunnerName*" }
    if ($svc) { Restart-Service $svc.Name; Write-Host "Restarted $($svc.Name)" } else { Write-Host "No runner service found; restart it by hand if it's installed." }
    Write-Host "Done. Check the runner shows 'Idle' at $Repo/settings/actions/runners"
    exit 0
}

Say "Git (bash for the workflow steps)"
if (-not (Test-Path "$GitBin\bash.exe")) { Winget-Install "Git.Git" @() }
& "$GitBin\..\cmd\git.exe" config --system core.longpaths true
Write-Host (& "$GitBin\bash.exe" --version | Select-Object -First 1)

Say "Long paths (deep Rust/Tauri build folders)"
Set-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" -Name LongPathsEnabled -Value 1 -Type DWord

Say "PowerShell 7 (pwsh; the workflow's script check uses it)"
if (-not (Have pwsh)) { Winget-Install "Microsoft.PowerShell" @() }
if (Have pwsh) { & pwsh -NoProfile -Command '$PSVersionTable.PSVersion.ToString()' }

Say "Node.js (the workflow pins Node 22 itself with actions/setup-node)"
if (-not (Have node)) { Winget-Install "OpenJS.NodeJS.LTS" @() }
if (Have node) { & node --version }

Say "Visual Studio Build Tools (C++ workload, for Rust's MSVC linker)"
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vc = $null
if (Test-Path $vswhere) { $vc = & $vswhere -products * -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath }
if (-not $vc) {
    Write-Host "Installing (takes a while, ~6 GB)..."
    Winget-Install "Microsoft.VisualStudio.2022.BuildTools" @("--override", "--wait --quiet --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended")
    $vc = & $vswhere -products * -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}
if (-not $vc) { throw "The C++ build tools still aren't there. Install 'Desktop development with C++' in the Visual Studio Installer." }
Write-Host "C++ build tools: $vc"

Say "Rust (rustup, MSVC) in $RustDir for every account"
$rustupHome = Join-Path $RustDir "rustup"
$cargoHome = Join-Path $RustDir "cargo"
[Environment]::SetEnvironmentVariable("RUSTUP_HOME", $rustupHome, "Machine")
[Environment]::SetEnvironmentVariable("CARGO_HOME", $cargoHome, "Machine")
$env:RUSTUP_HOME = $rustupHome; $env:CARGO_HOME = $cargoHome
if (-not (Test-Path "$cargoHome\bin\rustup.exe")) {
    New-Item -ItemType Directory -Force -Path $RustDir | Out-Null
    $init = Join-Path $env:TEMP "rustup-init.exe"
    Invoke-WebRequest -UseBasicParsing -Uri "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe" -OutFile $init
    & $init -y --profile minimal --default-toolchain stable-x86_64-pc-windows-msvc --no-modify-path
    if ($LASTEXITCODE -ne 0) { throw "rustup-init failed" }
    Remove-Item $init -Force
}
$machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
if (($machinePath -split ";") -notcontains "$cargoHome\bin") {
    [Environment]::SetEnvironmentVariable("Path", "$machinePath;$cargoHome\bin", "Machine")
}
Refresh-Path
& "$cargoHome\bin\rustc.exe" --version

Say "Service account: $Account"
if ($ServiceUser) {
    if (-not (Get-LocalUser -Name $ServiceUser -ErrorAction SilentlyContinue)) {
        $pw = Read-Host -AsSecureString "Password for the new local user $ServiceUser (you'll type it again for config.cmd)"
        New-LocalUser -Name $ServiceUser -Password $pw -PasswordNeverExpires -AccountNeverExpires -Description "GitHub Actions runner for Outhouse CI" | Out-Null
        Write-Host "Created $ServiceUser (a normal user, not an administrator)."
    }
}
# The runner updates toolchains (dtolnay/rust-toolchain) and caches in these folders.
& icacls $RustDir /grant "${Account}:(OI)(CI)M" /T /C /Q | Out-Null

Say "GitHub Actions runner in $RunnerDir"
New-Item -ItemType Directory -Force -Path $RunnerDir | Out-Null
if (-not (Test-Path (Join-Path $RunnerDir "config.cmd"))) {
    $rel = Invoke-RestMethod -UseBasicParsing -Uri "https://api.github.com/repos/actions/runner/releases/latest"
    $ver = $rel.tag_name.TrimStart("v")
    $zip = Join-Path $env:TEMP "actions-runner-win-x64-$ver.zip"
    Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/actions/runner/releases/download/v$ver/actions-runner-win-x64-$ver.zip" -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $RunnerDir -Force
    Remove-Item $zip -Force
    Write-Host "Runner $ver unpacked."
}
& icacls $RunnerDir /grant "${Account}:(OI)(CI)M" /T /C /Q | Out-Null

if ($DefenderExclusion) {
    Say "Windows Defender: skip scanning the runner folder and Rust (faster builds)"
    Add-MpPreference -ExclusionPath $RunnerDir, $RustDir
}

Say "Done. Register the runner (one time)"
$logon = if ($ServiceUser) { " --windowslogonaccount .\$ServiceUser --windowslogonpassword <PASSWORD>" } else { "" }
Write-Host @"

 1. On GitHub: $Repo/settings/actions/runners/new  (Windows, x64)
    copy the token from the "Configure" box (valid for an hour).

 2. In a NEW elevated PowerShell (so it sees the new PATH):
      cd $RunnerDir
      .\config.cmd --url $Repo --token <TOKEN> --labels outhouse --name $RunnerName --runasservice$logon --unattended

 3. Then put Git's bash first for the runner and restart it:
      powershell -ExecutionPolicy Bypass -File "$PSCommandPath" -AfterConfig$(if ($ServiceUser) { " -ServiceUser $ServiceUser" })

 4. Check it shows "Idle" on $Repo/settings/actions/runners, then set the repository variable
      RUNNER_WINDOWS = ["self-hosted","windows","outhouse"]

 Tauri downloads its NSIS tools on the first build and keeps them (in the service account's
 AppData\Local\tauri), so later builds don't depend on GitHub's downloads.
"@
