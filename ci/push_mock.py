"""Stand-in for a Web Push service (and for ntfy / webhook alert targets).

  GET  /new/<name>       -> a fresh browser-style subscription {endpoint, keys}
  POST /push/<name>      -> checks the VAPID JWT, decrypts the payload, keeps it
  POST /push/gone        -> answers 410 (the browser unsubscribed)
  GET  /received/<name>  -> what arrived for that subscription
  POST /alert/<name>     -> keeps alert bodies and headers (ntfy, webhooks)
  GET  /alerts/<name>    -> what arrived
  GET  /ok               -> 200 (a stand-in "LiveKit is up" address)

Decryption uses http_ece (Mozilla's reference implementation) when it's
installed, so the server's aes128gcm encryption is checked against an
independent implementation; otherwise a small RFC 8291 decoder below.
"""

import base64
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from cryptography.hazmat.primitives import hashes, hmac, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

try:
    import http_ece  # type: ignore
except ImportError:  # pragma: no cover
    http_ece = None

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8097
ORIGIN = f"http://127.0.0.1:{PORT}"
lock = threading.Lock()
subs = {}       # name -> (private key, auth secret, public bytes)
received = {}   # name -> [entries]
alerts = {}     # name -> [entries]


def b64u(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def unb64u(s: str) -> bytes:
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))


def hkdf_expand(salt: bytes, ikm: bytes, info: bytes, length: int) -> bytes:
    h = hmac.HMAC(salt, hashes.SHA256())
    h.update(ikm)
    prk = h.finalize()
    h = hmac.HMAC(prk, hashes.SHA256())
    h.update(info + b"\x01")
    return h.finalize()[:length]


def own_decrypt(body: bytes, priv, auth: bytes, ua_public: bytes) -> bytes:
    salt, rs, idlen = body[:16], int.from_bytes(body[16:20], "big"), body[20]
    as_public = body[21:21 + idlen]
    ciphertext = body[21 + idlen:]
    assert rs >= 18
    peer = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), as_public)
    secret = priv.exchange(ec.ECDH(), peer)
    ikm = hkdf_expand(auth, secret, b"WebPush: info\x00" + ua_public + as_public, 32)
    cek = hkdf_expand(salt, ikm, b"Content-Encoding: aes128gcm\x00", 16)
    nonce = hkdf_expand(salt, ikm, b"Content-Encoding: nonce\x00", 12)
    plain = AESGCM(cek).decrypt(nonce, ciphertext, None)
    assert plain.rstrip(b"\x00").endswith(b"\x02"), "missing last-record delimiter"
    return plain.rstrip(b"\x00")[:-1]


def check_vapid(header: str) -> str:
    """Returns "" if the Authorization header is a valid VAPID token, else why not."""
    if not header.startswith("vapid "):
        return "not a vapid header"
    parts = dict(p.strip().split("=", 1) for p in header[6:].split(","))
    token, key = parts.get("t", ""), parts.get("k", "")
    head, claims, sig = token.split(".")
    pub = ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256R1(), unb64u(key))
    raw = unb64u(sig)
    if len(raw) != 64:
        return "signature isn't 64 bytes"
    der = encode_dss_signature(int.from_bytes(raw[:32], "big"), int.from_bytes(raw[32:], "big"))
    try:
        pub.verify(der, f"{head}.{claims}".encode(), ec.ECDSA(hashes.SHA256()))
    except Exception:
        return "bad signature"
    h = json.loads(unb64u(head))
    c = json.loads(unb64u(claims))
    if h.get("alg") != "ES256":
        return "alg isn't ES256"
    if c.get("aud") != ORIGIN:
        return f"aud is {c.get('aud')}"
    if not (time.time() < c.get("exp", 0) <= time.time() + 24 * 3600):
        return "bad exp"
    if not str(c.get("sub", "")).startswith(("mailto:", "https:")):
        return "bad sub"
    return ""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        sys.stderr.write("push-mock: " + (fmt % args) + "\n")

    def reply(self, code, body=None):
        data = json.dumps(body).encode() if body is not None else b""
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        parts = self.path.strip("/").split("/")
        if parts[0] == "ok":
            return self.reply(200, {"ok": True})
        if parts[0] == "new" and len(parts) == 2:
            priv = ec.generate_private_key(ec.SECP256R1())
            pub = priv.public_key().public_bytes(serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint)
            auth = bytes(range(16)) if parts[1] == "fixed" else __import__("os").urandom(16)
            with lock:
                subs[parts[1]] = (priv, auth, pub)
            return self.reply(200, {"endpoint": f"{ORIGIN}/push/{parts[1]}", "keys": {"p256dh": b64u(pub), "auth": b64u(auth)}})
        if parts[0] == "received" and len(parts) == 2:
            with lock:
                return self.reply(200, received.get(parts[1], []))
        if parts[0] == "alerts" and len(parts) == 2:
            with lock:
                return self.reply(200, alerts.get(parts[1], []))
        self.reply(404, {"error": "not found"})

    def do_POST(self):
        parts = self.path.strip("/").split("/")
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        if parts[0] == "alert" and len(parts) == 2:
            with lock:
                alerts.setdefault(parts[1], []).append({
                    "body": body.decode("utf-8", "replace"),
                    "title": self.headers.get("Title", ""),
                    "priority": self.headers.get("Priority", ""),
                    "content_type": self.headers.get("Content-Type", ""),
                })
            return self.reply(200, {"ok": True})
        if parts[0] != "push" or len(parts) != 2:
            return self.reply(404, {"error": "not found"})
        name = parts[1]
        if name == "gone":
            return self.reply(410, {"error": "unsubscribed"})
        entry = {"problems": []}
        if self.headers.get("Content-Encoding") != "aes128gcm":
            entry["problems"].append("Content-Encoding isn't aes128gcm")
        if not self.headers.get("TTL"):
            entry["problems"].append("no TTL")
        why = check_vapid(self.headers.get("Authorization", ""))
        if why:
            entry["problems"].append("VAPID: " + why)
        with lock:
            sub = subs.get(name)
        if not sub:
            return self.reply(404, {"error": "no such subscription"})
        priv, auth, pub = sub
        try:
            if http_ece is not None:
                plain = http_ece.decrypt(body, private_key=priv, auth_secret=auth, version="aes128gcm")
                entry["decrypted_by"] = "http_ece"
                # Cross-check with the small decoder too.
                if own_decrypt(body, priv, auth, pub) != plain:
                    entry["problems"].append("decoders disagree")
            else:
                plain = own_decrypt(body, priv, auth, pub)
                entry["decrypted_by"] = "push_mock"
            entry["payload"] = json.loads(plain)
        except Exception as e:  # noqa: BLE001
            entry["problems"].append(f"decrypt failed: {e!r}")
        entry["ok"] = not entry["problems"]
        with lock:
            received.setdefault(name, []).append(entry)
        self.reply(201)


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
