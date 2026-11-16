"""把 tools/make-gif.mjs 抓下来的一串 PNG 合成 GIF。

为什么用 Pillow 而不是纯标准库：GIF 的 LZW 编码自己写不划算，而 Pillow 在这里是
一次性的开发工具依赖（`pip install pillow`），不进程序、不进安装包。

用法：python tools/gif.py [帧目录] [输出.gif] [宽度]
帧目录里的 timing.json 由 make-gif.mjs 写出，是每帧的停留毫秒数。
"""

import glob
import json
import os
import sys

from PIL import Image

src = sys.argv[1] if len(sys.argv) > 1 else '.tmp/frames'
dst = sys.argv[2] if len(sys.argv) > 2 else 'docs/demo.gif'
width = int(sys.argv[3]) if len(sys.argv) > 3 else 900

files = sorted(glob.glob(os.path.join(src, '*.png')))
if not files:
    sys.exit(f'没找到帧：{src}/*.png')

timing_path = os.path.join(src, 'timing.json')
if os.path.exists(timing_path):
    with open(timing_path, encoding='utf-8') as fh:
        timing = json.load(fh)
else:
    timing = [700] * len(files)
if len(timing) != len(files):
    sys.exit(f'帧数（{len(files)}）和 timing.json（{len(timing)}）对不上')

frames = []
for f in files:
    im = Image.open(f).convert('RGB')
    h = round(im.height * width / im.width)
    frames.append(im.resize((width, h), Image.LANCZOS))

# GIF 只有 256 色。先用第一帧算一套全局调色板再逐帧映射，文字边缘不会因为每帧
# 各自量化而闪；抖动保留，深色渐变里的过渡色才不至于变成色块。
pal = frames[0].quantize(colors=128, method=Image.MEDIANCUT, dither=Image.FLOYDSTEINBERG)
out = [fr.quantize(palette=pal, dither=Image.FLOYDSTEINBERG) for fr in frames]

out[0].save(
    dst,
    save_all=True,
    append_images=out[1:],
    duration=timing,
    loop=0,
    optimize=True,
    disposal=2,
)

kb = os.path.getsize(dst) / 1024
print(f'{dst}  {frames[0].width}x{frames[0].height}  {len(out)} 帧  {sum(timing) / 1000:.1f}s  {kb:.0f} KB')
