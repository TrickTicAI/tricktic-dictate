#!/usr/bin/env python3
"""Derive every shipped icon from writer-master.png.

The master is 1254x1254 RGB with no alpha channel: the rounded square sits on
solid white. Resizing it straight into an .ico leaves a white box around the
mark on the taskbar and in the browser tab, so the white surround is removed
and the rounded corners are given real transparency first.

Run from the repository root:

    python3 assets/icons/derive.py

Outputs land beside the master, in apps/web/public/ and in
apps/desktop/tauri/icons/. Re-running is idempotent.
"""

from __future__ import annotations

import io
import pathlib
import struct
import sys
from collections import deque

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parents[2]
ICONS = ROOT / "assets" / "icons"
MASTER = ICONS / "writer-master.png"
WEB_PUBLIC = ROOT / "apps" / "web" / "public"
TAURI_ICONS = ROOT / "apps" / "desktop" / "tauri" / "icons"

# Sampled from the master: the dark brown ground and the terracotta waveform.
GROUND = (55, 46, 41)
ACCENT = (178, 91, 41)
# The surround is 253-255 on every channel; the cream page never exceeds 250 on
# green or blue, so this threshold separates them without touching the artwork.
WHITE = 250


def outside_mask(image: Image.Image) -> set[tuple[int, int]]:
    """Flood fill the near-white surround from the four corners.

    A flood fill rather than a colour test, so that white *inside* the mark —
    were any ever added — stays opaque.
    """
    width, height = image.size
    pixels = image.load()
    seen: set[tuple[int, int]] = set()
    queue: deque[tuple[int, int]] = deque()
    for start in ((0, 0), (width - 1, 0), (0, height - 1), (width - 1, height - 1)):
        if min(pixels[start]) >= WHITE:
            seen.add(start)
            queue.append(start)
    while queue:
        x, y = queue.popleft()
        for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
            if 0 <= nx < width and 0 <= ny < height and (nx, ny) not in seen and min(pixels[nx, ny]) >= WHITE:
                seen.add((nx, ny))
                queue.append((nx, ny))
    return seen


def cut_alpha(image: Image.Image) -> Image.Image:
    """Return the mark cropped to its bounds with a transparent surround.

    Transparent pixels keep the ground colour rather than white, so that
    downsampling averages brown into brown at the rounded corners instead of
    fringing every derived size with a pale halo.
    """
    outside = outside_mask(image)
    width, height = image.size
    cut = Image.new("RGBA", (width, height))
    source = image.load()
    target = cut.load()
    for y in range(height):
        for x in range(width):
            if (x, y) in outside:
                target[x, y] = (*GROUND, 0)
            else:
                target[x, y] = (*source[x, y], 255)
    return cut.crop(cut.getchannel("A").getbbox())


# The seven waveform bars, measured off the master with measure_bars() below.
# Heights are relative to the tallest bar.
BAR_HEIGHTS = (0.198, 0.385, 0.658, 1.0, 0.608, 0.368, 0.190)
# Bar width as a fraction of the bar-to-bar period, also measured: 53 of 86 px.
BAR_DUTY = 0.616


def measure_bars(image: Image.Image) -> tuple[tuple[float, ...], float]:
    """Re-measure the waveform from the master, for when the artwork changes.

    Not called during a normal run: the constants above are the recorded result.
    A stray anti-aliased smudge in the artwork's lower right also matches the
    terracotta test, so only the seven tall, bar-shaped components count.
    """
    width, height = image.size
    pixels = image.load()
    columns = [
        [y for y in range(height) if is_accent(pixels[x, y])]
        for x in range(width)
    ]
    spans: list[tuple[int, int]] = []
    start = None
    for x, column in enumerate(columns):
        if column and start is None:
            start = x
        elif not column and start is not None:
            spans.append((start, x - 1))
            start = None
    bars = [
        (a, b, min(min(columns[x]) for x in range(a, b + 1) if columns[x]),
         max(max(columns[x]) for x in range(a, b + 1) if columns[x]))
        for a, b in spans
        if b - a + 1 > width * 0.02
    ][:7]
    tallest = max(bottom - top for _, _, top, bottom in bars)
    heights = tuple(round((bottom - top) / tallest, 3) for _, _, top, bottom in bars)
    period = (bars[-1][0] - bars[0][0]) / (len(bars) - 1)
    duty = round(sum(b - a + 1 for a, b, _, _ in bars) / len(bars) / period, 3)
    return heights, duty


def is_accent(pixel: tuple[int, int, int]) -> bool:
    r, g, b = pixel
    return r > 120 and r - g > 40 and g >= b - 10


def waveform_silhouette(size: int) -> Image.Image:
    """A single-colour tray mark: the waveform bars alone.

    At 16 px the document outline and its three text lines turn to mush, so the
    tray drops them. The bars are drawn at the target resolution rather than
    downsampled from the master, because a 1 px bar resampled from a 53 px one
    is a smudge, while a drawn one is a clean vertical stroke. Bar positions and
    widths are whole pixels so the verticals stay crisp; only the rounded caps
    are anti-aliased, via supersampling.

    Terracotta holds its own against both a white and a black taskbar, which a
    pure black or pure white silhouette cannot do.
    """
    pad = round(size * 0.08)
    content = size - 2 * pad
    count = len(BAR_HEIGHTS)
    # Widest whole-pixel bar whose seven bars and six gaps still fit.
    width_px = max(1, min(content // count, round(content / (count + (count - 1) * (1 - BAR_DUTY) / BAR_DUTY))))
    gap = max(1, round(width_px * (1 - BAR_DUTY) / BAR_DUTY))
    # Narrower gaps before narrower bars, so the mark keeps its size. The shell
    # draws the tray icon at runtime by the same rule (tray_mark.rs), for the
    # sizes and states this file does not write.
    while count * width_px + (count - 1) * gap > content and gap > 1:
        gap -= 1
    while count * width_px + (count - 1) * gap > content and width_px > 1:
        width_px -= 1
        gap = min(gap, max(1, round(width_px * (1 - BAR_DUTY) / BAR_DUTY)))
    span = count * width_px + (count - 1) * gap
    left = (size - span) // 2

    scale = 8
    canvas = Image.new("L", (size * scale, size * scale), 0)
    pen = ImageDraw.Draw(canvas)
    for index, ratio in enumerate(BAR_HEIGHTS):
        bar_height = max(1, round(content * ratio))
        x0 = left + index * (width_px + gap)
        y0 = (size - bar_height) // 2
        box = [x0 * scale, y0 * scale, (x0 + width_px) * scale - 1, (y0 + bar_height) * scale - 1]
        radius = min(width_px, bar_height) * scale / 2
        if radius >= 2:
            pen.rounded_rectangle(box, radius=radius, fill=255)
        else:
            pen.rectangle(box, fill=255)
    # BOX, not LANCZOS: every bar edge already sits on a whole target pixel, so
    # a plain block average keeps the verticals crisp and anti-aliases only the
    # caps. A wide resampling kernel smears a 1 px bar back into a blur, which
    # is exactly what makes a 16 px tray icon unreadable.
    alpha = canvas.resize((size, size), Image.BOX)
    out = Image.new("RGBA", (size, size), (*ACCENT, 0))
    out.putalpha(alpha)
    return out


def resized(mark: Image.Image, size: int) -> Image.Image:
    return mark.resize((size, size), Image.LANCZOS)


def write_ico(path: pathlib.Path, frames: list[Image.Image]) -> None:
    """Write a multi-size .ico from one prepared bitmap per size.

    Pillow's own ICO writer takes a single image and resamples it for every
    requested size, which throws away per-size rendering: the tray bars are
    drawn for the size they will be shown at, and the application mark is
    resampled from the full-resolution master rather than from an already
    reduced copy. Both matter most at 16 px, which is where an icon is judged.

    Sizes up to 64 px are stored as 32-bit DIBs and 256 px as PNG, the layout
    Windows shells expect.
    """
    frames = sorted((frame.convert("RGBA") for frame in frames), key=lambda f: f.width)
    payloads: list[bytes] = []
    for frame in frames:
        if frame.width >= 256:
            buffer = io.BytesIO()
            frame.save(buffer, format="PNG")
            payloads.append(buffer.getvalue())
            continue
        width, height = frame.size
        pixels = frame.load()
        # BITMAPINFOHEADER: doubled height covers the colour and mask planes.
        header = struct.pack("<IiiHHIIiiII", 40, width, height * 2, 1, 32, 0, 0, 0, 0, 0, 0)
        colour = bytearray()
        for y in range(height - 1, -1, -1):  # DIB rows run bottom-up.
            for x in range(width):
                r, g, b, a = pixels[x, y]
                colour += bytes((b, g, r, a))
        # The AND mask is unused for 32-bit icons but must still be present.
        stride = ((width + 31) // 32) * 4
        mask = bytes(stride * height)
        payloads.append(bytes(header) + bytes(colour) + mask)

    offset = 6 + 16 * len(frames)
    directory = bytearray(struct.pack("<HHH", 0, 1, len(frames)))
    for frame, payload in zip(frames, payloads):
        dimension = 0 if frame.width >= 256 else frame.width
        directory += struct.pack("<BBBBHHII", dimension, dimension, 0, 0, 1, 32, len(payload), offset)
        offset += len(payload)
    path.write_bytes(bytes(directory) + b"".join(payloads))


def main() -> int:
    if not MASTER.exists():
        print(f"Missing {MASTER}", file=sys.stderr)
        return 1
    master = Image.open(MASTER).convert("RGB")
    mark = cut_alpha(master)
    print(f"master {master.size} -> mark {mark.size} with alpha")

    # Windows executable, window and installer shortcut.
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    write_ico(ICONS / "writer.ico", [resized(mark, s) for s in ico_sizes])

    # Tray: single colour, real small sizes so Windows never scales one bitmap.
    tray_sizes = [16, 20, 24, 32]
    trays = [waveform_silhouette(s) for s in tray_sizes]
    write_ico(ICONS / "writer-tray.ico", trays)
    trays[-1].save(ICONS / "writer-tray-32.png")

    # Browser tab and installed-web icons.
    for size in (16, 32):
        resized(mark, size).save(ICONS / f"favicon-{size}.png")
    resized(mark, 180).save(ICONS / "apple-touch-icon.png")

    WEB_PUBLIC.mkdir(parents=True, exist_ok=True)
    for name in ("favicon-16.png", "favicon-32.png", "apple-touch-icon.png"):
        (WEB_PUBLIC / name).write_bytes((ICONS / name).read_bytes())
    # The title bar and the About page draw the mark at 32 CSS pixels or less.
    resized(mark, 64).save(WEB_PUBLIC / "writer-icon.png")

    TAURI_ICONS.mkdir(parents=True, exist_ok=True)
    (TAURI_ICONS / "icon.ico").write_bytes((ICONS / "writer.ico").read_bytes())
    for size in (32, 128, 256, 512):
        resized(mark, size).save(TAURI_ICONS / f"{size}x{size}.png")
    resized(mark, 256).save(TAURI_ICONS / "128x128@2x.png")
    # Tauri's Unix context generator requires one RGBA PNG named icon.png.
    resized(mark, 512).save(TAURI_ICONS / "icon.png")
    trays[-1].save(TAURI_ICONS / "tray.png")

    print("wrote:", ", ".join(sorted(p.name for p in ICONS.glob("*") if p.name != "derive.py")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
