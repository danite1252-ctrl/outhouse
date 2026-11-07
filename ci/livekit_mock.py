"""A tiny stand-in for LiveKit's RoomService API (Twirp), so CI can check the
calls Outhouse makes for server mute, deafen, move and disconnect.

    python3 ci/livekit_mock.py [port] [api_secret]     (default 8098, "secret")

Every POST /twirp/livekit.RoomService/<Method> is recorded with its JSON
body, the JWT's claims and whether the JWT's HS256 signature checks out.
GET /__calls lists them; POST /__reset forgets them.
"""

import base64
import hashlib
import hmac
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8098
SECRET = (sys.argv[2] if len(sys.argv) > 2 else "secret").encode()
CALLS = []


def b64decode(part):
    return base64.urlsafe_b64decode(part + "=" * (-len(part) % 4))


def check_jwt(header):
    if not header.startswith("Bearer "):
        return False, None
    token = header[len("Bearer "):]
    try:
        head, payload, sig = token.split(".")
        expected = hmac.new(SECRET, f"{head}.{payload}".encode(), hashlib.sha256).digest()
        claims = json.loads(b64decode(payload))
        return hmac.compare_digest(expected, b64decode(sig)), claims
    except Exception:
        return False, None


class Handler(BaseHTTPRequestHandler):
    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/__calls":
            return self.reply(200, CALLS)
        self.reply(404, {"code": "not_found", "msg": "nothing here"})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if self.path == "/__reset":
            CALLS.clear()
            return self.reply(200, {})
        prefix = "/twirp/livekit.RoomService/"
        if not self.path.startswith(prefix):
            return self.reply(404, {"code": "bad_route", "msg": self.path})
        method = self.path[len(prefix):]
        ok, claims = check_jwt(self.headers.get("Authorization", ""))
        try:
            body = json.loads(raw or b"{}")
        except ValueError:
            body = None
        CALLS.append({
            "method": method,
            "body": body,
            "jwt_ok": ok,
            "claims": claims,
            "content_type": self.headers.get("Content-Type"),
        })
        if not ok:
            return self.reply(401, {"code": "unauthenticated", "msg": "bad token"})
        if method == "GetParticipant":
            return self.reply(200, {
                "sid": "PA_mock",
                "identity": body.get("identity"),
                "tracks": [
                    {"sid": "TR_mic", "type": "AUDIO", "source": "MICROPHONE"},
                    {"sid": "TR_screen", "type": "VIDEO", "source": "SCREEN_SHARE"},
                ],
            })
        if method in ("MutePublishedTrack", "UpdateParticipant", "RemoveParticipant"):
            return self.reply(200, {})
        self.reply(404, {"code": "bad_route", "msg": method})

    def log_message(self, *args):
        pass


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
