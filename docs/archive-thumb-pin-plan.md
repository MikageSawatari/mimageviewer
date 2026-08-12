# 変換対象アーカイブ (RAR / 7z / LZH) の代表サムネ固定 — 実装プラン

**正本**。実装 = Codex、brief / レビュー / 統合 = ClaudeCode。

## 1. 背景

ユーザー報告 (2026-08-12): 「RAR だと代表サムネに固定が出来ないの?」

調査の結果、**RAR / 7z / LZH を「親コンテナの代表サムネ」に指定できない** ことが原因と確定した。
コンテナの中で付けるピンは種別に関係なく動いており、欠落しているのは 1 経路だけ。

### 1.1 動いていること (実機 + ログで確認済み)

- RAR を開いて中のページを固定する経路は正常。ピンは**元 RAR のパス**をコンテナキーにして
  書かれる (`zip_pin_root_path` が変換キャッシュ ZIP ではなく元アーカイブへ戻す)。

  ```
  folder_thumb_pin set: C:\tmp\miv-pin-test\03_rar\book_flat.rar
  ```

- 親フォルダの RAR タイルにその固定ページが出る (`make_load_request` の `ConvertibleArchive`
  分岐、[src/app.rs](../src/app.rs) の `GridItem::ConvertibleArchive` ケース)。実機で P03 表示を確認。

### 1.2 動いていないこと

通常フォルダ上の RAR/7z/LZH タイルを、そのフォルダの代表サムネに指定できない。

- [src/folder_thumb_pins.rs](../src/folder_thumb_pins.rs) の `source_from_grid_item` が
  `GridItem::ConvertibleArchive` に対して `None` を返す
- 結果、右クリックの「📌 代表サムネに固定」は**項目ごと出ない**
  ([src/ui_dialogs/context_menu.rs](../src/ui_dialogs/context_menu.rs) の
  `native_folder_pin_context_label`)、アドレスバーの 📌 は disabled +
  「変換後に設定可能」ツールチップ
- `.rar` は変換済みかどうかに関わらず常に `ConvertibleArchive` として列挙される
  ([src/app/folder_scan.rs](../src/app/folder_scan.rs)) ので、この
  **「変換後に設定可能」は現状どうやっても成立しない (文言が嘘)**

ZIP は `File { kind: ZipFile }` として固定でき、さらに cascade で ZIP 内のピンまで解決される。
そのため同じ操作をしても RAR だけ上位フォルダに何も出ない:

```
apply container=...\02_zip kind=Folder -> resolved=...\book_nested.zip (kind=ZipEntry)
  pinned_key=...02_zip#pin:cascade:...:zipentry||bookB/002.jpg     ← ZIP は 3 段 cascade
resolve_folder_thumb_image: folder=...\03_rar ... pin_aware=true -> <none>   ← RAR は 1 段目が無い
```

### 1.3 前提 (仕様として確定済み、変更しない)

フォルダの自動代表選定 (`resolve_folder_thumb_image_inner`) は画像とサブフォルダしか走査せず、
アーカイブは開かない。これは ZIP でも RAR でも同じで、**種別差ではない**。重い処理を自動で
走らせないための意図的な仕様なので今回も維持する (ユーザー合意済み)。

## 2. 確定した仕様判断 (ユーザー合意済み)

1. **`FileKind` に新しい variant を追加する** (DB 文字列 `"archive"`)。
   既存 `"zipfile"` を流用して拡張子で分岐する案は、`FileKind::ZipFile` が 2 つの意味を持ち
   判別子が `source_rel` の拡張子という暗黙の場所に移るため却下 (保守性優先)。
2. **旧バージョンでの取り込み失敗は仕様とする**。新版で RAR/7z/LZH の代表サムネを固定した
   バンドルを旧版でインポートすると、プリフライト検証で
   「代表サムネ設定が不正です: (パス)」となり**取り込み全体が失敗する** (DB は無変更)。
   バンドルの `FORMAT_VERSION` は**上げない** (完全一致チェックのため、上げると旧版が
   すべてのバンドルを拒否して悪化する)。
3. **今回から取り込み側を寛容化する**。代表サムネの未知 `source_kind` は、その 1 件だけ
   スキップして続行する。現行の旧版は救えないが、次に kind を増やすときに同じ罠を踏まない。
4. **未変換アーカイブへのピンは guard で拒否する** (動画ピンと同じ作法)。
   自動選定はアーカイブを展開しないため、許可しても実質サムネが出ないままになる。
5. 同名 ZIP があると変換元アーカイブが一覧から隠れる既存仕様
   (`filter_convertible_archive_duplicates`) はそのまま。ピンも一緒に見えなくなるが許容。

## 3. 実装スコープ

### S1. `FileKind::ConvertibleArchive` の追加

[src/folder_thumb_pins.rs](../src/folder_thumb_pins.rs)

- `FileKind` に variant 追加、`as_db_str` / `from_db_str` に `"archive"` を対応付け
- `source_from_grid_item`: `GridItem::ConvertibleArchive { path, .. }` を
  `File { rel, kind: ConvertibleArchive }` で返す (現在の `None` を解除)。
  doc comment の「ピン留め不可」記述も更新する

### S2. `ResolvedKind::ArchiveFirstImage` と cascade

[src/folder_thumb_pins.rs](../src/folder_thumb_pins.rs)

- `ResolvedKind` に variant 追加
- `resolve_pin_target`: `FileKind::ConvertibleArchive => ResolvedKind::ArchiveFirstImage`。
  **`abs_path` は元アーカイブのまま**にする (mtime/size = 元アーカイブのもの →
  RAR を差し替えたら `source_id` が変わってキャッシュが正しく無効化される)。
  変換キャッシュ ZIP への読み替えは dispatch 側 (S3) の責務。
- cascade のコンテナ判定 (`Folder | ZipFirstImage | PdfFirstPage | ZipDirRepresentative` の
  `matches!`) に `ArchiveFirstImage` を追加。次段の container は `resolved.abs_path`
  (= 元アーカイブのパス)。**アーカイブ内のピンは元アーカイブのパスをキーにして
  保存されている**ので、これで既存の DB 行とそのまま繋がる (§1.1 のログが根拠)
- compat 表に 2 行追加。`ZipFirstImage` と同じ規則:
  - `(ArchiveFirstImage, ZipEntry { zip_rel })` → `zip_rel.is_empty()`
  - `(ArchiveFirstImage, ZipDir { zip_rel })` → `zip_rel.is_empty()`
  - `(ArchiveFirstImage, _)` → false

### S3. LoadRequest への dispatch (変換キャッシュ ZIP への読み替え)

[src/app.rs](../src/app.rs)

- `apply_folder_thumb_pin` の dispatch に `ArchiveFirstImage` を追加:
  `path` = 変換キャッシュ ZIP、`resolve_override = ResolveStrategy::ZipFirstImage`
- **cascade 後の leaf にも読み替えが要る**。例: フォルダ → `book_flat.rar` →
  (RAR 自身のピン: `ZipEntry`) と辿ると、`resolved.kind = ZipEntry` で
  `resolved.abs_path` は**元 RAR**になる。ワーカーは ZIP として開けないので、
  **最終 `abs_path` が変換対象アーカイブならキャッシュ ZIP に差し替える**共通ヘルパーを 1 つ置き、
  `ZipEntry` / `ZipDirRepresentative` / `ArchiveFirstImage` の 3 経路で共有する
  (種別ごとに同じ差し替えを書かない)
- `edit_preview_key` / `pinned_page_adjustment_key` は**キャッシュ ZIP のパス**で作る
  (既存の `ConvertibleArchive` 分岐が `cached_zip` を使っているのと揃える)
- `cache_key_override` は従来どおり `resolved.source_id` ベース (元アーカイブの identity)
- **キャッシュが引けなかった場合は `base_req` に戻す** (ログ 1 行)。dead video pin と同じ作法で、
  pinned_key 下に auto-pick を書き戻す churn を防ぐ
- `ContainerKindForPin` と `pin_source_compatible_with_container` に
  変換対象アーカイブを追加 (`ZipFile` と同じ規則)
- `pinned_edit_preview_target` の分岐に新 kind
- `delete_missing` 用の存続キー計算 (`container_surviving_cache_keys` 相当) に
  新 kind の pinned key を追加する。漏れると毎ロード prune → 再生成の churn になる

### S4. `converted_archive_cache_paths` の入力拡張

[src/app.rs](../src/app.rs) `start_converted_archive_cache_paths_refresh`

現在は「現在の items にある `ConvertibleArchive` / 元アーカイブ由来 `ZipDir`」しか集めていない。
フォルダタイルのピンが指すアーカイブ (例: `03_rar` フォルダ → `03_rar\book_flat.rar`) が
入らないので、ここを広げる。

- 推奨: **worker 側で cascade を解決**し、経路上に現れた変換対象アーカイブのパスを全部集める。
  worker は UI スレッド外で、pin DB も archive_cache DB も開ける
- refresh 開始時は現在の map を clear せず、worker 結果で置き換える。worker は archive path
  だけでなく、その path に pin 解決が依存する item index も返す。poll は旧/new map の差分 key
  に依存するタイルだけを共通 thumbnail reload helper で Evicted に戻す。同じ map の再適用では
  何もしないため、`folder_thumb_pin_dirty` / `load_folder` の再帰ループを作らない
- 最小実装 (1 段だけ) にする場合は、`folder_pin_map` の値から
  `container.join(rel)` が変換対象拡張子のものを集める。多段 cascade は将来対応として
  この doc に残すこと

### S5. 未変換アーカイブの guard と UI 状態

[src/app.rs](../src/app.rs)

- `try_set_folder_thumb_pin_with_video_guard` を**代表サムネ設定の guard として一般化**する
  (種別ごとの分岐を並列に増やさない。単一の所有者に集約):
  - Video: `video_pins` に WebP があるか (既存)
  - ConvertibleArchive: `ArchiveCacheDb::lookup(path, mtime, size)` が
    有効なキャッシュを返すか ([src/archive_cache.rs](../src/archive_cache.rs))
  - 判定はキー押下時の 1 回だけなので UI スレッドで可 (1 行 SELECT + 存在確認)
- 拒否時のトースト: 「まだ変換されていないため代表サムネに固定できません。
  一度開くか、選択してバッチ変換してください」(文言は実装時に最終調整)
- 📌 ボタン / 右クリックメニューの状態はメモリ上の `converted_archive_cache_paths` で判定する
  (I/O ゼロ):
  - 未変換 → disabled + 既存文言「代表サムネ固定: 変換後に設定可能」
    (**この文言がここで初めて正しくなる**)
  - 変換済み → ZIP と同じ通常表示。`is_convertible` による無条件 disabled は撤去
  - マップは非同期に届くため、到着前の数フレームは disabled になる。トースト guard が
    最後の防波堤として残る

### S6. メタ情報バンドル (metadata_transfer)

[src/metadata_transfer.rs](../src/metadata_transfer.rs)

- `portable_folder_pin_source` の許可 kind に `"archive"` を追加
- **寛容化**: 代表サムネの `source_kind` が未知の場合、バンドル全体を失敗させず
  **その pin 1 件だけスキップして続行**する。
  - 検証側 (`validate_container_state` の代表サムネ検証) と
    適用側 (`insert_folder_pin`) の両方を直す
  - スキップは既存の部分成功表示 (`incomplete_error` / `failed_items`) に載せ、
    ログにも残す (無言で落とさない)
  - **緩めるのは「未知 kind」だけ**。`..` を含む rel、`\0`、長すぎるキーなどの
    不正値は従来どおり厳格に失敗させる

### S7. テスト

- `folder_thumb_pins.rs` ユニット:
  - `source_from_grid_item` が `ConvertibleArchive` に対して source を返す
  - `"archive"` の DB ラウンドトリップ
  - `ArchiveFirstImage` の cascade (アーカイブ → その中の `ZipEntry` ピン) が leaf まで辿る
  - compat 表: `zip_rel` 非空の source が弾かれる
- `app/tests.rs`:
  - フォルダピン → 変換対象アーカイブ で `make_load_request` が
    **キャッシュ ZIP** の `ZipFirstImage` リクエストになる
  - 同 cascade で `ZipEntry` leaf のとき path がキャッシュ ZIP に差し替わる
  - 変換キャッシュが無い場合に `base_req` へ戻る
  - 存続キーに新 kind の pinned key が含まれる
  - 既存の ZIP 版テスト (`folder_pin_to_zip_file_cascades_to_the_zips_pinned_page` 等) の
    アーカイブ版を対で置く
- `metadata_transfer`: 未知 kind の pin を含むバンドルが「1 件スキップして残りは成功」になる

### S8. ドキュメント

- [docs/virtual-folders.md](virtual-folders.md): 変換対象アーカイブのピン経路と
  キャッシュ ZIP 読み替えを追記
- [docs/spec.md](spec.md): 代表サムネ固定の対象種別を更新
- 旧バージョン互換の仕様 (§2-2) を明記
- `htdocs/mimageviewer/manual/` の代表サムネ固定の説明を更新 (バージョン表記・内部用語は書かない)

## 4. 非スコープ

- フォルダの自動代表選定でアーカイブを展開すること (§1.3、意図的に維持)
- 変換キャッシュ ZIP を直接開いた場合のピンキー統一 (`archive_cache.db` に
  cached_zip → src の逆引きを足す話。通常操作では到達しないので後日)
- 同名 ZIP がある変換元アーカイブを一覧に出すこと (§2-5)

## 5. 検証

再現データ: `C:\tmp\miv-pin-test\` (生成スクリプトと手順は同フォルダの README.md)。

1. `03_rar` で `book_flat.rar` タイルを選んで `P` → 固定できる (今は不可)
2. 1 階層上の `miv-pin-test` に戻る → `03_rar` フォルダタイルが P01 (RAR の 1 枚目) になる
3. `book_flat.rar` の中で P03 を固定 → `03_rar` タイルが P03、上位の `03_rar`
   フォルダタイルも P03 (cascade)
4. `04_mixed` で `same_pages.zip` を固定 → フォルダタイルに反映
5. 変換キャッシュを削除 (環境設定 → 変換済みアーカイブキャッシュ管理) → 未変換 RAR に
   `P` → トーストで拒否、📌 ボタンが disabled + 「変換後に設定可能」
6. 02_zip / 01_folder の既存挙動が変わっていないこと (回帰)
7. メタ情報エクスポート → インポートで archive pin が往復すること
