#!/usr/bin/env python3
"""Outhouse image detector: a tiny local HTTP service that scores pictures
for nudity with the NudeNet 320n model (ONNX, CPU only).

Nothing leaves the machine: the Outhouse server posts the picture's bytes,
this answers with a score and labels, and keeps nothing (no files, no logs
of the picture). It listens on the compose network only.

  GET  /health  -> {"ok": true, "model": "...", "test_mode": false}
  POST /scan    (body: the picture's bytes)
                -> 200 {"score": 0.0-1.0, "labels": [{"label", "score"}], "model", "frames", "ms"}
                -> 415 not a picture (PNG, JPEG, GIF or WebP), 413 too big,
                   422 couldn't be read, 504 took too long

One picture at a time: inference runs in a child process (low priority, one
thread). A scan that runs over the time limit kills that process and a new
one starts, so a bad file can't wedge the service.

DETECTOR_TEST_MODE=1 (CI only): a PNG with a tEXt chunk
"OuthouseDetectorTest" = "flag" scores 0.97, "slow" waits a few seconds
first; everything else is capped at 0.05. Real explicit pictures are never
needed for the tests.
"""

import io
import json
import multiprocessing as mp
import os
import struct
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_PATH = os.environ.get("DETECTOR_MODEL", "/models/320n.onnx")
MODEL_NAME = os.environ.get("DETECTOR_MODEL_NAME", "nudenet-320n (3.4.2)")
PORT = int(os.environ.get("DETECTOR_PORT", "8095"))
BIND = os.environ.get("DETECTOR_BIND", "0.0.0.0")
TEST_MODE = os.environ.get("DETECTOR_TEST_MODE", "") == "1"
MAX_BYTES = int(os.environ.get("DETECTOR_MAX_BYTES", str(32 * 1024 * 1024)))
SCAN_TIMEOUT = float(os.environ.get("DETECTOR_SCAN_TIMEOUT", "20"))
MAX_PIXELS = 50_000_000
SIZE = 320

LABELS = [
    "FEMALE_GENITALIA_COVERED", "FACE_FEMALE", "BUTTOCKS_EXPOSED", "FEMALE_BREAST_EXPOSED",
    "FEMALE_GENITALIA_EXPOSED", "MALE_BREAST_EXPOSED", "ANUS_EXPOSED", "FEET_EXPOSED",
    "BELLY_COVERED", "FEET_COVERED", "ARMPITS_COVERED", "ARMPITS_EXPOSED", "FACE_MALE",
    "BELLY_EXPOSED", "MALE_GENITALIA_EXPOSED", "ANUS_COVERED", "FEMALE_BREAST_COVERED",
    "BUTTOCKS_COVERED",
]
# What counts towards the score (the rest are only reported as labels).
EXPLICIT = {
    "FEMALE_GENITALIA_EXPOSED", "MALE_GENITALIA_EXPOSED", "FEMALE_BREAST_EXPOSED",
    "ANUS_EXPOSED", "BUTTOCKS_EXPOSED",
}
LABEL_FLOOR = 0.25  # labels below this aren't reported


def sniff(data):
    """The picture type from its first bytes (None = not a picture we scan)."""
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if data.startswith(b"\xff\xd8\xff"):
        return "jpeg"
    if data[:6] in (b"GIF87a", b"GIF89a"):
        return "gif"
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "webp"
    return None


def test_marker(data):
    """The value of a PNG tEXt chunk "OuthouseDetectorTest", if any."""
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        return None
    pos = 8
    while pos + 8 <= len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"tEXt" and b"\0" in body:
            key, value = body.split(b"\0", 1)
            if key == b"OuthouseDetectorTest":
                return value.decode("latin-1")
        if kind == b"IEND":
            break
        pos += 12 + length
    return None


# ---- the child process that runs the model ----------------------------------

def _frames(data):
    """Up to three RGB frames (first, and a couple from the middle of an animation)."""
    from PIL import Image
    Image.MAX_IMAGE_PIXELS = MAX_PIXELS
    img = Image.open(io.BytesIO(data))
    if img.format == "JPEG":
        # Decode JPEGs at a reduced size straight away (much less memory).
        img.draft("RGB", (SIZE * 2, SIZE * 2))
    n = getattr(img, "n_frames", 1) or 1
    picks = sorted({0, n // 3, (2 * n) // 3}) if n > 2 else list(range(n))
    out = []
    for i in picks[:3]:
        if i:
            img.seek(i)
        frame = img.copy()
        # Shrink before anything else: the model only sees 320x320.
        frame.thumbnail((SIZE * 2, SIZE * 2))
        frame = frame.convert("RGBA")
        # Transparent areas go black (like NudeNet's own loader).
        flat = Image.new("RGB", frame.size, (0, 0, 0))
        flat.paste(frame, mask=frame.getchannel("A"))
        out.append(flat)
    return out


def _tensor(frame):
    """Pad to a square (top-left), shrink to 320x320, 0-1 floats, NCHW."""
    import numpy as np
    from PIL import Image
    w, h = frame.size
    side = max(w, h)
    square = Image.new("RGB", (side, side), (0, 0, 0))
    square.paste(frame, (0, 0))
    small = square.resize((SIZE, SIZE), Image.BILINEAR)
    arr = np.asarray(small, dtype=np.float32) / 255.0
    return arr.transpose(2, 0, 1)[None, ...]


def _worker(conn):
    try:
        os.nice(10)
    except OSError:
        pass
    import onnxruntime as ort
    opts = ort.SessionOptions()
    opts.intra_op_num_threads = 1
    opts.inter_op_num_threads = 1
    opts.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    session = ort.InferenceSession(MODEL_PATH, sess_options=opts, providers=["CPUExecutionProvider"])
    input_name = session.get_inputs()[0].name
    conn.send({"ready": True})
    while True:
        try:
            data = conn.recv()
        except EOFError:
            return
        try:
            frames = _frames(data)
            best = {}
            for frame in frames:
                out = session.run(None, {input_name: _tensor(frame)})[0][0]  # (22, anchors)
                per_class = out[4:].max(axis=1)
                for i, s in enumerate(per_class.tolist()):
                    if s > best.get(LABELS[i], 0.0):
                        best[LABELS[i]] = s
            score = max([best.get(k, 0.0) for k in EXPLICIT] + [0.0])
            labels = sorted(
                ({"label": k, "score": round(float(v), 4)} for k, v in best.items() if v >= LABEL_FLOOR),
                key=lambda x: -x["score"],
            )[:8]
            conn.send({"score": round(float(score), 4), "labels": labels, "frames": len(frames)})
        except Exception as e:  # a picture Pillow can't read, a bomb, …
            conn.send({"error": f"couldn't read the picture ({type(e).__name__})"})


class Scanner:
    """One child process, one picture at a time, killed on a timeout."""

    def __init__(self):
        self.lock = threading.Lock()
        self.proc = None
        self.conn = None
        self.ctx = mp.get_context("spawn")
        self._start()

    def _start(self):
        parent, child = self.ctx.Pipe()
        self.proc = self.ctx.Process(target=_worker, args=(child,), daemon=True)
        self.proc.start()
        child.close()
        self.conn = parent
        if not parent.poll(120) or not parent.recv().get("ready"):
            raise RuntimeError("the model didn't load")

    def _restart(self):
        try:
            self.proc.kill()
            self.proc.join(5)
        except Exception:
            pass
        self._start()

    def scan(self, data):
        with self.lock:
            if not self.proc.is_alive():
                self._restart()
            self.conn.send(data)
            if not self.conn.poll(SCAN_TIMEOUT):
                self._restart()
                return {"timeout": True}
            return self.conn.recv()


SCANNER = None


class Handler(BaseHTTPRequestHandler):
    server_version = "OuthouseDetector/1"

    def log_message(self, fmt, *args):  # no request logs (nothing about pictures)
        pass

    def _json(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == "/health":
            alive = SCANNER is not None and SCANNER.proc.is_alive()
            return self._json(200 if alive else 503, {"ok": alive, "model": MODEL_NAME, "test_mode": TEST_MODE})
        self._json(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/scan":
            return self._json(404, {"error": "not found"})
        try:
            length = int(self.headers.get("Content-Length") or "0")
        except ValueError:
            length = 0
        if length <= 0:
            return self._json(400, {"error": "send the picture's bytes"})
        if length > MAX_BYTES:
            return self._json(413, {"error": "too big"})
        head = self.rfile.read(min(length, 16))
        if sniff(head) is None:
            # Reject quickly (read and drop the rest so the connection stays sane).
            remaining = length - len(head)
            while remaining > 0:
                chunk = self.rfile.read(min(remaining, 1 << 16))
                if not chunk:
                    break
                remaining -= len(chunk)
            return self._json(415, {"error": "not a picture (PNG, JPEG, GIF or WebP)"})
        data = head + self.rfile.read(length - len(head))
        started = time.monotonic()
        marker = test_marker(data) if TEST_MODE else None
        if marker == "slow":
            time.sleep(float(os.environ.get("DETECTOR_TEST_SLOW_SECS", "5")))
        result = SCANNER.scan(data)
        ms = int((time.monotonic() - started) * 1000)
        if result.get("timeout"):
            return self._json(504, {"error": "took too long"})
        if "error" in result:
            return self._json(422, {"error": result["error"]})
        if TEST_MODE:
            if marker == "flag":
                result["score"] = 0.97
                result["labels"] = [{"label": "TEST_MARKER", "score": 0.97}]
            else:
                result["model_score"] = result["score"]
                result["score"] = min(result["score"], 0.05)
        result["model"] = MODEL_NAME + (" [test mode]" if TEST_MODE else "")
        result["ms"] = ms
        self._json(200, result)


def main():
    global SCANNER
    if "--check" in sys.argv:
        # Build-time self test: the model loads and a plain picture scores low.
        from PIL import Image
        buf = io.BytesIO()
        Image.new("RGB", (64, 48), (90, 140, 200)).save(buf, "PNG")
        s = Scanner()
        r = s.scan(buf.getvalue())
        print(json.dumps(r))
        s.proc.kill()
        sys.exit(0 if "score" in r and r["score"] < 0.2 else 1)
    SCANNER = Scanner()
    httpd = ThreadingHTTPServer((BIND, PORT), Handler)
    httpd.daemon_threads = True
    print(f"outhouse detector listening on {BIND}:{PORT} ({MODEL_NAME}{', TEST MODE' if TEST_MODE else ''})", flush=True)
    httpd.serve_forever()


if __name__ == "__main__":
    main()
