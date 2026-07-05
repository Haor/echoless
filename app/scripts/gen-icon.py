#!/usr/bin/env python3
"""Echoless app icon master (1024px) — H2「一滴橙」定稿版。

圆角炭黑底板 + 直角三杠(纸灰=人声 / 橙=参考 / 暗灰=残响)+ 白噪点颗粒。
提案页在仓库外层:../Design/icon-ideas.html(H2)。
再喂给 `tauri icon` 生成全尺寸(icns/ico/android/ios)。
"""
from PIL import Image, ImageDraw
import random
import sys, os

SS = 4          # supersample
SIZE = 1024
S = SIZE * SS

BG = (29, 29, 27, 255)        # --bg #1d1d1b
LINE = (53, 53, 47, 255)      # --line #35352f
ACC = (255, 114, 53, 255)     # --acc #ff7235
PAPER = (214, 213, 205, 255)  # --t-strong #d6d5cd
MUT = (138, 137, 127, 255)    # 残响暗灰 #8a897f

M, R = 100, 224               # 底板边距 / 圆角(1024 坐标系)

# 圆角底板(留 ~10% 透明边距,macOS 风格)
base = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(base)
d.rounded_rectangle([M * SS, M * SS, S - M * SS, S - M * SS], radius=R * SS,
                    fill=BG, outline=LINE, width=6 * SS)
img = base.resize((SIZE, SIZE), Image.LANCZOS)

# 白噪点:逐像素稀疏白点(固定种子,字节稳定 → 跨构建可复现),裁进底板
mask = Image.new("L", (SIZE, SIZE), 0)
ImageDraw.Draw(mask).rounded_rectangle([M, M, SIZE - M, SIZE - M], radius=R, fill=255)
rng = random.Random(7)
noise = Image.new("L", (SIZE, SIZE), 0)
np_ = noise.load()
mp = mask.load()
for y in range(SIZE):
    for x in range(SIZE):
        if mp[x, y]:
            # 对齐 SVG 版:alpha = clamp(0.9*n - 0.55) * 0.5
            a = 0.9 * rng.random() - 0.55
            if a > 0:
                np_[x, y] = int(a * 0.5 * 255)
white = Image.new("RGBA", (SIZE, SIZE), (255, 255, 255, 255))
img.paste(white, (0, 0), noise)

# 直角三杠(超采样后缩,保边缘锐利):x=300, h=64, y=350/480/610
bars = Image.new("RGBA", (S, S), (0, 0, 0, 0))
db = ImageDraw.Draw(bars)
for (w, col, y) in [(325, PAPER, 350), (435, ACC, 480), (215, MUT, 610)]:
    db.rectangle([300 * SS, y * SS, (300 + w) * SS, (y + 64) * SS], fill=col)
img.alpha_composite(bars.resize((SIZE, SIZE), Image.LANCZOS))

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "..", "icon-master-1024.png")
img.save(out)
print(f"wrote {out}")
