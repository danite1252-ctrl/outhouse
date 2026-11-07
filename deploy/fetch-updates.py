#!/usr/bin/env python3
"""Copies the newest Outhouse app builds from GitHub to this server.

Run every 10 minutes by cron (setup.sh adds it). It reads GITHUB_TOKEN,
GITHUB_REPO and DOMAIN from .env, downloads the files of the repo's "latest"
release into ./updates (served at https://<DOMAIN>/updates/), and points the
update manifests at this server. The token never leaves the server.
"""

import json
import os
import shutil
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "updates"
WANTED = {"latest.json", "android.json", "Outhouse-Windows-Setup.exe", "Outhouse-Android.apk"}


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
    with github(f"https://api.github.com/repos/{repo}/releases/tags/latest", token) as res:
        release = json.load(res)
    marker = f"{release['id']}:{release.get('published_at')}"
    stamp = OUT / ".release"
    if stamp.exists() and stamp.read_text() == marker:
        return 0  # already have it

    OUT.mkdir(exist_ok=True)
    base = f"https://{domain}/updates"
    with tempfile.TemporaryDirectory(dir=HERE) as tmp:
        tmp = Path(tmp)
        for asset in release["assets"]:
            if asset["name"] not in WANTED:
                continue
            with github(asset["url"], token, "application/octet-stream") as res, open(tmp / asset["name"], "wb") as f:
                shutil.copyfileobj(res, f)
        for name in ("latest.json", "android.json"):
            p = tmp / name
            if p.exists():
                p.write_text(p.read_text().replace("__BASE__", base))
        # Installers first, manifests last, so an app never sees a manifest
        # pointing at a file that isn't there yet.
        files = sorted(tmp.iterdir(), key=lambda p: p.suffix == ".json")
        for p in files:
            os.chmod(p, 0o644)
            os.replace(p, OUT / p.name)
    stamp.write_text(marker)
    print(f"Updated to {release.get('name')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
