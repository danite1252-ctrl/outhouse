<#
.SYNOPSIS
  Outhouse security check-up from OUTSIDE your home network (Windows).

.DESCRIPTION
  The Linux scan kit (deploy/security-scan.sh) runs on the Outhouse VM, so its
  port scan sees your public address from inside the house, through the
  router. This is the outside view: run it on a laptop connected to a phone
  hotspot (not your home Wi-Fi). It's read-only and polite: a TCP connect to
  a short list of ports, a few HTTPS requests, and a TLS handshake check.

    powershell -ExecutionPolicy Bypass -File .\security-scan.ps1
    powershell -ExecutionPolicy Bypass -File .\security-scan.ps1 -Domains oh.hartspace.cc

  Writes $HOME\outhouse-security\<date-time>\report.md and prints a summary.
  UDP ports (voice: 7882) can't be checked this way; join a voice channel
  from the phone itself to test those.
#>
param(
  [string[]]$Domains = @("oh.hartspace.cc", "hartspace.cc"),
  [int[]]$Ports = @(21, 22, 23, 25, 53, 80, 110, 135, 139, 143, 443, 445, 1433, 2375, 3306, 3389, 5432, 5900, 6379, 7880, 7881, 8080, 8443, 9000, 27017),
  [int[]]$ExpectedOpen = @(80, 443, 7881)
)
$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$out = Join-Path $HOME "outhouse-security\$stamp"
New-Item -ItemType Directory -Force -Path $out | Out-Null
$report = Join-Path $out "report.md"
$script:pass = 0; $script:warn = 0; $script:fail = 0
$script:problems = New-Object System.Collections.Generic.List[string]

function Md($text) { Add-Content -Path $report -Value $text -Encoding UTF8 }
function Ok($text) { $script:pass++; Md "- OK: $text" }
function Warn($text) { $script:warn++; $script:problems.Add("WARN $text"); Md "- WARN: $text" }
function Bad($text) { $script:fail++; $script:problems.Add("FIX  $text"); Md "- FIX: $text" }

Md "# Outhouse security check-up (outside view), $(Get-Date -Format 'yyyy-MM-dd HH:mm')"
Md ""
try { $myIp = (Invoke-RestMethod -Uri "https://api.ipify.org" -TimeoutSec 10) } catch { $myIp = "unknown" }
Md "This computer's public address: $myIp (should NOT be your home's address: use a phone hotspot)."

foreach ($d in $Domains) {
  Md ""
  Md "## $d"
  try { $ips = [System.Net.Dns]::GetHostAddresses($d) | Where-Object { $_.AddressFamily -eq "InterNetwork" } | ForEach-Object { $_.ToString() } }
  catch { Bad "$d doesn't resolve"; continue }
  Md "Resolves to: $($ips -join ', ')"
  if ($ips -contains $myIp) { Warn "You're scanning from the same address the site is on: this is the inside view, not the outside one" }

  # ---- http -> https and headers (curl.exe ships with Windows 10 and 11)
  $curl = (Get-Command curl.exe -ErrorAction SilentlyContinue).Source
  if (-not $curl) { Warn "curl.exe not found: header checks skipped" } else {
    $res = & $curl -s -m 15 -o NUL -w "%{http_code} %{redirect_url}" "http://$d/"
    if ($res -match "^(301|308) https://") { Ok "http://$d/ redirects permanently to https" }
    elseif ($res -match "^(302|307) https://") { Warn "http://$d/ redirects to https temporarily ($res); use 301" }
    elseif ($res -match "^000") { Warn "http://$d/ didn't answer (port 80 closed?)" }
    else { Bad "http://$d/ doesn't redirect to https ($res)" }

    $hfile = Join-Path $out "headers-$d.txt"
    & $curl -s -m 20 -o NUL -D $hfile "https://$d/"
    $h = @{}
    if (Test-Path $hfile) {
      Get-Content $hfile | ForEach-Object { if ($_ -match "^([A-Za-z0-9-]+):\s*(.*)$") { $h[$matches[1].ToLower()] = $matches[2] } }
    }
    if ($h.Count -eq 0) { Bad "https://$d/ didn't answer" } else {
      if ($h["strict-transport-security"]) { Ok "HSTS: $($h['strict-transport-security'])" } else { Bad "no Strict-Transport-Security (HSTS)" }
      if ($h["content-security-policy"]) { Ok "Content-Security-Policy set" } else { Warn "no Content-Security-Policy" }
      if ($h["x-frame-options"] -or "$($h['content-security-policy'])" -match "frame-ancestors") { Ok "framing restricted" } else { Warn "no X-Frame-Options / frame-ancestors" }
      if ("$($h['x-content-type-options'])" -match "nosniff") { Ok "X-Content-Type-Options nosniff" } else { Warn "no X-Content-Type-Options: nosniff" }
      if ($h["referrer-policy"]) { Ok "Referrer-Policy $($h['referrer-policy'])" } else { Warn "no Referrer-Policy" }
      if ("$($h['server'])" -match "\d\.\d|Ubuntu|Debian") { Warn "Server header gives away versions: $($h['server'])" }
      if ($h["x-powered-by"]) { Warn "X-Powered-By: $($h['x-powered-by'])" }
    }

    # ---- files that must not be public
    $exposed = 0
    $paths = @("/.env", "/.git/HEAD", "/.git/config", "/deploy/.env", "/docker-compose.yml", "/backup.sql", "/server-status", "/server-info", "/updates/", "/.htpasswd")
    foreach ($p in $paths) {
      $body = Join-Path $out "probe.tmp"
      $code = & $curl -s -m 15 -o $body -w "%{http_code}" "https://$d$p"
      if ($code -eq "200") {
        $content = Get-Content $body -Raw -ErrorAction SilentlyContinue
        if ("$content" -match "Index of|SECRET|PASSWORD|ref: refs|\[core\]|Apache Status|Server Information" -or "$content" -notmatch "<html") {
          Bad "$d$p is publicly readable"; $exposed++
        }
      }
      Remove-Item $body -ErrorAction SilentlyContinue
    }
    if ($exposed -eq 0) { Ok "no sensitive files exposed (checked $($paths.Count) paths)" }
  }

  # ---- TLS: protocol versions and certificate
  foreach ($proto in @("Tls", "Tls11", "Tls12", "Tls13")) {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
      $client.Connect($d, 443)
      $ssl = New-Object System.Net.Security.SslStream($client.GetStream(), $false, { $true })
      $ssl.AuthenticateAsClient($d, $null, [System.Security.Authentication.SslProtocols]::$proto, $false)
      $accepted = $true
      if ($proto -eq "Tls12") {
        $cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($ssl.RemoteCertificate)
        $days = [int]($cert.NotAfter - (Get-Date)).TotalDays
        if ($days -lt 14) { Bad "certificate expires in $days days ($($cert.NotAfter))" } else { Ok "certificate valid for $days more days (issuer: $($cert.Issuer))" }
        $chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
        if ($chain.Build($cert)) { Ok "certificate chain is trusted" } else { Bad "certificate chain isn't trusted by Windows" }
      }
      $ssl.Dispose()
    } catch { $accepted = $false } finally { $client.Dispose() }
    switch ($proto) {
      "Tls"   { if ($accepted) { Bad "TLS 1.0 is still accepted (turn it off)" } else { Ok "TLS 1.0 refused" } }
      "Tls11" { if ($accepted) { Bad "TLS 1.1 is still accepted (turn it off)" } else { Ok "TLS 1.1 refused" } }
      "Tls12" { if ($accepted) { Ok "TLS 1.2 works" } else { Warn "TLS 1.2 handshake failed" } }
      "Tls13" { if ($accepted) { Ok "TLS 1.3 works" } else { Md "- (TLS 1.3 not tested: needs Windows 11 / Server 2022, or not offered)" } }
    }
  }
}

# ---- ports, once per address
Md ""
Md "## Ports (TCP connect from here)"
$addresses = $Domains | ForEach-Object { try { [System.Net.Dns]::GetHostAddresses($_) | Where-Object { $_.AddressFamily -eq "InterNetwork" } | ForEach-Object { $_.ToString() } } catch { } } | Sort-Object -Unique
foreach ($ip in $addresses) {
  Md ""
  Md "### $ip"
  foreach ($port in $Ports) {
    $c = New-Object System.Net.Sockets.TcpClient
    $open = $false
    try { $open = $c.ConnectAsync($ip, $port).Wait(2500) -and $c.Connected } catch { $open = $false } finally { $c.Dispose() }
    if ($open) {
      if ($ExpectedOpen -contains $port) { Ok "TCP $port open (expected)" }
      elseif ($port -eq 22) { Warn "TCP 22 (SSH) is open to the internet: keys only + fail2ban, or close it and use a VPN" }
      else { Bad "TCP $port is open to the internet (Postgres 5432, LiveKit API 7880, Outhouse 8080 and the like must be private)" }
    }
  }
  Md "- (closed/filtered ports aren't listed; checked: $($Ports -join ', '))"
}

Md ""
Md "## Summary"
Md "$($script:pass) fine, $($script:warn) worth a look, $($script:fail) to fix."
Write-Host ""
Write-Host "Done: $($script:pass) fine, $($script:warn) worth a look, $($script:fail) to fix." -ForegroundColor Yellow
$script:problems | ForEach-Object { Write-Host "  $_" }
Write-Host "Report: $report"
