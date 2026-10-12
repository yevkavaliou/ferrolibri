"""Generate crates/rulibre-ui/assets/app-icon.ico (and a 256 px PNG) from code.

Palette comes from crates/rulibre-ui/src/theme.rs (background, accent). Run:
    python scripts/make-ui-icon.py
Requires Pillow. The output is committed, so the build does not need Python.
"""
from pathlib import Path
from PIL import Image, ImageDraw

BG = (23, 24, 27, 255)
PANEL = (30, 32, 36, 255)
ACCENT = (91, 155, 255, 255)
SIZE = 1024  # drawn large, then downsampled for smooth edges


def draw() -> Image.Image:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((40, 40, SIZE - 40, SIZE - 40), radius=220, fill=BG)
    d.rounded_rectangle((70, 70, SIZE - 70, SIZE - 70), radius=190, fill=PANEL)
    # An open book: two pages meeting at the spine, with a bookmark ribbon.
    left = [(512, 300), (512, 760), (230, 700), (230, 240)]
    right = [(512, 300), (512, 760), (794, 700), (794, 240)]
    d.polygon(left, fill=ACCENT)
    d.polygon(right, fill=(61, 118, 214, 255))
    d.polygon([(512, 300), (512, 760), (500, 760), (500, 300)], fill=BG)
    d.polygon([(600, 240), (680, 240), (680, 420), (640, 380), (600, 420)], fill=(255, 255, 255, 255))
    return img


def main() -> None:
    out = Path(__file__).resolve().parent.parent / "crates" / "rulibre-ui" / "assets"
    out.mkdir(parents=True, exist_ok=True)
    base = draw()
    sizes = [16, 24, 32, 48, 64, 128, 256]
    base.resize((256, 256), Image.LANCZOS).save(out / "app-icon.png")
    base.save(out / "app-icon.ico", format="ICO", sizes=[(s, s) for s in sizes])


if __name__ == "__main__":
    main()
