"""Packs a built FerroLibri program with the files that must travel with it.

Usage:
    python scripts/package-release.py TAG PLATFORM BINARY FORMAT LICENSES [OUTDIR]

  TAG       the release tag, for example v0.8.0
  PLATFORM  the name used in the archive, for example windows-x64
  BINARY    the built program (target/<triple>/release/ferrolibri[.exe])
  FORMAT    zip or tar.gz
  LICENSES  the file written by scripts/third-party-licenses.py
  OUTDIR    where the archive goes (default dist)

The archive ferrolibri-TAG-PLATFORM.FORMAT holds one folder of the same name with the program,
README.md, LICENSE, CHANGELOG.md and THIRD_PARTY_LICENSES.txt. The program keeps its executable
bit in a tar.gz. The path of the archive is printed.
"""
import shutil
import sys
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXTRA = ["README.md", "LICENSE", "CHANGELOG.md"]


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    tag, platform, binary, fmt, licenses = sys.argv[1:6]
    out = Path(sys.argv[6]) if len(sys.argv) == 7 else Path("dist")
    if fmt not in ("zip", "tar.gz"):
        sys.exit("FORMAT must be zip or tar.gz")
    binary = Path(binary)
    if not binary.is_file():
        sys.exit(f"{binary} does not exist")
    name = f"ferrolibri-{tag}-{platform}"
    folder = out / name
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    shutil.copy2(binary, folder / binary.name)
    for extra in EXTRA:
        shutil.copy2(ROOT / extra, folder / extra)
    shutil.copy2(licenses, folder / "THIRD_PARTY_LICENSES.txt")

    archive = out / f"{name}.{fmt}"
    files = sorted(p for p in folder.iterdir() if p.is_file())
    if fmt == "zip":
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
            for p in files:
                z.write(p, f"{name}/{p.name}")
    else:
        with tarfile.open(archive, "w:gz") as t:
            for p in files:
                info = t.gettarinfo(p, f"{name}/{p.name}")
                info.mode = 0o755 if p.name == binary.name else 0o644
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                with open(p, "rb") as handle:
                    t.addfile(info, handle)
    shutil.rmtree(folder)
    print(archive)


if __name__ == "__main__":
    main()
