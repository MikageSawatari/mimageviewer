# RAR 直読み + 明示 ZIP 変換 + 同名 ZIP 優先表示 設計

ステータス: 実装済み（実 RAR の最終スモークのみ実機確認対象）。

関連: [virtual-folders.md](virtual-folders.md)（分岐表・キー規則）/ [async-architecture.md](async-architecture.md)（worker 判定）/
`src/archive_converter.rs`（`convert_to_zip` / `scan_summary_rar` / `expand_rar` の流用元）。

---

## 0. 目的 / 背景

- RAR を開くたびに永続 cache ZIP（`archive_cache/<hash>/book.zip`）を作る現状は、disk 累積・二重化を生む。
- 一般的な RAR（**非ソリッド・入れ子アーカイブなし・画像のみ**）は ZIP と同様にエントリ単位のランダムアクセスが可能なので、
  変換せず直読みできる（cache ファイルを作らない）。
- ソリッド RAR / 入れ子 RAR は、直読みしようとすると「途中ページ到達に O(N²) 展開」「内側アーカイブの一時展開機構」
  「順次アクセス不変条件」などが必要になり、**過去に何度も手戻りした種類の不安定さ**を招く。よって直読みの対象外にして、
  従来の変換に委譲する（安定第一・退行はユーザー選択で許容）。
- 経緯（会話まとめ）:
  - Rust の `unrar` クレートは official UnRAR C ライブラリのラッパで **ファイル名からしか開けない**（メモリ/ストリーム
    open API が無い）。これは crate ではなく UnRAR の C API 自体の制約。
  - NeeView は stream ライブラリ（7z.dll の `IInStream`）+ **ソリッドの丸ごと事前展開（メモリ/一時、設定・非同期）** で
    入れ子・ソリッドも直読みするが、これは大きな機構（memory/temp/hybrid extractor + pre-extract 状態機械）を伴う。
    mIV で同等をやると手戻りリスクが大きい。
  - そこで **「直読みはフラット非ソリッド RAR のみ、それ以外は従来変換」** に割り切る。ソリッドは順次モードでも回避できず
    （NeeView も materialize している）、mIV はアップスケール/先読みでランダムアクセス依存が強いため、なおさら直読み対象外が妥当。

## 1. 決定方針（このバックログのスコープ）

1. **RAR 直読み**: ソリッド圧縮でなく、かつ入れ子アーカイブを含まない `.rar` / `.cbr` のみ直読みする（変換・cache 生成なし）。
2. **フォールバック**: 上記以外（ソリッド RAR / 入れ子を含む RAR / 7z / LZH / 入れ子を含む ZIP）を検知したら
   **従来通り**（ZIP cache 変換）で開く。
3. **明示変換メニュー**: メニュー「**変換 > ZIP ファイルに変換**」で、選択した RAR/CBR/7z/CB7/LZH/LHA から
   **同じフォルダに同名の `.zip` を生成**する（cache ではなくユーザー所有の実ファイル）。
4. **同名 ZIP 優先表示**: 同じ basename の `.zip` と RAR/7z/LZH が同一フォルダに並ぶとき、mIV 上では **`.zip` だけ表示**する
   設定を追加。**既定 ON**。既存「同名ファイル処理」設定群（動画/画像の同名スキップ等がある場所）に置く。

## 2. スコープ外 / 非目標

- ソリッド・入れ子 RAR の直読み（temp materialize / 順次専用モード等）は **やらない**（会話で不安定と判断）。将来必要なら別バックログ。
- 7z / LZH の直読みは **やらない**（変換のみ）。
- 直読みの永続 cache 化はしない（直読みはそもそも cache を作らない）。
- 既存 `archive_cache`（`archive_cache.rs` / `archive_cache.db` / cache manager UI）は **変更しない**。従来通り併存する。

## 3. 直読み可否の判定

- `unrar::Archive::new(p).open_for_listing()?.is_solid()` で **開封時（ヘッダのみ・展開なし）にソリッド判定**
  （crate 内 `ROADF_SOLID` を `RAROpenArchiveEx` 時に取得済み、エントリ反復不要）。
- listing の各エントリ名を `archive_converter::nested_archive_kind()` で走査し、入れ子アーカイブ
  （`.zip/.cbz/.rar/.cbr/.7z/.cb7/.lzh/.lha`）が 1 つでもあれば「入れ子あり」。
- **判定タイミング**: フォルダスキャン時には **やらない**（各 RAR を開くと UI が固まる。`docs/ui-responsiveness.md` §4）。
  open / サムネ生成の **worker 内** で 1 回 list して判定する。
- 判定結果は `(path, mtime, size)` をキーにキャッシュし、再 list を避ける（メモリ保持で十分。catalog 併用は未決 §11）。
- 分岐:
  - `!is_solid() && 入れ子なし` → **直読み**。
  - それ以外 → **従来 convert**。
- 実装は既存 `archive_converter::scan_summary_rar`（listing 走査 + is_image/nested 分類）とほぼ同じ処理なので流用できる。
- per-entry の solid フラグは `unrar` 0.5.8 が非公開（`FileHeader.flags` が private）。頼れるのは archive レベル `is_solid()` のみ。
  RAR5（solid が per-file 管理）で確実に立つかを **実サンプルで 1 度確認**しておく（保険）。

## 4. 実装戦略 — 「道B: ZipImage 再利用 + zip_loader 末端で分岐」

道A（新 `RarImage` variant を全分岐に追加）は `ZipImage`（コード内 303 箇所）/ `zip_path`（660 箇所）に波及し、
**1 万行規模・高リスク**（枝の取りこぼしで RAR だけ機能が静かに壊れる）なので **採らない**。道B を採る。

- 直読み対象の RAR は `GridItem::ZipImage { zip_path: <.rar>, entry_name }` として提示する。ZipImage を扱う全下流コード
  （フルスクリーン・サムネ・DB キー・ナビ・ピン・snapshot・rating・tag・sidecar）は `(zip_path, entry_name)` を
  **不透明に扱う**ので無改修。
- フォーマット分岐は **`zip_loader` の末端 read 関数だけ**に閉じ込める（アーカイブ **閲覧**のバイトアクセスは全て zip_loader
  経由。`zip::ZipArchive` の直叩きは DL パッケージ展開系のみで閲覧経路に無いことを確認済み）。以下に
  「path が直読み RAR なら `rar_loader` へ dispatch」を追加:
  - `enumerate_image_entries` / `enumerate_image_entries_detailed`
  - `first_image_entry` / `read_first_image_bytes`
  - `read_entry_bytes`
  - `open_archive` / `read_entry_from_archive`
- 新規 `src/rar_loader.rs`: 上記に対応する RAR 版（`open_for_listing` で列挙、`open_for_processing` で目的エントリまで
  skip して read）。土台は `archive_converter` の `scan_summary_rar` / `expand_rar`。
- **DB キー互換性**: ZipImage キーは常に `zip_path::entry`。直読みは `rar_path::entry`、
  変換 cache 経由はリリース済みデータと同じ `cache.zip::entry` で、両者は意図的に一致させない。
  parity には既存の回転 / ★ / タグ / 補正等を移す明示 migration が必要なので別判断とする。
- **静的述語は据え置き**: `is_zip_extension` / `is_virtual_folder` は `.rar` を今まで通り false（= `ConvertibleArchive` 扱い）
  のまま。scan / nav / startup を触らない。直読みか否かは「開く瞬間」に list で決める。

### 4.1 routing

- `.rar` を開く経路（`load_folder_or_convert_archive` / `ConvertibleArchive` のサムネ経路）で、判定結果を見て:
  - 直読み可 → `load_zip_as_folder` 相当に `.rar` パスを渡す（zip_loader が内部 dispatch）。`current_folder` = `.rar` 本体。
  - 不可 → 既存の変換ダイアログ / cache 経路（従来通り）。

### 4.2 主な注意所（局在リスク）

- **`is_virtual_folder(current_folder)` 系（~5–10 箇所）**: `current_folder` が直読み `.rar` のとき、これらは今 false を返す
  （`is_virtual_folder(.rar)=false`）。変換版は `current_folder` = cache `.zip` なので true を無料で得ていた。直読みでは
  「今コンテナの中か」を判定する箇所（BS / 親 / アドレスバー / 退出ルーティング / `last_folder` 保存）を、直読み `.rar` も
  真と扱うよう調整する。→ `is_open_as_container(path)` 相当の述語 or 実行時フラグを導入。該当候補:
  `app.rs:9083 / 10395 / 21432`、`ui_fullscreen.rs:8863`、`folder_pane.rs:740`、`startup_ops.rs:253` 付近を精査。
- **`last_folder` / `effective_folder`**: 直読み中の保存・復元先を `.rar` にする（cache `.zip` を漏らさない。変換版の
  `archive_source_override` 相当の配慮）。

## 5. 明示 ZIP 変換メニュー（方針 3）

- メニュー項目「**変換 > ZIP ファイルに変換**」。対象 = 選択中の RAR/CBR/7z/CB7/LZH/LHA（直読み可能なフラット RAR も
  対象にしてよい＝永続 `.zip` が欲しい場合）。
- 出力 = **同じフォルダに `<basename>.zip`**（sibling、STORE、実ファイル）。
- 実装 = 既存 `archive_converter::convert_to_zip(src, dst = <sibling>.zip, …)` をそのまま使う（入れ子再帰展開・ソリッド逐次
  展開・STORE 出力・進捗 / パスワード / エラーダイアログ流用）。
- cache（`archive_cache/`、hash 管理、隠し）との違い: これは **ユーザー所有・可視・恒久** の変換物。cache 無効化ロジックとは無関係。
- 生成後に同名 `.zip` が並ぶので、方針 4 の設定（既定 ON）により一覧は自動で `.zip` だけ表示になる（クリーンな移行フロー）。

## 6. 同名 ZIP 優先表示（方針 4）

- 新設定 `skip_archive_if_zip_exists`（仮名, `#[serde(default = "default_true")]` = 既定 ON）を `settings.rs` の
  「同名ファイル処理」群（`skip_zip_if_folder_exists` / `skip_image_if_video_exists` / `skip_duplicate_images` の隣、~L2066）
  に追加。
- 意味: 同一フォルダに同 basename の native `.zip` / `.cbz` が存在する `.rar/.cbr/.7z/.cb7/.lzh/.lha` を一覧から除外
  （`.zip` だけ表示）。basename 比較は case-insensitive（Windows）。
- 適用箇所（既存 dedup と同じ場所）:
  - `folder_tree.rs` の `DedupOptions`（~L22–34、`skip_zip` と同型で追加）
  - `app.rs`（~L27033–27039 の dedup 適用ブロック）
  - `app/subfolder_expansion.rs`（サブフォルダ展開ビューの dedup、~L22–32 / L357–364）
- UI: `ui_dialogs/preferences/pages.rs` の同名処理チェックボックス群（~L6160）に 1 行追加。ラベル例
  「同名の .zip と RAR/7z/LZH がある場合、RAR/7z/LZH を隠して .zip だけ表示」。

## 7. 相互作用まとめ

| フォルダ内の状態 | mIV の表示 / 開き方 |
|---|---|
| フラット非ソリッド RAR（同名 zip なし） | RAR を表示、開くと **直読み**（cache なし） |
| ソリッド / 入れ子 RAR・7z・LZH（同名 zip なし） | 表示、開くと **従来 convert（cache）** |
| 任意アーカイブ + 同名 `.zip`（方針 3 で変換済み等） | **`.zip` だけ表示**（方針 4 ON）、開くと native ZIP |

## 8. 触るファイル（見積り: 道B で概ね 1,000–2,500 行）

- 新規 `src/rar_loader.rs`（~400–700）
- `src/zip_loader.rs` 末端 dispatch（~50–150）
- routing: `src/app.rs`（`load_folder_or_convert_archive` 周辺）+ `ConvertibleArchive` サムネ判定（~150–300）
- `is_virtual_folder(current_folder)` 系の直読み対応（~100–300、局在）
- 方針 3 メニュー + 変換起動（既存 `convert_to_zip` 流用、~100–200）
- 方針 4 設定 + dedup: `settings.rs` / `folder_tree.rs` / `app.rs` / `app/subfolder_expansion.rs` / `preferences/pages.rs`（~150–300）
- テスト（~300–500）

## 9. リスク / 判断

- 直読みはフラット非ソリッドのみ = materialize / 一時展開 / LRU / 順次不変条件を **一切導入しない**ので低リスク。難ケースは
  枯れた convert に委譲。
- 主リスクは局在（`zip_loader` dispatch の網羅性、`is_virtual_folder(current_folder)` 漏れ）。303 箇所には散らない。
- 永続データ: 直読みは新規スキーマを作らない（キーは `rar_path::entry`）。既存の変換 cache
  ページは従来どおり `cache.zip::entry` を保持する。方針 4 設定は新規 bool（後方互換 `default_true`）。
  `archive_cache` は変更しない（従来通り併存）。→ **未リリース機能ではなく、既存の永続ストア（archive_cache / rating / rotation 等）
  との整合を壊さない**方向。

## 10. テスト項目

- 直読み判定: 非ソリッド・入れ子なし RAR → 直読み / ソリッド RAR → convert / 入れ子含む RAR → convert
  （`is_solid` と `nested_archive_kind` の分岐）。
- `rar_loader`: enumerate / first image / `read_entry_bytes` の round-trip（フラット RAR fixture、サブフォルダ RAR fixture）。
  CP932 名の解釈が zip_loader と一致すること。
- キー互換性: 直読み RAR は `rar::entry`、従来の変換 cache は `cache.zip::entry` となり、
  リリース済み cache キーが元 RAR へ remap されないこと。
- 方針 4: 同名 `.zip` 存在時に RAR/7z/LZH が一覧から消える / 設定 OFF で両方出る / サブフォルダ展開ビューでも一致。
- `is_virtual_folder(current_folder)` 系: 直読み RAR 内で BS / 親移動 / 退出ルーティング / `last_folder` 復元が cache 版と同じ挙動。

## 11. 未決 / 実装時に確定

- 「変換」メニューの置き場（メインメニュー vs 右クリックコンテキスト、両方か）。既存メニュー構造に合わせる。
- 方針 3 の出力 `.zip` が既存の場合の挙動（上書き確認 / 連番 / skip）。
- 方針 4 の対象拡張子に `.cbz` を含めるか（`.zip` と `.cbz` が同名の稀ケースの扱い）。
- 直読み判定キャッシュ（path + mtime + size）の保持場所（メモリのみ / 既存 catalog 併用）。
- 全サムネ 1 パス生成最適化（ソリッド / 入れ子は対象外だが、フラット RAR で per-entry 再オープンがサムネ大量生成時に
  遅い場合の将来最適化。§0 の「materialize/convert 経路内の最適化」であって直読みの代替ではない）。

## 12. 段階実装順（推奨）

1. `rar_loader` + zip_loader dispatch で「**フラット RAR 1 本を直読みで開く**」を end-to-end で通す spike
   （`is_virtual_folder(current_folder)` 対応は最小限）。ここで方式の成立を確認してから広げる。
2. 判定（`is_solid` / nested）+ routing（直読み / convert 分岐）。
3. `is_virtual_folder(current_folder)` 系の網羅対応 + リリース済みキー互換テスト。
4. 方針 4 同名 ZIP 優先表示（設定 + dedup + UI）。
5. 方針 3 変換メニュー（`convert_to_zip` 流用）。
6. docs 更新（`virtual-folders.md` の分岐表に「RAR 直読み」行と拡張子対応を追記）。
