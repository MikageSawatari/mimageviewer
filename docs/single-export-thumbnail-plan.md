# 1 枚の書き出し (Ctrl+E) の元の縮小画像の扱い 設計 (§1.888)

状態: export-888 ブランチで実装済み、master 未統合・実機未確認 (2026-11-25)。44dcac70f の関連テストは設計担当が共有 runner で lib の `save_with_metadata::` / `export_batch::tests::` / `ui_dialogs::export_batch::tests::` / `export_dialog::` (139 件) と `export_integration` (23 件) の成功を確認。警告画面のスナップショットは再生成・目視確認済み。独立実装レビュー 3 回目 ACCEPT (§8)。
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
`books.rs` の保存 (転記しない)。一括の書き出しは §1.362 のままで、PNG の Raw profile の扱い (§3.2・§3.4) と
警告の説明文 (§3.4) だけが共通に変わる (一括は未リリース)。

## 2. 今のコード (master `206768a81`、コード読み)

1 枚の書き出しは `SaveOptions { include_metadata, caller_applied_orientation: true, batch_metadata: false }` で
`save_image_with_metadata` を呼ぶ。形式ごとの転記:

| 形式 | 転記するもの (今) | EXIF の処理 (今) |
| --- | --- | --- |
| JPEG | すべての APP1 (EXIF・XMP・拡張 XMP)。ICC (APP2) は転記しない | `reset_exif_orientation_in_app1` で Orientation を 1。IFD1 はそのまま |
| PNG | tEXt / iTXt / zTXt だけ (`eXIf`・`iCCP` は転記しない) | `eXIf` は無し。ただしテキストチャンクの「Raw profile」形式 (下) の EXIF はそのまま転記される |
| WebP | `ICCP` (色空間を問わず)・`EXIF`・`XMP ` | 何もしない (Orientation も IFD1 もそのまま) |

PNG のテキストチャンクには、キーワード `Raw profile type exif` / `Raw profile type APP1` / `Raw profile type xmp`
(大文字小文字を問わない) で EXIF・XMP を 16 進の文字列にして入れる形式がある (ImageMagick などが書く。
本文は「改行・種類名・改行・10 進の長さ・改行・16 進の数字 (途中に改行)」)。今の転記はこれらもそのまま残し、
§1.362 の検出 (`inspect_png_xmp`) は `XML:com.adobe.xmp` しか見ないので、一括でも 1 枚でも見落としている
(独立設計レビュー 1 回目の P1)。

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
  したがって「縮小画像を処理しない転記」は製品の経路から無くなる。Single は今の形式ごとの転記
  (`encode_jpeg_rgba` などの APP1 / テキスト / WebP チャンクの選び方と JPEG の Orientation の書き換え) をそのまま使い、
  そこに §3.2 の IFD1 の処理と §3.4 の検出を足す。

### 3.2 EXIF (Single)

- **JPEG の EXIF APP1・WebP の `EXIF`**: §1.362 の `clean_exif` (TIFF を読み、IFD1 を外して縮小画像の範囲を 0 で埋め、
  MakerNote の有無を返す) を同じく通す。PNG は EXIF を転記しないので対象外。
- **Orientation は今の扱いのまま**: `clean_exif` に `reset_orientation: bool` を足す。`clean_exif` は IFD0 だけでなく
  Exif IFD などの下位の IFD の Orientation もすべて書き換えるので、今の 1 枚の出力 (IFD0 の最初の Orientation だけ)
  と範囲が違う (独立設計レビュー 1 回目の P2)。
  - Single は常に `false` (Orientation に触れない。形が不正な Orientation を読めない理由にしない)。そのうえで JPEG だけ、
    今と同じく `caller_applied_orientation` なら `reset_exif_orientation_in_app1` を当てる (処理の順番は
    `clean_exif` の後。IFD1 を外しても IFD0 の位置は変わらない)。WebP は今と同じく Orientation を書き換えない。
  - Batch は今と同じく常に `true`。
  - したがって Single の EXIF の変化は「IFD0 の次の IFD を 0 にする・IFD1 が指す縮小画像の範囲を 0 で埋める」だけで、
    それ以外のバイトは今の出力と同じになる (テストで比べる)。
- **PNG の「Raw profile」の EXIF** (`Raw profile type exif` / `APP1`、§2): Single と Batch の両方で扱う
  (一括は未リリースなので、ここで合わせて直す)。
  - 本文を読む: tEXt はそのまま、zTXt と圧縮の iTXt は展開してから (展開の上限と合計の上限は XMP と共通の
    16 MiB / 32 MiB)。種類名の行・長さの行の後の 16 進の数字を、空白と改行を飛ばしてバイトにする。**空白を除いた 16 進の桁数が、長さの行の値のちょうど 2 倍で
    なければ読めない**とする (宣言より後ろに残った 16 進を調べ落とさないため。独立設計レビュー 2 回目の P2)。
    先頭が `Exif\0\0` ならその後を TIFF とする。`APP1` で先頭が XMP の見出しなら下の XMP として調べる。
  - `clean_exif` (Single は `false`、Batch は `true`) を通し、**変わったバイトに当たる 16 進の数字だけを元の位置で
    書き換える** (大文字小文字は元に合わせる)。長さは変わらないので、改行の位置・種類名・長さの行は元のまま。
    圧縮されていたチャンクは同じ圧縮の有無で作り直す (iTXt の言語・訳語のキーワードは元のまま)。CRC は計算し直す。
  - Raw profile の EXIF の処理は、XMP の縮小画像を見つけた後の検査の省略 (今の `transfer` の PNG の分岐は、
    `notice.xmp` が立つと以降のチャンクの XMP 検査を飛ばす) と独立させる。チャンクの順番に関係なく、すべての
    Raw profile の EXIF を処理する。
  - 読めないとき (形式が違う・16 進でない・桁数が長さと合わない・上限を超えた・`clean_exif` が `None`) は、下の
    「TIFF として読めない EXIF」と同じ (Single は残して警告、Batch はそのチャンクを転記しない)。
  - `eXIf` チャンクの扱い (Single は転記しない、Batch は §1.362 のまま) は変えない。
- **TIFF として読めない EXIF (`clean_exif` が `None`)**: Single では**今と同じ処理で転記し** (JPEG は
  `reset_exif_orientation_in_app1`、WebP と PNG の Raw profile はそのまま)、警告の種類「EXIF (外せませんでした)」
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
    「調べられない → 警告」も同じ)、PNG の `Raw profile type xmp` (と XMP の入った `APP1`。16 進を戻してから
    名前空間を探す。戻せない・桁数が長さと合わない・上限を超えたら「調べられない」。Raw profile の XMP は
    Single でも Batch でも転記したまま警告する (外さない))、WebP の `XMP `。Raw profile の XMP は書き換えない。
    PNG の Raw profile の検出は Batch にも同じく効く。
  - MakerNote: 「隠す編集」がある出力だけ。1 枚の書き出しでは、合成するページの編集 snapshot の
    `erase` / `conceal` / `export_crop` / `comic` / `local_adjust` のどれかが `Some`、**または**その出力の
    `crop_override` が `Some` (SNS 分割の枠。snapshot に切り取りが無くても枠の外を落とす)。
    - 隠蔽プリセットの出力 (`_1`〜`_4`) は、選べる条件が snapshot に隠蔽マスクがあること
      (`has_conceal_mask`) なので、`_0` も含めて snapshot の `conceal` で対象になる。`conceal_preset` は条件に足さない
      (今の製品の経路では重複になる。独立設計レビュー 1 回目の P3)。
    - 見開き合成は元の情報を転記しないので対象外 (MakerNote も検出しない)。
  - 元の情報を転記しない出力 (見開き・PDF・形式変更・OFF) は検出しない。
- **送り方**: `run_export` は、その出力の `Completed` を送った直後に、警告があれば
  `ExportEvent::ThumbnailNotice { label, path, kinds }` を送り、ログに
  `export thumbnail metadata: <出力パス>: <kinds>` を 1 行残す (一括の行は今どおり `batch export ...`)。
  `save_image_with_metadata` は保存結果の警告を返す形にする (`encode_image_with_metadata_report` と同じ値)。
- **画面**: 共通の進捗ダイアログ (§1.362 §3.7) がそのまま出す。完了で警告があれば、今の「成功 1 件なら
  保存の通知 (『保存しました: <名前>』、押すと Explorer で示す) を出してダイアログを閉じる」ではなく、警告の画面を残す
  (共通の `poll_export_pending` の今の規則どおり、新しい分岐は足さない)。
  - 種類の表示に「EXIF (外せませんでした)」を足す (`exif_uninspectable`、Single だけが立てる)。
  - 説明文の最後の文を、1 枚と一括の両方に合う言い方にする:
    「公開する前に確認するか、元の情報を残さない設定 (一括では『元の情報を残す』、1 枚の書き出しでは
    『AI プロンプト / EXIF を保持』をオフ) で書き出し直してください。」
- **書き出しの前の案内**: 1 枚のダイアログのチェックボックスに説明 (hover) を付ける。
  「EXIF の縮小画像は外します。外せなかったときや、XMP などに縮小画像が残っていそうなときは、書き出しの後で
  知らせます (隠した部分が縮小画像に残ることがあります)。」 行を足さないので、ダイアログの高さは変わらない。

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
  - JPEG: IFD0 と Exif IFD の両方に Orientation がある EXIF → IFD0 だけが 1 になり、Exif IFD の値は元のまま。
    IFD1 の範囲と次の IFD の 4 バイト以外は、今の処理 (`reset_exif_orientation_in_app1` だけ) の出力と同じバイト列。
  - PNG: テキストチャンクが今と同じ。`eXIf`・`iCCP` が出力に無い。
  - PNG の Raw profile (tEXt と zTXt の両方): IFD1 付きの EXIF → 16 進の該当の数字だけが `0` になり、改行・種類名・
    長さの行は同じ。展開すると縮小画像の範囲が 0。Raw profile の XMP に xmpGImg → `xmp`。16 進でない本文 →
    Single は元のまま + `exif_uninspectable`、Batch はそのチャンクが無い。
  - 宣言の長さより 16 進が多い Raw profile (EXIF・XMP の両方。XMP は後ろに xmpGImg) → 読めない扱い
    (EXIF は上と同じ、XMP は両方の profile で残して `xmp_uninspectable`)。
  - 順番: xmpGImg のある XMP チャンク → IFD1 付きの Raw EXIF の順でも IFD1 が外れる。
  - 共有の合計上限 (32 MiB) に達した後の Raw EXIF → Single は残して `exif_uninspectable`、Batch はそのチャンクが無い。
  - iTXt (圧縮・非圧縮) の Raw profile の EXIF → 言語・訳語のキーワードと圧縮の有無が元のまま。
  - 3 形式とも xmpGImg の XMP で `xmp` が立つ。名前空間の無い XMP では立たない。
  - MakerNote: `thumbnail_notice_for_hidden_edits` の有無で `maker_note` が変わる。
  - 読めない EXIF (例: IFD の循環): 出力が今の処理のバイト列と同じで、`exif_uninspectable` が立つ。
  - Batch は今のテストのまま通り、`exif_uninspectable` を立てない。
- `tests/export_integration.rs` (1 枚の worker):
  - IFD1 と xmpGImg を持つ JPEG を書き出す → `Completed` の後に `ThumbnailNotice` (xmp)、出力に IFD1 が無い。
  - 隠蔽マスクのある snapshot と MakerNote で `_0` と隠蔽プリセットの出力を書く → 両方に `maker_note`。
    snapshot に隠す編集が無い SNS 分割 (`crop_override: Some`) → `maker_note`。隠す編集が無い通常の出力 → 立たない。
  - ZIP 内の JPEG (IFD1 付き) → 出力に IFD1 が無く、xmpGImg なら `ThumbnailNotice`。
  - 成功 1 件 + 警告 → 保存の通知に移らず、警告の画面が残る (`App` のテスト)。
  - 「保持」OFF・形式変更では `ThumbnailNotice` を送らない。
- UI: 警告の一覧に新しい種類の文言が出る (既存の一括の警告テストに足す)。`python scripts/check_ui_glyphs.py`。
  スナップショットに説明文が写っていれば更新する。

## 6. 文書の更新 (実装と同時)

- マニュアル `htdocs/mimageviewer/manual/export.html`: 1 枚の書き出しの「メタデータ保持」の行と説明に、
  EXIF の縮小画像を外すこと、XMP などに残りそうなときは書き出しの後に知らせることを書く。一括の段落の
  「『元の情報を残す』をオフ」は §3.4 の説明文に合わせる。
- [batch-export-plan.md](batch-export-plan.md): §3.6 の「単ページの出力は変えない」の項を本書への参照に直す。
  §3.6 の PNG の行と §3.7 の「調べる XMP」に Raw profile を足す。§3.7 の「単ページの worker は送らない」を直し、
  種類の一覧と説明文を §3.4 に合わせる。
- [spec.md](spec.md) の「Ctrl+E エクスポート」のメタデータ転記の項に、EXIF の縮小画像を外すことと書き出し後の警告を書く。
- [docs/README.md](README.md) の索引に本書を足す。バックログ §1.888 の状態。
- 永続データ: 変えない (設定・DB のスキーマ変更なし)。

## 7. 独立設計レビューの記録

### 1 回目 (2026-11-25、対象 752919710、gpt-6.1-sol / xhigh): REVISE

- P1 PNG のテキストの Raw profile の EXIF・XMP が対象から漏れる (一括も同じ) → §2・§3.2・§3.4 で扱う。
- P2 `clean_exif` の Orientation の書き換えが下位の IFD まで及び、今の JPEG の出力より範囲が広い → Single は
  `clean_exif` で Orientation に触れず、JPEG だけ今の関数を当てる (§3.2)。比べるテストを足した (§5)。
- P3 隠蔽プリセットの条件が重複し、テストの期待が実際と合わない → `conceal_preset` を条件から外し、テストを直した。
- P3 読めない EXIF のときの案内・文書の更新先 (spec.md・一括の設計書)・今の成功 1 件の通知の説明 → 直した。

### 2 回目 (2026-11-25、対象 d92a57476、同じ会話の続き): REVISE

- 1 回目の 4 件は解消と判断。
- P2 Raw profile の宣言の長さより後ろの 16 進を調べ落とす → 桁数が長さの 2 倍と一致しなければ読めない扱い (§3.2・§3.4)。
- P3 XMP 検出後の省略との組み合わせ・合計上限・iTXt のテストが足りない → §3.2 に独立を明記し、§5 にテストを足した。

### 3 回目 (2026-11-25、対象 1ca147ab2、同じ会話の続き): ACCEPT

- 指摘なし。2 回目の 2 件は解消と判断。

## 8. 独立実装レビューの記録 (gpt-6.1-sol / xhigh、実装担当とは別の会話)

- 1 回目 (ad7b47611): REVISE。P2 Raw APP1 の拡張 XMP の断片をまたぐ名前空間を見落とす、P2 長さの合わない Raw APP1 の
  XMP を EXIF 扱いにする、P3 到達しない旧転記分岐とそれだけを通るテスト、P3 文書の検証状態。→ b4b30e865 で対応。
- 2 回目 (b4b30e865): REVISE。1 回目の 4 件は解消。P2 上限を超えた Raw APP1 の XMP を EXIF 扱いにする。→ 44dcac70f で対応
  (分類だけに先頭 1 KiB を raw deflate で展開する。zlib のままだと小さい流れでは checksum の誤りで先頭も得られない)。
- 3 回目 (44dcac70f): ACCEPT、指摘なし。
