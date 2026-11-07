#!/usr/bin/env python3
"""
从 Qt 的 .rcc 资源包里取出素材（PokerStars 客户端用这套格式）。

RCC 结构（Qt 5/6，version 3）：
    header   magic "qres"(4) version(4) treeOffset(4) dataOffset(4) nameOffset(4)
    data     若干文件数据，图片通常不压缩、原样存放
    names    [uint16 长度][uint32 哈希][长度*2 字节 UTF-16BE]，连续排列
    tree     节点数组，末尾

这个脚本不追求逐字节还原树节点（各版本字段有差异），改用两条稳的路径：
    1. 图片：直接扫 PNG/JPG/WEBP 魔数，自结束标记切出完整文件
    2. 名字：按长度前缀读出来，与图片按顺序配对（Qt 按树顺序写数据，二者一致）

用法：
    python3 tools/rcc-extract.py <input.rcc> <out_dir>
    python3 tools/rcc-extract.py <input.rcc> <out_dir> --names-only
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

PNG = b"\x89PNG\r\n\x1a\n"
PNG_END = b"IEND\xaeB`\x82"
JPG = b"\xff\xd8\xff"
JPG_END = b"\xff\xd9"
WEBP = b"RIFF"


def read_header(data: bytes) -> tuple[int, int, int, int]:
    if data[:4] != b"qres":
        raise SystemExit("不是 rcc 文件（缺少 qres 魔数）")
    version, tree_off, data_off, name_off = struct.unpack(">IIII", data[4:20])
    return version, tree_off, data_off, name_off


def read_names(data: bytes, name_off: int, tree_off: int) -> list[str]:
    """名字段：[uint16 len][uint32 hash][len*2 UTF-16BE]。读到 tree 段为止。"""
    names: list[str] = []
    i = name_off
    while i < tree_off - 6:
        ln = struct.unpack(">H", data[i : i + 2])[0]
        if ln == 0 or ln > 128:
            break
        start = i + 6
        names.append(data[start : start + ln * 2].decode("utf-16-be", "ignore"))
        i = start + ln * 2
    return names


def find_pngs(data: bytes, start: int, end: int) -> list[tuple[int, int]]:
    """扫 PNG 魔数，用 IEND 切出完整文件，返回 [(offset, length)]。"""
    out: list[tuple[int, int]] = []
    i = start
    while True:
        i = data.find(PNG, i, end)
        if i < 0:
            break
        tail = data.find(PNG_END, i, end)
        if tail < 0:
            break
        out.append((i, tail + len(PNG_END) - i))
        i = tail + len(PNG_END)
    return out


def find_jpgs(data: bytes, start: int, end: int) -> list[tuple[int, int]]:
    out: list[tuple[int, int]] = []
    i = start
    while True:
        i = data.find(JPG, i, end)
        if i < 0:
            break
        tail = data.find(JPG_END, i, end)
        if tail < 0:
            break
        out.append((i, tail + 2 - i))
        i = tail + 2
    return out


def safe(name: str) -> str:
    return "".join(c if c.isalnum() or c in "-_." else "_" for c in name) or "unnamed"


def main() -> int:
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    src = Path(sys.argv[1])
    out = Path(sys.argv[2])
    data = src.read_bytes()

    version, tree_off, data_off, name_off = read_header(data)
    names = read_names(data, name_off, tree_off)
    print(f"{src.name}: version={version} names={len(names)}")

    if "--names-only" in sys.argv:
        for n in names:
            print(n)
        return 0

    images = sorted(find_pngs(data, data_off, name_off) + find_jpgs(data, data_off, name_off))
    print(f"  图片 {len(images)} 个，合计 {sum(n for _, n in images) // 1024} KB")

    out.mkdir(parents=True, exist_ok=True)
    # 名字按顺序配对：图片资源名在 name 列表里是按写入顺序排的
    idx = 0
    manifest = []
    for off, size in images:
        name = names[idx] if idx < len(names) else f"image-{idx:04d}"
        idx += 1
        ext = ".png" if data[off : off + 4] == b"\x89PNG" else ".jpg"
        path = out / f"{idx:04d}-{safe(name)}{ext}"
        path.write_bytes(data[off : off + size])
        manifest.append((idx, name, size, path.name))

    with open(out / "manifest.tsv", "w", encoding="utf-8") as fh:
        fh.write("idx\tname\tbytes\tfile\n")
        for row in manifest:
            fh.write("\t".join(map(str, row)) + "\n")

    print(f"  已导出到 {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
