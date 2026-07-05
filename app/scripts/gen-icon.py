#!/usr/bin/env python3
"""Echoless app icon master (1024px) — Brass Hands theme.

炭黑圆角方 + 衰减回声波形:首杠橙(--acc, 活信号),
其后回声杠逐级变矮变暗直至熄灭(echo → less)。
再喂给 `tauri icon` 生成全尺寸(icns/ico/android/ios)。
"""
from PIL import Image, ImageDraw
import sys, os

SS = 4          # supersample
SIZE = 1024
S = SIZE * SS

BG = (29, 29, 27, 255)        # --bg #1d1d1b
LINE = (53, 53, 47, 255)      # --line #35352f
ACC = (255, 114, 53, 255)     # --acc #ff7235
PAPER = (214, 213, 205, 255)  # --t-strong #d6d5cd

img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# 圆角底板(留 ~10% 透明边距,macOS 风格)
m = 100 * SS
r = 224 * SS
d.rounded_rectangle([m, m, S - m, S - m], radius=r, fill=BG,
                    outline=LINE, width=6 * SS)

def blend(fg, a):
    # 渐隐色预混进底板色(ImageDraw 不做 alpha 合成,直接写会捅穿到透明层)
    return tuple(BG[i] + (fg[i] - BG[i]) * a // 255 for i in range(3)) + (255,)

# 衰减波形:垂直居中的圆头杠,首杠橙色(人声),其后纸灰回声渐隐熄灭。
# 首杠后间距(gap1)刻意拉大:信号 | 回声 两组。
cy = S // 2
bar_w = 92 * SS
gap = 52 * SS
gap1 = 84 * SS
heights = [0.640, 0.420, 0.280, 0.180, 0.112]   # 占内容高比例
alphas = [255, 230, 160, 100, 58]
inner_h = S - 2 * m
n = len(heights)
total_w = n * bar_w + gap1 + (n - 2) * gap
x = (S - total_w) // 2
for i, (hf, a) in enumerate(zip(heights, alphas)):
    h = int(inner_h * hf)
    color = ACC if i == 0 else blend(PAPER, a)
    d.rounded_rectangle([x, cy - h // 2, x + bar_w, cy + h // 2],
                        radius=bar_w // 2, fill=color)
    x += bar_w + (gap1 if i == 0 else gap)

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "..", "icon-master-1024.png")
img.resize((SIZE, SIZE), Image.LANCZOS).save(out)
print(f"wrote {out}")
