# Hosting Outhouse on your own Ubuntu server

Runs the whole thing on a Linux box at home, reachable as `https://outhouse.chat`
(or your domain) from anywhere, with a real certificate and no warnings.

## Before you start
- **Ubuntu 22.04 or newer** on the server, with SSH access from your PC.
- **A fixed home-network IP for the server.** In your router, add a DHCP
  reservation for it (e.g. `192.168.1.122`), so port forwarding keeps working.
- **Check you're not behind CGNAT.** Compare the "WAN IP" on your router's
  status page with https://api.ipify.org. If they differ (or the router shows
  `100.64.x.x`–`100.127.x.x`), your ISP shares one IP between customers and
  outside connections can't reach you. Most Australian NBN providers will
  switch you off CGNAT or sell a static IP if you ask.

## 1. Point your domain at home
At your domain registrar, add an **A record**: `outhouse.chat` → your public IP
(https://api.ipify.org shows it).

Home IPs can change. If your plan doesn't include a static IP, use your
registrar's dynamic DNS (or Cloudflare + a DDNS updater on the server) so the
record follows your IP.

## 2. Forward ports on your router to the server
| Port | Protocol | For |
|---|---|---|
| 80 | TCP | Certificate setup (Let's Encrypt) |
| 443 | TCP + UDP | The website |
| 7881 | TCP | Voice (fallback) |
| 7882 | UDP | Voice |

Moving voice to its own server: see [LIVEKIT-SERVER.md](LIVEKIT-SERVER.md).

## Already have Apache (or another web server) on ports 80/443?
Your router can only send 80/443 to one machine, so let Apache handle HTTPS
and pass the Outhouse subdomain through to this server:
1. Answer **y** to "Does another web server already handle HTTPS" during setup.
   Outhouse then listens on plain HTTP port **8080** instead of 80/443.
2. Use `deploy/oh.hartspace.cc.conf` as the Apache site (set up for Apache
   on 192.168.1.32 and Outhouse on 192.168.1.122 at oh.hartspace.cc). The
   commands to enable it and get the certificate are at the top of the file.
   (`apache-outhouse.conf` is an older template; use the oh.hartspace.cc one.)
3. On the router, only forward **7881 TCP** and **7882 UDP** to the Outhouse
   server. Voice can't go through Apache. 80/443 stay with Apache.

## 3. Send it over and start it
From `C:\dev\Outhouse Discord` on your PC:
```powershell
.\deploy\push.ps1 -Server yourname@192.168.1.122
```
It packs the code, uploads it, and runs `deploy/setup.sh` on the server. The
first time, it asks for your domain and an email address, installs Docker if
needed, makes strong passwords and voice keys (kept in `~/outhouse/deploy/.env`
on the server), opens the firewall and builds everything. The first build takes
a few minutes. Then open `https://outhouse.chat`.

**To update later:** run the same command. Your data, passwords and keys stay.

## Handy commands (on the server, in `~/outhouse/deploy`)
```bash
docker compose ps                 # what's running
docker compose logs -f server     # Outhouse's log
docker compose logs -f caddy      # certificate / HTTPS problems
docker compose restart            # restart everything
```
## Backups
Setup schedules a backup every night at 3:30 am into `~/outhouse-backups`
(the newest 14 are kept). Each one holds the database, every uploaded file
and the `.env` settings.

```bash
cd ~/outhouse/deploy
./backup.sh                                              # back up right now
./restore.sh ~/outhouse-backups/outhouse-2026-11-08_0330.tgz   # put one back (replaces everything)
```

A backup on the same VM won't survive the VM's disk dying, so let
`backup.sh` copy each one somewhere else (below), or copy the folder by hand
now and then, for example from Windows:
`scp -r danite@192.168.1.122:outhouse-backups C:\Backups\Outhouse`.

### Off-site copies (a Windows share and/or rclone)
`backup.sh` can copy each night's backup somewhere else and keep the newest
14 there too. `setup.sh` asks about it once (run `./setup.sh --offsite` to
change your answers later):

- **A shared folder on your Windows PC.** In Windows, make a folder (e.g.
  `C:\Backups\Outhouse`), right-click it → Properties → Sharing → Share…,
  and give your Windows user access. Then answer the question with
  `//<PC's IP>/<share name>` (e.g. `//192.168.1.10/Outhouse`), the Windows
  user name and password. Setup installs `cifs-utils`, keeps the password in
  `/etc/outhouse-backups.cred` (only root can read it), and mounts the share
  at `/mnt/outhouse-backups` whenever it's used (an `/etc/fstab` line with
  `nofail` and automount, so a sleeping PC never stops the VM booting).
  If the PC is off at 3:30 am, that night's copy fails and you get an alert
  (see below); the backup on the VM is still made.
- **An rclone remote** (Google Drive, OneDrive, Backblaze B2, another SSH
  box…): `sudo apt install rclone`, `rclone config` to add the remote, then
  give its name and folder, e.g. `gdrive:outhouse-backups`.

The settings live in `~/outhouse/deploy/.env`:

| Setting | What it does |
|---|---|
| `OFFSITE_DIR` | A mounted folder to copy backups to (`/mnt/outhouse-backups`). It must really be mounted, so a missing share never fills the VM's own disk (`OFFSITE_REQUIRE_MOUNT=no` turns that check off for a plain folder). |
| `OFFSITE_KEEP` | How many backups to keep there (14) |
| `RCLONE_REMOTE` | An rclone remote and folder (`gdrive:outhouse-backups`) |
| `RCLONE_KEEP` | How many backups to keep there (14) |

Each run writes `deploy/status/backup.json`; Outhouse settings → **Server
status** shows the last backup and whether the off-site copy worked, and the
uptime check below alerts you if a backup or its copy fails, or if no backup
has happened for 36 hours. Check by hand any time: `./backup.sh` (it says
"Copied to …" or why it couldn't) and `ls /mnt/outhouse-backups`.

## Uptime alerts
`deploy/healthcheck.sh` checks every 5 minutes that Outhouse answers (its
`/api/health` also checks the database), that voice (LiveKit) answers, and
that last night's backup worked. When something **changes** (goes down,
comes back, a backup fails) it sends one alert, so you're not pinged every
5 minutes. `setup.sh` asks for where to send them once (`./setup.sh --alerts`
to change):

- **ntfy** (easiest, free): install the ntfy app on your phone, subscribe to
  a topic with a long random name (e.g. `outhouse-3f9c1a2b`), and use
  `https://ntfy.sh/outhouse-3f9c1a2b` as `ALERT_NTFY_URL`. Anyone who knows
  the topic name can read it, so keep it random.
- **A webhook** (`ALERT_WEBHOOK_URL`): any Discord-compatible webhook, e.g. a
  Discord channel's webhook, or an Outhouse channel's webhook (Outhouse
  settings → Webhooks) if you'd like alerts in a channel (though if
  Outhouse itself is down, that one can't arrive: use ntfy as well).

The cron job is only installed when at least one of them is set. Other
settings (in `.env`): `HEALTH_URL` (default `http://127.0.0.1:8080/api/health`
behind Apache, else `https://<DOMAIN>/api/health`), `LIVEKIT_HEALTH_URL`
(default `http://127.0.0.1:7880`, `off` to skip) and `BACKUP_MAX_AGE_HOURS`
(36). Run `./healthcheck.sh` to check now; it logs changes to
`~/outhouse-backups/health.log` and remembers the last state in
`deploy/status/health.state` (delete that file to get a fresh "all good"
baseline).

Outhouse settings → **Server status** (people with Manage Outhouse) shows
the version, uptime, database size and the last backup.

## Push notifications (phones and desktops)
Nothing to set up: on first start the server makes its own Web Push key
(kept in the database, so it's in every backup) and Settings →
**Notifications** has a **Turn on push notifications** button. Pushes go to
a device when you're away (no Outhouse tab or app open, or it's been hidden
or idle for a while): DMs, @mentions and replies, raid reminders and bench
promotions, following each person's per-outhouse level (all / mentions /
nothing) and muted channels.

- **Android phones:** open the site in Chrome, ⋮ → **Install app**, open it,
  sign in, Settings → Notifications → Turn on. (The Android APK can't get
  push while it's closed: Android's web view has no Web Push. It still shows
  notifications while it's open.)
- **iPhone/iPad:** Share → **Add to Home Screen**, open it from there, then
  turn push on (iOS 16.4+).
- **Windows:** Chrome or Edge can do it too; the Windows app shows its own
  notifications while it's running.

Optional: `VAPID_SUBJECT=mailto:you@example.com` in `.env` (push services
use it to contact the server's owner; it defaults to the site's address).
The server only sends pushes to the big push services (Google, Mozilla,
Microsoft, Apple); `PUSH_ALLOWED_HOSTS` adds others.

## Invite links
**Invite people** (the person-plus icon next to the outhouse name, or the
outhouse menu) gives a link like `https://oh.hartspace.cc/invite/AbC12345`
with a **Copy link** button, a choice of how long it lasts (30 minutes to 7
days, or never) and how many people can use it. Outhouse settings → **Invites**
(Manage Outhouse) lists every working invite with who made it and how often it's
been used, and revokes them.

The link opens a page on your server (with a preview card when pasted into
Discord, Messenger or WhatsApp). From there people sign up or sign in and land
straight in the outhouse, or open it in the apps:
- **Android:** the app claims `https://<your domain>/invite/…` links (Android
  App Links), so tapping one opens the app if it's installed and the web page
  if not. Android checks `https://<your domain>/.well-known/assetlinks.json`,
  which Outhouse serves itself with the app's package name and the release
  signing certificate's SHA-256 (the public one CI prints as "Signing
  certificate SHA-256"; it's in `crates/server/src/state.rs`). If you ever sign
  the APK with a different key, set `ANDROID_CERT_SHA256=AB:CD:…` (comma-separate
  several) in `.env`; `ANDROID_CERT_SHA256=off` switches App Links off. The
  app's link host comes from `OUTHOUSE_SERVER_URL` in CI. Phones verify the
  link when the app is installed or updated, so after the first build with
  invite links, update the app once (or reinstall) for links to open it.
- **Windows:** the installer registers `outhouse://` links; the invite page's
  **Open in the Outhouse app** button uses `outhouse://invite/<code>`, and if
  nothing answers it suggests downloading the app.

Behind Apache, `assetlinks.json` is just proxied through like everything else
on 443 (only `/.well-known/acme-challenge/` on port 80 is answered by Apache
for certbot), so no Apache change is needed. To check from anywhere:
`curl -i https://oh.hartspace.cc/.well-known/assetlinks.json` should give
`200`, `application/json` and no redirect.

## Audit log
Outhouse settings → **Audit log** (people with View audit log or Manage
Outhouse) lists who changed what: channels and their permissions, roles,
kicks, bans, timeouts, deleted messages (by moderators), pins, emoji,
sounds, voice mutes/moves/disconnects, bots, webhooks, feeds, cancelled
raids and badges. Entries are kept for 90 days (`AUDIT_RETENTION_DAYS` in
`.env` changes that).

## Safety & reports
Outhouse has a report button so you (the site owner) hear about anything
nasty and can deal with the account. This is how it works and what to do;
it's general information, not legal advice.

**Who's the site owner?** The first account made on the server, unless
`SITE_OWNER` in `.env` names others (usernames or IDs, comma-separated).
Only site owners see reports. Outhouse admins don't (you stay the final say).

**Reporting.** Anyone can report a message (channels, DMs, group DMs,
threads: right-click or ⋯ → **Report message**, ⚑ in threads), a file, a
sticker or custom emoji on a message, a person (their profile card's ⋯, or
right-click them → **Report…**) or a whole outhouse (right-click its icon).
They pick a category (child safety; sexual content; harassment; hate or
threats; self-harm; spam or scams; illegal activity; impersonation; other),
can add up to 1000 characters of detail and tick **Block this person too**.
The person reported is never told who reported them. Someone can send 20
reports an hour (`REPORTS_PER_HOUR`); a second report of the same thing adds
to the first (the count shows how many people reported it).

**Evidence.** When something is reported the server keeps a snapshot: the
message text, who sent it and when, where (outhouse, channel, thread or the
people in the DM), the 10 messages before and after it, the files' names,
sizes and SHA-256 hashes, and any later edits or deletion. Deleting the
message doesn't delete the snapshot, and the files stay on disk until you
close the report.

**Child-safety reports** go to the top of the list. Straight away:
- the files are **quarantined**: nobody can download them, not you, not the
  person who sent them, not admins (`/files/…` answers *451 removed pending
  review*), but they stay on the server, unchanged, as evidence;
- the message (and any forwarded copies of the files) is hidden from
  everyone;
- you get the shield badge on the left rail, a push notification (if you
  turned those on), a desktop notification in the Windows app, and an alert
  to the same ntfy topic / webhook as the uptime alerts (`ALERT_NTFY_URL`,
  `ALERT_WEBHOOK_URL`), all saying *"Review and suspend the account now if
  in doubt."* The alert never contains any of the content. Tapping the push
  (or the browser / desktop notification) opens that report.

**Suspend straight away (police advice).** Nothing is suspended
automatically: suspending is a one-tap step you take yourself, at once.
Police advised that leaving a reported account active once you know about
it could be seen as helping, so err on the side of caution. A child-safety
report opens with **Suspend now** at the top: one confirmation and the
account is suspended *until you've reviewed it*: signed out everywhere,
can't sign in, its bots stop, its profile is hidden, it's dropped from
voice. Nothing else of theirs is hidden (the reported message already is).
They only ever see *"This account has been suspended pending review of a
report."*, never the category, the details or who reported them. The report
then shows **Suspended at <time> by you** with **Lift suspension** (it was a
mistake) and **Make permanent…** (see **Blocked networks** below for the IP
block it offers).

The review screen never shows a child-safety report's pictures or video:
just the file name, type, size and SHA-256. **Don't view, download, copy,
screenshot or forward suspected child sexual abuse material, even to report
it.** Report it instead:
- **Australian Centre to Counter Child Exploitation (ACCCE):** https://www.accce.gov.au/report
- **eSafety Commissioner:** https://www.esafety.gov.au/report
- **A child in immediate danger: call 000.**
- Outside Australia: the NCMEC CyberTipline (US) https://report.cybertip.org, or local police.

**Handing evidence to police (without ever opening it).** You must not
open, view, download to your PC or forward the files. Do it in the app,
ideally with police there (or when they've asked): the report's **Hand over
to police** panel. Outhouse builds one sealed `.tar.gz` on the server:
```text
evidence-<id>-<UTC time>/
  files/<storage key>   each original file, byte for byte (quarantined ones
                        too; never opened, decoded or thumbnailed)
  report-<id>.json      everything known: the report, snapshot and context,
                        who reported it, the reported account's history and
                        suspension state, each file's name/type/size/SHA-256
                        and image-detector result, IP addresses (UTC and
                        Sydney time), attempts from blocked networks, and the
                        report's whole safety log
  HANDOVER.txt          report id, time (UTC and Sydney), who made it, the
                        officer / reference, file count, how to check it,
                        "Do not open these files"
  MANIFEST.sha256       SHA-256 of every file above (`sha256sum -c`)
```
Two ways to give it to them (both ask for your password again, a tick that
police are with you or asked for it, and an optional officer / reference):
- **Create a download link for police** (recommended). You get a one-time
  link, the package's SHA-256 and the expiry, each with a Copy button. Police
  open the link on *their* device (or you paste it into their portal); the
  page shows the file's name, size, SHA-256 and expiry with a **Download**
  button (opening the page doesn't use it up, so chat and mail link previews
  can't burn it; only the download does). It works once by default (up to
  3), for 24 hours by default (1 hour or 7 days). After the last download,
  when it expires or if you **Revoke** it, it answers *410 Gone* and the
  package is deleted from the server. The link is shown once (only its hash
  is stored); the report lists every link with its status and downloads.
- **Download here** (only if police ask): the same package straight to the
  browser you're using. Don't open or extract it; hand it straight over and
  delete it.

The safety log records *"Evidence packaged for police"* with the package's
SHA-256, the route and the reference, and every download separately with the
time, the downloader's IP address and browser. Wrong passwords are limited
(10 per 15 minutes) and so is guessing at links (30 tries per address per 15
minutes). Packages wait in `/data/uploads/.evidence-packages` (in the uploads
volume; never served except through a live link, and left out of backups).
Server logs only ever show a link's first 8 characters; the Apache configs
here keep `/evidence/…` out of the access log too.

**Export details only (JSON, no files)** downloads just `report-<id>.json`
(the same details, without the files).

**Fallback: from the server over SSH.** If the app can't be used, the
report's **Advanced: from the server over SSH** shows the command:
```bash
cd ~/outhouse && ./deploy/evidence-handover.sh <report id>
```
The server builds the very same package (`outhouse-server evidence-package`),
the script saves it as `~/outhouse-evidence/evidence-<id>-<UTC time>.tar.gz`
(readable only by you), prints its SHA-256 and the server logs it. It never
prints anything from the files. Over SSH it only packs child-safety reports
or reports with quarantined files unless you add `--any`; `--ref "Det.
Smith, 1234/26"` adds the officer / reference; `--zip` makes a .zip as well
(`sudo apt install zip` first). Then let police copy the `.tar.gz` from the
server under supervision, or upload it to their portal from the server.

Nightly backups (and off-site copies) include the upload folder, so older
backups keep a copy until they're rotated out; tell police that too.

**The console** (the shield on the left rail, or Outhouse settings →
**Reports**): open / in review / closed lists (child safety first, then the
most serious, newest first), and each report's snapshot, context, files,
reporters, the reported person's history (account age, age group, outhouses,
messages in the last 30 days, other reports about them, reports they've
made; their IP addresses are under People, see **IP addresses** below) and
actions:
- **Mark in review**, **Close…** (with a note of what you did), **Dismiss…**
  (a mistake: a dismissed child-safety report unlocks its files and shows the
  message again), **Reopen**;
- **Delete message**, **Remove files** (the report keeps its copy);
- **Warn…** (a DM from *Outhouse Safety*), **Time out…**, **Kick…**, **Ban…**
  in one of their outhouses;
- **Suspend now** (child-safety reports: until you've reviewed it, neutral
  reason, see above), then **Make permanent…** or **Lift suspension**;
- **Suspend account…** for 1–90 days or permanently: they're signed out
  everywhere at once, can't sign in (they see the reason), their bots'
  tokens are revoked (reset them after), their profile is hidden, and their
  messages stay for evidence unless you tick "hide all their messages".
  **Lift suspension** undoes it;
- **Hide all their messages** / **Show their messages again** (child-safety
  evidence stays hidden);
- **Add note** (private), **Hand over to police** (above), **Export details
  only (JSON, no files)**, and for a closed
  child-safety report **Delete evidence files** once police have what they
  need (the names and hashes stay).

Everything done is in the **Safety log** (kept for good). **People** finds
any account to set their age group or act without a report. **This week**
shows new accounts, reports and same-IP hints; Outhouse Safety also DMs it
to you weekly.

**IP addresses.** For police (child-safety cases) and as a hint about ban
evasion, Outhouse keeps the IP addresses people use: when an account is
made, at every sign-in, and at most once an hour per session while they're
signed in (bots aren't recorded). One row per account and address (first
seen, last seen, how it was first seen, how many times). They're deleted
**90 days** after they were last seen (`IP_RETENTION_DAYS` in `.env`), checked
every hour. Sign-up and Privacy settings tell people: "To keep Outhouse
safe, sign-in IP addresses are kept for 90 days and only the site owner can
see them."
- **Only the site owner** sees them: Safety console → **People** → someone
  → **Show IP addresses** lists their recent addresses and the other
  accounts seen on the same ones (age group, suspended or banned where,
  last seen), with suspended or banned accounts highlighted. Every look is
  written to the safety log. Nobody else can see any address, not even
  their own.
- **Reports keep them.** A report copies the reported person's addresses
  when it's made, when you export it and before retention would delete
  them, so they're kept while the report is open (and for the retention
  period after you close it; child-safety evidence until you delete it).
- **Same-IP hints.** If someone signs up or signs in from an address that a
  suspended account, or one banned from any outhouse, used in the last 90
  days, a quiet notice appears in the console (dashed, at the bottom of
  Open, with a small "same-IP hint" count; no badge, push or alert), once per
  pair of accounts. Home-network (LAN) and proxy addresses never count.
  **Shared addresses are common** (households, mobile networks, VPNs): a
  match is a hint, not proof, and nothing happens to anyone automatically.
- Outhouse doesn't write addresses into its own logs beyond the existing
  "failed sign-in" and "refused: blocked network" lines.

**Blocked networks.** When you make a suspension permanent (**Make
permanent…**, or **Block IPs…** on a permanently suspended person in People)
you can also block their IP addresses: the ones they used in the last 90
days (their own and their reports' copies). It's ticked by default when they
have a child-safety report (one not dismissed as a mistake), and offered but
unticked otherwise. IPv4 blocks that address; IPv6 blocks its /64. It lasts
**until you remove it** (or 1 year / 90 days if you pick that).
- **It's strict, on purpose.** While a block is active nobody can sign up,
  sign in, use the API or connect from that address, whichever account it
  is: *"Outhouse can't be used from your network right now. If you think
  this is a mistake, contact the server owner."* (sign-up says *"New
  accounts can't be created from your network right now..."*). Everyone
  already signed in who was seen on it is signed out at once. That locks
  out their whole household, and possibly a mobile network, school or
  workplace that shares the address; that's your choice under your safety
  policy, and a new network or a VPN can still get around it. Nobody is
  ever suspended because of a block.
- **Safety valves:** your own (site owner) account always works; a network
  you or your household use is skipped (with a warning in the dialog)
  unless you tick "block my own address too"; and in People, **Allow from
  blocked networks** (off by default, logged) lets someone you know is
  innocent through. Private, home-network (LAN), loopback, link-local,
  carrier-grade NAT (100.64.0.0/10) and trusted-proxy addresses are never
  blocked.
- **Every refused attempt is kept** (time, address, account or username
  tried, and what it was: sign-up, sign-in, using Outhouse or a live
  connection, the last two at most once every 10 minutes per account). They
  show under People → *Attempts from blocked networks*, in **Blocked
  networks**, and go in that person's evidence package and hand-over
  package. Attempts are kept a year, or as long as the person's report keeps
  its evidence.
- **Blocked networks** (Safety console) lists every block: network, account,
  reason, when, how long, attempts, with **Extend** and **Remove**. Block
  rows are a deliberate safety record: they're kept for their own lifetime
  (removing one marks it removed), whatever the 90-day IP retention. Blocks,
  removals, extensions and exemptions are in the safety log.

**Age groups and teens.** Sign-up asks for a date of birth: under 13s can't
make an account, 13–17 are *teens*, 18+ *adults*. Only the age group is
stored, never the date. Accounts made before this count as adults: set
anyone's age group in the console (People). For teens, by default:
- friend requests only from people they share an outhouse with **and** who
  are friends of friends or in an outhouse you mark as **family** (Teen
  protections → Family outhouses);
- adults can't start a DM with a teen they aren't friends with (they see
  "This person can't receive messages from you"), and existing one-to-one
  DMs stop if they're no longer friends; teens' DMs start as friends-only;
- age-restricted (18+) channels stay closed to them, even if they click
  "I'm 18+";
- they can always report and block.

Each of these can be switched off under **Teen protections** (as can asking
for a date of birth at sign-up).

### Image detector

A small nudity detector checks uploaded pictures: message attachments,
avatars, banners, custom emoji and stickers (PNG, JPEG, WebP and GIF; for
an animation the first frame and a couple from the middle). It runs
**entirely on your server**: no picture is ever sent anywhere else, and
Outhouse stores only the score, the labels, the faces' age estimates (numbers)
and what was decided, never a copy, thumbnail, face crop or face "fingerprint".
Link previews of pictures on other websites aren't checked. It also estimates
the age of faces in every picture it checks, to escalate explicit pictures of
young-looking people to child safety (see **Age check** below).

**How it runs.** A separate container, `detector` (built from
`deploy/detector`), on an internal Docker network with no way out to the
internet and no port opened on the server. By default it's capped at **1 CPU
and 1 GB of memory** (`DETECTOR_CPUS`, `DETECTOR_MEMORY`), runs at low
priority with one thread, and checks **one picture at a time** (more on a
bigger server: see **More than one core** below). Checking happens in the
background *after* the upload is saved, so chat never waits for it. On a
small VM it uses about 170 MB idle and 260–360 MB once it has checked a few
pictures with faces (the age model is the big part), well inside the 1 GB
cap, and takes roughly 0.05–0.3 s of one CPU per picture for nudity, plus
about 0.02 s per frame to look for faces and about 0.25 s per face found
for its age (at most 10 faces: a group photo can take 2–3 s). Big photos
are shrunk before the models see them, and every scan has a hard 60-second
limit (a scan that runs out of time is tried 3 times). If it's down (restarting, out of memory, being
rebuilt) uploads still work: pictures wait in the queue, Outhouse retries
with growing gaps (up to 5 minutes), carries on where it left off after a
restart, and **Server status** shows *Detector offline* and the queue length.

**More than one core.** On a server with cores to spare, the detector can
check several pictures at once. Set these in `deploy/.env` and run
`docker compose up -d` (both the server and the detector read
`DETECTOR_WORKERS`):

| Setting | Default | What it does |
|---|---|---|
| `DETECTOR_WORKERS` | `1` | Pictures checked at once (1–8). Each worker is its own process with its own copy of the models and one thread, so it uses up to one CPU core. |
| `DETECTOR_CPUS` | `1.0` | The detector container's CPU cap. Give it about one per worker; more workers than CPUs just share them. |
| `DETECTOR_MEMORY` | `1g` | The detector container's memory cap. |

Measured (onnxruntime 1.31, 1600×1200 JPEGs, a fifth of them with a face
that gets an age estimate): each worker is about **230 MB idle and
300–320 MB once it has checked pictures with faces**, so each extra worker
needs **about 300 MB more** (the container: ~230 MB → ~315 MB with one
worker, ~445 MB → ~595 MB with two). One worker on one CPU checked about
**380 pictures a minute** of that mix (~150 ms each); two workers on two CPUs
about **760 a minute**; two workers squeezed onto one CPU no more than one.
Real photos with several faces are slower (about 0.25 s per face).

| Server | Settings |
|---|---|
| Home VM, 2 vCPU | the defaults (1 worker, 1 CPU, 1 GB) |
| VPS-2, 4 vCPU / 8 GB | `DETECTOR_WORKERS=2`, `DETECTOR_CPUS=2`, `DETECTOR_MEMORY=2g` |
| Bigger (8+ vCPU) | `DETECTOR_WORKERS=3` or `4`, `DETECTOR_CPUS` the same, `DETECTOR_MEMORY=3g` |

For scale: 10,000 people posting 10–15k pictures a day is about 10 a minute
on average and 50–100 a minute at peaks; one worker keeps up with that on
ordinary pictures, and a second gives headroom for bursts and group photos.
Leave the server's other cores for chat, the database and voice. **Chat
never waits for the detector either way**: pictures are posted (or shown
as *Checking…* to teens) straight away and checked in the background.
**Server status → Image detector** shows the workers (busy / total, and
memory each), the queue, the average scan time over the last 100 scans,
and a hint to consider more workers if more than 50 pictures have been
waiting for over 10 minutes. Each queued picture is claimed by one worker
at a time (a claim that's left behind by a crash runs out after 5 minutes),
so nothing is checked twice.

**The model.** [NudeNet](https://github.com/notAI-tech/NudeNet) 3.4.2's
`320n` detector (a YOLOv8n trained to spot exposed and covered body parts;
12 MB ONNX, CPU only via onnxruntime). The score is the highest confidence
for an *exposed* breast, genitals, buttocks or anus. It's downloaded while
the image builds (from PyPI, checked against a pinned SHA-256; it isn't in
git), and the image is about 590 MB with the face and age models (430 MB
without them). **Licence:** NudeNet's code and
weights are under the **GNU AGPL-3.0** (the copy in the package says so,
even though PyPI's summary says MIT; the YOLOv8 base is AGPL-3.0 too).
Using it unmodified on your own server is fine; if you ever change the
detector and let people outside use it, or hand the image to someone else,
the AGPL asks you to offer them the source. The licence text is in the
image at `/models/LICENSE-nudenet.txt`.

**The face and age models.** Faces are found with OpenCV Zoo's
[YuNet](https://huggingface.co/opencv/face_detection_yunet) (2023mar, 230 KB,
**MIT** licence, `/models/LICENSE-yunet.txt`); each face's age group is
estimated with [dima806/fairface_age_image_detection](https://huggingface.co/dima806/fairface_age_image_detection),
a ViT-B/16 fine-tuned on the FairFace dataset, as onnx-community's int8 ONNX
(87 MB, **Apache-2.0**, `/models/LICENSE-age.txt`). Both are downloaded
from Hugging Face at a pinned commit while the image builds and checked
against pinned SHA-256s; both are fine for any use. The age model sorts a
face into 0–2, 3–9, 10–19, 20–29 … 70+; Outhouse turns those probabilities
into one number of years (each group's middle age, weighted). Its makers
measured about 75–80 % right for 0–2 and 3–9 year olds, but only about half
right for 10–19, and it's worse for faces that are small, turned away,
blurred, made up or partly covered. Set `DETECTOR_AGE=0` in the detector's
environment to switch faces and ages off entirely.

**What happens to a flagged picture** (score at or above your threshold):
- **In an 18+ channel where everyone who can see it is an adult:** posted as normal.
- **Sent to or from a teen** (DMs, group DMs, any channel a teen can see):
  fully held. Teens never receive it; everyone else sees *Held for review*;
  nobody can open it (you included) and a **detector report** (category
  *Sexual or explicit content*, never child safety) lands in Reports.
- **Anywhere else:** *Held for review* for everyone except the sender, who
  can still look at it, as can you; plus a detector report. Or, if you pick
  **Blur only**, it's blurred with *click to show* for everyone.
- **Avatars, banners, emoji and stickers:** replaced by a placeholder
  (an orange shield) until you approve them.
- **While a picture is being checked:** pictures to or from teens show
  *Checking…* for a moment instead (in a channel a teen can see, that's for
  everyone in it); adults elsewhere see them straight away.

On a detector report you can **Approve (release)** (it appears for everyone
at once, live), **Keep blurred** (*Keep hidden* for avatars and the like;
pictures to or from teens stay held), **Remove file** (taken out of every
message it's in, or the avatar/emoji/sticker removed), plus the usual
warn / time out / suspend actions. Held pictures can't be forwarded; a
picture that was only allowed because it's in an adult-only 18+ channel can
only be forwarded to another one.

**Child-safety reports** show the detector's result for each file as a
line of text, e.g. *Detector: 0.02 — nothing explicit found*, and, under it,
*Detector: nudity 0.82 · faces 2 · youngest estimated ~9 (preliminary
automated estimate, not evidence)*. Every report with files shows these
lines when you open it. If a file was never checked (or was checked before
the age check existed) it's checked then (the quarantined copy is read on
the server; it's still never shown or served, `/files/…` still answers 451).
A score near zero suggests a false report: you can dismiss it without
viewing anything. Neither score is proof of anything; follow the steps above.

#### Age check (explicit + a young-looking face → child safety)

Faces in **every** checked picture (attachments, avatars, banners, emoji,
stickers, the frames sampled from a GIF) get an age estimate: up to the 10
largest faces, ignoring faces smaller than 32 pixels. Only numbers are kept
(the face count, the youngest estimate and each face's estimate and
confidence); no face picture or face fingerprint is stored anywhere.

If a picture's nudity score is at or above the **escalation score** (default
**0.40**, lower than the normal sensitivity on purpose: it's the combination
that matters) **and** the youngest face is estimated at or under the **age
threshold** (default **13**), it's escalated **automatically**, in every
context including adult-only 18+ channels:
- a **child-safety report** appears (from the detector; preview *Automatic:
  explicit content with a face estimated 13 or under*, no content), or any
  open report about that file (whatever category the reporter picked) is
  re-classified to child safety and the safety log says it was automatic;
- the file is **quarantined**: 451 for everyone, you included; the message
  (and any forward of the same file) is hidden live;
- you get the **urgent alert** (push, ntfy / webhook: *Review and suspend the
  account now if in doubt*), and the report has the **Suspend now** panel,
  exactly like a child-safety report someone made.

A young face **without** nudity does nothing (family photos of kids are
fine), and teen protections are unchanged. When someone reports a file, or
you open a report, the file's estimate is shown and the same rule is applied
(re-classifying the report if it matches). The evidence package and police
hand-over carry the estimates, labelled as preliminary automated estimates.

**It errs on the side of caution.** A wrong escalation is a locked file you
judge by the numbers (you can never view a child-safety file) and
**Dismiss**: the file and message come back, the picture goes back to the
detector's normal handling (a held picture gets its normal detector report
again), and that file isn't escalated automatically again.

**Limits.** The age is a *preliminary automated estimate*, not evidence: a
guess from a face alone, often wrong by years (it struggles most around
16–20, which is why the threshold is well below that). No face found (turned
away, cropped, covered, too small) means no escalation. Drawn, cartoon and
anime faces may or may not be detected as faces, and their "age" is
meaningless; YuNet sometimes sees faces in patterns. Nothing about this
replaces people reporting things.

**Settings** (site owner only, Reports → Image detector → **Age check**):
on/off (default on), **Counts as young** 10 / **13** / 16 or under, and
**Counts as explicit** 0.25 / **0.40** / 0.60. Server status lists the
three models.

**Settings** (site owner only): Reports → **Image detector** for the whole
site, and Outhouse settings → **Image detector** for each outhouse.
- **Check uploaded pictures:** on by default, site-wide and per outhouse.
- **Sensitivity:** Low (score ≥ 0.85), **Medium** (≥ 0.60, the default),
  High (≥ 0.40: catches more, more false alarms).
- **Outside 18+ channels:** **Hold for approval** (default) or **Blur only**
  (viewers can click through). Teen rules always apply whatever you pick.
- **Hold images for teens until checked:** on by default (per outhouse, and
  site-wide for DMs).
- To run without it at all, put `DETECTOR_URL=off` in `.env`.

**It's an aid, not a guarantee.** It misses things (drawings, odd angles,
small or cropped pictures) and flags harmless ones (swimwear, babies in the
bath, skin-coloured objects, art). People can still report anything it
misses, and a false alarm costs one click to approve.

**Checking the age check in production** (harmless pictures only, never
anything explicit and never a picture of a child): add
`DETECTOR_ESCALATION_AGE=99` and `DETECTOR_ESCALATION_SCORE=0.001` to
`deploy/.env` and run `docker compose up -d server`. Now any picture with an
adult's face counts as "explicit with a young face": post a photo of your own
face (or any adult volunteer's) with a test account in a test channel. Within
a few seconds it should vanish from the chat, `/files/…` should answer 451 for
you too, an urgent alert should arrive and Reports should show *Automatic:
explicit content with a face estimated 99 or under* with the face count and
estimate. A photo with no face (a cup of tea) should do nothing. Dismiss the
report (the photo comes back), then delete both lines from `.env` and run
`docker compose up -d server` again (Server status shows red *Test age rule*
lines while they're set).

**Checking it works in production** (with harmless pictures only): add
`DETECTOR_THRESHOLD=0.001` to `deploy/.env` and run
`docker compose up -d server`. That flags nearly every picture, so post any
harmless photo (a cup of tea) in a test channel or DM with a test account: it
should show *Held for review*, the report should appear in Reports with its
score, and **Approve** should bring it back. (Server status shows a red
*Test threshold* line while it's set.) Then delete the line from `.env`, run
`docker compose up -d server` again and approve or remove the test picture.
**Server status** also shows whether the detector is online, its model and
how many pictures are waiting. The first deploy after this update builds the
detector image too (a few extra minutes; it downloads about 80 MB from PyPI
and 90 MB from Hugging Face).

**Workflow, in short:** check the shield when it lights up (red = child
safety) → read the snapshot and context → act (warn, delete, time out,
suspend) → for child safety, report to the ACCCE / eSafety (000 if a child
is in danger) and export the evidence package → note what you did and close
the report → once police are done, delete the evidence files.

## WoW tools (optional settings)
The group finder (🔎 / `/lfg`), `/item`, `/char`, characters on profiles and
Wowhead / Warcraft Logs / Raider.IO link cards work out of the box. Item cards
come from Wowhead's Classic data; lookups are cached in the database for a
week, and if Wowhead can't be reached the card falls back to a Wowhead search
link. Optional settings, added to `~/outhouse/deploy/.env` (then
`docker compose up -d`):

| Setting | Default | What it does |
|---|---|---|
| `WOWHEAD_URL` | `https://www.wowhead.com` | Where item links and name searches go |
| `WOWHEAD_BRANCH` | `classic` | Wowhead's section (if Wowhead adds a WoW: Forever section, put its name here) |
| `WOWHEAD_TOOLTIP_URL` | `https://nether.wowhead.com` | Wowhead's item-details service |
| `WOWHEAD_DATA_ENV` | `4` | Wowhead's data set number (4 = Classic era) |
| `WOWHEAD_BRANCH_FOREVER` | `forever,classic` | Wowhead sections to read WoW: Forever raid loot lists from, tried in order (one Wowhead doesn't have, or refuses with 403 as it does `/forever` today, is skipped and re-checked daily). On `classic`, Season of Discovery items and bosses are left out |
| `WOWHEAD_BRANCH_TBC` | `tbc` | Wowhead's Burning Crusade section (TBC raids' loot lists and item links) |
| `WOWHEAD_DATA_ENV_TBC` | `5` | Wowhead's data set number for TBC item tooltips |
| `WOWHEAD_MIN_GAP_MS` | `1500` | Least time between two Wowhead page reads for loot lists (be polite; bosses' pages for drop chances are read ten times slower, in the background) |
| `WOWHEAD_ICON_URL` | `https://wow.zamimg.com/images/wow/icons/large` | Where item icons come from (the server keeps a copy and serves them itself) |
| `LINK_PREVIEWS_OFFLINE` | off | `1` = never fetch pages for link previews (WoW links still get a basic card) |

### Battle.net and the Armory (optional)
With Battle.net API keys set, Outhouse can:

- **Link Battle.net** (Settings → Profile & picture): imports your WoW: Forever
  characters (first and last name, class, race including Skyborne, faction,
  level, ruleset), plus any Classic Era / TBC ones. Linking again updates them
  instead of adding them twice. The sign-in token is used once and not kept.
- **Look up on the Armory** in the *Add a character* form: type the first and
  last name, pick the region, and it fills in class, race, level, ruleset,
  guild, item level and the portrait. Nobody has to link anything for this.
- **Refresh** each WoW: Forever character, and refresh characters signed up for
  raids in the next two weeks every 6 hours by itself (slowly, a few at a time).
- Show portraits, item level and guild on character cards, profile cards, raid
  rosters (hover a name) and `/char`, and use Blizzard's WoW: Forever item data
  for `/item` when Wowhead doesn't have an item.

Without the keys all of that is hidden, everything else works, and Settings →
Profile & picture says an admin can switch it on. "View on Armory" links work
either way.

**Switching it on (about 5 minutes):**

1. Go to https://develop.battle.net and sign in with your Battle.net account
   (any account works; you may need to accept the developer terms and verify
   your email).
2. Open **API Access** (https://develop.battle.net/access/clients) and click
   **Create Client**.
3. Fill in:
   - **Client name:** `Outhouse`
   - **Redirect URLs:** `https://oh.hartspace.cc/api/bnet/callback`
     (exactly that: your domain + `/api/bnet/callback`)
   - **Service URL:** `https://oh.hartspace.cc`
   - **Intended use:** something like "Guild chat site: imports our members'
     WoW characters and shows them from the Armory."
   - Leave everything else as it is, then **Save**.
4. On the client's page, copy the **Client ID**, and click to show and copy the
   **Client Secret**. (The secret is like a password: don't paste it in chat or
   anywhere public. If it ever leaks, use "Generate new secret" there.)
5. On the server, edit `~/outhouse/deploy/.env` and add:
   ```
   BNET_CLIENT_ID=paste-the-client-id
   BNET_CLIENT_SECRET=paste-the-client-secret
   BNET_REGION=us
   ```
   `BNET_REGION` is where your characters are: `us` (the Americas *and*
   Oceania, so Australian players on the Oceanic servers are `us`), `eu`, `kr`
   or `tw`. It's only the default: people can pick another region when they
   look a character up.
6. Redeploy: in `~/outhouse/deploy` run `docker compose up -d` (or use
   `push.ps1` from your PC as usual).
7. Check it: Settings → Profile & picture should show **Link Battle.net**, and
   *Add character* should show **Look up on the Armory**.
   `docker compose logs outhouse | grep -i battle.net` shows any problems
   (the secret and tokens are never written to the log).

Running it on your PC instead? Set `PUBLIC_URL=http://localhost:8080` in `.env`
and also add `http://localhost:8080/api/bnet/callback` as a redirect URL.

**Blizzard hasn't documented WoW: Forever's API yet**, so these can be changed
in `.env` if Blizzard moves things (defaults are what players found in November 2026):

| Setting | Default | What it does |
|---|---|---|
| `BNET_PROFILE_NAMESPACES` | `profile-forever-{region},profile-classic1x-{region},profile-classic-{region}` | Character lists tried by **Link Battle.net**, in order |
| `BNET_NAMESPACE_GAMES` | (worked out from the name: `forever` → WoW: Forever, `classic1x` → Classic Era, `classic` → TBC) | Which game each list's characters are from, e.g. `profile-classic-{region}=tbc` |
| `BNET_FOREVER_PROFILE_NAMESPACE` | `profile-forever-{region}` | Where Armory look-ups find characters |
| `BNET_FOREVER_STATIC_NAMESPACE` | `static-forever-{region}` | WoW: Forever item data for `/item` |
| `ARMORY_URL_TEMPLATE` | `https://worldofwarcraft.blizzard.com/en-us/forever/{region}/armory/character/{slug}` | "View on Armory" link (Blizzard's Forever armory address). `{region}`, `{slug}` (`name-surname`), `{name}`, `{surname}`, `{Name}`, `{Surname}` are filled in. `off` hides it |
| `ARMORY_URL_TEMPLATE_TBC` | `https://worldofwarcraft.blizzard.com/en-us/classicann/{region}/armory/character/{realm}/{name}` | "View on Armory" for TBC Anniversary characters. `{realm}` is the realm's slug (`Pyrewood Village` → `pyrewood-village`), `{name}` the name in lower case; the region is the character's (set in its Region box, default US). `off` hides it |
| `ARMORY_URL_TEMPLATE_ERA` | `https://worldofwarcraft.blizzard.com/en-us/classic1x/{region}/armory/character/{realm}/{name}` | The same for Classic Era characters. `off` hides it |
| `ARMORY_COMMUNITY_URL_TEMPLATE` | `https://www.aotc.gg/forever/character/{region}/{slug}` | Second link (a community armory). `off` hides it |
| `ARMORY_REFRESH_HOURS` | `6` | How often characters going to upcoming raids are refreshed (`0` = only when someone presses Refresh) |

If the Armory doesn't know a character (Blizzard's EU Forever data started
late, and brand-new characters can take a while), people see a friendly note
and can still save the character by hand.

## Email & sign-in

Outhouse uses email for three things: confirming the address people sign
up with, **"Forgot password?"** links, and short security notices
("your password was changed", "two-factor was turned off"). Every new
account gives an email; older accounts see a banner asking them to add one.
Two-factor sign-in (authenticator-app codes, with "Trust this device") is
optional for everyone, but the app suggests it, more firmly for you and
anyone who can manage or ban in an outhouse.

### What goes in `deploy/.env`

```
SMTP_HOST=smtp-relay.brevo.com      # your provider's SMTP server
SMTP_PORT=587                       # 587 (STARTTLS) or 465 (TLS)
SMTP_USERNAME=...                   # from the provider
SMTP_PASSWORD=...                   # from the provider (an SMTP key / app password)
SMTP_TLS=starttls                   # starttls for 587, tls for 465 (blank = picked from the port)
MAIL_FROM=Outhouse <no-reply@hartspace.cc>
AUTH_SECRET_KEY=...                 # made by ./setup.sh; see below
```

Then `docker compose up -d` and, in Outhouse, **User settings → SITE OWNER →
Server status → Email & sign-in → Send a test email**. It says straight
away whether the provider took the mail (and the error if not). Links in
emails always use `PUBLIC_URL` (`https://<your domain>`, set in
docker-compose.yml), never whatever address a request came in on.

Providers (any SMTP service works; these have free tiers):

- **Brevo**: Settings → SMTP & API → SMTP: host `smtp-relay.brevo.com`, port
  587, the login shown there, and a generated SMTP key as the password.
  Add and verify your sending domain under Senders & domains.
- **Mailgun**: Sending → Domain settings → SMTP credentials: host
  `smtp.mailgun.org` (or `smtp.eu.mailgun.org`), port 587, user like
  `postmaster@mg.hartspace.cc`, its password.
- **Amazon SES**: SMTP settings → Create SMTP credentials: host
  `email-smtp.<region>.amazonaws.com`, port 587, the generated user and
  password (not your AWS keys). Verify the domain and ask AWS to leave the
  sandbox, or it only sends to verified addresses.
- **Gmail** (small servers): turn on 2-Step Verification for the Google
  account, make an **app password** (myaccount.google.com → Security → App
  passwords), then host `smtp.gmail.com`, port 587, your Gmail address as
  the user, the app password as the password, and `MAIL_FROM` with that
  same Gmail address. Gmail limits how much you can send a day.

**So it doesn't land in spam**, the sending domain (the part after @ in
`MAIL_FROM`) needs DNS records from your provider: an **SPF** TXT record
(e.g. `v=spf1 include:spf.brevo.com ~all`, merged with any SPF record you
already have; a domain has only one) and the **DKIM** record(s) they give
you. A **DMARC** record (`_dmarc` TXT `v=DMARC1; p=none`) helps too. The
provider's domain page shows when they're right.

Without `SMTP_HOST`, nothing breaks: sign-up still asks for an email (the
confirmation link goes out once email works), "Forgot password?" tells
people to ask you, and Server status says "Email not set up".

### `AUTH_SECRET_KEY` (two-factor)

Two-factor secrets are stored encrypted with this key, so a copy of the
database alone doesn't give anyone the codes. `./setup.sh` adds one to
`.env` (`openssl rand -hex 32`, 32+ characters). Keep a copy with your
backups (the database backup doesn't include it) and **never change it**:
with a different key, everyone's authenticator codes stop working and they
need a recovery code (or you remove their two-factor in People). If it
isn't set, the server makes a key file in the uploads volume and Server
status warns about it; setting the key later is fine (secrets made with
the file still open).

### When someone is locked out

- **Forgot the password**: "Forgot password?" on the sign-in screen sends a
  link (30 minutes, one use) to a confirmed email. The answer is the same
  whether or not the account exists. Resetting signs them out everywhere
  and forgets trusted devices; it doesn't lift a suspension, and blocked
  networks are refused as for sign-in.
- **No email, or lost it**: People → (them) → Sign-in & recovery → **Create
  password reset link**: works once for 24 hours, shown to you once, logged.
  Send it privately.
- **Two-factor on**: the reset page also asks for a code from their app or
  a recovery code. Lost the phone *and* the codes? Check it's really them,
  then **Remove two-factor** (needs a reason; logged; they get an email).

Email changes, resets, two-factor on/off/removed and recovery codes used are
all in the safety log. Bots, webhooks and feeds never sign in, so none of
this applies to them.

## Real client IPs
Outhouse needs the visitor's real address for sign-in limits and the IP
addresses above. The chain at home:

1. The internet → **Apache on 192.168.1.32** (HTTPS). `oh.hartspace.cc.conf`
   has `RequestHeader unset X-Forwarded-For early`, so whatever a visitor
   sends is thrown away, then mod_proxy adds `X-Forwarded-For: <visitor>`.
2. → **Caddy** (the `caddy-proxied` container, `Caddyfile.behind-proxy`).
   `trusted_proxies static private_ranges` makes Caddy trust Apache, keep its
   header and add Apache's address: `X-Forwarded-For: <visitor>, 192.168.1.32`.
3. → **Outhouse**. Its peer is the Caddy container (a Docker address), which
   is in `TRUSTED_PROXIES` (default: this machine, home and Docker networks),
   so it reads `X-Forwarded-For` from the right and takes the first address
   that isn't a trusted proxy: the visitor. If every address is private
   (someone at home), it takes the first one: their LAN address.

Nothing needs changing for this set-up. **To check:** Safety console →
People → yourself → Show IP addresses. Connect from outside your home (phone
on mobile data, say) and your public address shows up; at home on the LAN
you'll see a `192.168.1.x` address (marked LAN). If everyone shows as
`192.168.1.32` or a `172.x` Docker address, something in the chain above has
changed.

Anyone on the home network can reach port 8080 directly and could send a
made-up `X-Forwarded-For`; the `DOCKER-USER` rule under "Security checks"
(allow 8080 only from Apache) stops that.

**If Cloudflare proxying (the orange cloud) is ever turned on** for the
domain, Apache sees Cloudflare's addresses instead of visitors'. Tell Apache
to believe Cloudflare's `CF-Connecting-IP`, from Cloudflare only:
```bash
sudo a2enmod remoteip
```
then add to the `<VirtualHost *:443>` in `oh.hartspace.cc.conf` (one
`RemoteIPTrustedProxy` line per range; take the current list from
https://www.cloudflare.com/ips/ and keep it up to date):
```apache
RemoteIPHeader CF-Connecting-IP
RemoteIPTrustedProxy 173.245.48.0/20
RemoteIPTrustedProxy 103.21.244.0/22
# ...every IPv4 and IPv6 range on https://www.cloudflare.com/ips/
```
and `sudo systemctl reload apache2`. Keep `RequestHeader unset
X-Forwarded-For early`: mod_proxy builds the new `X-Forwarded-For` from the
client address mod_remoteip sets, so Outhouse still gets the visitor's
address and nothing a visitor typed.

**Moving to the cloud later** (`COMPOSE_PROFILES=https`): Caddy faces the
internet and handles HTTPS itself (`Caddyfile`), so there's no Apache step.
Caddy doesn't trust a visitor's `X-Forwarded-For` (they aren't a trusted
proxy), sets it to the address it sees, and Outhouse reads it from the
Caddy container as above. Behind a cloud load balancer, add its addresses to
Caddy's `trusted_proxies` and to `TRUSTED_PROXIES`.

## Security checks
Outhouse sends its own security headers (a strict Content-Security-Policy,
HSTS, no framing, no sniffing), limits sign-in attempts (10 wrong passwords
for one account or 20 from one address in 15 minutes pause sign-in for that
account/address), new accounts (20 an hour per address) and invite look-ups
(120 a minute per address), and closes other open sessions when you sign
out or change your password.

**Who can sign up.** Set `REGISTRATION` in `deploy/.env`, then
`docker compose up -d`:
- `open` (the default): anyone who can reach the site.
- `invite`: only people with a working invite link (the invite page passes
  the code along when they make their account).
- `closed`: nobody new (the very first account on a new server always works).

**Check your sites yourself** (oh.hartspace.cc and hartspace.cc by default).
On the Outhouse VM, which has Docker:

    cd ~/outhouse/deploy && ./security-scan.sh
    ./security-scan.sh --quick            # skip ZAP and the full port scan
    ./security-scan.sh oh.hartspace.cc    # one site

It checks TLS (testssl.sh), headers and http→https redirects, that `.env`,
`.git`, backups, `server-status`, folder listings and `/updates/` listings
aren't public, open ports on your public address (nmap: top 1000 TCP and the
UDP ports that matter; Postgres 5432, LiveKit's API 7880 and 8080 must be
closed) and runs an OWASP ZAP baseline (passive) scan. Nothing it does is an
attack: no password guessing, no floods. The report lands in
`~/outhouse-security/<date-time>/report.md`, with every tool's raw output
next to it, and a summary is printed at the end (it exits non-zero if
anything needs fixing).

From inside the house your router "hairpins" traffic to your own public
address, so the port scan may show the inside view. For the real outside
view, run the Windows version on a laptop connected to a **phone hotspot**:

    powershell -ExecutionPolicy Bypass -File .\deploy\security-scan.ps1

(or point an online port scanner at your public IP). Expected open: 80 and
443 (Apache), 7881 TCP and 7882 UDP (voice). Anything else, especially
5432, 7880, 8080 or 22 with passwords allowed, should be closed on the
router.

**Apache** (it also serves hartspace.cc): `deploy/apache-hardening.conf` is
a server-wide snippet (ServerTokens Prod, ServerSignature Off, TraceEnable
Off, TLS 1.2+ only with modern ciphers, no folder listings, dotfiles and
backup files refused, mod_status for localhost only, HSTS and nosniff for
every site). Install steps are at the top of the file. The Outhouse site
file (`oh.hartspace.cc.conf`) also drops any `X-Forwarded-For` a visitor
sends, so the sign-in limits see the real address.

**Other things worth knowing**
- Outhouse believes `X-Forwarded-For` only from `TRUSTED_PROXIES` (default:
  this machine, home and Docker networks). If Apache isn't on a private
  address, set `TRUSTED_PROXIES=127.0.0.1/32,172.16.0.0/12,<apache ip>/32`.
- Docker's published port 8080 (the behind-Apache set-up) skips ufw, so
  anyone on your home network can reach it directly. That's fine for a home
  LAN; to allow only Apache, add a `DOCKER-USER` rule:
  `sudo iptables -I DOCKER-USER -i eth0 -p tcp --dport 8080 ! -s 192.168.1.32 -j DROP`
  (and save it with `iptables-persistent`). Replace `eth0` with your LAN
  interface (`ip route get 192.168.1.32` shows it after `dev`). The `-i` part
  matters: without it the rule can also drop Caddy's traffic to the server
  container inside Docker, which takes the site down.
- Backups contain `.env` (every password). They're readable only by you;
  the off-site copies are as safe as the share they're on.
- `RATE_LIMITS=off` turns the limits off (for testing only).

## If something's off
- **Certificate errors / site won't load from outside:** port 80 or 443 isn't
  reaching the server, or DNS isn't pointing at your IP yet. `docker compose logs caddy`.
- **Site works from outside but not at home:** your router doesn't support
  "NAT loopback" (hairpinning). Turn it on if there's a setting, or add
  `outhouse.chat` → the server's home IP in your router's local DNS.
- **Voice connects but no audio:** UDP 7882 isn't forwarded, or the server's
  firewall blocks it (`sudo ufw status`).
- **Server mute / disconnect only work on the next rejoin:** Outhouse can't
  reach LiveKit's API. It uses `http://host.docker.internal:7880` (set as
  `LIVEKIT_API_URL` in `docker-compose.yml`); `docker compose logs server | grep LiveKit`
  shows why. The firewall rule setup.sh adds for Caddy (172.16.0.0/12 → 7880)
  covers it.
- **The old Windows setup:** your PC copy still works for development
  (`.\start.ps1`); it's a separate database from the server's.

## Running CI on your own machines

To stop paying for GitHub Actions minutes, run the builds on your own Linux/WSL and Windows machines: see [ci-runner/README.md](ci-runner/README.md).
