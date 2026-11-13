#!/usr/bin/env python3
"""Outhouse image detector: a tiny local HTTP service that scores pictures
for nudity with the NudeNet 320n model, and finds faces (OpenCV Zoo's YuNet)
and estimates each face's age group (a FairFace ViT classifier). ONNX, CPU
only.

Nothing leaves the machine: the Outhouse server posts the picture's bytes,
this answers with a score, labels and age estimates (numbers only), and
keeps nothing (no files, no face crops, no embeddings, no logs of the
picture). It listens on the compose network only.

  GET  /health  -> {"ok": true, "model": "...", "models": {...}, "test_mode": false}
  POST /scan    (body: the picture's bytes)
                -> 200 {"score": 0.0-1.0, "labels": [{"label", "score"}],
                        "faces": n, "min_age": years or null,
                        "ages": [{"age", "confidence", "range"}], youngest first,
                        "model", "frames", "ms", "age_ms"}
                -> 415 not a picture (PNG, JPEG, GIF or WebP), 413 too big,
                   422 couldn't be read, 504 took too long

Age estimates are a preliminary automated estimate (an age group's
probabilities turned into a number of years), not evidence: the server only
uses them to escalate explicit pictures with a face estimated clearly young.

One picture at a time: inference runs in a child process (low priority, one
thread). A scan that runs over the time limit kills that process and a new
one starts, so a bad file can't wedge the service.

DETECTOR_TEST_MODE=1 (CI only): a PNG with a tEXt chunk
"OuthouseDetectorTest" = "flag" scores 0.97, "slow" waits a few seconds
first; everything else is capped at 0.05. Faces and ages can be simulated
too: "young-flag" (0.97 and one face estimated ~8), "young" (a face ~8,
nothing explicit) and "adult-flag" (0.97 and a face ~34). Real explicit
pictures, and real photos of people, are never needed for the tests.
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
FACE_MODEL_PATH = os.environ.get("DETECTOR_FACE_MODEL", "/models/yunet.onnx")
FACE_MODEL_NAME = "yunet-2023mar (OpenCV Zoo)"
AGE_MODEL_PATH = os.environ.get("DETECTOR_AGE_MODEL", "/models/age.onnx")
AGE_MODEL_NAME = "fairface-age-vit (dima806, int8)"
# DETECTOR_AGE=0 turns faces and ages off (less memory and CPU).
AGE_ON = os.environ.get("DETECTOR_AGE", "1") != "0" and os.path.exists(AGE_MODEL_PATH) and os.path.exists(FACE_MODEL_PATH)
FACE_SCORE = float(os.environ.get("DETECTOR_FACE_SCORE", "0.6"))
MIN_FACE = int(os.environ.get("DETECTOR_MIN_FACE", "32"))
MAX_FACES = int(os.environ.get("DETECTOR_MAX_FACES", "10"))
PORT = int(os.environ.get("DETECTOR_PORT", "8095"))
BIND = os.environ.get("DETECTOR_BIND", "0.0.0.0")
TEST_MODE = os.environ.get("DETECTOR_TEST_MODE", "") == "1"
MAX_BYTES = int(os.environ.get("DETECTOR_MAX_BYTES", str(32 * 1024 * 1024)))
SCAN_TIMEOUT = float(os.environ.get("DETECTOR_SCAN_TIMEOUT", "60"))
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

FACE_SIZE = 640  # YuNet's input (fixed)
AGE_SIZE = 224
# The age model's groups and the age each one counts as (its middle).
AGE_GROUPS = ["0-2", "3-9", "10-19", "20-29", "30-39", "40-49", "50-59", "60-69", "70+"]
AGE_MIDS = [1.0, 6.0, 14.5, 24.5, 34.5, 44.5, 54.5, 64.5, 75.0]

# Test mode: what each marker simulates (score, faces' ages).
TEST_FACES = {
    "young-flag": (0.97, [8.0]),
    "young": (None, [8.0]),
    "adult-flag": (0.97, [34.0]),
}


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


def _faces(face_session, frame, min_score):
    """YuNet: faces in one frame as (x, y, w, h, score, right eye, left eye),
    in the frame's pixels. The frame (at most 640 on its long side) is padded
    to 640x640 at the top left; YuNet wants BGR, 0-255."""
    import numpy as np
    arr = np.zeros((FACE_SIZE, FACE_SIZE, 3), dtype=np.float32)
    rgb = np.asarray(frame, dtype=np.float32)
    h, w = rgb.shape[:2]
    arr[:h, :w] = rgb[:, :, ::-1]
    names = [o.name for o in face_session.get_outputs()]
    outs = dict(zip(names, face_session.run(None, {face_session.get_inputs()[0].name: arr.transpose(2, 0, 1)[None]})))
    found = []
    for stride in (8, 16, 32):
        n = FACE_SIZE // stride
        cls = np.clip(outs[f"cls_{stride}"][0, :, 0], 0, 1)
        obj = np.clip(outs[f"obj_{stride}"][0, :, 0], 0, 1)
        score = np.sqrt(cls * obj)
        keep = np.nonzero(score >= min_score)[0]
        if not len(keep):
            continue
        box = outs[f"bbox_{stride}"][0][keep]
        kps = outs[f"kps_{stride}"][0][keep]
        row, col = keep // n, keep % n
        cx = (col + box[:, 0]) * stride
        cy = (row + box[:, 1]) * stride
        bw = np.exp(box[:, 2]) * stride
        bh = np.exp(box[:, 3]) * stride
        for i in range(len(keep)):
            eyes = ((kps[i, 0] + col[i]) * stride, (kps[i, 1] + row[i]) * stride,
                    (kps[i, 2] + col[i]) * stride, (kps[i, 3] + row[i]) * stride)
            found.append((float(cx[i] - bw[i] / 2), float(cy[i] - bh[i] / 2), float(bw[i]), float(bh[i]),
                          float(score[i]), tuple(float(e) for e in eyes)))
    # Non-maximum suppression (IoU 0.3, like OpenCV's YuNet demo).
    found.sort(key=lambda f: -f[4])
    kept = []
    for f in found:
        if all(_iou(f, k) < 0.3 for k in kept):
            kept.append(f)
    # Only faces inside the picture (not the padding) and big enough to judge.
    return [f for f in kept if f[2] >= MIN_FACE and f[3] >= MIN_FACE and f[0] + f[2] / 2 < w and f[1] + f[3] / 2 < h]


def _iou(a, b):
    ax2, ay2, bx2, by2 = a[0] + a[2], a[1] + a[3], b[0] + b[2], b[1] + b[3]
    iw = max(0.0, min(ax2, bx2) - max(a[0], b[0]))
    ih = max(0.0, min(ay2, by2) - max(a[1], b[1]))
    inter = iw * ih
    union = a[2] * a[3] + b[2] * b[3] - inter
    return inter / union if union > 0 else 0.0


def _face_crop(frame, face):
    """The face, levelled by its eyes, with a margin like FairFace's crops
    (about 1.5x the face box), 224x224, as the age model's input."""
    import math
    import numpy as np
    from PIL import Image
    x, y, w, h, _, (rx, ry, lx, ly) = face
    cx, cy = x + w / 2, y + h / 2
    side = 1.5 * max(w, h)
    angle = math.degrees(math.atan2(ly - ry, lx - rx))
    # A bigger square first (so rotating doesn't cut corners), then level and trim.
    big = side * 1.5
    box = (int(round(cx - big / 2)), int(round(cy - big / 2)), int(round(cx + big / 2)), int(round(cy + big / 2)))
    region = frame.crop(box)  # outside the picture is black
    if abs(angle) > 1:
        region = region.rotate(angle, resample=Image.BILINEAR, center=(region.width / 2, region.height / 2))
    off = (region.width - side) / 2
    region = region.crop((int(round(off)), int(round(off)), int(round(off + side)), int(round(off + side))))
    region = region.resize((AGE_SIZE, AGE_SIZE), Image.BILINEAR)
    arr = (np.asarray(region, dtype=np.float32) / 255.0 - 0.5) / 0.5
    return arr.transpose(2, 0, 1)


def _ages(age_session, crops):
    """Each face's age estimate: the age groups' probabilities, weighted by
    each group's middle age; plus the likeliest group and its probability."""
    import numpy as np
    logits = age_session.run(None, {age_session.get_inputs()[0].name: np.stack(crops)})[0]
    logits = logits - logits.max(axis=1, keepdims=True)
    probs = np.exp(logits)
    probs /= probs.sum(axis=1, keepdims=True)
    out = []
    for p in probs:
        best = int(p.argmax())
        out.append({
            "age": round(float((p * np.array(AGE_MIDS)).sum()), 1),
            "confidence": round(float(p[best]), 3),
            "range": AGE_GROUPS[best],
        })
    return out


def _sessions():
    import onnxruntime as ort
    opts = ort.SessionOptions()
    opts.intra_op_num_threads = 1
    opts.inter_op_num_threads = 1
    opts.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL

    def load(path):
        return ort.InferenceSession(path, sess_options=opts, providers=["CPUExecutionProvider"])

    session = load(MODEL_PATH)
    if not AGE_ON:
        return session, None, None
    return session, load(FACE_MODEL_PATH), load(AGE_MODEL_PATH)


def _face_ages(face_session, age_session, frames, min_score=None, with_ages=True):
    """Faces in the frames (the biggest MAX_FACES of them) and their ages.
    Without ages (the slow part), only the faces are counted."""
    min_score = FACE_SCORE if min_score is None else min_score
    faces, most = [], 0
    for frame in frames:
        found = _faces(face_session, frame, min_score)
        most = max(most, len(found))
        faces += [(f[2] * f[3], frame, f) for f in found]
    faces.sort(key=lambda t: -t[0])
    faces = faces[:MAX_FACES]
    if not with_ages:
        return {"faces": most, "min_age": None, "ages": [], "age_skipped": bool(faces)}
    ages = _ages(age_session, [_face_crop(frame, f) for _, frame, f in faces]) if faces else []
    ages.sort(key=lambda a: a["age"])
    return {"faces": most, "min_age": ages[0]["age"] if ages else None, "ages": ages}


def _worker(conn):
    try:
        os.nice(10)
    except OSError:
        pass
    session, face_session, age_session = _sessions()
    input_name = session.get_inputs()[0].name
    conn.send({"ready": True})
    while True:
        try:
            data, age_min = conn.recv()
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
            result = {"score": round(float(score), 4), "labels": labels, "frames": len(frames)}
            if face_session is not None:
                started = time.monotonic()
                # Ages only matter for pictures explicit enough for the age
                # rule (the server sends its score); below it they're skipped.
                result.update(_face_ages(face_session, age_session, frames, with_ages=score >= age_min))
                result["age_ms"] = int((time.monotonic() - started) * 1000)
            conn.send(result)
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

    def scan(self, data, age_min=0.0):
        with self.lock:
            if not self.proc.is_alive():
                self._restart()
            self.conn.send((data, age_min))
            if not self.conn.poll(SCAN_TIMEOUT):
                self._restart()
                return {"timeout": True}
            return self.conn.recv()


SCANNER = None
MODELS = {"nudity": MODEL_NAME, "faces": FACE_MODEL_NAME if AGE_ON else None, "age": AGE_MODEL_NAME if AGE_ON else None}


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
            return self._json(200 if alive else 503, {
                "ok": alive, "model": MODEL_NAME, "models": MODELS, "age": AGE_ON, "test_mode": TEST_MODE,
            })
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
        try:
            age_min = float(self.headers.get("X-Age-Min-Score") or "0")
        except ValueError:
            age_min = 0.0
        result = SCANNER.scan(data, age_min)
        ms = int((time.monotonic() - started) * 1000)
        if result.get("timeout"):
            return self._json(504, {"error": "took too long"})
        if "error" in result:
            return self._json(422, {"error": result["error"]})
        if TEST_MODE:
            if marker in ("flag", "young-flag", "adult-flag"):
                result["score"] = 0.97
                result["labels"] = [{"label": "TEST_MARKER", "score": 0.97}]
            else:
                result["model_score"] = result["score"]
                result["score"] = min(result["score"], 0.05)
            if marker in TEST_FACES:
                ages = TEST_FACES[marker][1]
                result["faces"] = len(ages)
                result["ages"] = [{"age": a, "confidence": 0.9, "range": "3-9" if a < 10 else "30-39"} for a in ages]
                result["min_age"] = min(ages)
        result["model"] = MODEL_NAME + (" [test mode]" if TEST_MODE else "")
        if AGE_ON:
            result["age_model"] = f"{FACE_MODEL_NAME} + {AGE_MODEL_NAME}"
        result["ms"] = ms
        self._json(200, result)


def _drawn_face():
    """A simple drawn face (shapes, no real person) for the self test."""
    from PIL import Image, ImageDraw
    size = 400
    im = Image.new("RGB", (size, size), (200, 220, 235))
    d = ImageDraw.Draw(im)
    cx, cy, r = size // 2, size // 2, size // 4
    d.ellipse([cx - r, cy - int(r * 1.25), cx + r, cy + int(r * 1.25)], fill=(224, 182, 150))
    d.ellipse([cx - r, cy - int(r * 1.35), cx + r, cy - int(r * 0.55)], fill=(90, 60, 40))
    for ex in (cx - r // 2.5, cx + r // 2.5):
        d.ellipse([ex - r // 6, cy - r // 6, ex + r // 6, cy + r // 12], fill=(255, 255, 255))
        d.ellipse([ex - r // 12, cy - r // 10, ex + r // 12, cy + r // 24], fill=(60, 40, 30))
    d.polygon([(cx, cy), (cx - r // 10, cy + r // 3), (cx + r // 10, cy + r // 3)], fill=(200, 150, 120))
    d.arc([cx - r // 3, cy + r // 3, cx + r // 3, cy + int(r * 0.75)], 20, 160, fill=(150, 60, 60), width=4)
    buf = io.BytesIO()
    im.save(buf, "PNG")
    return buf.getvalue()


def main():
    global SCANNER
    if "--check" in sys.argv:
        # Build-time self test: the models load and a plain picture scores
        # low with no faces...
        from PIL import Image
        buf = io.BytesIO()
        Image.new("RGB", (64, 48), (90, 140, 200)).save(buf, "PNG")
        s = Scanner()
        r = s.scan(buf.getvalue())
        print(json.dumps(r))
        s.proc.kill()
        ok = "score" in r and r["score"] < 0.2
        if AGE_ON:
            ok = ok and r.get("faces") == 0 and r.get("min_age") is None
            # ...and a drawn face (no real person) is found and gets an age
            # estimate (the face and age models are wired up right).
            face, age, frames = _sessions()[1:] + (_frames(_drawn_face()),)
            fa = _face_ages(face, age, frames, min_score=0.3)
            print(json.dumps(fa))
            ok = ok and fa["faces"] >= 1 and 0 <= fa["min_age"] <= 80
        sys.exit(0 if ok else 1)
    SCANNER = Scanner()
    httpd = ThreadingHTTPServer((BIND, PORT), Handler)
    httpd.daemon_threads = True
    print(f"outhouse detector listening on {BIND}:{PORT} ({MODEL_NAME}{', faces + ages' if AGE_ON else ''}{', TEST MODE' if TEST_MODE else ''})", flush=True)
    httpd.serve_forever()


if __name__ == "__main__":
    main()
