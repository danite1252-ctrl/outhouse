"""Harmless test pictures for the image detector's tests (no real explicit
pictures are ever needed). The detector's test mode (DETECTOR_TEST_MODE=1)
scores a PNG with the tEXt chunk "OuthouseDetectorTest" = "flag" at 0.97,
"slow" waits a few seconds first, and caps everything else at 0.05. Faces
and ages are simulated the same way: "young-flag" (0.97 + a face estimated
~8), "young" (a face ~8, nothing explicit), "adult-flag" (0.97 + a face ~34).
They're plain coloured squares: no people, nothing explicit.

  python3 ci/detector_images.py /tmp/dt   -> plain.png flag.png slow.png avatar-flag.png young-*.png …
"""

import os
import struct
import sys
import zlib


def png(w, h, rgb, text=None):
    raw = b"".join(b"\0" + bytes(rgb) * w for _ in range(h))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    if text:
        out += chunk(b"tEXt", b"OuthouseDetectorTest\0" + text.encode())
    return out + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/dt"
    os.makedirs(out, exist_ok=True)
    files = {
        "plain.png": png(96, 64, (70, 130, 180)),             # steel blue: scores low
        "flag.png": png(96, 64, (255, 0, 255), "flag"),        # solid magenta + marker: flagged
        "slow.png": png(96, 64, (90, 90, 90), "slow"),         # grey + marker: slow, then low
        "avatar-flag.png": png(64, 64, (255, 0, 255), "flag"),
        "plain2.png": png(80, 80, (40, 160, 90)),
        # The age check (solid colours + markers).
        "young-flag.png": png(96, 64, (250, 120, 30), "young-flag"),
        "young-flag2.png": png(90, 60, (240, 110, 40), "young-flag"),
        "young-flag3.png": png(84, 56, (230, 100, 50), "young-flag"),
        "young.png": png(96, 64, (120, 200, 255), "young"),
        "adult-flag.png": png(96, 64, (150, 60, 200), "adult-flag"),
    }
    # A burst of different pictures (several detector workers): 2 of 10 flagged.
    for i in range(10):
        files[f"burst-{i}.png"] = png(72 + i, 48, (20 * i, 200 - 15 * i, 90), "flag" if i in (3, 7) else None)
    for name, data in files.items():
        with open(os.path.join(out, name), "wb") as f:
            f.write(data)
    print(out)


if __name__ == "__main__":
    main()
