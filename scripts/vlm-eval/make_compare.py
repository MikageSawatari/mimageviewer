"""Build a local comparison page from V0 results (docs/local-vlm-v0-procedure.md).

The page shows each image with every condition's caption and keywords side by side.
With --blind, condition names become A/B/C and the column order is shuffled per image;
the key is written to a separate file. The page stays local: it embeds the user's
images by relative path and must not be published.

    python -I make_compare.py --images ..\\inputs\\1024 --out compare.html C1=r1.jsonl C2=r2.jsonl ...
"""

import argparse
import html
import json
import os
import pathlib
import random


def load(path):
    rows = {}
    for line in open(path, encoding="utf-8"):
        r = json.loads(line)
        rows[r["image"]] = r
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--images", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--blind", action="store_true")
    ap.add_argument("--seed", type=int, default=20261102)
    ap.add_argument("runs", nargs="+", help="LABEL=results.jsonl")
    a = ap.parse_args()

    runs = []
    for spec in a.runs:
        label, path = spec.split("=", 1)
        runs.append((label, load(path)))
    out_dir = pathlib.Path(a.out).resolve().parent
    images = sorted({name for _, rows in runs for name in rows})
    rng = random.Random(a.seed)
    letters = "ABCDEFGH"
    key = {}

    parts = []
    for name in images:
        order = list(range(len(runs)))
        if a.blind:
            rng.shuffle(order)
        img_rel = os.path.relpath(pathlib.Path(a.images).resolve() / name, out_dir).replace("\\", "/")
        cells = []
        for col, ri in enumerate(order):
            label, rows = runs[ri]
            shown = letters[col] if a.blind else label
            if a.blind:
                key.setdefault(name, {})[shown] = label
            r = rows.get(name, {})
            if r.get("json_ok"):
                cap = r.get("caption") or ""
            elif r.get("error"):
                cap = "(失敗: " + r["error"] + ")"
            else:
                cap = "(失敗: 説明文が出力されませんでした)"
            kws = r.get("keywords") or []
            meta = "%.1f 秒" % r["wall_s"] if "wall_s" in r else ""
            radios = "".join(
                '<label><input type="radio" name="%s|%s" value="%s"> %s</label>'
                % (html.escape(name), shown, v, t)
                for v, t in (("good", "良い"), ("ok", "使える"), ("bad", "不可"))
            )
            cells.append(
                '<div class="cell"><div class="lab">%s <span class="meta">%s</span></div>'
                '<p>%s</p><p class="kw">%s</p><div class="rate">%s</div></div>'
                % (shown, meta, html.escape(cap or ""), html.escape("、".join(map(str, kws))), radios)
            )
        parts.append(
            '<section><h2>%s</h2><div class="row"><img src="%s" loading="lazy">%s</div>'
            '<textarea name="%s|note" placeholder="気づいた点"></textarea></section>'
            % (html.escape(name), html.escape(img_rel), "".join(cells), html.escape(name))
        )

    page = """<!doctype html><html lang="ja"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>VLM 比較</title>
<style>
:root{--bg:#fff;--fg:#111;--sub:#666;--line:#ddd;--card:#f6f6f6}
@media (prefers-color-scheme: dark){:root{--bg:#161616;--fg:#eee;--sub:#aaa;--line:#333;--card:#222}}
body{background:var(--bg);color:var(--fg);font:15px/1.6 system-ui,sans-serif;margin:0 16px 80px}
section{border-top:1px solid var(--line);padding:12px 0}
h2{font-size:14px;color:var(--sub);margin:4px 0}
.row{display:flex;gap:12px;align-items:flex-start;flex-wrap:wrap}
img{max-width:360px;width:100%;height:auto;border-radius:4px}
.cell{flex:1 1 240px;background:var(--card);border-radius:6px;padding:8px 10px}
.lab{font-weight:600}.meta{font-weight:400;color:var(--sub);font-size:12px}
.kw{color:var(--sub);font-size:13px}.rate label{margin-right:10px;white-space:nowrap}
textarea{width:100%;min-height:2.5em;margin-top:6px;background:var(--bg);color:var(--fg);border:1px solid var(--line)}
#bar{position:fixed;bottom:0;left:0;right:0;background:var(--bg);border-top:1px solid var(--line);padding:8px 16px}
</style></head><body><h1>VLM 比較</h1>
<p>画像ごとに各列を「良い / 使える / 不可」で評価し、気づいた点があれば書いてください。最後に「結果をコピー」を押して会話に貼ってください。</p>
""" + "".join(parts) + """
<div id="bar"><button id="copy">結果をコピー</button> <span id="msg"></span></div>
<script>
const K='vlm-compare-'+document.title;
function collect(){const o={};document.querySelectorAll('input:checked').forEach(e=>o[e.name]=e.value);
document.querySelectorAll('textarea').forEach(e=>{if(e.value)o[e.name]=e.value});return o}
function save(){try{localStorage.setItem(K,JSON.stringify(collect()))}catch(e){}}
try{const s=JSON.parse(localStorage.getItem(K)||'{}');for(const[k,v]of Object.entries(s)){
const r=document.querySelector('input[name="'+CSS.escape(k)+'"][value="'+v+'"]');if(r)r.checked=true;
const t=document.querySelector('textarea[name="'+CSS.escape(k)+'"]');if(t)t.value=v}}catch(e){}
document.addEventListener('change',save);document.addEventListener('input',save);
document.getElementById('copy').onclick=async()=>{const txt=JSON.stringify(collect());
try{await navigator.clipboard.writeText(txt);document.getElementById('msg').textContent='コピーしました'}
catch(e){prompt('この内容をコピーしてください',txt)}};
</script></body></html>"""
    pathlib.Path(a.out).write_text(page, encoding="utf-8")
    if a.blind:
        key_path = pathlib.Path(a.out).with_suffix(".key.json")
        key_path.write_text(json.dumps(key, ensure_ascii=False, indent=1), encoding="utf-8")
        print("key:", key_path)
    print("page:", a.out, "images:", len(images))


if __name__ == "__main__":
    main()
