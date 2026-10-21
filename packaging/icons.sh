#!/usr/bin/env bash
# Regenerate native icons from the ERPCons contributor-original 1024 PNG. Needs Pillow.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "$ROOT/assets/app-icon" <<'PYICON'
from pathlib import Path
import sys
from PIL import Image

root = Path(sys.argv[1])
with Image.open(root / "wordcraft-1024.png") as source:
    image = source.convert("RGBA")
for size in (16, 24, 32, 48, 64, 128, 256, 512):
    target = root / "hicolor" / f"{size}x{size}" / "apps" / "vn.erpcons.vanban.png"
    target.parent.mkdir(parents=True, exist_ok=True)
    image.resize((size, size), Image.Resampling.LANCZOS).save(target)
image.save(root / "ERPCons-VanBan.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
image.save(root / "wordcraft.icns")
PYICON
