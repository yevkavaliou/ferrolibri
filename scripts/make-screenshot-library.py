"""A made-up Calibre library of popular books, for screenshots and manual checks (needs Pillow).

Usage: python scripts/make-screenshot-library.py <folder> [--real-covers]

Writes <folder>/config.toml, <folder>/ui.toml, <folder>/window.toml and
<folder>/Calibre Library with twenty popular books. Run the app with
FERROLIBRI_UI_DATA_DIR=<folder> so nothing touches the real profile; scripts/capture-screenshot.ps1
does that and saves a picture of the window.

The books are real titles, but the files are placeholders and the descriptions are written for
this fixture. By default the covers are drawn here (title and author on a coloured card).
With --real-covers the cover of each edition is fetched from Open Library (openlibrary.org,
one request per second, cached in <folder>/../screenshot-covers so a second run is offline);
a book with no cover there gets a drawn one. The downloaded pictures are the publishers'
artwork: keep them out of the repository, only the screenshot is published.
"""
import colorsys
import hashlib
import io
import json
import os
import random
import sys
import textwrap
import time
import urllib.parse
import urllib.request
from xml.sax.saxutils import escape

from PIL import Image, ImageDraw, ImageFont

# title | author | year | series | number | tags | publisher | description
BOOKS = """
Fourth Wing|Rebecca Yarros|2023|The Empyrean|1|Fantasy, Romance|Red Tower Books|A young woman is sent to a war college where cadets bond with dragons, or die trying.
Atomic Habits|James Clear|2018||0|Self-help, Non-fiction|Avery|Small daily changes add up. A practical method for building good habits and breaking bad ones.
Project Hail Mary|Andy Weir|2021||0|Science fiction|Ballantine Books|A teacher wakes alone on a spaceship with no memory of how he got there, and slowly learns that he is humanity's last chance.
The Martian|Andy Weir|2014||0|Science fiction|Crown|An astronaut is left behind on Mars and has to grow food, fix machines and do the math to stay alive.
Demon Copperhead|Barbara Kingsolver|2022||0|Literary fiction|Harper|A boy born in a trailer in Appalachia grows up through foster care, football and the opioid crisis.
The Women|Kristin Hannah|2024||0|Historical fiction|St. Martin's Press|A young nurse goes to Vietnam and comes home to a country that does not want to hear about it.
The Midnight Library|Matt Haig|2020||0|Fantasy, Fiction|Viking|Between life and death, a woman finds a library where every book is a life she could have lived.
Lessons in Chemistry|Bonnie Garmus|2022||0|Historical fiction|Doubleday|A gifted chemist in the 1960s is pushed out of the laboratory and ends up teaching a nation to cook.
Where the Crawdads Sing|Delia Owens|2018||0|Mystery, Fiction|G. P. Putnam's Sons|A girl raised alone in the marsh of North Carolina becomes the town's prime suspect in a murder.
The Thursday Murder Club|Richard Osman|2020|Thursday Murder Club|1|Mystery, Humour|Viking|Four retirees meet weekly to study cold cases, until a real murder lands on their doorstep.
Piranesi|Susanna Clarke|2020||0|Fantasy|Bloomsbury|A man lives in an endless house of statues and tides, and keeps a journal of its wonders.
The Seven Husbands of Evelyn Hugo|Taylor Jenkins Reid|2017||0|Historical fiction, Romance|Atria Books|A reclusive film star finally tells her life story to an unknown young journalist.
Dune|Frank Herbert|1965|Dune|1|Science fiction|Chilton Books|On a desert planet that holds the most valuable substance in the universe, a noble family is betrayed.
Tomorrow, and Tomorrow, and Tomorrow|Gabrielle Zevin|2022||0|Literary fiction|Knopf|Two friends build video games together across three decades of friendship, rivalry and loss.
It Ends with Us|Colleen Hoover|2016|It Ends with Us|1|Romance, Fiction|Atria Books|A young woman starts a flower shop in Boston and falls for a neurosurgeon, while an old love returns.
Mistborn: The Final Empire|Brandon Sanderson|2006|Mistborn|1|Fantasy, Adventure|Tor Books|A street thief joins a crew planning to overthrow an immortal ruler, and learns that she can burn metals to gain strange powers. The first book of a trilogy set in a world of ash and mist.
The Hobbit|J. R. R. Tolkien|1937||0|Fantasy|George Allen & Unwin|A quiet hobbit is hired as a burglar by thirteen dwarves and a wizard.
Educated|Tara Westover|2018||0|Memoir, Non-fiction|Random House|A girl raised by survivalists, who never went to school, ends up at Cambridge.
The Hunger Games|Suzanne Collins|2008|The Hunger Games|1|Young adult, Dystopia|Scholastic Press|A girl volunteers in her sister's place for a televised fight to the death.
Circe|Madeline Miller|2018||0|Fantasy, Myth|Little, Brown|The witch of the island, banished by the gods, meets the heroes who come ashore.
""".strip()

# (hue, saturation, value) of the cover cards; a book's colour comes from a hash of its title.
FONT_CANDIDATES = [
    "C:/Windows/Fonts/georgiab.ttf",
    "C:/Windows/Fonts/arialbd.ttf",
    "/System/Library/Fonts/Supplemental/Georgia Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
]
SMALL_FONT_CANDIDATES = [
    "C:/Windows/Fonts/segoeui.ttf",
    "C:/Windows/Fonts/arial.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
]


def safe(name):
    """What Calibre does to characters a file system does not allow."""
    return "".join("_" if c in '<>:"/\\|?*' else c for c in name).strip()


def font(candidates, size):
    for path in candidates:
        if os.path.exists(path):
            return ImageFont.truetype(path, size)
    return ImageFont.load_default()


def cover(title, author, path):
    digest = hashlib.sha256(title.encode()).digest()
    hue = digest[0] / 255
    top = tuple(int(c * 255) for c in colorsys.hsv_to_rgb(hue, 0.55, 0.55))
    bottom = tuple(int(c * 255) for c in colorsys.hsv_to_rgb((hue + 0.08) % 1, 0.65, 0.25))
    width, height = 400, 600
    img = Image.new("RGB", (width, height), top)
    pixels = ImageDraw.Draw(img)
    for y in range(height):
        t = y / (height - 1)
        pixels.line([(0, y), (width, y)], fill=tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)))
    draw = ImageDraw.Draw(img)
    draw.rectangle([18, 18, width - 18, height - 18], outline=(255, 255, 255), width=2)
    title_font = font(FONT_CANDIDATES, 44)
    lines = textwrap.wrap(title, width=14) or [title]
    if len(lines) > 5:
        title_font = font(FONT_CANDIDATES, 34)
        lines = textwrap.wrap(title, width=18)
    y = 90
    for line in lines[:7]:
        w = draw.textlength(line, font=title_font)
        draw.text(((width - w) / 2, y), line, font=title_font, fill=(255, 255, 255))
        y += title_font.size + 10
    draw.line([(width / 2 - 40, height - 130), (width / 2 + 40, height - 130)], fill=(255, 255, 255), width=2)
    author_font = font(SMALL_FONT_CANDIDATES, 26)
    w = draw.textlength(author, font=author_font)
    draw.text(((width - w) / 2, height - 105), author, font=author_font, fill=(235, 235, 235))
    img.save(path, quality=90)


# Editions picked by hand (Open Library cover ids): the first English edition it lists is sometimes
# a box set, a library copy with a barcode or a photo of a book on a table.
PICKED = {
    "Mistborn: The Final Empire": 15164217,
    "The Martian": 12065533,
    "Where the Crawdads Sing": 13011470,
    "Fourth Wing": 14605482,
    "The Hunger Games": 15261620,
}

USER_AGENT = "FerroLibri-screenshot/0.8 (https://github.com/yevkavaliou/ferrolibri)"


def get(url):
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=30) as reply:
        return reply.read()


def real_cover(title, author, cache, key):
    """An English edition's cover from Open Library, cached as a JPEG; None when there is none."""
    path = os.path.join(cache, key + ".jpg")
    if title in PICKED:
        path = os.path.join(cache, key + "-picked.jpg")
    if os.path.exists(path):
        return path
    if title in PICKED:
        return fetch_cover(PICKED[title], path)
    short = title.split(":")[-1].strip()
    queries = [{"title": short, "author": author}, {"q": f"{title} {author}"}]
    for query in queries:
        query = dict(query, limit="5", fields="key,title,author_name,cover_i")
        try:
            found = json.loads(get("https://openlibrary.org/search.json?" + urllib.parse.urlencode(query)))
        except OSError:
            continue
        finally:
            time.sleep(1)
        docs = [d for d in found.get("docs", []) if d.get("title", "").lower() == short.lower()] or found.get("docs", [])[:1]
        for doc in docs[:1]:
            for cover_id in cover_ids(doc):
                if fetch_cover(cover_id, path):
                    return path
    return None


def fetch_cover(cover_id, path):
    try:
        data = get(f"https://covers.openlibrary.org/b/id/{cover_id}-L.jpg?default=false")
        img = Image.open(io.BytesIO(data))
        img.load()
    except (OSError, ValueError):
        return None
    finally:
        time.sleep(1)
    img = img.convert("RGB")
    if img.height < 300:
        return None
    img.thumbnail((400, 600))
    img.save(path, quality=90)
    return path


def cover_ids(doc):
    """Cover ids of the English editions of a work that are not large print, then the work's own."""
    ids = []
    try:
        editions = json.loads(get(f"https://openlibrary.org{doc['key']}/editions.json?limit=100"))
    except (OSError, KeyError, ValueError):
        editions = {}
    finally:
        time.sleep(1)
    for edition in editions.get("entries", []):
        languages = [l.get("key") for l in edition.get("languages", [])]
        words = " ".join([edition.get("physical_format") or "", edition.get("subtitle") or "", edition.get("title") or ""]).lower()
        if "/languages/eng" in languages and "large print" not in words and "library" not in words:
            ids += [c for c in edition.get("covers", []) if c and c > 0]
    if doc.get("cover_i"):
        ids.append(doc["cover_i"])
    return ids[:4]


def opf(title, author, year, series, number, tags, publisher, description):
    lines = [
        '<?xml version="1.0" encoding="utf-8"?>',
        '<package xmlns="http://www.idpf.org/2007/opf" version="2.0">',
        '<metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">',
        f"<dc:title>{escape(title)}</dc:title>",
        f"<dc:creator>{escape(author)}</dc:creator>",
    ]
    if publisher:
        lines.append(f"<dc:publisher>{escape(publisher)}</dc:publisher>")
    lines.append(f"<dc:date>{year}-01-01T00:00:00+00:00</dc:date>")
    lines.append("<dc:language>eng</dc:language>")
    lines.append(f"<dc:description>{escape(description)}</dc:description>")
    for tag in tags:
        lines.append(f"<dc:subject>{escape(tag)}</dc:subject>")
    if series:
        lines.append(f'<meta name="calibre:series" content="{escape(series, {chr(34): "&quot;"})}"/>')
        lines.append(f'<meta name="calibre:series_index" content="{number}"/>')
    lines += ["</metadata>", "</package>"]
    return "\n".join(lines) + "\n"


def main():
    arguments = [a for a in sys.argv[1:] if not a.startswith("--")]
    real = "--real-covers" in sys.argv[1:]
    if len(arguments) != 1 or any(a.startswith("--") and a != "--real-covers" for a in sys.argv[1:]):
        sys.exit(__doc__)
    root = os.path.abspath(arguments[0])
    cache = os.path.join(os.path.dirname(root), "screenshot-covers")
    if real:
        os.makedirs(cache, exist_ok=True)
    missing = []
    library = os.path.join(root, "Calibre Library")
    os.makedirs(library, exist_ok=True)
    open(os.path.join(library, "metadata.db"), "wb").write(b"marker")
    rng = random.Random(11)
    for number, line in enumerate(BOOKS.splitlines(), start=1):
        title, author, year, series, index, tags, publisher, description = line.split("|")
        folder = os.path.join(library, safe(author), f"{safe(title)} ({number})")
        os.makedirs(folder, exist_ok=True)
        formats = ["epub"]
        if title in ("Project Hail Mary", "Mistborn: The Final Empire"):
            formats += ["azw3", "pdf"]
        elif number % 4 == 0:
            formats.append("mobi")
        elif number % 9 == 0:
            formats.append("pdf")
        for ext in formats:
            with open(os.path.join(folder, f"{safe(title)}.{ext}"), "wb") as out:
                out.write(rng.randbytes(rng.randint(250_000, 1_400_000)))
        target = os.path.join(folder, "cover.jpg")
        downloaded = real_cover(title, author, cache, hashlib.sha256(f"{title}|{author}".encode()).hexdigest()[:16]) if real else None
        if downloaded:
            with open(downloaded, "rb") as source, open(target, "wb") as out:
                out.write(source.read())
        else:
            cover(title, author, target)
            if real:
                missing.append(title)
        with open(os.path.join(folder, "metadata.opf"), "w", encoding="utf-8") as out:
            out.write(opf(title, author, year, series, index, [t.strip() for t in tags.split(",")], publisher, description))
    with open(os.path.join(root, "config.toml"), "w", encoding="utf-8") as out:
        out.write("library_path = '" + library + "'\n")
    # An opaque window with the system look, a cover grid, the details pane open.
    with open(os.path.join(root, "ui.toml"), "w", encoding="utf-8") as out:
        out.write(
            "view = 'Grid'\nlibrary_expanded = true\nsidebar_visible = true\ndetails_visible = true\n"
            "window_opacity = 100\nwindow_blur = false\ncover_size = 160\ndetails_width = 340\n"
        )
    with open(os.path.join(root, "window.toml"), "w", encoding="utf-8") as out:
        out.write("width = 1360.0\nheight = 1000.0\n")
    print(f"{number} books in {library}")
    if missing:
        print(f"No cover found, drawn instead: {', '.join(missing)}")


if __name__ == "__main__":
    main()
