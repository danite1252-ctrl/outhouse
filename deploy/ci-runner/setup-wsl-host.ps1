# Windows side of running the Outhouse Linux CI runner inside WSL2 (Ubuntu 24.04)
# on this machine. Run in an elevated Windows PowerShell, as the Windows user
# that owns the Ubuntu distro (WSL distros belong to one user):
#
#   powershell -ExecutionPolicy Bypass -File .\setup-wsl-host.ps1
#
# 1. Writes %UserProfile%\.wslconfig limits: memory 12GB, 4 processors, 4GB swap,
#    vmIdleTimeout=-1 (the WSL VM stays up when idle) and sparseVhd=true (the
#    virtual disk gives space back). These apply to ALL your WSL distros.
# 2. Registers the scheduled task "Start WSL for Outhouse runner": at start-up,
#    whether you're logged on or not, it starts the distro and keeps it running
#    (wsl.exe ... sleep infinity), so systemd starts the runner service after reboots.
param(
    [string]$Distro = "Ubuntu-24.04",
    [string]$Memory = "12GB",
    [int]$Processors = 4,
    [string]$Swap = "4GB",
    [switch]$NoTask
)
$ErrorActionPreference = "Stop"
$TaskName = "Start WSL for Outhouse runner"

function Say($m) { Write-Host ""; Write-Host "== $m" -ForegroundColor Yellow }

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw "Run this in PowerShell opened with 'Run as administrator'." }

Say "WSL distros"
$wsl = Join-Path $env:SystemRoot "System32\wsl.exe"
if (-not (Test-Path $wsl)) { throw "WSL isn't installed. Run:  wsl --install -d $Distro   then restart and run this again." }
# wsl.exe prints UTF-16; strip the NULs so the names compare.
$names = (& $wsl --list --quiet) -replace "`0", "" | ForEach-Object { $_.Trim() } | Where-Object { $_ }
$names | ForEach-Object { Write-Host "  $_" }
if ($names -notcontains $Distro) {
    throw "There's no WSL distro called '$Distro'. Install it with:  wsl --install -d $Distro  (then open it once to create your Linux user), or pass -Distro <name>."
}

Say "$env:USERPROFILE\.wslconfig"
function Set-IniValue([System.Collections.Generic.List[string]]$lines, [string]$section, [string]$key, [string]$value) {
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i].Trim() -ieq "[$section]") { $start = $i; break } }
    if ($start -lt 0) {
        if ($lines.Count -gt 0 -and $lines[$lines.Count - 1].Trim() -ne "") { $lines.Add("") }
        $lines.Add("[$section]"); $lines.Add("$key=$value"); return
    }
    $end = $lines.Count
    for ($i = $start + 1; $i -lt $lines.Count; $i++) { if ($lines[$i].Trim().StartsWith("[")) { $end = $i; break } }
    for ($i = $start + 1; $i -lt $end; $i++) {
        if ($lines[$i] -match "^\s*$([regex]::Escape($key))\s*=") { $lines[$i] = "$key=$value"; return }
    }
    $lines.Insert($end, "$key=$value")
}
$cfg = Join-Path $env:USERPROFILE ".wslconfig"
$lines = New-Object System.Collections.Generic.List[string]
if (Test-Path $cfg) {
    Copy-Item $cfg "$cfg.bak" -Force
    Get-Content $cfg | ForEach-Object { $lines.Add($_) }
    Write-Host "(old file saved as .wslconfig.bak)"
}
Set-IniValue $lines "wsl2" "memory" $Memory
Set-IniValue $lines "wsl2" "processors" "$Processors"
Set-IniValue $lines "wsl2" "swap" $Swap
Set-IniValue $lines "wsl2" "vmIdleTimeout" "-1"
Set-IniValue $lines "experimental" "sparseVhd" "true"
Set-Content -Path $cfg -Value $lines -Encoding ASCII
Get-Content $cfg | ForEach-Object { Write-Host "  $_" }
Write-Host "These limits apply to every WSL distro. Docker Desktop's own WSL VM is limited by them too."

if (-not $NoTask) {
    Say "Scheduled task '$TaskName'"
    $me = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    Write-Host "It runs as $me whether or not you're logged on, so Windows needs your password to store with it."
    $cred = Get-Credential -UserName $me -Message "Your Windows password (stored by Task Scheduler for '$TaskName')"
    # A long-lived wsl.exe keeps the distro (and systemd, Docker and the runner service) running.
    $action = New-ScheduledTaskAction -Execute $wsl -Argument "-d $Distro --exec /bin/sh -c `"exec sleep infinity`""
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $trigger.Delay = "PT1M"
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -StartWhenAvailable
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
        -User $me -Password $cred.GetNetworkCredential().Password -RunLevel Highest -Force | Out-Null
    Start-ScheduledTask -TaskName $TaskName
    Write-Host "Registered and started."
}

Say "Next"
Write-Host @"
 - Restart WSL so the new limits apply:   wsl --shutdown   (then the task, or opening Ubuntu, starts it again;
   to start the task by hand: Start-ScheduledTask -TaskName '$TaskName')
 - Docker Desktop -> Settings -> Resources -> WSL integration: switch OFF '$Distro' (CI runs its own Docker
   Engine inside it), Apply & restart.
 - In Ubuntu: sudo bash deploy/ci-runner/setup-linux-runner.sh   (from a copy of the Outhouse repo)

 Check it's all running (also after a reboot):
   wsl -l -v                                          ($Distro should say Running)
   wsl -d $Distro -- systemctl status 'actions.runner.*' --no-pager
   Get-ScheduledTask -TaskName '$TaskName' | Get-ScheduledTaskInfo
"@
