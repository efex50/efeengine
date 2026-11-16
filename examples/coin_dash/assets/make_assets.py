#!/usr/bin/env python3
"""Generates the coin_dash pixel art: player.png, enemy.png, coin_sheet.png, tiles.png.

Standard library only (zlib + struct). Run from anywhere:  python3 -I assets/make_assets.py
Every frame is 16x16. The PNGs are committed; rerun this only to change the art.
"""
import os
import struct
import zlib

OUT = os.path.dirname(os.path.abspath(__file__))
T = (0, 0, 0, 0)  # transparent


def png(path, width, height, pixels, palette=False, filter_type=0):
    """Write `pixels` (rows of (r,g,b,a)) as PNG: RGBA8, or 8-bit palette + tRNS if `palette`.
    `filter_type` is the scanline filter used for every row (0 none, 1 sub)."""
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    if palette:
        colors = []
        for row in pixels:
            for p in row:
                if p not in colors:
                    colors.append(p)
        assert len(colors) <= 256
        rows = [bytes(colors.index(p) for p in row) for row in pixels]
        color_type = 3
    else:
        rows = [bytes(c for p in row for c in p) for row in pixels]
        color_type = 6
    bpp = 1 if palette else 4
    raw = bytearray()
    prev = bytes(len(rows[0]))
    for row in rows:
        raw.append(filter_type)
        if filter_type == 1:
            raw += bytes((row[i] - (row[i - bpp] if i >= bpp else 0)) & 255 for i in range(len(row)))
        else:
            raw += row
        prev = row
    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, color_type, 0, 0, 0))
    if palette:
        data += chunk(b"PLTE", bytes(c for p in colors for c in p[:3]))
        data += chunk(b"tRNS", bytes(p[3] for p in colors))
    data += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    data += chunk(b"IEND", b"")
    with open(os.path.join(OUT, path), "wb") as f:
        f.write(data)


class Canvas:
    def __init__(self, w, h, bg=T):
        self.w, self.h = w, h
        self.px = [[bg] * w for _ in range(h)]

    def set(self, x, y, c):
        if 0 <= x < self.w and 0 <= y < self.h:
            self.px[y][x] = c

    def rect(self, x, y, w, h, c, ox=0):
        for j in range(h):
            for i in range(w):
                self.set(ox + x + i, y + j, c)

    def ellipse(self, cx, cy, rx, ry, c, ox=0):
        for y in range(self.h):
            for x in range(self.w):
                dx, dy = (x + 0.5 - cx) / rx, (y + 0.5 - cy) / ry
                if dx * dx + dy * dy <= 1.0:
                    self.set(ox + x, y, c)


OUTLINE = (30, 24, 46, 255)


def outline(cv, ox=0, w=16):
    """Dark one-pixel outline around the opaque pixels of the frame at column offset ox."""
    src = [[cv.px[y][ox + x] for x in range(w)] for y in range(cv.h)]
    for y in range(cv.h):
        for x in range(w):
            if src[y][x][3] == 0:
                near = any(0 <= x + dx < w and 0 <= y + dy < cv.h and src[y + dy][x + dx][3] == 255
                           for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)))
                if near:
                    cv.px[y][ox + x] = OUTLINE


def player():
    cv = Canvas(16, 16)
    body, light, dark = (70, 130, 230, 255), (120, 175, 250, 255), (40, 80, 170, 255)
    cv.rect(4, 5, 8, 8, body)
    cv.rect(5, 4, 6, 1, body)
    cv.rect(5, 6, 6, 1, light)         # highlight band
    cv.rect(4, 11, 8, 2, dark)         # lower shade
    cv.rect(4, 13, 3, 2, dark)         # feet
    cv.rect(9, 13, 3, 2, dark)
    white, pupil = (255, 255, 255, 255), (20, 20, 40, 255)
    cv.rect(5, 7, 2, 3, white)         # eyes
    cv.rect(9, 7, 2, 3, white)
    cv.rect(6, 8, 1, 2, pupil)         # looking right
    cv.rect(10, 8, 1, 2, pupil)
    outline(cv)
    return cv


def enemy():
    cv = Canvas(16, 16)
    body, light, dark = (220, 60, 60, 255), (250, 120, 110, 255), (140, 30, 50, 255)
    cv.ellipse(8, 9, 6, 5, body)
    cv.rect(4, 12, 8, 2, body)
    for x in (3, 6, 9, 12):            # spikes along the top
        cv.rect(x, 2, 1, 3, dark)
        cv.rect(x, 4, 2, 1, dark)
    cv.rect(5, 6, 3, 1, light)
    cv.rect(4, 13, 1, 1, T)            # wavy bottom
    cv.rect(7, 13, 2, 1, T)
    cv.rect(11, 13, 1, 1, T)
    white, pupil = (255, 255, 255, 255), (20, 10, 20, 255)
    cv.rect(4, 7, 3, 3, white)
    cv.rect(9, 7, 3, 3, white)
    cv.rect(5, 8, 2, 2, pupil)
    cv.rect(9, 8, 2, 2, pupil)
    cv.rect(4, 6, 3, 1, dark)          # angry brows
    cv.rect(9, 6, 3, 1, dark)
    outline(cv)
    return cv


def coin_sheet():
    """4 frames of a spinning coin: the half width goes 6, 4, 1, 4."""
    cv = Canvas(64, 16)
    gold, light, dark = (250, 200, 40, 255), (255, 240, 140, 255), (190, 130, 20, 255)
    for f, rx in enumerate((6, 4, 1.2, 4)):
        ox = f * 16
        cv.ellipse(8, 8, rx + 0.5, 6.5, dark, ox)
        cv.ellipse(8, 8, max(rx - 0.8, 0.6), 5.5, gold, ox)
        if rx > 3:
            cv.rect(7 - (1 if rx > 5 else 0), 5, 2 if rx > 5 else 1, 6, dark, ox)  # the slot
            cv.rect(5 if rx > 5 else 6, 4, 1, 2, light, ox)
        outline(cv, ox)
    return cv


def tiles():
    """5 tiles: grass, grass 2, flowers, stone path, and a soft shadow (transparent edges)."""
    cv = Canvas(80, 16)
    seed = 12345

    def rnd(n):
        nonlocal seed
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        return (seed >> 8) % n

    greens = [(86, 160, 70, 255), (80, 152, 66, 255)]
    for t in range(3):
        base = greens[t % 2]
        cv.rect(0, 0, 16, 16, base, t * 16)
        for _ in range(14):
            cv.set(t * 16 + rnd(16), rnd(16), (104, 184, 84, 255) if rnd(2) else (66, 130, 56, 255))
    for fx, fy, col in ((4, 5, (250, 240, 120, 255)), (11, 10, (250, 130, 170, 255)), (12, 3, (255, 255, 255, 255))):
        cv.set(32 + fx, fy, col)
        cv.set(32 + fx - 1, fy, col)
        cv.set(32 + fx, fy + 1, (60, 120, 50, 255))
    cv.rect(0, 0, 16, 16, (170, 160, 150, 255), 48)
    for y in range(0, 16, 8):
        for x in range(0, 16, 8):
            cv.rect(x + 1, y + 1, 6, 6, (196, 186, 174, 255), 48)
    for _ in range(10):
        cv.set(48 + rnd(16), rnd(16), (140, 132, 124, 255))
    cv.ellipse(8, 8, 7.5, 4.5, (0, 0, 0, 90), 64)  # shadow
    return cv


def main():
    p = player()
    png("player.png", 16, 16, p.px)
    e = enemy()
    png("enemy.png", 16, 16, e.px, palette=True)      # palette + tRNS: exercises that decoder path
    c = coin_sheet()
    png("coin_sheet.png", 64, 16, c.px)
    t = tiles()
    png("tiles.png", 80, 16, t.px, filter_type=1)       # Sub filter on every row
    print("wrote player.png enemy.png coin_sheet.png tiles.png in", OUT)


if __name__ == "__main__":
    main()
