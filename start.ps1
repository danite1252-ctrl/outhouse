# Starts everything for local development:
#   .\start.ps1               database + voice server + Outhouse
#   .\start.ps1 -NoRun        just the database and voice server
#   .\start.ps1 -NativeVoice  run the voice server directly on Windows instead
#                             of in Docker (smoother audio; Docker Desktop's
#                             UDP forwarding can drop or delay voice packets)
#
# Works out this PC's home-network IP so voice works from other devices too
# (it also works from this PC). Re-run it if your IP changes.
param([switch]$NoRun, [switch]$NativeVoice)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if (-not (Test-Path .env)) {
    Copy-Item .env.example .env
    Write-Host "Created .env from .env.example"
}

# The adapter with a default gateway is the one on your home network.
$config = Get-NetIPConfiguration |
    Where-Object { $_.IPv4DefaultGateway -and $_.NetAdapter.Status -eq "Up" } |
    Select-Object -First 1
$ip = if ($config) { $config.IPv4Address[0].IPAddress } else { "127.0.0.1" }

# Overrides LIVEKIT_NODE_IP from .env for this run.
$env:LIVEKIT_NODE_IP = $ip

# Stop any voice server left over from an earlier -NativeVoice run.
Get-Process livekit-server -ErrorAction SilentlyContinue | Stop-Process -Force

if ($NativeVoice) {
    $lkDir = Join-Path $PSScriptRoot "tools\livekit"
    $lkExe = Join-Path $lkDir "livekit-server.exe"
    if (-not (Test-Path $lkExe)) {
        Write-Host "Downloading LiveKit for Windows (one time)..."
        $release = Invoke-RestMethod "https://api.github.com/repos/livekit/livekit/releases/latest"
        $asset = $release.assets | Where-Object { $_.name -match "windows_amd64\.zip$" } | Select-Object -First 1
        if (-not $asset) {
            Write-Host "Couldn't find a Windows download in LiveKit's latest release ($($release.tag_name))." -ForegroundColor Red
            Write-Host "Download it from https://github.com/livekit/livekit/releases and put livekit-server.exe in $lkDir"
            exit 1
        }
        New-Item -ItemType Directory -Force $lkDir | Out-Null
        $zip = Join-Path $lkDir $asset.name
        Invoke-WebRequest $asset.browser_download_url -OutFile $zip
        Expand-Archive $zip -DestinationPath $lkDir -Force
        Remove-Item $zip
        $found = Get-ChildItem $lkDir -Recurse -Filter "livekit-server.exe" | Select-Object -First 1
        if (-not $found) { Write-Host "The download didn't contain livekit-server.exe." -ForegroundColor Red; exit 1 }
        if ($found.FullName -ne $lkExe) { Move-Item $found.FullName $lkExe -Force }
        Write-Host "If Windows asks whether to allow livekit-server through the firewall, tick both Private and Public." -ForegroundColor Yellow
    }

    # The Docker copy would hold the same ports.
    docker compose stop livekit 2>$null | Out-Null
    $env:LIVEKIT_UPSTREAM = "host.docker.internal:7880"
    Write-Host "Starting LiveKit natively on Windows (voice on $ip)..."
    Start-Process -FilePath $lkExe -WindowStyle Minimized -ArgumentList @(
        "--dev", "--bind", "0.0.0.0", "--node-ip", $ip, "--udp-port", "7882")

    Write-Host "Starting Postgres and the HTTPS proxy..."
    docker compose up -d db caddy
} else {
    $env:LIVEKIT_UPSTREAM = "livekit:7880"
    Write-Host "Starting Postgres, LiveKit and the HTTPS proxy (voice on $ip)..."
    docker compose up -d db livekit caddy
}
if ($LASTEXITCODE -ne 0) {
    Write-Host "Docker didn't start. Is Docker Desktop running?" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "  On this PC:          http://localhost:8080" -ForegroundColor Green
Write-Host "  Other devices here:  https://${ip}:8443" -ForegroundColor Green
Write-Host "     (click past the one-time certificate warning; then voice and mic work)"
Write-Host ""

if (-not $NoRun) {
    cargo run -p outhouse-server
}
