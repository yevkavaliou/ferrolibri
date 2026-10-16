"""Generate crates/ferrolibri-ui/assets/app-icon.ico (and a 256 px PNG) from code.

The icon is a row of books standing on an iron girder shelf (a nod to Calibre)
on a rust-coloured rounded square. Run:
    python scripts/make-ui-icon.py
Requires Pillow and NumPy. The output is committed, so the build does not need Python.
"""
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

SIZE = 1024  # drawn large, then downsampled for smooth edges
BG_TOP = (232, 116, 58)
BG_BOTTOM = (168, 56, 26)
PAPER = (246, 236, 216, 255)
GIRDER = (43, 50, 60, 255)
GIRDER_LIP = (91, 101, 115, 255)
RIVET = (138, 147, 158, 255)

MARGIN = 40
RADIUS = 210
SHELF_TOP = 740
SHELF_HEIGHT = 90
# (x, top, width) of the upright books; they all stand on SHELF_TOP.
BOOKS = [(170, 223, 124), (323, 341, 106), (459, 176, 136)]
LEANING = (630, 282, 148, 12)  # x, top, width, tilt in degrees


def gradient() -> Image.Image:
    y, x = np.mgrid[0:SIZE, 0:SIZE].astype(np.float32)
    t = np.clip((x + y) / (2 * SIZE), 0, 1)[..., None]
    top = np.array(BG_TOP, np.float32)
    bottom = np.array(BG_BOTTOM, np.float32)
    rgb = (top * (1 - t) + bottom * t).astype(np.uint8)
    alpha = np.full((SIZE, SIZE, 1), 255, np.uint8)
    return Image.fromarray(np.concatenate([rgb, alpha], axis=2), "RGBA")


def draw(detail: bool = True) -> Image.Image:
    layer = gradient()
    d = ImageDraw.Draw(layer)
    for x, top, width in BOOKS:
        d.rounded_rectangle((x, top, x + width, SHELF_TOP + 12), radius=14, fill=PAPER)
    # The leaning book pivots on its bottom-right corner, resting on its neighbour.
    x, top, width, tilt = LEANING
    book = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    ImageDraw.Draw(book).rounded_rectangle(
        (x, top, x + width, SHELF_TOP + 12), radius=14, fill=PAPER
    )
    layer.alpha_composite(book.rotate(-tilt, center=(x + width, SHELF_TOP), resample=Image.BICUBIC))
    d = ImageDraw.Draw(layer)
    d.rectangle((0, SHELF_TOP, SIZE, SHELF_TOP + SHELF_HEIGHT), fill=GIRDER)
    d.rectangle((0, SHELF_TOP, SIZE, SHELF_TOP + 12), fill=GIRDER_LIP)
    if detail:
        for cx in (170, 512, 854):
            cy = SHELF_TOP + 52
            d.ellipse((cx - 15, cy - 15, cx + 15, cy + 15), fill=RIVET)

    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (MARGIN, MARGIN, SIZE - MARGIN, SIZE - MARGIN), radius=RADIUS, fill=255
    )
    out = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    out.paste(layer, mask=mask)
    return out


def main() -> None:
    out = Path(__file__).resolve().parent.parent / "crates" / "ferrolibri-ui" / "assets"
    out.mkdir(parents=True, exist_ok=True)
    full, simple = draw(True), draw(False)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [(simple if s < 32 else full).resize((s, s), Image.LANCZOS) for s in sizes]
    frames[-1].save(out / "app-icon.png")
    frames[-1].save(
        out / "app-icon.ico",
        format="ICO",
        sizes=[(s, s) for s in sizes],
        append_images=frames[:-1],
    )


if __name__ == "__main__":
    main()
