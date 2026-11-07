# Copies Outhouse from this PC to your Ubuntu server and (re)starts it.
#   .\deploy\push.ps1 -Server you@192.168.1.50
# The first run asks the setup questions on the server. Later runs just
# upload the new code and rebuild. Uses Windows' built-in ssh/scp/tar; you'll
# be asked for the server password unless you've set up an SSH key.
param(
    [Parameter(Mandatory = $true)][string]$Server,
    [string]$Folder = "outhouse"
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$archive = Join-Path $env:TEMP "outhouse-upload.tgz"

# A version label for Outhouse settings > Server status: the date, plus the
# git commit when git is installed.
$version = Get-Date -Format "yyyy.MM.dd-HHmm"
try {
    $commit = (& git -C $root rev-parse --short HEAD 2>$null)
    if ($LASTEXITCODE -eq 0 -and $commit) { $version = "$version ($commit)" }
} catch { }
Set-Content -Path (Join-Path $root "VERSION") -Value $version -NoNewline

Write-Host "Packing the code (skipping build output and local data)..."
tar -czf $archive -C $root `
    --exclude=./target --exclude=./data --exclude=./tools --exclude=./.env `
    --exclude=./deploy/.env --exclude=./deploy/livekit.yaml --exclude=./.git --exclude=./desktop --exclude=./deploy/updates --exclude=./deploy/status --exclude=./github-secrets.txt .
if ($LASTEXITCODE -ne 0) { throw "Packing failed." }

Write-Host "Uploading to $Server..."
scp $archive "${Server}:outhouse-upload.tgz"
if ($LASTEXITCODE -ne 0) { throw "Upload failed. Can you 'ssh $Server'?" }

Write-Host "Unpacking and starting..."
ssh -t $Server "mkdir -p ~/$Folder && tar -xzf ~/outhouse-upload.tgz -C ~/$Folder && rm ~/outhouse-upload.tgz && sed -i 's/\r$//' ~/$Folder/deploy/setup.sh && bash ~/$Folder/deploy/setup.sh"
Remove-Item $archive
