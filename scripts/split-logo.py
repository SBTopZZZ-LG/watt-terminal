#!/usr/bin/env python3
"""Splits the Watt logo into the robot mark and the wordmark, and writes every derived brand image.

Reads assets/brand/watt-logo-full.png (1254x1254 RGBA, transparent background) and writes:
  assets/brand/watt-mark.png        the robot, full resolution, cropped to its ink with PAD px of transparency
  assets/brand/watt-wordmark.png    the WATT text, the same way
  assets/brand/app-icon-1024.png    the app icon: the mark on a dark rounded plate (input to `npx tauri icon`)
  src/assets/watt-mark.png          the mark, 256px tall, for the UI
  src/assets/watt-wordmark.png      the wordmark, 480px wide, for the UI
  public/favicon.png                the mark, 64x64, on transparent

Run it from anywhere: python3 scripts/split-logo.py  (needs Pillow).
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets" / "brand"
UI_ASSETS = ROOT / "src" / "assets"
PUBLIC = ROOT / "public"

FULL = BRAND / "watt-logo-full.png"

# Alpha at or above this counts as ink for the bounding boxes. The source has a faint halo (alpha 1 to 19) between the
# robot and the wordmark, which is ignored here; the crops still copy those pixels, and they are invisible.
INK_MIN_ALPHA = 16
# Transparent padding around each cropped part, in source pixels.
PAD = 24
# The rows between the robot and the wordmark. Checked below: no ink may appear in them.
GAP_TOP, GAP_BOTTOM = 868, 908

# The app icon colours. Keep in step with --bg and --accent in src/App.css (the Blueprint theme).
ICON_BG = (0x0A, 0x22, 0x33)
ICON_ACCENT = (0x6F, 0xD3, 0xFF)
ICON_BORDER = tuple(round(0.4 * a + 0.6 * b) for a, b in zip(ICON_ACCENT, ICON_BG))  # the accent dimmed into the plate
ICON_SIZE = 1024
PLATE = 824
PLATE_RADIUS = 185
PLATE_BORDER_PX = 2
MARK_FRACTION_OF_PLATE = 0.62


def ink_box(img: Image.Image, top: int, bottom: int) -> tuple[int, int, int, int]:
    """The bounding box of the ink in rows [top, bottom), in full-image coordinates."""
    alpha = img.getchannel("A").crop((0, top, img.width, bottom))
    box = alpha.point(lambda v: 255 if v >= INK_MIN_ALPHA else 0).getbbox()
    if box is None:
        raise SystemExit(f"no ink in rows {top}-{bottom}")
    x0, y0, x1, y1 = box
    return x0, y0 + top, x1, y1 + top


def padded(img: Image.Image, box: tuple[int, int, int, int], pad: int) -> Image.Image:
    x0, y0, x1, y1 = box
    return img.crop((max(0, x0 - pad), max(0, y0 - pad), min(img.width, x1 + pad), min(img.height, y1 + pad)))


def fit_height(img: Image.Image, height: int) -> Image.Image:
    return img.resize((round(img.width * height / img.height), height), Image.LANCZOS)


def fit_width(img: Image.Image, width: int) -> Image.Image:
    return img.resize((width, round(img.height * width / img.width)), Image.LANCZOS)


def save(img: Image.Image, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, optimize=True)
    print(f"wrote {path.relative_to(ROOT)} {img.width}x{img.height}")


def app_icon(mark_ink: Image.Image) -> Image.Image:
    """A 1024 square: a rounded dark plate with a thin accent border, and the mark centred on it."""
    s = 4  # draw at four times the size, then reduce, for smooth corners
    canvas = Image.new("RGBA", (ICON_SIZE * s, ICON_SIZE * s), (0, 0, 0, 0))
    inset = (ICON_SIZE - PLATE) // 2 * s
    d = ImageDraw.Draw(canvas)
    d.rounded_rectangle(
        (inset, inset, ICON_SIZE * s - inset - 1, ICON_SIZE * s - inset - 1),
        radius=PLATE_RADIUS * s,
        fill=ICON_BG + (255,),
        outline=ICON_BORDER + (255,),
        width=PLATE_BORDER_PX * s,
    )
    icon = canvas.resize((ICON_SIZE, ICON_SIZE), Image.LANCZOS)
    height = round(PLATE * MARK_FRACTION_OF_PLATE)
    mark = fit_height(mark_ink, height)
    icon.alpha_composite(mark, ((ICON_SIZE - mark.width) // 2, (ICON_SIZE - mark.height) // 2))
    return icon


def favicon(mark: Image.Image) -> Image.Image:
    fitted = mark.copy()
    fitted.thumbnail((64, 64), Image.LANCZOS)
    out = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    out.alpha_composite(fitted, ((64 - fitted.width) // 2, (64 - fitted.height) // 2))
    return out


def main() -> None:
    full = Image.open(FULL).convert("RGBA")
    if full.size != (1254, 1254):
        raise SystemExit(f"expected the 1254x1254 source, got {full.size}")

    gap_alpha = full.getchannel("A").crop((0, GAP_TOP, full.width, GAP_BOTTOM + 1))
    if gap_alpha.getextrema()[1] >= INK_MIN_ALPHA:
        raise SystemExit("the rows between the robot and the wordmark contain ink; the split rows are wrong")

    split = (GAP_TOP + GAP_BOTTOM + 1) // 2
    mark_ink = ink_box(full, 0, split)
    word_ink = ink_box(full, split, full.height)
    print(f"mark ink box {mark_ink}, wordmark ink box {word_ink}")

    mark = padded(full, mark_ink, PAD)
    wordmark = padded(full, word_ink, PAD)

    save(mark, BRAND / "watt-mark.png")
    save(wordmark, BRAND / "watt-wordmark.png")
    save(app_icon(full.crop(mark_ink)), BRAND / "app-icon-1024.png")
    save(fit_height(mark, 256), UI_ASSETS / "watt-mark.png")
    save(fit_width(wordmark, 480), UI_ASSETS / "watt-wordmark.png")
    save(favicon(mark), PUBLIC / "favicon.png")


if __name__ == "__main__":
    main()
