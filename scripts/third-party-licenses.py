"""Write the licenses of the libraries that end up in the FerroLibri executable.

Usage:
    python scripts/third-party-licenses.py OUTPUT.txt [TARGET]

TARGET is a Rust target triple (default x86_64-pc-windows-msvc). Only the normal
dependencies of the `ferrolibri-ui` crate for that target are listed: what is linked into
the program, not build tools or test helpers. Every package contributes the license files
shipped in its source; packages with the same text are grouped. Packages without a license
file are listed at the end (their license is named from Cargo.toml) and reported on stderr.

MIT and Apache-2.0 require the notice to travel with binaries; MPL-2.0 packages are used
unmodified and their sources are available from the addresses given here.
"""
import hashlib
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT_PACKAGE = "ferrolibri-ui"
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|unlicense|notice)", re.IGNORECASE)


def metadata(target):
    out = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        capture_output=True,
        check=True,
    ).stdout.decode("utf-8")
    return json.loads(out)


def linked_packages(meta):
    by_id = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == ROOT_PACKAGE)
    seen, stack = set(), [root]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    return [by_id[i] for i in seen if by_id[i]["name"] != ROOT_PACKAGE and not is_workspace(by_id[i], meta)]


def is_workspace(package, meta):
    return package["id"] in meta["workspace_members"]


# Packages whose crates.io archive has no license file but whose project keeps one in this repository.
FALLBACKS = {"github.com/iced-rs/iced": Path(__file__).resolve().parent.parent / "vendor" / "LICENSE-iced"}


def license_texts(package):
    folder = Path(package["manifest_path"]).parent
    found = []
    for path in sorted(folder.iterdir()):
        if path.is_file() and LICENSE_FILE.match(path.name):
            found.append(path.read_text(encoding="utf-8", errors="replace").strip())
    if not found:
        repository = package.get("repository") or ""
        for marker, path in FALLBACKS.items():
            if repository.rstrip("/").endswith(marker) and path.is_file():
                found.append(path.read_text(encoding="utf-8", errors="replace").strip())
    return found


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    output = Path(sys.argv[1])
    target = sys.argv[2] if len(sys.argv) > 2 else "x86_64-pc-windows-msvc"
    packages = sorted(linked_packages(metadata(target)), key=lambda p: (p["name"], p["version"]))

    groups = defaultdict(list)
    texts = {}
    missing = []
    for package in packages:
        found = license_texts(package)
        if not found:
            missing.append(package)
            continue
        for text in found:
            key = hashlib.sha256(text.encode("utf-8")).hexdigest()
            texts[key] = text
            if package not in groups[key]:
                groups[key].append(package)

    def label(package):
        source = package.get("repository") or package.get("homepage") or ""
        return f"{package['name']} {package['version']} ({package['license'] or 'see its files'}) {source}".rstrip()

    lines = [
        "FerroLibri third-party licenses",
        "================================",
        "",
        f"FerroLibri itself is released under the MIT License (see LICENSE). The program contains the {len(packages)} libraries",
        "below. Each group names the libraries and gives the license text they ship with.",
        "",
    ]
    for key, members in sorted(groups.items(), key=lambda item: (-len(item[1]), item[0])):
        lines.append("-" * 78)
        lines.extend(label(p) for p in members)
        lines.append("")
        lines.append(texts[key])
        lines.append("")
    if missing:
        lines.append("-" * 78)
        lines.append("Libraries that ship no license file; their license is the one named here and their repository has the text:")
        lines.extend(label(p) for p in missing)
        lines.append("")
        print(f"{len(missing)} packages without a license file: {', '.join(p['name'] for p in missing)}", file=sys.stderr)

    output.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"{len(packages)} packages, {len(groups)} distinct license texts -> {output}")


if __name__ == "__main__":
    main()
