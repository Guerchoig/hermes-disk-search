"""Генерация иконки hermes-disk-search: диск/документ + лупа.
Создаёт assets/icon.png (512), assets/icon.ico (Windows), assets/icon.icns (macOS).
Запуск: .venv\\Scripts\\python.exe tools\\make_icon.py
"""
import math
import os
import sys

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "assets")
os.makedirs(OUT, exist_ok=True)

S = 512
BG = (79, 70, 229, 255)        # индиго
WHITE = (255, 255, 255, 255)
GREY = (203, 213, 225, 255)

img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# фон — скруглённый квадрат
d.rounded_rectangle([16, 16, S - 16, S - 16], radius=96, fill=BG)

# «документ» слева
dx0, dy0, dx1, dy1 = 120, 96, 316, 392
d.rounded_rectangle([dx0, dy0, dx1, dy1], radius=24, fill=WHITE)
d.rounded_rectangle([dx0, dy0, dx1, dy1], radius=24, outline=WHITE, width=2)

# «строки текста» на документе
for y in (156, 212, 268):
    d.rounded_rectangle([dx0 + 36, y, dx0 + 160, y + 20], radius=10, fill=GREY)

# лупа (справа-снизу, поверх документа)
cx, cy, r = 322, 322, 78
d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(255, 255, 255, 40))
d.ellipse([cx - r, cy - r, cx + r, cy + r], outline=WHITE, width=24)
a = math.radians(45)
d.line(
    [cx + (r - 6) * math.cos(a), cy + (r - 6) * math.sin(a),
     cx + (r + 74) * math.cos(a), cy + (r + 74) * math.sin(a)],
    fill=WHITE, width=30,
)

# диск внизу — три полоски-диска
for i, y in enumerate((438, 458, 478)):
    d.rounded_rectangle([64, y, S - 64, y + 10], radius=5,
                        fill=(255, 255, 255, 255 - i * 40))

img.save(os.path.join(OUT, "icon.png"))

# Windows .ico — набор размеров
ico = os.path.join(OUT, "icon.ico")
img.save(ico, sizes=[(16, 16), (24, 24), (32, 32), (48, 48),
                     (64, 64), (128, 128), (256, 256)])

# macOS icns
try:
    img.save(os.path.join(OUT, "icon.icns"))
    icns = "ok"
except Exception as e:  # noqa: BLE001
    icns = "ошибка: %s" % e

print("icon.png, icon.ico созданы; icon.icns: %s" % icns)