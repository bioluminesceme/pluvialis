"""Build the Pluvialis app icon from a square PNG.

    py -X utf8 tools/make_icon.py
    cargo build --release

Edit `crates/pluvialis-app/assets/icon-source.png` in any image editor, save it,
run those two. The first writes `icon.png` and `icon.ico`; the second embeds the
`.ico` into the executable, because `build.rs` does that at build time.

## This script does not crop, and must not start

Whatever is in the source file is what ends up in the icon, at every size. An
earlier version tried to be clever: it cut the white page away by flood filling
from the corners, and zoomed in for the small sizes on the theory that a tighter
crop survives being shrunk. Both were wrong. The zoom shipped a visible bug,
because the artwork already filled about 80% of its frame and there was no room
to zoom into, so the bird's bill and tail ran off the edges at exactly the 24
and 32 pixel sizes the taskbar draws.

Framing is a decision for whoever draws the icon, in the editor, where it can be
seen. Not for this file.

## What to give it

A square PNG, 512 pixels or larger, ideally 1024. Transparency is kept, so cut
the rounded corners in the editor if you want them. It has to survive being
shrunk to 16 pixels, where thin lines and fine detail disappear: big shapes and
strong contrast are what read at that size.

A non-square image is padded to a square with transparency rather than being
stretched or cropped, since both of those are decisions this script should not
be making either.
"""

import struct
from io import BytesIO
from pathlib import Path

from PIL import Image, ImageFilter

SOURCE = (
    Path(__file__).resolve().parents[1]
    / "crates" / "pluvialis-app" / "assets" / "icon-source.png"
)
ASSETS = SOURCE.parent

# What Windows asks for. 256 is the one Explorer shows at large sizes, 16 is the
# title bar and the details view.
SIZES = (256, 128, 64, 48, 32, 24, 16)

# Below this, shrinking softens the outline. Sharpening buys the definition
# back, which is the honest way to help the small sizes: it costs no margin.
SHARPEN_AT_OR_BELOW = 48


def squared(image: Image.Image) -> Image.Image:
    """Pad to a square with transparency, centred. Never stretch, never crop."""
    if image.width == image.height:
        return image
    side = max(image.size)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(image, ((side - image.width) // 2, (side - image.height) // 2))
    return canvas


def scaled(art: Image.Image, size: int) -> Image.Image:
    """One icon size, sharpened at the small end.

    The sharpen touches the colour channels only. Running it over the alpha
    would put a bright halo around the edge, which is the one place the icon is
    silhouetted against whatever is behind it.
    """
    small = art.resize((size, size), Image.LANCZOS)
    if size > SHARPEN_AT_OR_BELOW:
        return small

    alpha = small.getchannel("A")
    out = small.convert("RGB").filter(
        ImageFilter.UnsharpMask(radius=0.8, percent=70, threshold=0)
    ).convert("RGBA")
    out.putalpha(alpha)
    return out


def write_ico(images: dict[int, Image.Image], path: Path) -> None:
    """Assemble an ICO holding every size.

    Written by hand rather than with Pillow's ICO writer so each size is stored
    exactly as produced above, sharpening included, instead of being resized
    again from a single source.

    Each entry is stored as PNG, which every Windows since Vista reads, and
    which keeps the alpha channel without the AND mask a BMP entry needs.
    """
    sizes = sorted(images, reverse=True)
    payloads = []
    for size in sizes:
        buffer = BytesIO()
        images[size].save(buffer, format="PNG", optimize=True)
        payloads.append(buffer.getvalue())

    offset = 6 + 16 * len(sizes)
    entries = b""
    for size, payload in zip(sizes, payloads):
        entries += struct.pack(
            "<BBBBHHII",
            0 if size == 256 else size,  # 0 means 256 in this format
            0 if size == 256 else size,
            0,  # not a palette
            0,  # reserved
            1,  # colour planes
            32,  # bits per pixel
            len(payload),
            offset,
        )
        offset += len(payload)

    path.write_bytes(
        struct.pack("<HHH", 0, 1, len(sizes)) + entries + b"".join(payloads)
    )


def main() -> None:
    art = squared(Image.open(SOURCE).convert("RGBA"))
    if art.width < 256:
        raise SystemExit(
            f"{SOURCE.name} is only {art.width} pixels square. "
            "Give it 512 or more, ideally 1024."
        )
    # Reduced once to a common size, so every icon is a clean reduction of the
    # same picture rather than a reduction of a reduction.
    art = art.resize((1024, 1024), Image.LANCZOS)

    per_size = {size: scaled(art, size) for size in SIZES}

    # eframe's runtime window icon: the title bar and alt-tab.
    per_size[256].save(ASSETS / "icon.png")
    # Embedded into the exe by build.rs: the taskbar and Explorer icon.
    write_ico(per_size, ASSETS / "icon.ico")

    print(f"read     {SOURCE.name}")
    print(f"icon.png {(ASSETS / 'icon.png').stat().st_size:>7} bytes")
    print(f"icon.ico {(ASSETS / 'icon.ico').stat().st_size:>7} bytes, "
          f"sizes {', '.join(str(s) for s in SIZES)}")
    print("\nNow: cargo build --release")


if __name__ == "__main__":
    main()
