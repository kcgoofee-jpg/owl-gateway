#!/usr/bin/env bash
# 从 PokerStars 客户端安装包/缓存目录里抽出视觉与音效素材，归一化成牌桌能直接用的命名。
#
#   tools/extract-poker-assets.sh ~/Downloads/PokerStars.dmg
#   tools/extract-poker-assets.sh ~/Downloads/PokerStarsSetup.exe
#   tools/extract-poker-assets.sh ~/Library/Caches/com.pokerstars.desktop   # 网页版/桌面端缓存
#
# 产物：
#   mirror/poker-assets/cards/{2..A}{s,h,d,c}.png   牌面（能匹配上的）
#   mirror/poker-assets/back.png                    牌背
#   mirror/poker-assets/table/*                     桌面、绒布、按钮等大图
#   mirror/poker-assets/sound/*                     音效（wav/mp3/ogg）
#   mirror/poker-assets/manifest.json               抓到了什么、漏了什么
#
# 说明：官方资源是打包进客户端的，格式各家不同（.pak / Unity bundle / Electron asar）。
# 这个脚本负责「先解开外层、再按尺寸和文件名归类」，解不开的内层会原样留下并报出来。
set -euo pipefail
cd "$(dirname "$0")/.."

SRC=${1:?用法: tools/extract-poker-assets.sh <安装包或缓存目录>}
OUT=mirror/poker-assets
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }

[[ -e $SRC ]] || { echo "找不到 $SRC" >&2; exit 1; }

say "解开外层：$SRC"
case "$SRC" in
  *.dmg)
    hdiutil attach "$SRC" -mountpoint "$TMP/mnt" -nobrowse -quiet
    cp -R "$TMP/mnt"/* "$TMP/raw" 2>/dev/null || true
    hdiutil detach "$TMP/mnt" -quiet || true
    ;;
  *.app)
    cp -R "$SRC" "$TMP/raw"
    ;;
  *.exe)
    if command -v 7z >/dev/null; then 7z x -o"$TMP/raw" "$SRC" >/dev/null
    else echo "需要 p7zip：brew install p7zip" >&2; exit 1; fi
    ;;
  *.apk|*.zip)
    unzip -q -o "$SRC" -d "$TMP/raw"
    ;;
  *.asar)
    npx --yes @electron/asar extract "$SRC" "$TMP/raw"
    ;;
  *)
    cp -R "$SRC" "$TMP/raw"
    ;;
esac

# Unity / Unreal 打包的资源再拆一层，拆不动就留着让调用方看
for pak in "$TMP"/raw/**/*.pak "$TMP"/raw/**/*.bundle; do
  [[ -e $pak ]] || continue
  command -v UnityPy >/dev/null && UnityPy "$pak" -o "$TMP/raw" >/dev/null 2>&1 || true
done

say "归类文件"
mkdir -p "$OUT"/{cards,table,sound}
python3 - "$TMP/raw" "$OUT" <<'PY'
import json, pathlib, re, shutil, sys

raw, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
IMG = {".png", ".jpg", ".jpeg", ".webp"}
AUD = {".wav", ".mp3", ".ogg"}
RANKS = {"2":"2","3":"3","4":"4","5":"5","6":"6","7":"7","8":"8","9":"9","t":"10","10":"10",
         "j":"J","q":"Q","k":"K","a":"A"}
SUITS = {"s":"s","spade":"s","h":"h","heart":"h","d":"d","diamond":"d","c":"c","club":"c"}

# 文件名里能认出来的牌：as.png / 10_h.png / card_As@2x.png / K-d.webp
CARD = re.compile(r"(?:^|[^a-z0-9])(10|[2-9tjqka])[_\-]?(s|h|d|c|spade|heart|diamond|club)(?:[^a-z0-9]|$)", re.I)

found = {"cards": [], "images": 0, "audio": 0, "unknown": 0}
for f in raw.rglob("*"):
    if not f.is_file():
        continue
    ext = f.suffix.lower()
    if ext in AUD:
        dest = out / "sound" / f.name
        shutil.copy2(f, dest)
        found["audio"] += 1
        continue
    if ext not in IMG:
        continue
    found["images"] += 1
    m = CARD.search(f.stem)
    if m:
        rank = RANKS[m.group(1).lower()]
        suit = SUITS[m.group(2).lower()]
        name = f"{rank}{suit}.png"
        shutil.copy2(f, out / "cards" / name)
        found["cards"].append(name)
        continue
    if re.search(r"(back|rear|reverse)", f.stem, re.I):
        shutil.copy2(f, out / "back.png")
        found["cards"].append("back.png")
        continue
    # 大图归到 table（桌面/背景/按钮），小图归 unknown 供人工挑
    if max(f.stat().st_size, 0) > 200_000:
        shutil.copy2(f, out / "table" / f.name)
    else:
        found["unknown"] += 1

manifest = {
    "cards": sorted(set(found["cards"])),
    "missing_cards": sorted({f"{r}{s}.png" for r in RANKS.values() for s in "shdc"} - set(found["cards"])),
    "images_seen": found["images"],
    "audio_seen": found["audio"],
    "small_images_unclassified": found["unknown"],
}
(out / "manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False))
print(json.dumps(manifest, indent=2, ensure_ascii=False))
PY

say "同步到服务器（可选）：tools/sync-mirror.sh 只同步 nai-app，这份单独 rsync"
echo "rsync -a mirror/poker-assets/ root@\$OWL_IP:/opt/owl/nai-mirror/poker-assets/"
