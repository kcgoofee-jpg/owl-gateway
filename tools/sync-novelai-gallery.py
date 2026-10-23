#!/usr/bin/env python3
"""Refresh public image/prompt data without executing any upstream JavaScript.

Run before a reviewed release: python3 tools/sync-novelai-gallery.py
--source-dir supports replaying downloaded public chunks for offline verification.
A changed upstream format fails closed and leaves the previous snapshot intact.
"""
import argparse
import ast
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import html
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import urljoin, urlparse

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "web/src/pages/landing/gallery-data.json"
PAGE = "https://novelai.net/image"
STRING = r'''(?:"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')'''


def fetch(url):
    return subprocess.check_output([
        "curl", "--silent", "--show-error", "--fail", "--location",
        "--max-time", "25", "--max-filesize", "15000000", url,
    ], text=True)


def extract(source):
    listing = re.search(
        r'(\[[\d,]+\])\.map\([^=]+=>\(\{src:`https://static\.novelai\.net/quickstart/v5/',
        source,
    )
    if not listing:
        raise ValueError("Official gallery list format changed")
    ids = [f"{n:04d}" for n in json.loads(listing[1])]
    prompts = {
        match[1]: ast.literal_eval(match[2])
        for match in re.finditer(r'"(\d{4})\.webp":\{Description:(' + STRING + ')', source)
    }
    if not 20 <= len(ids) <= 500 or len(set(ids)) != len(ids):
        raise ValueError("Unexpected gallery size or duplicate IDs")
    if any(not isinstance(prompts.get(id), str) or not 3 <= len(prompts[id]) <= 20000 for id in ids):
        raise ValueError("Missing or invalid official prompt; refusing partial update")
    return ids, prompts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=Path)
    args = parser.parse_args()
    if args.source_dir:
        sources = [p.read_text() for p in args.source_dir.glob("*.js")]
    else:
        page = fetch(PAGE)
        urls = list(dict.fromkeys(
            urljoin(PAGE, html.unescape(src))
            for src in re.findall(r'<script[^>]+src="([^"]+)"', page)
            if src.startswith("/_next/static/chunks/")
        ))
        if not 1 <= len(urls) <= 200 or any(urlparse(u).hostname != "novelai.net" for u in urls):
            raise ValueError("Unexpected public script inventory")
        with ThreadPoolExecutor(max_workers=6) as pool:
            sources = list(pool.map(fetch, urls))
    candidates = [s for s in sources if "https://static.novelai.net/quickstart/v5/" in s and 'Description:' in s]
    if len(candidates) != 1:
        raise ValueError("Expected exactly one official gallery data bundle")
    ids, prompts = extract(candidates[0])
    previous = json.loads(DEST.read_text())["images"] if DEST.exists() else []
    order = list(dict.fromkeys([i["id"] for i in previous if i["id"] in ids] + ids))
    images = [{"id": id, "prompt": prompts[id]} for id in order]
    if images == previous:
        print(f"No gallery changes found ({len(images)} public examples).")
        return
    data = {
        "source": PAGE,
        "syncedAt": datetime.now(timezone.utc).date().isoformat(),
        "images": images,
    }
    temp = DEST.with_suffix(".json.tmp")
    temp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
    temp.replace(DEST)
    print(f"Updated {len(order)} public image/prompt pairs; review the diff before publishing.")


if __name__ == "__main__":
    main()
