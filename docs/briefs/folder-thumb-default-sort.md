# フォルダ代表サムネの既定を一覧の既定に合わせる

正本は [next-release-backlog.md](../next-release-backlog.md) **§2.17**。
**着手前に §2.17 を全部読むこと。**直す方向は利用者が決定済みで、代替案も検討済み。

## 1. 観測された失敗

利用者報告 (2026-08-22)。`00表紙.jpg` / `00表紙2.jpg` の 2 枚だけのフォルダで、
一覧の先頭は `00表紙.jpg` なのに、**フォルダタイルの代表サムネだけ `00表紙2.jpg` になる**。
実害は「表紙のつもりが空白ページがタイルに出る」。**どちらも既定値のまま**で起きる。

既定が 2 つに分かれているのが原因:

| 設定 | 既定 |
| --- | --- |
| 一覧の並び順 `sort_order` | ファイル名順 |
| 代表画像の選択基準 `folder_thumb_sort` | **番号順 (区切り無視)** |

代表画像の自動選定 ([thumb_loader.rs:2349](../../src/thumb_loader.rs:2349)
`resolve_folder_thumb_image_inner`) は**一覧の `sort_order` を見ない**。

番号順で反転する理屈は §2.17 に書いてある (`natural_sort_key` が拡張子を直前の文字塊へ
溶かすため `"表紙" < "表紙jpg"` になる)。**この理屈は直さない** — §4 参照。

## 2. やること

### 2.1 既定をファイル名順にする

`default_folder_thumb_sort()` ([settings.rs:5091](../../src/settings.rs:5091)) を
`SortOrder::FileName` にする。

**これだけでは新規インストールにしか効かない。**
[settings_db.rs:1550](../../src/settings_db.rs:1550) `write_settings_kv` が Settings の
全フィールドを毎回書くので、既存利用者の `settings_kv` には**全員** `Numeric` が入っており、
「保存されている」= 「利用者が選んだ」を**判別できない**。

### 2.2 一度きりの移行

`schema_meta` に一度きりのフラグ (例 `folder_thumb_sort_default_v2`) を置く。
既存の `bootstrap_complete` ([settings_db.rs:800](../../src/settings_db.rs:800)) /
`migrated_from_json_at` ([settings_db.rs:888](../../src/settings_db.rs:888)) と
**同じ `INSERT OR IGNORE` の形**で足す。新しい仕組みを作らない。

- **フラグが無い既存 DB**: 起動時に一度だけ `folder_thumb_sort` を `FileName` へ書き換え、
  フラグを立てる。
- **クリーンインストール**: bootstrap 時にフラグ**だけ**立て、値は触らない
  (既定が既に `FileName` なので書き換え不要)。

> ⚠️ **値とフラグを同じトランザクションで書くこと。**
> フラグだけ先に立って値の書き込みが永続化されないと、次回起動ではフラグがあるので
> 移行が走らず、**利用者は番号順のまま取り残される**。「保存は後で誰かがやる」に依存しない。

### 2.3 一度だけ戻ることを告知する

意図して「番号順」を選んでいた利用者も 1 回だけ戻る (§2.1 のとおり区別できない)。
[version_highlights.rs](../../src/version_highlights.rs) の `TABLE` に **`3.2.0`** の
`must_read` として載せ、**戻し方** (環境設定 → フォルダ → 「代表画像の選択基準」) を明示する。

- 内部用語を出さない (CLAUDE.md「マニュアル・製品ページの記述方針」)。
- 追加後 `cargo test --lib version_highlights::` が通ること。

### 2.4 マニュアルの既定表記

[settings.html:470](../../htdocs/mimageviewer/manual/settings.html:470) 付近の既定表記を
同時に直す。バージョン番号を本文に書かない。

## 3. 確認すること

- 代表サムネのキャッシュキーにソート順が入る (`auto-v2:numeric:d3:` → `auto-v2:filename:d3:`)
  ので選び直しは自動で走る。**旧キーのエントリが残って容量だけ増えないか**を確認し、
  分かったことを報告に書く。掃除が要るなら、要否の判断も含めて報告してから実装する。
- 移行が走った後、`folder_thumb_sort` を利用者が明示的に `Numeric` へ戻したら、
  **次の起動で再び `FileName` へ戻されないこと** (フラグが立っているので戻らないはず)。

## 4. やらないこと

- **`natural_sort_key` を変えない** ([ui_helpers.rs:979](../../src/ui_helpers.rs:979))。
  「番号順の natural key から拡張子を除く」案でも期待どおりになるが、この関数は
  一覧ソート ([app/folder_scan.rs:132](../../src/app/folder_scan.rs:132))、
  ファイル名スタック、ZIP ツリー、スマートフォルダが共有しており、
  **番号順を選んでいる全一覧の並びが変わる**。§2.17 で影響範囲が小さい方を選んである。
- 代表画像の選定が一覧の `sort_order` を直接見るようにする、という設計変更もしない
  (設定が 2 つあること自体は仕様)。
- 新しい設定項目を足さない。

## 5. 制約

- **時間窓・sleep・retry で吸収しない。**
- 移行は**冪等**であること。2 回走っても壊れない。
- **移行できなかったときに無言で先へ進まない。**失敗が観測できること
  (CLAUDE.md「バグ修正の一般原則」/ テストできない分岐を残さない)。
- リリース済みの永続ストアを触るので、後方互換を壊さない
  (CLAUDE.md「永続データ・スキーマ変更時の判断」)。

## 6. テスト

移行は純粋なロジックとして固定できるはず。少なくとも:

- **フラグ無し + `Numeric` の既存 DB** → 値が `FileName` になり、フラグが立つ。
- **フラグ有り + `Numeric` の DB** → 値が**変わらない** (利用者が戻した後を守る)。
- **クリーンインストール** → フラグが立ち、値は既定 `FileName` のまま。
- 2 回開いても結果が変わらないこと (冪等)。
- 値とフラグが**同時に**永続化されること (片方だけ残らない)。
- `00表紙.jpg` / `00表紙2.jpg` のフォルダで、既定の代表画像が `00表紙.jpg` になること
  (`resolve_folder_thumb_image_inner` を直接呼ぶ形でよい)。
- `version_highlights` に `3.2.0` の `must_read` が入り、テーブルがパースできること。

## 7. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `python scripts/check_ui_glyphs.py` が 0 件
- マニュアルの既定表記を更新
- **報告に、旧キャッシュキーの残留について分かったこと**を書く
