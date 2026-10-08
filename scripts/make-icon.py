"""Generate the xitter-dl app icon.

The mark: a bookmark ribbon whose lower notch reads as a "V", in X's blue,
on the app's Lights Out black. Drawn programmatically rather than committed as
a binary blob so it can be regenerated and tweaked by anyone reading the repo.

Usage:
    python scripts/make-icon.py <output.png> [size]

Then run `pnpm tauri icon <output.png>` from the repo root to produce the
platform icon set that Tauri bundles.
"""

import sys
from PIL import Image, ImageDraw

# X's accent blue, and the Lights Out background. Same values as
# ui/src/styles/x.css, so the icon and the app agree.
BLUE = (29, 155, 240, 255)
BLACK = (0, 0, 0, 255)
WHITE = (255, 255, 255, 255)


def rounded_square(size: int, radius_ratio: float = 0.22) -> Image.Image:
    """A macOS/Tauri-style rounded square, on a transparent canvas."""
    # Supersample 4x for smooth edges, then downscale.
    s = size * 4
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, s - 1, s - 1], radius=int(s * radius_ratio), fill=BLACK)
    return img.resize((size, size), Image.LANCZOS)


def draw_mark(img: Image.Image) -> None:
    """Draw the bookmark ribbon, centred with padding.

    Proportions matter more than they look: a notch that is too deep turns the
    ribbon into a pennant, and a ribbon that is too wide stops reading as a
    bookmark at 16px. These values were picked by rendering and looking.
    """
    size = img.width
    s = size * 4
    big = img.resize((s, s), Image.LANCZOS)
    d = ImageDraw.Draw(big)

    w = s * 0.36
    h = s * 0.52
    left = (s - w) / 2
    # Optically centred: a shape with a notch cut out of its bottom looks
    # bottom-heavy when its bounding box is centred, so nudge it up.
    top = (s - h) / 2 - s * 0.012
    right = left + w
    bottom = top + h
    # Shallower than it looks like it should be — see the docstring.
    notch = h * 0.20

    d.polygon(
        [
            (left, top),
            (right, top),
            (right, bottom),
            ((left + right) / 2, bottom - notch),
            (left, bottom),
        ],
        fill=BLUE,
    )

    img.paste(big.resize((size, size), Image.LANCZOS), (0, 0))


def main() -> int:
    out = sys.argv[1] if len(sys.argv) > 1 else "icon.png"
    size = int(sys.argv[2]) if len(sys.argv) > 2 else 1024

    img = rounded_square(size)
    draw_mark(img)
    img.save(out, "PNG")
    print(f"wrote {out} ({size}x{size})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
