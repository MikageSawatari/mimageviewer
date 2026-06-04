#!/usr/bin/env python3
"""Generate comic_lab verification samples for vertical-text / IVS / 縦中横.

Outputs (regenerable; edit this script, not the outputs):
  - docs/comic-lab-sample-text.md       : copy-paste strings + expected results
  - docs/comic-lab-sample-scene.comic.json : a loadable scene (vertical text blocks)

The samples embed real codepoints that are hard to type by hand:
  - IVS (ideographic variation selectors) U+E0100.. select name-kanji variants.
    辻 葛 芦 茨 鄭 鞄 + U+E0100 visibly change in Yu Gothic / Meiryo / MS Gothic.
  - U+3099 combining voiced sound mark (decomposed/NFD dakuten).

The scene uses font_key="" so the lab falls back to its eagerly-loaded default JP
font (Yu Gothic / Meiryo / MS Gothic) — all of which carry the IVS used here.
"""

import json
import os

VS17 = "\U000E0100"  # ideographic variation selector 1 (IVS)
DAKUTEN = "゙"   # combining katakana-hiragana voiced sound mark

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
DOCS = os.path.join(REPO, "docs")

# ---- shared building blocks for the .comic.json scene --------------------

WHITE = {"r": 255, "g": 255, "b": 255, "a": 255}
BLACK = {"r": 0, "g": 0, "b": 0, "a": 255}


def text_obj(oid, pivot, text, *, markup=False, size=44.0):
    """One vertical standalone-text annotation (white fill, black 袋文字)."""
    return {
        "id": oid,
        "enabled": True,
        "z": oid,
        "pivot": list(pivot),
        "rotation_rad": 0.0,
        "kind": {
            "Text": {
                "text": text,
                "font_key": "",  # -> lab default JP font
                "size_px": size,
                "color": WHITE,
                "orientation": "Vertical",
                "align": "Start",
                "markup_enabled": markup,
                "outline": {"color": BLACK, "width_px": 3.0},
            }
        },
    }


# Each block is placed top-left at `pivot`; vertical columns grow rightward and
# downward, so pivots step left-to-right with gaps.
scene_objects = [
    # 1) 約物 (punctuation): 「」 vertical brackets, …… stacked, 。 upper-right.
    text_obj(1, (120.0, 80.0), "「あれ……？」\nうん。"),
    # 2) 縦中横 (tate-chu-yoko): 12 / 25 become horizontal-in-cell; single 3 upright;
    #    !? combines.
    text_obj(2, (300.0, 80.0), "12月25日\n午後3時\nえっ!?"),
    # 3) IVS 人名異体字: right column = standard glyphs, left column = IVS variants.
    text_obj(3, (520.0, 80.0), "辻葛芦\n" + f"辻{VS17}葛{VS17}芦{VS17}"),
    # 4) マーカー記法: {..}=横倒し (LOVE rotates), [..]=縦中横 (AI in one cell).
    text_obj(4, (700.0, 80.0), "{LOVE}な[AI]", markup=True),
    # 5) 結合文字: row0 precomposed がぎ, row1 decomposed か+U+3099 / き+U+3099.
    #    Both must render as one cell each (the dakuten must not split off).
    text_obj(5, (820.0, 80.0), "がぎ\n" + f"か{DAKUTEN}き{DAKUTEN}"),
]

sidecar = {"schema_version": 1, "objects": scene_objects}

scene_path = os.path.join(DOCS, "comic-lab-sample-scene.comic.json")
with open(scene_path, "w", encoding="utf-8") as f:
    json.dump(sidecar, f, ensure_ascii=False, indent=2)
    f.write("\n")

# ---- the copy-paste reference doc ----------------------------------------

md = f"""# comic_lab 縦書き検証サンプル

`tools/comic_lab` で縦書きの約物 / 縦中横 / 横倒し / 異体字(IVS) / 結合文字を
実機確認するためのサンプル。設計の正本は
[vertical-text-opentype-plan.md](vertical-text-opentype-plan.md)。

## 使い方 (どちらでも)

### A. シーンを丸ごと開く (おすすめ)
1. comic_lab で任意の画像を開く (大きめの画像が見やすい)。
2. その画像と同じフォルダに [comic-lab-sample-scene.comic.json](comic-lab-sample-scene.comic.json)
   を **`<画像ファイル名>.comic.json`** という名前でコピーする
   (例: `bg.png` を開くなら `bg.png.comic.json`)。
3. 画像を開き直すと 5 つの縦書きテキストが自動で読み込まれる。
   フォントは未指定 (`font_key=""`) なので lab の既定日本語フォントで描かれる。

### B. 1 行ずつ貼って試す
「テキスト追加」で縦書きテキストを作り、本文に下の文字列を貼り付ける。
**異体字 (IVS) と結合文字は不可視のセレクタを含む**ので、この .md をエディタで開いて
行ごとコピーすること (見た目では区別できないが、貼って縦書きにすると差が出る)。

## サンプルと期待結果

### 1. 約物 (句読点・括弧・三点リーダ)
```
「あれ……？」
うん。
```
- `「` `」` が**縦書き字形**(横倒しにならない)。
- `……` が縦に積まれる。`？` も縦向き。
- `。` がセルの**右上**に寄る (横書きのように中央・下に来ない)。

### 2. 縦中横 (tate-chu-yoko)
```
12月25日
午後3時
えっ!?
```
- `12` `25` が**1 セル内に横並び**(縦中横)。
- 単独の `3` は**正立**(縦中横にならない)。
- `!?` が 1 セルに合成される。`!!!!` のような同種連続は縦に積まれる(別途確認可)。

### 3. 異体字 (IVS・人名漢字) ★今回の対応ポイント
```
辻葛芦
辻{VS17}葛{VS17}芦{VS17}
```
- 2 列になり、**右列=標準字形 / 左列=異体字**(各字の 2 バイト目に U+E0100)。
- 左列の `辻` は しんにょうの点の数、`葛` は下部の形が右列と変わる
  (Yu Gothic / Meiryo / MS Gothic で確認可)。
- **重要**: 異体字セレクタが別セルに割れて空マスが出ない (= 今回の修正点)。
- フォントがその異体字を持たない場合は標準字形のまま + 空マスも出ない(悪化しない)。

### 4. マーカー記法 (横倒し / 縦中横)
記法 ON で:
```
{{LOVE}}な[AI]
```
- `{{LOVE}}` → **横倒し**(各文字を 90 度寝かせて縦に積む)。
- `な` → 正立。
- `[AI]` → **縦中横**(1 セルに横並び)。

### 5. 結合文字 (分解形の濁点)
```
がぎ
か{DAKUTEN}き{DAKUTEN}
```
- 上段 `がぎ` は合成済み(1 コードポイント)。
- 下段は `か`+U+3099 / `き`+U+3099 の**分解形(NFD)**。
- **両段とも同じ見た目・各 1 セル**になる(濁点が次のセルに分離して浮かない)。

## 補足
- IVS の見え方はフォント依存。確実に差を見たいときは Yu Gothic 系を選ぶ。
- このファイルとシーンは `python scripts/gen_comic_sample.py` で再生成できる。
"""

md_path = os.path.join(DOCS, "comic-lab-sample-text.md")
with open(md_path, "w", encoding="utf-8") as f:
    f.write(md)

print(f"wrote {scene_path}")
print(f"wrote {md_path}")
print(f"scene objects: {len(scene_objects)}")
