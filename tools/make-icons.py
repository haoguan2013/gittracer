#!/usr/bin/env python
"""生成 Git Tracer 的应用图标（纯标准库，无 Pillow 依赖）。

形状：深色圆角方块 + 青绿色 git 分支图（两个主干节点 + 一个分支节点）。
输出：src-tauri/icons/ 下的 png / ico / icns。
用法：python tools/make-icons.py
"""
from __future__ import annotations

import math
import os
import struct
import zlib

BASE = 512                      # 内部渲染尺寸
ICON_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src-tauri", "icons")

# 调色板（线性 0..1 RGBA）
BG_TOP = (0x16, 0x2B, 0x3A)
BG_BOTTOM = (0x08, 0x10, 0x16)
BORDER = (0x2A, 0x54, 0x66)
ACCENT = (0x4F, 0xD1, 0xC5)
NODE = (0xEC, 0xFB, 0xF8)


# ---------------------------------------------------------------- 几何工具
def clamp(x, lo=0.0, hi=1.0):
    return lo if x < lo else hi if x > hi else x


def seg_dist(px, py, ax, ay, bx, by):
    vx, vy = bx - ax, by - ay
    wx, wy = px - ax, py - ay
    L2 = vx * vx + vy * vy
    t = 0.0 if L2 == 0 else clamp((wx * vx + wy * vy) / L2)
    dx, dy = wx - t * vx, wy - t * vy
    return math.hypot(dx, dy)


def bezier(p0, p1, p2, p3, n=64):
    """三次贝塞尔采样成折线点列表。"""
    pts = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        x = u * u * u * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t * t * t * p3[0]
        y = u * u * u * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t * t * t * p3[1]
        pts.append((x, y))
    return pts


def rounded_box_sdf(x, y, cx, cy, half, radius):
    dx = abs(x - cx) - (half - radius)
    dy = abs(y - cy) - (half - radius)
    ax, ay = max(dx, 0.0), max(dy, 0.0)
    outside = math.hypot(ax, ay)
    inside = min(max(dx, dy), 0.0)
    return outside + inside - radius


# ---------------------------------------------------------------- 绘制
def render(size=BASE):
    """返回 size*size 的 RGBA bytes（逐像素解析式抗锯齿）。"""
    S = size
    # 单位坐标系（0..1）
    trunk_x = 0.335
    top_node = (trunk_x, 0.255)
    bot_node = (trunk_x, 0.760)
    br_node = (0.690, 0.415)
    stroke = 0.056          # 半宽
    node_r = 0.083

    curve = bezier((trunk_x, 0.600), (trunk_x, 0.430), (0.500, 0.415), (br_node[0], br_node[1]), 96)
    curve_segs = list(zip(curve[:-1], curve[1:]))

    pixels = bytearray(S * S * 4)
    half = 0.5
    radius = 0.225

    for py in range(S):
        y = (py + 0.5) / S
        for px in range(S):
            x = (px + 0.5) / S
            # 背景圆角方块
            d = rounded_box_sdf(x, y, 0.5, 0.5, half, radius)
            bg_a = clamp(0.5 - d * S)          # 以像素为单位做 1px 过渡
            if bg_a <= 0.0:
                continue
            t = clamp((y - 0.06) / 0.88)
            r = BG_TOP[0] + (BG_BOTTOM[0] - BG_TOP[0]) * t
            g = BG_TOP[1] + (BG_BOTTOM[1] - BG_TOP[1]) * t
            b = BG_TOP[2] + (BG_BOTTOM[2] - BG_TOP[2]) * t

            # 内描边（贴边一圈亮线，增强轮廓）
            ring = 1.0 - clamp((abs(d + 0.012) - 0.0035) * S)
            if ring > 0:
                r += (BORDER[0] - r) * ring * 0.85
                g += (BORDER[1] - g) * ring * 0.85
                b += (BORDER[2] - b) * ring * 0.85

            # 主干
            cov = clamp((stroke + 0.5 / S - seg_dist(x, y, trunk_x, top_node[1], trunk_x, bot_node[1])) * S)
            # 分支曲线
            if cov < 1.0:
                best = min(seg_dist(x, y, a[0], a[1], b_[0], b_[1]) for a, b_ in curve_segs)
                cov = max(cov, clamp((stroke + 0.5 / S - best) * S))
            if cov > 0:
                r += (ACCENT[0] - r) * cov
                g += (ACCENT[1] - g) * cov
                b += (ACCENT[2] - b) * cov

            # 三个节点
            for nx, ny in (top_node, bot_node, br_node):
                dd = math.hypot(x - nx, y - ny)
                cov = clamp((node_r + 0.5 / S - dd) * S)
                if cov > 0:
                    r += (NODE[0] - r) * cov
                    g += (NODE[1] - g) * cov
                    b += (NODE[2] - b) * cov
                # 节点中心挖回青色小点，做出“环”的感觉
                dd2 = math.hypot(x - nx, y - ny)
                hole = clamp((node_r * 0.42 + 0.5 / S - dd2) * S)
                if hole > 0:
                    r += (ACCENT[0] - r) * hole
                    g += (ACCENT[1] - g) * hole
                    b += (ACCENT[2] - b) * hole

            i = (py * S + px) * 4
            pixels[i] = int(clamp(r, 0, 255) + 0.5)
            pixels[i + 1] = int(clamp(g, 0, 255) + 0.5)
            pixels[i + 2] = int(clamp(b, 0, 255) + 0.5)
            pixels[i + 3] = int(bg_a * 255 + 0.5)
    return bytes(pixels)


def downsample(px, src, dst):
    """盒式降采样，src -> dst 方图。"""
    S, D = src, dst
    out = bytearray(D * D * 4)
    scale = S / D
    for oy in range(D):
        y0 = int(oy * scale)
        y1 = max(y0 + 1, int((oy + 1) * scale))
        for ox in range(D):
            x0 = int(ox * scale)
            x1 = max(x0 + 1, int((ox + 1) * scale))
            r = g = b = a = 0.0
            n = 0
            for y in range(y0, min(y1, S)):
                row = y * S
                for x in range(x0, min(x1, S)):
                    i = (row + x) * 4
                    pa = px[i + 3] / 255.0
                    # 预乘 alpha 再平均，避免边缘发黑
                    r += px[i] * pa
                    g += px[i + 1] * pa
                    b += px[i + 2] * pa
                    a += pa
                    n += 1
            if n == 0:
                n = 1
            a /= n
            i = (oy * D + ox) * 4
            if a > 0:
                out[i] = int(clamp(r / n / a, 0, 255) + 0.5)
                out[i + 1] = int(clamp(g / n / a, 0, 255) + 0.5)
                out[i + 2] = int(clamp(b / n / a, 0, 255) + 0.5)
            out[i + 3] = int(clamp(a * 255, 0, 255) + 0.5)
    return bytes(out)


# ---------------------------------------------------------------- 编码
def png_bytes(px, size):
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)  # filter: None
        raw += px[y * stride:(y + 1) * stride]

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def ico_bytes(images):
    """images: [(size, rgba bytes)] -> ICO（每帧 32bpp BMP）。"""
    header = struct.pack("<HHH", 0, 1, len(images))
    entries = b""
    blobs = b""
    offset = 6 + 16 * len(images)
    for size, px in images:
        # BITMAPINFOHEADER，高度按 ICO 规范写两倍
        bih = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, size * size * 4, 0, 0, 0, 0)
        rows = bytearray()
        for y in range(size - 1, -1, -1):           # BMP 自底向上
            row = px[y * size * 4:(y + 1) * size * 4]
            rows += row
        mask_row = ((size + 31) // 32) * 4
        mask = bytes(mask_row * size)               # 全 0 = 全部不透明
        blob = bih + bytes(rows) + mask
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
        blobs += blob
    return header + entries + blobs


def icns_bytes(png_by_size):
    """最小可用 icns：塞入若干 PNG 帧。"""
    type_for = {32: b"ic11", 64: b"ic12", 128: b"ic07", 256: b"ic08", 512: b"ic09"}
    body = b""
    for size, data in png_by_size:
        tag = type_for.get(size)
        if not tag:
            continue
        body += tag + struct.pack(">I", len(data) + 8) + data
    return b"icns" + struct.pack(">I", len(body) + 8) + body


# ---------------------------------------------------------------- 主流程
def main():
    os.makedirs(ICON_DIR, exist_ok=True)
    print(f"渲染 {BASE}x{BASE} …")
    master = render(BASE)

    sizes = [16, 24, 32, 48, 64, 128, 256, 512]
    cache = {}
    for s in sizes:
        cache[s] = downsample(master, BASE, s)
        print(f"  {s}x{s} ok")

    pngs = {}
    for s in (32, 128, 256, 512):
        data = png_bytes(cache[s], s)
        pngs[s] = data
    files = {
        "32x32.png": pngs[32],
        "128x128.png": pngs[128],
        "128x128@2x.png": pngs[256],
        "512x512.png": pngs[512],
        "icon.png": pngs[512],
    }
    for name, data in files.items():
        with open(os.path.join(ICON_DIR, name), "wb") as fh:
            fh.write(data)
        print(f"写入 {name} ({len(data)} B)")

    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    with open(os.path.join(ICON_DIR, "icon.ico"), "wb") as fh:
        fh.write(ico_bytes([(s, cache[s]) for s in ico_sizes]))
    print(f"写入 icon.ico ({os.path.getsize(os.path.join(ICON_DIR, 'icon.ico'))} B)")

    icns = icns_bytes([(s, pngs[s]) for s in (32, 128, 256, 512) if s in pngs] + [(64, png_bytes(cache[64], 64))])
    with open(os.path.join(ICON_DIR, "icon.icns"), "wb") as fh:
        fh.write(icns)
    print(f"写入 icon.icns ({len(icns)} B)")


if __name__ == "__main__":
    main()
