# 1 枚の書き出し (Ctrl+E) の元の縮小画像の扱い 設計 (§1.888)

状態: 設計中 (2026-11-25)。独立設計レビュー前。実装なし。
関連: [一括エクスポートの設計](batch-export-plan.md) §3.6 (EXIF の共通処理)・§3.7 (縮小画像の警告)・§13 Q3、
[バックログ §1.888](next-release-backlog.md)。

## 1. 目的と範囲

フルスクリーンの 1 枚の書き出し (Ctrl+E、`export_dialog::run_export`) で「AI プロンプト / EXIF を保持」が ON
(既定) のとき、消しゴム・隠蔽などで隠した部分が、転記した元の情報の縮小画像に残り得る。一括の書き出し (§1.362) と
同じ扱いにそろえる。

- **EXIF の縮小画像 (IFD1) は消す。**
- **XMP の縮小画像があれば書き出しの後に目立つ形で警告する。** MakerNote は隠す編集がある出力だけ警告する
  (一括と同じ規則)。
- 利用者決定 (2026-11-25、統括経由「5 件で進める」)。統括の指示: リリース済みの挙動の変更なので、
  「メタデータを保持」の**他の中身 (EXIF・ICC・プロンプト等) は変えない**、**既定値は変えない**。

範囲外 (今と同じ): 見開き合成・PDF・形式が変わる書き出し (どれも元の情報を転記しない)、
`books.rs` の保存 (転記しない)、一括の書き出し (§1.362 のまま。下の §3.4 の文言だけ共通)。

## 2. 今のコード (master `206768a81`、コード読み)

1 枚の書き出しは `SaveOptions { include_metadata, caller_applied_orientation: true, batch_metadata: false }` で
`save_image_with_metadata` を呼ぶ。形式ごとの転記:

| 形式 | 転記するもの (今) | EXIF の処理 (今) |
| --- | --- | --- |
| JPEG | すべての APP1 (EXIF・XMP・拡張 XMP)。ICC (APP2) は転記しない | `reset_exif_orientation_in_app1` で Orientation を 1。IFD1 はそのまま |
| PNG | tEXt / iTXt / zTXt だけ (`eXIf`・`iCCP` は転記しない) | EXIF を転記しないので無し |
| WebP | `ICCP` (色空間を問わず)・`EXIF`・`XMP ` | 何もしない (Orientation も IFD1 もそのまま) |

ダイアログの既定は `settings.export_embed_metadata` (既定 ON)。進捗と結果は一括と同じ `ExportPending` と
`ui_dialogs::export_batch::draw_progress_window`・`App::poll_export_pending` で扱い、警告の画面
(§1.362 §3.7) は既に共通。1 枚の worker が `ExportEvent::ThumbnailNotice` を送らないだけである。

## 3. 変更

### 3.1 転記の型をまとめる (`SaveOptions`)

今の `batch_metadata: bool` は「一括用の転記」と「縮小画像の処理・検出」の 2 つの意味を持つ。1 枚の書き出しが
後者だけを使うので、転記の種類を 1 つの enum にする (bool の組み合わせを足さない)。

```rust
pub enum MetadataProfile {
    /// 1 枚の書き出し。転記する種類は §2 の表のまま。EXIF の IFD1 を外し、縮小画像を検出する。
    Single,
    /// 一括の書き出し (§1.362 §3.6 のまま)。
    Batch,
}
// SaveOptions: `batch_metadata: bool` を `metadata_profile: MetadataProfile` (既定 Single) に置き換える。
```

- 転記しないとき (`include_metadata == false`) は今と同じで、どちらの値でも何も転記・検出しない。
- 今 `batch_metadata: false` の呼び出し元は `export_dialog::run_export` と、転記しない `books.rs`、テストだけ。
  したがって「縮小画像を処理しない転記」は製品の経路から無くなる。古い転記の関数は Single の中で、
  EXIF を読めないときの扱い (§3.2) にだけ残る。

### 3.2 EXIF (Single)

- **JPEG の EXIF APP1・WebP の `EXIF`**: §1.362 の `clean_exif` (TIFF を読み、IFD1 を外して縮小画像の範囲を 0 で埋め、
  MakerNote の有無を返す) を同じく通す。PNG は EXIF を転記しないので対象外。
- **Orientation は今の扱いのまま**: `clean_exif` に `reset_orientation: bool` を足す。
  - JPEG: `caller_applied_orientation` の値 (1 枚の書き出しは `true`、今と同じく 1 にする)。
  - WebP: `false` (今は WebP の Orientation を書き換えていない。変えると統括の指示に反するので変えない。
    書き換えないときは、Orientation の形が不正なことを読めない理由にしない)。
  - Batch は今と同じく常に `true`。
- **TIFF として読めない EXIF (`clean_exif` が `None`)**: Single では**今と同じ処理で転記し** (JPEG は
  `reset_exif_orientation_in_app1`、WebP はそのまま)、警告の種類「EXIF (調べられませんでした)」
  (`ThumbnailNoticeKinds::exif_uninspectable`) を立てる。一括は今どおり転記しない (この種類を立てない)。
  - 理由: 一括と同じく外すと、リリース済みの出力から EXIF が丸ごと消える (統括の指示に反する)。黙って残すと
    縮小画像が漏れ得る。XMP の「調べられない → 警告」と同じ考え方で警告側に倒す。
  - 読めない例: バイト順の印が無い・IFD の循環・範囲外のオフセット・構造の重なり。通常のカメラの EXIF は読める見込み
    (実ファイルでの確認はしていない)。
- EXIF のほかの中身・画素数タグ・MakerNote 本体は変えない (一括と同じ)。

### 3.3 変えないもの (Single)

- JPEG に ICC (APP2) を足さない。PNG に `eXIf`・`iCCP` を足さない。WebP の `ICCP` は色空間を問わず今どおり残す
  (RGB 以外を外す一括の規則は使わない)。
- XMP・テキストチャンク・AI プロンプトは解釈せずそのまま残す (今と同じ)。XMP の縮小画像は書き直さない (§1.362 §13 Q3)。
- `export_embed_metadata` の既定値 (ON)・チェックボックスの文言・永続化の規則。設定のスキーマは変えない。
- アニメーション WebP の拒否、形式が変わるときに転記しない規則。

### 3.4 縮小画像の検出と警告

- **検出** (worker、エンコードの中、追加の読み込みなし): §1.362 §3.7 の表と同じ。
  - XMP の縮小画像: 常に。JPEG の標準・拡張 XMP、PNG の `XML:com.adobe.xmp` テキストチャンク (展開の上限と
    「調べられない → 警告」も同じ)、WebP の `XMP `。
  - MakerNote: 「隠す編集」がある出力だけ。1 枚の書き出しでは、合成するページの編集 snapshot の
    `erase` / `conceal` / `export_crop` / `comic` / `local_adjust` のどれかが `Some`、**または**その出力の
    `ExportEntry::conceal_preset` が `Some` (隠蔽プリセットの出力 `_1`〜`_4`)、**または** `crop_override` が `Some`
    (SNS 分割の枠)。出力ごとに決める (同じ書き出しの `_0` と `_1` で違い得る)。
  - 元の情報を転記しない出力 (見開き・PDF・形式変更・OFF) は検出しない。
- **送り方**: `run_export` は、その出力の `Completed` を送った直後に、警告があれば
  `ExportEvent::ThumbnailNotice { label, path, kinds }` を送り、ログに
  `export thumbnail metadata: <出力パス>: <kinds>` を 1 行残す (一括の行は今どおり `batch export ...`)。
  `save_image_with_metadata` は保存結果の警告を返す形にする (`encode_image_with_metadata_report` と同じ値)。
- **画面**: 共通の進捗ダイアログ (§1.362 §3.7) がそのまま出す。完了で警告があれば、今の「成功 1 件なら Explorer で
  示して閉じる」ではなく、警告の画面を残す (共通の `poll_export_pending` の今の規則どおり、新しい分岐は足さない)。
  - 種類の表示に「EXIF (調べられませんでした)」を足す。
  - 説明文の最後の文を、1 枚と一括の両方に合う言い方にする:
    「公開する前に確認するか、元の情報を残さない設定 (一括では『元の情報を残す』、1 枚の書き出しでは
    『AI プロンプト / EXIF を保持』をオフ) で書き出し直してください。」
- **書き出しの前の案内**: 1 枚のダイアログのチェックボックスに説明 (hover) を付ける。
  「EXIF の縮小画像は外します。XMP などに縮小画像が残っていそうなときは、書き出しの後で知らせます
  (隠した部分が縮小画像に残ることがあります)。」 行を足さないので、ダイアログの高さは変わらない。

## 4. 簡素化と、まれな失敗の割り切り

- 警告の画面・終了状態・取消は §1.362 で作った共通の部品をそのまま使い、1 枚用の画面や状態を足さない。
- 書き出しの前に元ファイルを調べない (一括と同じ理由、§1.362 §3.7「時点」)。
- XMP の縮小画像を書き直す仕組みは作らない (利用者決定)。
- まれな失敗は割り切り (ログ + 通知)。読めない EXIF は §3.2 の警告で知らせ、回復の仕組みは足さない。

## 5. テスト

- `save_with_metadata` の単体 (Single):
  - JPEG: IFD1 付きの EXIF → 出力の IFD0 の次の IFD が 0、縮小画像の範囲が 0、Orientation が 1。ICC (APP2) は
    出力に無い (元に RGB の ICC があっても)。XMP の APP1 はバイト列が同じ。
  - WebP: IFD1 付き・Orientation 6 の EXIF → IFD1 が外れ、Orientation は 6 のまま。GRAY の `ICCP` も残る。
  - PNG: テキストチャンクが今と同じ。`eXIf`・`iCCP` が出力に無い。
  - 3 形式とも xmpGImg の XMP で `xmp` が立つ。名前空間の無い XMP では立たない。
  - MakerNote: `thumbnail_notice_for_hidden_edits` の有無で `maker_note` が変わる。
  - 読めない EXIF (例: IFD の循環): 出力が今の処理のバイト列と同じで、`exif_uninspectable` が立つ。
  - Batch は今のテストのまま通り、`exif_uninspectable` を立てない。
- `tests/export_integration.rs` (1 枚の worker):
  - IFD1 と xmpGImg を持つ JPEG を書き出す → `Completed` の後に `ThumbnailNotice` (xmp)、出力に IFD1 が無い。
  - 隠蔽プリセットの出力 (`conceal_preset: Some`) と MakerNote → その出力だけ `maker_note`。
  - 「保持」OFF・形式変更では `ThumbnailNotice` を送らない。
- UI: 警告の一覧に新しい種類の文言が出る (既存の一括の警告テストに足す)。`python scripts/check_ui_glyphs.py`。
  スナップショットに説明文が写っていれば更新する。

## 6. 文書の更新 (実装と同時)

- マニュアル `htdocs/mimageviewer/manual/export.html`: 1 枚の書き出しの「メタデータ保持」の行と説明に、
  EXIF の縮小画像を外すこと、XMP などに残りそうなときは書き出しの後に知らせることを書く。一括の段落の
  「『元の情報を残す』をオフ」は §3.4 の説明文に合わせる。
- [batch-export-plan.md](batch-export-plan.md) §3.6 の「単ページの出力は変えない」の項を本書への参照に直す。
- [docs/README.md](README.md) の索引に本書を足す。バックログ §1.888 の状態。
- 永続データ: 変えない (設定・DB のスキーマ変更なし)。

## 7. 独立設計レビューの記録

(まだ受けていない)
