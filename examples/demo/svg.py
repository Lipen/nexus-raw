#!/usr/bin/env python3
"""Draw a prepared cast as an animated SVG.

One file, two consumers with the same look: the landing page shows it until the
player takes over (and forever, for anyone without JavaScript), and the readmes
embed it directly — GitHub serves SVG as an image, so it animates there with no
script and no GIF, and stays sharp at any zoom.

Every line lands at `at[i]` and wears the color of `kinds[i]`, both written by
`stage.py`: this script decides layout, not timing or semantics.

Colors are the docs palette, converted from oklch here because a committed SVG
has to render the same in viewers that know nothing about CSS Color 4.

Run by `just demo-cast`; edit the scripts behind it, never the SVG by hand.
"""

import json
import math
import sys

# Geometry: the landing stylesheet's fractions, in this canvas's units.
# site.css sizes the screen's type at 1.85% of the frame width (--nxr-term-font)
# and expresses everything inside in em, so the same design in an 860-wide
# canvas is these numbers; .nxr-term__body is an 860/680 box. Change both sides
# together: they are one design seen twice, and a swap that moved the page
# would be the visible symptom of them drifting apart.
WIDTH = 860
FONT = 0.0185 * WIDTH  # 15.91
LINE_H = 1.5 * FONT  # 23.87
BAR_H = 52.8  # measured from the live bar: 0.78em padding, 0.93em title row
BODY_H = 680
RADIUS = 18.75  # 0.75rem
PAD_TOP = 1.24 * FONT  # 19.7
PAD_SIDE = 1.55 * FONT  # 24.7
PAD_BOTTOM = 1.55 * FONT
ASCENT = 1.1 * FONT  # pad + half-leading + ascent: where the first baseline sits
INDENT = 1.24 * FONT  # .nxr-l--hint padding
TITLE_FONT = 0.93 * FONT
DOT_R = 0.465 * FONT
BAR_PAD = 1.32 * FONT
DOTS_W = 4.03 * FONT
BAR_GAP = 0.45 * FONT
REVEAL_MS = 140

# The docs palette, oklch as written in site.css.
COLORS = {
    "ink": (0.20, 0.015, 210),
    "bar": (0.25, 0.018, 210),
    "border": (0.32, 0.02, 210),
    "title": (0.72, 0.015, 210),
    "text": (0.88, 0.01, 210),
    "cmd": (0.96, 0.005, 210),
    "ok": (0.78, 0.11, 190),
    "warn": (0.80, 0.14, 78),
    "err": (0.75, 0.16, 25),
    "hint": (0.70, 0.06, 60),
    "dim": (0.66, 0.015, 210),
    "note": (0.82, 0.02, 200),
    "dot-red": (0.72, 0.16, 25),
    "dot-amber": (0.82, 0.14, 85),
    "dot-green": (0.78, 0.15, 150),
}
MONO = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'DejaVu Sans Mono', monospace"


def oklch(lightness: float, chroma: float, hue: float) -> str:
    """oklch -> sRGB hex (Ottosson's conversion, clipped to the gamut)."""
    hue = math.radians(hue)
    a, b = chroma * math.cos(hue), chroma * math.sin(hue)
    l_ = lightness + 0.3963377774 * a + 0.2158037573 * b
    m_ = lightness - 0.1055613458 * a - 0.0638541728 * b
    s_ = lightness - 0.0894841775 * a - 1.2914855480 * b
    l, m, s = l_**3, m_**3, s_**3
    linear = (
        +4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    )
    gamma = [
        12.92 * c if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055 for c in linear
    ]
    return "#" + "".join(f"{round(min(max(c, 0.0), 1.0) * 255):02x}" for c in gamma)


def escape(text: str) -> str:
    return (
        text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace('"', "&quot;")
    )


def wrap(text: str, limit: int) -> list[str]:
    """Split on spaces; a single long token is hard-split, never dropped."""
    if len(text) <= limit:
        return [text]
    out, rest = [], text
    while len(rest) > limit:
        cut = rest.rfind(" ", 0, limit + 1)
        if cut <= 0:
            cut = limit
        out.append(rest[:cut])
        rest = rest[cut:].lstrip(" ")
    out.append(rest)
    return out


def render(cast: dict) -> str:
    lines = cast["lines"]
    at, kinds = cast["at"], cast["kinds"]
    palette = {name: oklch(*value) for name, value in COLORS.items()}

    limit = int((WIDTH - 2 * PAD_SIDE) / (0.6 * FONT))
    body: list[str] = []
    y = BAR_H + PAD_TOP + ASCENT
    for (_, text), when, kind in zip(lines, at, kinds):
        fill = palette.get(kind, palette["text"])
        weight = ' font-weight="600"' if kind == "cmd" else ""
        x = PAD_SIDE + (INDENT if kind == "hint" else 0)
        for part in wrap(text, limit):
            # A wrapped line continues flush left, the way the player wraps it.
            body.append(
                f'<text class="l" x="{x}" y="{y}" fill="{fill}"{weight}'
                f' style="animation-delay:{when}ms">{escape(part)}</text>'
            )
            y += LINE_H
    bottom = BAR_H + BODY_H - PAD_BOTTOM
    if y > bottom:
        raise SystemExit(
            f"svg: the transcript runs {y - bottom}px past the window; record a shorter session"
        )

    dots = "".join(
        f'<circle cx="{BAR_PAD + at_x:.1f}" cy="{BAR_H / 2:.1f}" r="{DOT_R:.1f}"'
        f' fill="{palette[name]}"/>'
        for at_x, name in ((0.58 * FONT, "dot-red"), (2.02 * FONT, "dot-amber"), (3.45 * FONT, "dot-green"))
    )
    title = cast["title"]
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{BAR_H + BODY_H}" viewBox="0 0 {WIDTH} {BAR_H + BODY_H}" role="img" font-family="{MONO}" font-size="{FONT}">
<title>{escape(title)}</title>
<desc>An animated transcript of a real nxr session against the mock server.</desc>
<style>
  @keyframes nxr-in {{ from {{ opacity: 0 }} to {{ opacity: 1 }} }}
  .l {{ opacity: 0; animation: nxr-in {REVEAL_MS}ms linear forwards }}
  @media (prefers-reduced-motion: reduce) {{ .l {{ opacity: 1; animation: none }} }}
</style>
<rect x="0.5" y="0.5" width="{WIDTH - 1}" height="{BAR_H + BODY_H - 1}" rx="{RADIUS}" fill="{palette["ink"]}" stroke="{palette["border"]}"/>
<path d="M0 {BAR_H} h{WIDTH}" stroke="{palette["border"]}"/>
<rect x="0.5" y="0.5" width="{WIDTH - 1}" height="{BAR_H}" rx="{RADIUS}" fill="{palette["bar"]}"/>
{dots}
<text x="{BAR_PAD + DOTS_W + BAR_GAP:.1f}" y="{BAR_H / 2 + TITLE_FONT / 3:.1f}" fill="{palette["title"]}" font-size="{TITLE_FONT:.1f}">{escape(title)}</text>
{"".join(body)}
</svg>
"""


def main(source: str, target: str) -> int:
    with open(source, encoding="utf-8") as handle:
        cast = json.load(handle)
    for field in ("at", "kinds", "title"):
        if field not in cast:
            raise SystemExit(f"svg: {source} has no {field}; run stage.py on it first")
    svg = render(cast)
    with open(target, "w", encoding="utf-8") as handle:
        handle.write(svg)
    print(f"svg: {target} ({len(svg)} bytes, {len(cast['lines'])} lines)")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__.strip(), file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1], sys.argv[2]))
