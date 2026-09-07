#!/usr/bin/env python3
"""Render the Prime butterfly SVG to ASCII via half-block encoding.

Usage:
  uv run scripts/render-logo.py [--width 60] [--threshold 96] [--svg path]

Outputs the rendered art to stdout.

Requires `cairo` on the system (brew install cairo). Uses cairosvg + pillow,
both pulled via uv's --with on each invocation so there's no committed venv.
"""

# /// script
# requires-python = ">=3.10"
# dependencies = ["cairosvg", "pillow"]
# ///

from __future__ import annotations

import argparse
import io
import os
import sys
from pathlib import Path


def render(svg_path: Path, width: int, threshold: int, style: str) -> str:
    # Late imports so --help works without the deps installed.
    import cairosvg
    from PIL import Image

    if style == "dots":
        # 1 pixel = 1 character cell. Compensate for terminal cells being ~2x taller than wide
        # so the rendered shape keeps the original proportions.
        # Use ▪ (BLACK SMALL SQUARE) so adjacent cells read as a tightly packed dot matrix —
        # mid-dot · is too small and reads as "spaced out" against the cell.
        png_bytes = cairosvg.svg2png(url=str(svg_path), output_width=width * 2)
        img = Image.open(io.BytesIO(png_bytes)).convert("RGBA")
        bg = Image.new("RGBA", img.size, (0, 0, 0, 255))
        bg.paste(img, (0, 0), img)
        img = bg.convert("L")
        target_h = max(1, img.size[1] // 4)
        img = img.resize((width, target_h), Image.LANCZOS)
        px = img.load()
        rows: list[str] = []
        for y in range(target_h):
            row = "".join("▪" if px[x, y] > threshold else " " for x in range(width))
            rows.append(row.rstrip())
    else:
        png_bytes = cairosvg.svg2png(url=str(svg_path), output_width=width)
        img = Image.open(io.BytesIO(png_bytes)).convert("RGBA")
        bg = Image.new("RGBA", img.size, (0, 0, 0, 255))
        bg.paste(img, (0, 0), img)
        img = bg.convert("L")
        w, h = img.size
        if h % 2:
            padded = Image.new("L", (w, h + 1), 0)
            padded.paste(img, (0, 0))
            img = padded
            h += 1
        px = img.load()
        table = {(False, False): " ", (True, False): "▀", (False, True): "▄", (True, True): "█"}
        rows = []
        for y in range(0, h, 2):
            rows.append(
                "".join(
                    table[(px[x, y] > threshold, px[x, y + 1] > threshold)] for x in range(w)
                ).rstrip()
            )

    while rows and not rows[0].strip():
        rows.pop(0)
    while rows and not rows[-1].strip():
        rows.pop()
    if rows:
        left = min(len(r) - len(r.lstrip(" ")) for r in rows if r.strip())
        rows = [r[left:] for r in rows]
    return "\n".join(rows)


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    default_svg = repo_root / "assets" / "brand" / "prime-butterfly.svg"

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--width", type=int, default=60, help="output columns (default: 60)")
    parser.add_argument(
        "--threshold",
        type=int,
        default=96,
        help="luminance cutoff 0-255; raise for thinner strokes (default: 96)",
    )
    parser.add_argument(
        "--svg", type=Path, default=default_svg, help=f"input SVG (default: {default_svg})"
    )
    parser.add_argument(
        "--style",
        choices=("blocks", "dots"),
        default="blocks",
        help="blocks = half-block ▀▄█; dots = mid-dot · matrix (default: blocks)",
    )
    args = parser.parse_args()

    if not args.svg.exists():
        print(f"svg not found: {args.svg}", file=sys.stderr)
        return 1

    # Set DYLD_FALLBACK_LIBRARY_PATH for homebrew cairo on Apple Silicon.
    if sys.platform == "darwin" and "DYLD_FALLBACK_LIBRARY_PATH" not in os.environ:
        os.environ["DYLD_FALLBACK_LIBRARY_PATH"] = "/opt/homebrew/lib:/usr/local/lib"

    print(render(args.svg, args.width, args.threshold, args.style))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
