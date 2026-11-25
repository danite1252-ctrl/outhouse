"""deploy/fetch-updates.py with a local stand-in release (no GitHub): the
version-numbered APK is copied next to the plain one, android.json points at
this server, and only the newest 2 version-numbered APKs stay."""

import importlib.util
import json
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("fetch_updates", HERE.parent / "deploy" / "fetch-updates.py")
fu = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fu)
BASE = "https://oh.example.test/updates"


def release(tmp, version, versioned=True):
    d = Path(tmp)
    d.mkdir(parents=True, exist_ok=True)
    (d / "Outhouse-Android.apk").write_bytes(version.encode())
    if versioned:
        (d / f"Outhouse-Android-{version}.apk").write_bytes(version.encode())
    (d / "android.json").write_text(json.dumps({"version": version, "url": f"__BASE__/Outhouse-Android-{version}.apk"}))
    return d


def apks(out):
    return sorted(p.name for p in Path(out).glob("Outhouse-Android-*.apk"))


assert fu.wanted("Outhouse-Android-0.2.512.apk") and fu.wanted("Outhouse-Android.apk") and fu.wanted("latest.json")
assert not fu.wanted("Outhouse-Android-../x.apk") and not fu.wanted("evil.apk") and not fu.wanted("Outhouse-Android-0.2.1.apk.part")

with tempfile.TemporaryDirectory() as root:
    out = Path(root) / "updates"
    # A stray file that isn't ours stays put.
    out.mkdir()
    (out / "notes.txt").write_text("keep me")
    for v in ("0.2.9", "0.2.10", "0.2.8", "0.2.11"):
        fu.install(release(Path(root) / f"rel-{v}", v), out, BASE)
        data = json.loads((out / "android.json").read_text())
        assert data["url"] == f"{BASE}/Outhouse-Android-{v}.apk", data
        assert (out / f"Outhouse-Android-{v}.apk").read_bytes() == v.encode()
        assert (out / "Outhouse-Android.apk").read_bytes() == v.encode()
    # Newest two by version (0.2.11, 0.2.10), not by arrival or text order
    # (0.2.8 stayed only while android.json pointed at it).
    assert apks(out) == ["Outhouse-Android-0.2.10.apk", "Outhouse-Android-0.2.11.apk"], apks(out)
    assert (out / "notes.txt").exists()
    # An older-style release without the version-numbered APK: android.json
    # falls back to the plain name.
    fu.install(release(Path(root) / "rel-old", "0.2.12", versioned=False), out, BASE)
    data = json.loads((out / "android.json").read_text())
    assert data["url"] == f"{BASE}/Outhouse-Android.apk", data
    assert not any(Path(root).glob("rel-0.2.11/*")), "files are moved, not copied"

    # complete(): the APK android.json names must be here, or the release is
    # fetched again (an older script marked releases done without it).
    assert fu.complete(out)
    (out / "android.json").write_text(json.dumps({"version": "0.2.13", "url": f"{BASE}/Outhouse-Android-0.2.13.apk"}))
    assert not fu.complete(out)

print("fetch-updates test passed: versioned copy, manifest URL, pruning to the newest 2.")
