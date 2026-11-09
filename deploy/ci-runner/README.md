# Running Outhouse CI on your own machines (self-hosted runners)

GitHub Actions minutes cost money on a private repo. With self-hosted runners the
builds run on your own machines instead, for free. It's a switch: four repository
variables decide where jobs run, and with none of them set everything runs on
GitHub's runners exactly as before.

| Variable | Example value | Unset means |
|---|---|---|
| `RUNNER_LINUX` | `["self-hosted","linux","outhouse"]` | `ubuntu-latest` (GitHub) |
| `RUNNER_WINDOWS` | `["self-hosted","windows","outhouse"]` | `windows-latest` (GitHub) |
| `RUNNER_LINUX_LIGHT` | `"ubuntu-latest"` | same as `RUNNER_LINUX` (the tiny "What changed" and "Publish downloads" jobs) |
| `SELF_HOSTED_ONLY` | `true` | Any of the above left unset means GitHub's runners |

Values are JSON: a list of labels in `[...]`, or one label in double quotes
(`"ubuntu-latest"`). A plain `self-hosted` without quotes breaks the workflow.

- **`RUNNER_LINUX` + `RUNNER_WINDOWS` set**: every job runs on your machines, so no
  GitHub minutes are used. If a runner is off, its jobs **wait** in the queue
  (up to 24 hours) and start when it comes back. They don't fail, and they don't
  move to GitHub.
- **`SELF_HOSTED_ONLY` = `true`**: a safety net. If a `RUNNER_*` variable is
  missing (or later deleted), jobs still go to your machines
  (`self-hosted` + `linux`/`windows` + `outhouse`). They never fall back to paid
  minutes.
- **To go back to GitHub's runners** (say a machine dies): delete `SELF_HOSTED_ONLY` and
  the `RUNNER_*` variables, then re-run the workflow (Actions → the run → Re-run jobs).
  You can do just one side, e.g. delete only `RUNNER_WINDOWS` to build the
  installer on GitHub while Linux stays at home.

Variables live in GitHub → the repo → **Settings → Secrets and variables → Actions → Variables tab → New repository variable**.
(Not the Secrets tab. Leave the existing secrets as they are.)

## Which job runs where

| Job | Runner | Needs |
|---|---|---|
| What changed, Publish downloads | `RUNNER_LINUX_LIGHT` → `RUNNER_LINUX` | git, gh, python3 |
| Server (build, tests, smoke test, UI test) | `RUNNER_LINUX` | Docker (Postgres service, LiveKit), Rust, Node, Python, Chrome libs, ports 5432/7880-7882/8080/8097-8099 |
| Android app (APK) | `RUNNER_LINUX` | Java 17, Rust, Node, Android SDK + NDK |
| DAST (ZAP) | `RUNNER_LINUX` | Docker, Rust, ports 5432/8080 |
| Security (cargo audit) | `RUNNER_LINUX` | Rust, Node |
| Desktop app (Windows installer) | `RUNNER_WINDOWS` | Rust (MSVC), VS C++ Build Tools, Node, Git bash, pwsh |
| Check Wowhead (by hand only) | `RUNNER_LINUX` | Rust |

One runner runs one job at a time, so on one Linux runner the server, DAST and
Android jobs queue behind each other instead of running side by side. A run
takes longer but costs nothing. **Register only one Linux runner per machine.** Two
on the same machine would fight over ports 5432/8080.

## Option A: both runners on one Windows machine (Danserver)

The Windows runner runs natively. The Linux runner runs inside **WSL2 Ubuntu 24.04**
on the same machine, with its own Docker Engine.

1. **Install WSL Ubuntu 24.04** (elevated PowerShell): `wsl --install -d Ubuntu-24.04`,
   restart if asked, open "Ubuntu 24.04" once and create your Linux user.
2. **Windows side of WSL**: copy this repo (or just `deploy\ci-runner`) to the
   machine, then in an elevated PowerShell, as the Windows user that owns the distro:
   ```powershell
   powershell -ExecutionPolicy Bypass -File .\deploy\ci-runner\setup-wsl-host.ps1
   ```
   This writes `%UserProfile%\.wslconfig` (12 GB RAM, 4 CPUs, 4 GB swap,
   `vmIdleTimeout=-1`, `sparseVhd=true`). **These limits apply to every WSL distro and to Docker
   Desktop's WSL VM.** It also registers the scheduled task **"Start WSL for Outhouse
   runner"**, which starts the distro at boot whether you're logged on or not, so the
   runner comes back after a reboot. It asks for your Windows password because
   Task Scheduler stores it for the task.
3. **Docker Desktop → Settings → Resources → WSL integration**: switch **off**
   Ubuntu-24.04 → Apply & restart. CI uses `docker run --network host` and
   Postgres on `localhost:5432`, and Docker Desktop's "host" is its own VM, not the
   distro. Your n8n/AMP containers in Docker Desktop are unaffected.
   Then `wsl --shutdown`.
4. **Inside Ubuntu** (open "Ubuntu 24.04"):
   ```bash
   git clone https://github.com/danite1252-ctrl/outhouse.git ~/outhouse-src   # or copy the deploy/ci-runner folder
   sudo bash ~/outhouse-src/deploy/ci-runner/setup-linux-runner.sh
   ```
   The first time it switches on systemd in `/etc/wsl.conf` and stops. Run
   `wsl --shutdown` in PowerShell, open Ubuntu again and re-run the script.
   It then installs Docker Engine (inside the distro), the build tools, the Android
   SDK/NDK, Chrome's libraries for the UI tests, the clean-up hook and the weekly
   disk clean-up, plus the runner itself (in `/home/runner/actions-runner`).
5. **Register the Linux runner**: follow what the script prints. In short:
   GitHub → Settings → Actions → Runners → **New self-hosted runner** → Linux,
   copy the token, then
   ```bash
   sudo -iu runner
   cd ~/actions-runner
   ./config.sh --url https://github.com/danite1252-ctrl/outhouse --token <TOKEN> --labels outhouse --name outhouse-linux --unattended
   exit
   cd /home/runner/actions-runner && sudo ./svc.sh install runner && sudo ./svc.sh start
   ```
   (`self-hosted` and `linux` labels are added automatically.)
6. **Windows runner** (elevated PowerShell):
   ```powershell
   powershell -ExecutionPolicy Bypass -File .\deploy\ci-runner\setup-windows-runner.ps1
   ```
   This installs or checks Git, PowerShell 7, Node, the VS C++ Build Tools and Rust (in
   `C:\rust`, usable by the service account), and unpacks the runner to
   `C:\actions-runner-outhouse`. Then, from a **new** elevated PowerShell:
   ```powershell
   cd C:\actions-runner-outhouse
   .\config.cmd --url https://github.com/danite1252-ctrl/outhouse --token <TOKEN> --labels outhouse --name outhouse-windows --runasservice --unattended
   powershell -ExecutionPolicy Bypass -File <path>\deploy\ci-runner\setup-windows-runner.ps1 -AfterConfig
   ```
   `-AfterConfig` puts Git's bash ahead of WSL's `bash.exe` for the runner. Without it, the
   workflow's bash steps would run inside WSL and fail. The service runs as NETWORK
   SERVICE (no admin needed: the installer and push-to-talk build fine without it). For a
   dedicated user instead, run the setup script with `-ServiceUser outhouse-ci` and add
   `--windowslogonaccount .\outhouse-ci --windowslogonpassword <PASSWORD>` to `config.cmd`.
   `-DefenderExclusion` makes builds faster by not virus-scanning the runner and Rust folders.
7. Both runners should show **Idle** on GitHub → Settings → Actions → Runners.
8. **Set the variables** (see the table above):
   `RUNNER_LINUX` = `["self-hosted","linux","outhouse"]`,
   `RUNNER_WINDOWS` = `["self-hosted","windows","outhouse"]`,
   and `SELF_HOSTED_ONLY` = `true` if you never want paid minutes used.
9. **Re-enable CI** only now (it was switched off to stop charges):
   GitHub → **Actions → CI → "Enable workflow"**, or `gh workflow enable CI -R danite1252-ctrl/outhouse`.
   Then Actions → CI → **Run workflow** on `main`, which builds everything.
10. **Check where it ran**: open the run → each job → "Set up job" shows
    `Runner name: 'outhouse-linux'` / `'outhouse-windows'`. A job that waits with
    "Waiting for a runner to pick up this job…" means that runner is offline.

**Ports (WSL)**: CI inside WSL uses 5432 (Postgres), 7880-7882 (LiveKit), 8080
(test server) and 8097-8099 (mocks), and kills leftover processes on them **inside
WSL only**. With WSL's default NAT networking these are separate from Windows' own
ports, but WSL forwards `localhost` ports to Windows, so make sure nothing on
Windows already listens on them:
```powershell
netstat -ano | findstr ":5432 :7880 :7881 :8080 :8097 :8098 :8099"
```
AMP's web panel often uses 8080. That's fine with NAT (the default), but **don't switch WSL
to `networkingMode=mirrored`**, where they would clash.

**Resources**: a CI run uses about 4 cores and up to 8 GB RAM for a few minutes
(Rust builds, Gradle, headless Chrome). AMP, Plex and game servers keep running,
but builds are slower while the box is busy. Lower `-Memory`/`-Processors` in
`setup-wsl-host.ps1` if the games suffer.

**Disk (WSL)**: the weekly `outhouse-ci-cleanup.timer` (Sun 04:00) prunes Docker images
older than a week, cargo downloads, Gradle and Playwright caches older than 30 days.
Run it now with `sudo /opt/outhouse-ci/cleanup-runner.sh`. WSL's virtual disk doesn't shrink
by itself. `sparseVhd=true` gives space back on newer WSL. Otherwise compact it now and then:
```powershell
wsl --shutdown
# find the disk: %LOCALAPPDATA%\Packages\CanonicalGroupLimited.Ubuntu24.04*\LocalState\ext4.vhdx (or under %LOCALAPPDATA%\wsl\)
Optimize-VHD -Path <path>\ext4.vhdx -Mode Full      # needs Hyper-V tools; or: diskpart → select vdisk file=<path> → compact vdisk
```

## Option B: a separate Ubuntu VM for Linux

A fresh **Ubuntu 24.04 VM**: 4 vCPU, 8-12 GB RAM, 80-100 GB disk. **Not the
Outhouse production VM (192.168.1.122)**: CI binds 5432, 7880-7882 and 8080
and kills leftovers on them, which would clash with the live server. Copy this
folder over, run `sudo bash setup-linux-runner.sh`, then steps 5 and 7-10 above.
Windows runner as in step 6 (skip `-AfterConfig` if WSL isn't installed; it's harmless either way).

## How jobs stay clean on a machine that's reused

GitHub's runners are thrown away after each job. Yours aren't, so:
- `actions/checkout` wipes the work folder (`git clean -ffdx`), including Rust's
  `target/` and the generated Android project. `Swatinem/rust-cache` brings the build
  cache back from GitHub's cache storage (free, 10 GB per repo, old entries expire).
- Every Linux job starts with `ci/runner-cleanup.sh` and ends with
  `ci/runner-cleanup.sh --end` (`if: always()`). They remove the `livekit` and ZAP containers,
  kill the leftover test server, mocks and bot (ports 8080, 8097-8099, 7880-7882),
  and delete temp files (`/tmp/outhouse-uploads`, `/tmp/ui-shots`, `/tmp/zap.*`, `/tmp/wh`, ...).
- The runner hook `/opt/outhouse-ci/job-started.sh` (set in the runner's `.env`) runs
  before each job's Postgres service starts and removes a stale Postgres left by a
  killed job.
- The Android signing key is only ever written to the runner's per-job temp folder,
  deleted by a `trap` in the build step and again by an `if: always()` step.
- The Python test tools go in a fresh venv per job. Playwright's Chromium stays
  in `~/.cache/ms-playwright`. Its system libraries were installed once by the
  setup script (marker `/etc/outhouse-ci-runner`), so the job doesn't need full sudo.
- The Android SDK lives in `/opt/android-sdk` (`ANDROID_HOME` in the runner's
  `.env`). If it's missing, the job installs the command-line tools
  (`android-actions/setup-android`), the NDK (`ANDROID_NDK_VERSION` in ci.yml),
  build-tools and the platform the generated project compiles against, and keeps them.
- The runner user is in the `docker` group (needed for service containers and
  LiveKit/ZAP). By default it has passwordless sudo **only for apt-get**
  (`RUNNER_SUDO=apt`). `none` and `full` are the other choices. Bear in mind the
  docker group and apt-get are both effectively root on that machine, which is
  another reason to keep CI off the production server.

Windows: Rust's `target` is wiped by checkout and restored from the cache. Tauri keeps its
NSIS tools in the service account's `AppData\Local\tauri` after the first build, so the
occasional GitHub download 504s stop mattering. To free space, delete
`C:\actions-runner-outhouse\_work\outhouse\outhouse\desktop\src-tauri\target` and
`C:\rust\cargo\registry\cache` now and then.

## Security

Self-hosted runners are only safe because this repo is **private**: anyone who
can run a workflow can run code on your machine. **Never make the repo public**
while these runners are registered, and never allow workflows from fork pull
requests. The secrets stay in GitHub as they are and only reach your
machines during the jobs that already use them (signing keys). To pull the plug,
stop the runner services (`sudo ./svc.sh stop` / `Stop-Service actions.runner.*`)
or remove the runners under Settings → Actions → Runners.
