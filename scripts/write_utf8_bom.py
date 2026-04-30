#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
指定したテキストファイルの先頭に **UTF-8 BOM** (`EF BB BF`) を付ける。

## 用途

Codex GUI / Windows メモ帳 / 一部のエディタは、UTF-8 でも BOM が無いと CP932
で読もうとして日本語が mojibake になる。Markdown ブリーフ等を
**外部ツールに食わせる前** に BOM を付与しておくと事故を回避できる。

Claude Code の Write tool は BOM を付けない。`Out-File -Encoding utf8BOM` 等の
PowerShell 経由でも代替可能だが、シェル間の引数渡しが面倒なのでスクリプトに
した。

## 使い方

```bash
python scripts/write_utf8_bom.py <target.md>
```

冪等: 既に BOM があれば no-op。

## 関連

- CLAUDE.md「Markdown / テキストファイルのエンコーディング」
- `scripts/check_ui_glyphs.py` (= UI 文字列の Unicode 文字確認)
"""

from __future__ import annotations

import sys
from pathlib import Path

UTF8_BOM = b"\xef\xbb\xbf"


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: python write_utf8_bom.py <file.md>", file=sys.stderr)
        return 2
    for arg in sys.argv[1:]:
        path = Path(arg)
        if not path.exists():
            print(f"error: {path} not found", file=sys.stderr)
            return 1
        data = path.read_bytes()
        if data.startswith(UTF8_BOM):
            print(f"ok: {path} already has UTF-8 BOM (no-op)")
            continue
        path.write_bytes(UTF8_BOM + data)
        print(f"ok: prepended UTF-8 BOM to {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
