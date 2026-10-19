"""生成 Aurtrack 的占位图标。

用法：
    python scripts/make-icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 256
BACKGROUND = (15, 17, 21, 255)
ACCENT = (110, 168, 254, 255)
MUTED = (70, 90, 130, 255)

image = Image.new("RGBA", (SIZE, SIZE), BACKGROUND)
draw = ImageDraw.Draw(image)
draw.rounded_rectangle([16, 16, 240, 240], radius=52, outline=ACCENT, width=10)

bars = [0.32, 0.55, 0.82, 0.62, 0.9, 0.45, 0.7]
bar_width = 14
gap = 10
total_width = len(bars) * bar_width + (len(bars) - 1) * gap
x = (SIZE - total_width) // 2
for ratio in bars:
    height = int(120 * ratio)
    top = 128 - height // 2
    bottom = 128 + height // 2
    color = ACCENT if ratio > 0.6 else MUTED
    draw.rounded_rectangle(
        [x, top, x + bar_width, bottom], radius=7, fill=color
    )
    x += bar_width + gap

output_dir = Path(__file__).resolve().parent.parent / "shells" / "desktop" / "icons"
output_dir.mkdir(parents=True, exist_ok=True)
image.save(output_dir / "icon.png")
image.save(
    output_dir / "icon.ico",
    sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
)
print(f"wrote {output_dir / 'icon.ico'}")
