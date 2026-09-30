# EffeTune 説明書の再生成

日本語スナップショットだけを使用する、ネットワーク不要の変換器。
生成 HTML は編集せず、`scripts/gen-effetune-docs.py` またはこのフォルダーの
訂正ルールを変更する。

```powershell
python scripts/gen-effetune-docs.py --version v0.11.1
python scripts/test_gen_effetune_docs.py
python scripts/gen-effetune-docs.py --version v0.11.1 --check
python scripts/gen-sitemap-xml.py
```

`vendor/effetune-mixwright/VERSION` がある場合は、その値が `--version` より優先する。
スナップショットと同梱版の対応は `SOURCE.md` でも検証する。

## 出力とリンク

- `htdocs/mimageviewer/manual/effetune/index.html`: 同梱版の目次への入口。
  meta refresh、canonical、表示リンクを持ち、noindex のためサイトマップには入らない。
- `<version>/<category>.html`: カテゴリの導入・共通操作・プラグイン一覧。
- `<version>/<category>-<heading-id>.html`: 一覧で指定された各プラグインの説明。
- バス機能と Visualizer は単一ページ。版ごとの目次、ライセンス、画像も生成する。

カテゴリの `プラグイン一覧` のリンクと実際の h2 を照合する。一覧以降の h2 が
未掲載、一致しない、重複している場合は失敗する。導入の h2 は一覧までに置く。
前後ナビゲーションと目次・サイドバーの順序は一覧の順序を使う（本文順ではない）。
各見出しの ID はカテゴリ全体で割り当ててから分割する。同名の小見出しに付く
`-1` なども維持する。旧 `plugins/eq.md#15band-geq` や `#15band-geq` は
`eq-15band-geq.html`、小見出しへのリンクは所有する個別ページと元のフラグメントに
変換する。カテゴリ導入へのリンクはカテゴリページに残す。
非掲載 Markdown へのリンクは従来どおりテキストにする。外部リンクは維持し、
`/dsp/...` などは上流サイトを指す。

`--check` は版フォルダー内のファイルに加え入口ページの欠落・古い内容も検出する。
入口を切り替えても、以前の版フォルダーは変更しない。予期しないファイルがある場合、
通常生成も勝手に削除せず失敗する。
生成 HTML には `.gitattributes` の `effetune/**/*.html` の LF 指定が適用される。

## 訂正ルールと分割

各 JSON のファイル名は上流ページの stem に対応する（`eq.json` など）。
`selection.json` はページ採否の完全な台帳。未知のページが増減したら生成を止める。

訂正はカテゴリ全体の HTML に適用してから分割する。このため既存の
`heading_id`、`scope_heading_id`、段落・インラインの完全一致アンカーは移行不要。
各アンカーは一意でなければ失敗する。更新で見出しや本文が変わった場合に、
訂正を黙って省略しない。訂正で削除された小見出しをリンクが参照しても失敗する。

2026-10-01 の分割で追加・変更したルール:

- `control.json`: 原文には一覧がないため、`section` 見出しの直前に明示的な
  一覧を挿入する `insert_before_heading` ルールを追加した。
  見出しが変わればアンカー検証で止まり、未掲載のプラグインが増えても照合で止まる。
- `basics.json`、`spatial.json`: カテゴリ先頭のステレオ制約注記に
  `repeat_on_plugins: true` を追加した。注記はカテゴリにも残し、各個別ページの
  タイトル直後にも配置する。この指定はカテゴリ h1 の `insert_note` に限定する。
  ほかの訂正ルールとスコープは変更していない。

個別ページには共通操作を読めるカテゴリへのリンクも置く。
公開するページ構成や訂正内容が変わったら、テストとサイトマップを再生成・検証する。
