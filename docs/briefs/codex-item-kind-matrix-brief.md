# 調査ブリーフ: 機能 × 対象種別の対応表を作る

## 目的

mIV は同じ操作を対象の種別ごとにバラバラに扱っている。**現状を機械的に洗い出して
表にする**のがこのタスク。設計変更・コード修正は**しない**。

この表は後で 3 つに使う:

1. どこを揃えるかの判断材料
2. タグのページ対応 (★と同じ 2 行表示) の設計入力
3. **利用者向けマニュアルに載せる**「ZIP・PDF のページでできること / できないこと」

正本は [docs/context-menu-unification-plan.md](../context-menu-unification-plan.md) §4.1。

## 成果物

**`docs/item-kind-capability-matrix.md` を新規作成**し、そこに表を書く。
このファイル以外は変更しない (コードも他の docs も触らない)。

## 表の形

### 行 = 機能

- レーティング (★)
- タグ
- 削除 (ゴミ箱 / 完全削除)
- コピー・移動 (ファイル操作、パスのコピー含む)
- 外部ツール起動 (本ブランチで追加した `ExternalTool`)
- スマートフォルダの対象になるか
- 検索の対象になるか

過不足があれば足してよい。足したら理由を書く。

### 列 = 対象種別

`GridItem` の variant を根拠にする ([src/grid_item.rs](../../src/grid_item.rs))。

- 実ファイル (Image / Video / Audio)
- フォルダ (Folder)
- コンテナファイル (ZipFile / PdfFile)
- **ZIP 内ページ (ZipImage)**
- **PDF ページ (PdfPage)**
- Stack
- ZipDir

### セルの値

次の 4 つに分類する。**「黙って何もしない」を独立の値として必ず区別すること** —
これを可視化するのが今回の主目的。

| 値 | 意味 |
| --- | --- |
| 対応 | その種別に対してその機能が効く |
| 拒否 | 明示的に断る (無効化 / トースト / メニューに出さない)。**どう断るかも書く** |
| コンテナへ寄せる | 利用者はページを指したつもりだが、実際は親の ZIP / PDF に作用する |
| **無反応** | メニューには出る (または操作はできる) が、実行しても何も起きない・何も言わない |

### グリッドとフルスクリーンで違う場合

**分けて書く。** 例: タグはフルスクリーンではコンテナへ寄り、グリッドでは
何も起きない ([src/tag_ops.rs:21-40](../../src/tag_ops.rs:21))。この種の食い違いが他にも
あるかを探すのがこの調査の価値。

### 「保存先」列

各機能について、**どこに保存されるか**と**ファイル自身を書き換えるか**を書く。

⚠ **ここは間違えやすいので、必ずコードで確認すること。** 私 (Claude) は一度
「タグは XMP `dc:subject` に書かれる」と書いて誤った。それは **v1.0 の旧仕様**。
確認済みの現状は次のとおりだが、**これも鵜呑みにせず裏を取ること**:

- タグの正本は `tags.db`。通常のタグ操作は**ファイルを一切書き換えない**
- `xmp_writer::apply_tag_op` は**呼び出し元が無い** (旧 XMP 取り込みは本ブランチで削除済み)
- ファイルを書く唯一の経路は ★ の `xmp:Rating`
  ([src/rating_write_worker.rs](../../src/rating_write_worker.rs))
- その ★ 書き込みも `write_rating_to_xmp` が ON のときだけで、**既定 OFF**。
  対象は `rating_xmp_target_for_idx` ([src/app.rs](../../src/app.rs) 内) が返すもの =
  実ファイルの JPEG / PNG / WebP で製本ページでないもの
- タグのサイドカーミラー (`mimageviewer.dat`) は実ファイルにしか作れない
  (`tag_write_worker::sidecar_target_for_real_file`)。`tag_sidecar_backup_enabled` は既定 OFF

## 根拠の書き方

**全セルに `file.rs:line` の根拠を付ける。** 根拠が見つからず推測になるセルは
値を書かず `要調査` とし、何が分からなかったかを書く。**推測で埋めない。**

## 特に見てほしい既知の食い違い

すでに分かっているもの。表の中で整合が取れているか確認してほしい。

- ★ は `RatingItemKind::ZipImage` = 6 / `PdfPage` = 7 を持ち**ページ単位で付く**
  ([src/rating_db.rs:22](../../src/rating_db.rs:22))
- タグは `GridItem::ZipImage` / `PdfPage` を `zip_path` / `pdf_path` へ**フォールバック**する
  ([src/tag_ops.rs:29](../../src/tag_ops.rs:29))
- `GridItem::file_operation_path()` / `drag_source_path()` は ZipImage / PdfPage を
  **除外**するが、`is_checkable()` は**含む** ([src/grid_item.rs:270-318](../../src/grid_item.rs:270))
- ★ 横断一覧 (「場所▼」→「レーティング ▸ ★N」) は ZipImage / PdfPage を
  **ページのまま復元する** ([src/rating_view.rs:191-211](../../src/rating_view.rs:191))。
  つまり実ファイルとページが**同じ一覧に混ざる**

## 最後に

表の下に **「食い違いの一覧」** を節として書く。各項目は
「どの機能とどの種別で」「利用者から見て何が起きるか」「根拠の file:line」の 3 点。
**直し方の提案は書かなくてよい** (それは表を見てから決める)。
