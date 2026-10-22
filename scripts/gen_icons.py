#!/usr/bin/env python3
"""Generates the application icons in assets/ (PNG + Windows ICO).

The design matches the window icon drawn at runtime in src/main.rs:
a dark rounded tile with a 4x4 grid of colored "byte" cells.
Uses only the standard library.
"""

import struct
import zlib
from pathlib import Path

PALETTE = [
    (90, 160, 255),
    (110, 210, 140),
    (235, 200, 110),
    (200, 150, 255),
    (255, 135, 100),
]
BACKGROUND = (32, 36, 46)
SUPERSAMPLE = 4


def sample(x: float, y: float):
    """Color and coverage at a point in the 64x64 design space."""
    dx = abs(x - 32.0) - 22.0
    dy = abs(y - 32.0) - 22.0
    outside = (max(dx, 0.0) ** 2 + max(dy, 0.0) ** 2) ** 0.5 + min(max(dx, dy), 0.0) - 8.0
    if outside > 0.0:
        return None
    gx, gy = x - 10.0, y - 10.0
    if 0 <= gx < 44 and 0 <= gy < 44 and gx % 11 < 8 and gy % 11 < 8:
        cell = int(gx // 11) + int(gy // 11) * 4
        return PALETTE[(cell * 7 + cell // 3) % len(PALETTE)]
    return BACKGROUND


def render(size: int) -> bytes:
    scale = 64.0 / size
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            r = g = b = a = 0
            for sy in range(SUPERSAMPLE):
                for sx in range(SUPERSAMPLE):
                    c = sample(
                        (px + (sx + 0.5) / SUPERSAMPLE) * scale,
                        (py + (sy + 0.5) / SUPERSAMPLE) * scale,
                    )
                    if c is not None:
                        r += c[0]
                        g += c[1]
                        b += c[2]
                        a += 1
            n = SUPERSAMPLE * SUPERSAMPLE
            if a:
                row += bytes((r // a, g // a, b // a, a * 255 // n))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return png(size, rows)


def png(size: int, rows) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    raw = b"".join(b"\0" + r for r in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(images) -> bytes:
    """ICO container with PNG-compressed entries (supported since Vista)."""
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    data = b""
    for size, blob in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(blob), offset + len(data))
        data += blob
    return header + entries + data


def main():
    assets = Path(__file__).resolve().parent.parent / "assets"
    assets.mkdir(exist_ok=True)
    (assets / "icon.png").write_bytes(render(512))
    (assets / "icon-256.png").write_bytes(render(256))
    sizes = [16, 24, 32, 48, 64, 128, 256]
    (assets / "icon.ico").write_bytes(ico([(s, render(s)) for s in sizes]))
    print(f"wrote icons to {assets}")


if __name__ == "__main__":
    main()
