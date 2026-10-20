"""从 `icons/icon-source.png` 生成 Aurtrack 的应用图标。

用法：
    python scripts/make-icon.py
"""

from io import BytesIO
from pathlib import Path
import struct

from PIL import Image

ICON_DIR = Path(__file__).resolve().parent.parent / "shells" / "desktop" / "icons"
SOURCE = ICON_DIR / "icon-source.png"
FAVICON_DIR = Path(__file__).resolve().parent.parent / "ui" / "public"
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]

source = Image.open(SOURCE).convert("RGBA")
source.resize((256, 256), Image.Resampling.NEAREST).save(ICON_DIR / "icon.png")


def ico_frame(size: int) -> bytes:
    # 16px 源图在 16/24/32/48 下用最近邻，保留像素轮廓；大尺寸再平滑放大。
    resampling = (
        Image.Resampling.NEAREST
        if size <= 48
        else Image.Resampling.LANCZOS
    )
    frame = source.resize((size, size), resampling)
    buffer = BytesIO()
    frame.save(buffer, format="PNG")
    return buffer.getvalue()


payloads = [ico_frame(size) for size in ICO_SIZES]
header = struct.pack("<HHH", 0, 1, len(ICO_SIZES))
offset = 6 + 16 * len(ICO_SIZES)
entries = []
for size, payload in zip(ICO_SIZES, payloads, strict=True):
    dimension = 0 if size == 256 else size
    entries.append(
        struct.pack(
            "<BBBBHHII",
            dimension,
            dimension,
            0,
            0,
            1,
            32,
            len(payload),
            offset,
        )
    )
    offset += len(payload)

(ICON_DIR / "icon.ico").write_bytes(
    header + b"".join(entries) + b"".join(payloads)
)

FAVICON_DIR.mkdir(parents=True, exist_ok=True)
source.resize((32, 32), Image.Resampling.NEAREST).save(
    FAVICON_DIR / "favicon.png"
)
print(f"wrote {ICON_DIR / 'icon.ico'}")
