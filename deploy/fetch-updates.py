#!/usr/bin/env python3
"""Copies the newest Outhouse app builds from GitHub to this server.

Run every 10 minutes by cron (setup.sh adds it). It reads GITHUB_TOKEN,
GITHUB_REPO and DOMAIN from .env, downloads the files of the repo's "latest"
release into ./updates (served at https://<DOMAIN>/updates/), and points the
update manifests at this server. The token never leaves the server.

The Android APK comes twice: Outhouse-Android.apk (older apps and old links)
and Outhouse-Android-<version>.apk, which android.json points at. Only the
newest KEEP_VERSIONED version-numbered APKs stay here.
"""

import json
import os
import re
import shutil
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "updates"
WANTED = {"latest.json", "android.json", "Outhouse-Windows-Setup.exe", "Outhouse-Android.apk"}
VERSIONED_APK = re.compile(r"^Outhouse-Android-(\d+(?:\.\d+)*)\.apk$")
KEEP_VERSIONED = 2


def wanted(name):
    return name in WANTED or bool(VERSIONED_APK.match(name))


def install(tmp, out, base):
    """Moves a downloaded release (files in tmp) into out, manifests last."""
    tmp, out = Path(tmp), Path(out)
    out.mkdir(exist_ok=True)
    for name in ("latest.json", "android.json"):
        p = tmp / name
        if p.exists():
            p.write_text(p.read_text().replace("__BASE__", base))
    # android.json names a version-numbered APK that didn't come (an older
    # release)? Point it at the plain name instead.
    manifest = tmp / "android.json"
    if manifest.exists():
        try:
            data = json.loads(manifest.read_text())
            name = str(data.get("url", "")).rsplit("/", 1)[-1]
            if VERSIONED_APK.match(name) and not (tmp / name).exists() and not (out / name).exists():
                data["url"] = f"{base}/Outhouse-Android.apk"
                manifest.write_text(json.dumps(data, indent=2))
        except ValueError:
            pass
    # Installers first, manifests last, so an app never sees a manifest
    # pointing at a file that isn't there yet.
    files = sorted(tmp.iterdir(), key=lambda p: p.suffix == ".json")
    for p in files:
        os.chmod(p, 0o644)
        os.replace(p, out / p.name)
    prune(out)


def prune(out, keep=KEEP_VERSIONED):
    """Deletes all but the newest `keep` version-numbered APKs (never the one
    android.json points at)."""
    current = None
    try:
        current = str(json.loads((Path(out) / "android.json").read_text()).get("url", "")).rsplit("/", 1)[-1]
    except (OSError, ValueError, AttributeError):
        pass
    found = []
    for p in Path(out).iterdir():
        m = VERSIONED_APK.match(p.name)
        if m and p.is_file():
            found.append((tuple(int(x) for x in m.group(1).split(".")), p))
    found.sort(reverse=True)
    for _, p in found[keep:]:
        if p.name != current:
            p.unlink(missing_ok=True)


def read_env():
    env = {}
    for line in (HERE / ".env").read_text().splitlines():
        if "=" in line and not line.lstrip().startswith("#"):
            k, v = line.split("=", 1)
            env[k.strip()] = v.strip()
    return env


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def github(url, token, accept="application/vnd.github+json"):
    req = urllib.request.Request(url, headers={
        "Authorization": f"Bearer {token}",
        "Accept": accept,
        "User-Agent": "outhouse-updates",
    })
    opener = urllib.request.build_opener(NoRedirect)
    try:
        return opener.open(req, timeout=60)
    except urllib.error.HTTPError as e:
        if e.code in (301, 302, 303, 307, 308):
            # Asset downloads redirect to storage that must NOT get the token.
            location = e.headers["Location"]
            return urllib.request.urlopen(
                urllib.request.Request(location, headers={"User-Agent": "outhouse-updates"}), timeout=300)
        raise


def main():
    env = read_env()
    token, repo, domain = env.get("GITHUB_TOKEN"), env.get("GITHUB_REPO"), env.get("DOMAIN")
    if not token or not repo:
        print("GITHUB_TOKEN / GITHUB_REPO not set in .env; app updates are off.")
        return 0
    try:
        with github(f"https://api.github.com/repos/{repo}/releases/tags/latest", token) as res:
            release = json.load(res)
    except urllib.error.HTTPError as e:
        if e.code in (401, 403):
            print(f"GitHub refused the token ({e.code}). Check GITHUB_TOKEN in .env: it should be one "
                  f"fine-grained token with Contents: Read-only on {repo}.")
        elif e.code == 404:
            print(f"No 'latest' release found in {repo} (or the token can't see that repo).")
        else:
            print(f"GitHub error {e.code}.")
        return 1
    marker = f"{release['id']}:{release.get('published_at')}"
    stamp = OUT / ".release"
    if stamp.exists() and stamp.read_text() == marker:
        return 0  # already have it

    OUT.mkdir(exist_ok=True)
    base = f"https://{domain}/updates"
    with tempfile.TemporaryDirectory(dir=HERE) as tmp:
        tmp = Path(tmp)
        for asset in release["assets"]:
            if not wanted(asset["name"]):
                continue
            with github(asset["url"], token, "application/octet-stream") as res, open(tmp / asset["name"], "wb") as f:
                shutil.copyfileobj(res, f)
        install(tmp, OUT, base)
    stamp.write_text(marker)
    print(f"Updated to {release.get('name')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
