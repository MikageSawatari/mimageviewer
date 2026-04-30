#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
mIV の UI 文字列に「フォント未収録 → 豆腐 (□) 化」しがちな Unicode 文字が
含まれていないか scan する lint。

## 背景

mIV は `setup_fonts` で Yu Gothic Medium / Meiryo / MS Gothic を
プライマリ proportional font に設定する (= src/main.rs)。これらは
日本語 + 基本 Latin + 一部記号は持つが、Misc Symbols ブロックの
✕ (U+2715) 等の "X 系記号" や絵文字は持たないことが多い。

egui のフォント fallback (NotoEmoji) は機能する**こともある**が、
プライマリ font が "tofu glyph" を返すと fallback まで到達せず
□ で表示される。各実環境で挙動がブレるので避けるのが安全。

## 安全な代替

| 危険 (フォント依存) | 安全 (Latin-1 / ASCII)            |
|---------------------|------------------------------------|
| ✕ (U+2715)          | × (U+00D7 multiplication sign)     |
| ✗ (U+2717)          | × もしくは "NG"                    |
| ✓ (U+2713)          | "OK" / "[v]"                       |
| 🎚 (U+1F39A)         | "VST" 等のテキスト                 |
| 🟢⚫🔴 (status emoji)  | "[ON]" "[OFF]" 等                  |

矢印 (↑ U+2191 / ↓ U+2193) は Yu Gothic に含まれており OK。

## 使い方

    python scripts/check_ui_glyphs.py

src/ 配下の .rs ファイルから疑わしい文字を含む行を列挙する。
コメント行 (`//` 以降) と SKIP 印 (`// glyph-lint:skip`) は除外。
何も出なければ exit 0、見つかれば exit 1 で CI 失敗。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# Windows の cp932 stdout で絵文字を print すると UnicodeEncodeError で死ぬので
# stdout を UTF-8 に強制する (= ターミナルが ASCII しか出せないなら ? に置換)。
if hasattr(sys.stdout, "reconfigure"):
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass

# 「**ユーザー報告で実際に tofu 化したと確認された**」 Unicode 文字。
# 推測で追加しない (Yu Gothic に含まれる場合もあるため誤検出が多い)。
# 新たに tofu 報告があった文字を追加していく。
DANGEROUS = {
    "✕": "✕ U+2715 MULTIPLICATION X (2026-04 報告: 黒パネルの GUI ボタン)",
    "✖": "✖ U+2716 HEAVY MULTIPLICATION X (✕ と同じ理由で危険)",
    # ✗ ✘ ✓ などは現状は未確認だが Yu Gothic に含まれず tofu 化する報告が
    # 出た場合はここに追加する (= 安全側として `×` U+00D7 で代替推奨)。
}
# 個別に「mIV ユーザー環境で tofu になった」と確認された絵文字。
# 一般の絵文字 (📁 📌 等) は egui の NotoEmoji fallback で OK ケースが多いため、
# 範囲一掃ではなく **実際にユーザー報告があったもののみ**ここに登録する。
DANGEROUS_EMOJI = {
    0x1F39A,  # 🎚 LEVEL SLIDER (2026-04 報告)
}
SKIP_MARKER = "glyph-lint:skip"


def is_emoji(c: str) -> bool:
    return ord(c) in DANGEROUS_EMOJI


def scan_file(path: Path) -> list[str]:
    """疑わしい文字を含む行を `path:line:reason` で返す。"""
    findings: list[str] = []
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return findings  # 非 UTF-8 はスキップ
    for lineno, line in enumerate(text.splitlines(), 1):
        if SKIP_MARKER in line:
            continue
        # コメントだけの行はスキップ (= ドキュメント中の説明用記述)
        stripped = line.lstrip()
        if stripped.startswith("//"):
            continue
        for ch in line:
            if ch in DANGEROUS:
                findings.append(
                    f"{path}:{lineno}: {DANGEROUS[ch]} -> {line.strip()[:120]}"
                )
                break
            if is_emoji(ch):
                findings.append(
                    f"{path}:{lineno}: emoji U+{ord(ch):04X} -> {line.strip()[:120]}"
                )
                break
    return findings


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    src = root / "src"
    if not src.exists():
        print(f"src/ not found at {src}", file=sys.stderr)
        return 2
    all_findings: list[str] = []
    for path in sorted(src.rglob("*.rs")):
        all_findings.extend(scan_file(path))
    if not all_findings:
        print("ok: no dangerous glyphs found in UI strings.")
        return 0
    print("error: dangerous glyphs found (likely tofu in Yu Gothic):")
    for f in all_findings:
        print(f"  {f}")
    print()
    print("Replace with safer characters. See scripts/check_ui_glyphs.py docstring")
    print("for the dangerous-vs-safe table. Add `// glyph-lint:skip` to suppress.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
