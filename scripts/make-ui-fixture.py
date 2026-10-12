"""Synthetic Calibre-like fixture for docs/UI_CHECKLIST.md (needs Pillow).

Usage: python scripts/make-ui-fixture.py <folder>
Writes <folder>/config.toml and <folder>/Calibre Library with 240 books; run the UI with
RULIBRE_UI_DATA_DIR=<folder> so nothing touches the real profile.
"""
import os, sys, random
from PIL import Image, ImageDraw

root = sys.argv[1]
lib = os.path.join(root, "Calibre Library")
os.makedirs(lib, exist_ok=True)
open(os.path.join(lib, "metadata.db"), "wb").write(b"marker")
random.seed(7)
authors = ["Лев Толстой", "Анна Ахматова", "Henryk Sienkiewicz", "Olga Tokarczuk", "Stanisław Lem",
           "Ursula K. Le Guin", "Иван Тургенев", "Bolesław Prus"]
titles = ["Математика. Утрата определённости", "Kamień filozoficzny", "Война и мир", "Lalka",
          "Solaris", "Dziady", "Отцы и дети", "The Dispossessed", "Quo vadis", "Бесы"]
for i in range(240):
    a = authors[i % len(authors)]
    t = f"{titles[i % len(titles)]} {i // len(titles) + 1}"
    d = os.path.join(lib, a, f"{t} ({i + 1})")
    os.makedirs(d, exist_ok=True)
    open(os.path.join(d, f"{t}.epub"), "wb").write(b"synthetic epub")
    if i % 3 == 0:
        open(os.path.join(d, f"{t}.pdf"), "wb").write(b"synthetic pdf")
    if i % 7 != 0:  # every seventh book has no cover
        img = Image.new("RGB", (400, 550), (40 + (i * 13) % 150, 60 + (i * 29) % 120, 110 + (i * 7) % 100))
        dr = ImageDraw.Draw(img)
        dr.rectangle([20, 20, 380, 530], outline=(240, 240, 240), width=4)
        dr.text((40, 60), t[:28], fill=(255, 255, 255))
        img.save(os.path.join(d, "cover.jpg"), quality=85)
    if i % 5 == 0:
        open(os.path.join(d, "metadata.opf"), "w", encoding="utf-8").write(
            '<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0">'
            '<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">'
            f"<dc:title>{t}</dc:title><dc:creator>{a}</dc:creator><dc:publisher>Fixture Press</dc:publisher>"
            "<dc:date>1999-05-01T00:00:00+00:00</dc:date><dc:language>pol</dc:language>"
            "<dc:description>Syntetyczny opis. Synthetic description.</dc:description></metadata></package>")
cfg = open(os.path.join(root, "config.toml"), "w", encoding="utf-8")
cfg.write("library_path = '" + lib + "'")
cfg.write(chr(10))
cfg.close()
print(open(os.path.join(root, "config.toml")).read())
