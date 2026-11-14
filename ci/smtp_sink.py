#!/usr/bin/env python3
"""A tiny SMTP server for the tests: accepts every mail and writes it to a
folder as JSON ({"to", "from", "subject", "text", "html"}), one file per
mail, numbered in arrival order. No TLS, no auth, standard library only
(Python 3.12+ dropped smtpd, so this speaks the few commands Outhouse's
mailer uses itself).

    python3 ci/smtp_sink.py PORT DIR
"""
import asyncio
import email
import email.policy
import json
import os
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 2525
DIR = sys.argv[2] if len(sys.argv) > 2 else "/tmp/outhouse-mail"
os.makedirs(DIR, exist_ok=True)
COUNT = len([f for f in os.listdir(DIR) if f.endswith(".json")])


def save(mail_from, rcpts, data):
    global COUNT
    msg = email.message_from_bytes(data, policy=email.policy.default)
    text = html = ""
    for part in msg.walk():
        ctype = part.get_content_type()
        if part.is_multipart():
            continue
        if ctype == "text/plain" and not text:
            text = part.get_content()
        elif ctype == "text/html" and not html:
            html = part.get_content()
    COUNT += 1
    out = {
        "n": COUNT,
        "from": mail_from,
        "to": rcpts,
        "header_to": str(msg.get("To", "")),
        "subject": str(msg.get("Subject", "")),
        "text": text,
        "html": html,
    }
    path = os.path.join(DIR, f"{COUNT:05d}.json")
    with open(path + ".tmp", "w") as f:
        json.dump(out, f)
    os.replace(path + ".tmp", path)
    print(f"mail {COUNT}: {rcpts} {out['subject']}", flush=True)


async def handle(reader, writer):
    async def send(line):
        writer.write((line + "\r\n").encode())
        await writer.drain()

    await send("220 smtp-sink ready")
    mail_from, rcpts = None, []
    try:
        while True:
            raw = await reader.readline()
            if not raw:
                break
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            cmd = line[:4].upper()
            if cmd in ("EHLO", "HELO"):
                writer.write(b"250-smtp-sink\r\n250-8BITMIME\r\n250-SMTPUTF8\r\n250 OK\r\n")
                await writer.drain()
            elif cmd == "MAIL":
                mail_from, rcpts = line.split(":", 1)[1].strip().split()[0].strip("<>"), []
                await send("250 OK")
            elif cmd == "RCPT":
                rcpts.append(line.split(":", 1)[1].strip().split()[0].strip("<>"))
                await send("250 OK")
            elif cmd == "DATA":
                await send("354 End data with <CR><LF>.<CR><LF>")
                lines = []
                while True:
                    l = await reader.readline()
                    if not l or l in (b".\r\n", b".\n"):
                        break
                    if l.startswith(b".."):
                        l = l[1:]
                    lines.append(l)
                save(mail_from, rcpts, b"".join(lines))
                await send("250 OK queued")
            elif cmd == "RSET":
                mail_from, rcpts = None, []
                await send("250 OK")
            elif cmd == "NOOP":
                await send("250 OK")
            elif cmd == "QUIT":
                await send("221 Bye")
                break
            else:
                await send("502 Not implemented")
    except (ConnectionError, asyncio.IncompleteReadError):
        pass
    finally:
        writer.close()


async def main():
    server = await asyncio.start_server(handle, "127.0.0.1", PORT)
    print(f"smtp sink on 127.0.0.1:{PORT}, mail in {DIR}", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
