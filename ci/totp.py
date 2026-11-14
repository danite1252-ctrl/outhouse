#!/usr/bin/env python3
"""Prints the authenticator-app code for a base32 secret (RFC 6238, SHA-1,
6 digits, 30 s), `offset` steps from now. Waits out the last two seconds
of a step so the code is still good when the server checks it.

    python3 ci/totp.py SECRET [OFFSET]
"""
import base64
import hashlib
import hmac
import struct
import sys
import time

secret = sys.argv[1].replace(" ", "").upper()
offset = int(sys.argv[2]) if len(sys.argv) > 2 else 0
key = base64.b32decode(secret + "=" * (-len(secret) % 8))
if time.time() % 30 > 27.5:
    time.sleep(30 - time.time() % 30 + 0.2)
step = int(time.time() // 30) + offset
h = hmac.new(key, struct.pack(">Q", step), hashlib.sha1).digest()
o = h[-1] & 0x0F
code = (struct.unpack(">I", h[o:o + 4])[0] & 0x7FFFFFFF) % 1_000_000
print(f"{code:06d}")
