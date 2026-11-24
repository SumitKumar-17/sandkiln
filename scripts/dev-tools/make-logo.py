#!/usr/bin/env python3
"""Generates every variant of the sandkiln mark from one definition.

The mark is a kiln arch (the enclosure) holding a stack of sand grains (the work
running inside). Variants: a symbol and a symbol+wordmark lockup, each in
a dark-background and a light-background colour set, plus the square favicon.
`--check` fails if a committed file differs (run by docs-check).

Usage: scripts/dev-tools/make-logo.py [--check]
"""
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]

# (ink, grain base, grain middle, grain top) ON a dark / ON a light background.
ON_DARK = ("#ece9e3", "#8fa3e8", "#a8b6ee", "#c2cdf4")
ON_LIGHT = ("#151515", "#4a5fc7", "#6578d4", "#8292df")
GROUND = "#151515"
SANS = "Geist, 'Helvetica Neue', Arial, system-ui, sans-serif"


def symbol(colors, x: float = 0, y: float = 0, scale: float = 1.0) -> str:
    """The mark on a 64x64 grid, translated and scaled.

    A kiln arch (the enclosure) holding a small stack of sand grains (the
    work running inside): three rows of squares, 3, 2 and 1, lightening
    toward the top. Flat shapes only, so it stays crisp from 16px to a banner.
    """
    ink, base, mid, top = colors
    arch = f'<path d="M12 54V31a20 20 0 0 1 40 0V54Z" fill="none" stroke="{ink}" stroke-width="5" stroke-linejoin="miter"/>'
    grains = "".join(
        f'<rect x="{gx}" y="{gy}" width="8" height="8" fill="{fill}"/>'
        for gx, gy, fill in (
            (18, 41, base), (28, 41, base), (38, 41, base),  # bottom row
            (23, 31, mid), (33, 31, mid),                    # middle row
            (28, 21, top),                                   # top grain
        )
    )
    return f'<g transform="translate({x} {y}) scale({scale})">{arch}{grains}</g>'


def mark_svg(colors) -> str:
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img" aria-label="sandkiln">{symbol(colors)}</svg>\n'


def lockup_svg(colors) -> str:
    ink = colors[0]
    w, h = 252, 64
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="sandkiln">'
        f"{symbol(colors)}"
        f'<text x="76" y="43" font-family="{SANS}" font-size="38" font-weight="500" letter-spacing="-0.8" fill="{ink}">sandkiln</text>'
        f"</svg>\n"
    )


def favicon_svg() -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" fill="{GROUND}"/>'
        f"{symbol(ON_DARK, x=0.5, y=0.5, scale=0.484)}</svg>\n"
    )


FILES = {
    "docs/images/logo-dark.svg": lockup_svg(ON_DARK),    # for dark backgrounds
    "docs/images/logo-light.svg": lockup_svg(ON_LIGHT),  # for light backgrounds
    "docs-site/images/logo-dark.svg": lockup_svg(ON_DARK),
    "docs-site/images/logo-light.svg": lockup_svg(ON_LIGHT),
    "docs-site/favicon.svg": favicon_svg(),
    "website/public/favicon.svg": favicon_svg(),
    "website/public/mark.svg": mark_svg(ON_DARK),
}

stale = []
for rel, text in FILES.items():
    path = root / rel
    if "--check" in sys.argv:
        if not path.exists() or path.read_text() != text:
            stale.append(rel)
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        print(f"wrote {rel}")
if stale:
    for rel in stale:
        print(f"{rel} is stale: run scripts/dev-tools/make-logo.py")
    sys.exit(1)
