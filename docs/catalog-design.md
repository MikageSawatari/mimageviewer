# サムネイルカタログ 設計書

### 音声アルバムアート (§1.347、2026-10-08)

MP3 の埋め込み画像の成功行は既存 `thumbnails` に保存し、`audio_art_absence` は保存対象の確定 NoArt だけを持つ。キーは `audioart:` namespace・drive を保持した実親 scope・版・basename、stamp は worker の fresh mtime 秒 / size。通常 `load_all` と `delete_missing` は SQL 段階でこの namespace を除外し、完全な物理 inventory を得た親だけ worker が成功・absence を prune する。旧 DB の hash / 既存行を維持し、absence schema は worker が保存するときだけ追加する。

`CatalogAccess` が cache-dir 単位の admission epoch・SQL lease・接続 retirement を所有する。削除受付で新規 SQL を失効させ、worker が既存 SQL と全 Local / Remote 接続を排出・close してから全件 / 期限 / 指定フォルダ削除を行う。旧 source 要求は表示を完了できるが DB を再作成しない。CacheOnly は一回だけ新 epoch の短命 read-only 接続で読取を続行する。既存 Loaded を保持し、削除 scope の Audio NoArt / Failed だけを memory 上で再要求可能にする。詳しくは [音声画像計画 §5](audio-album-art-plan.md)。


## 1. 目的・背景

画像ファイルからサムネイルをリアルタイム生成する場合、ファイルサイズによっては
1枚あたり 100〜500ms かかる。初回表示時のワンテンポ遅延を解消するため、
生成済みサムネイルをファイルにキャッシュし、次回以降は即座に読み込む。

### 現状の計測値（ログ実測）

| 処理 | 時間（目安） |
|------|------------|
| JPEG デコード（1枚・RTX4090環境） | 100〜500ms |
| Lanczos リサイズ（→900px） | 5〜20ms |
| GPU テクスチャ登録 | 1ms 以下 |
| カタログ読み込み目標（1枚） | 5〜20ms |

---

## 2. カタログファイルの配置

### 2.1 中央集約型レイアウト

```
{cache_dir}/
  {xx}/                   ← フォルダパスハッシュ（hex）の先頭2文字
    {full_hash}.db        ← フォルダ1つ分のサムネイルカタログ（SQLite）
    {full_hash}.db
    ...
  {xx}/
    ...
```

- `cache_dir` は設定で指定（デフォルト: `%APPDATA%\mimageviewer\cache`）
- 先頭2文字サブディレクトリにより、最大256フォルダへ分散（1フォルダあたり平均4〜5ファイル＠1000フォルダ）
- 画像フォルダへの書き込みが不要（ネットワークドライブ・読み取り専用フォルダでも動作）

### 2.2 ハッシュキーの計算

```
key = SHA-256( normalize(folder_path) )

normalize(path):
  1. ドライブ文字を除去（"C:\Photos\2024" → "\Photos\2024"）
  2. 小文字化（"\photos\2024"）
  3. バックスラッシュをスラッシュに正規化（"/photos/2024"）
```

**リムーバブルデバイス対応**: ドライブ文字を除去することで、同じリムーバブルデバイスが
別のドライブレターでマウントされても同じキャッシュにヒットする。

> 注意: `C:\photos` と `D:\photos` は同一ハッシュになる。これは意図的な設計で、
> パスが同じならキャッシュを共有する（mtime/file_size で整合性チェックするため実害なし）。

**例外: ドライブルート catalog**: `C:\` / `D:\` のようなルートだけはドライブ文字を
保持してハッシュ化する。ルート catalog は直下項目を `Photos` や `cover.jpg` のような
basename / full-path key で持つため、ドライブ文字を落とすと `C:\Photos` と `D:\Photos`
が同じ DB に混在し、ドライブ一覧のピン済み代表サムネで別ドライブの画像を表示し得る。
サブフォルダは従来どおりドライブ文字を落とすので、リムーバブルドライブのレター変更時の
キャッシュ継承は維持する。旧共有ルート catalog は移行せず、キャッシュ管理の通常掃除対象に
任せる。

### 2.3 ハッシュ計算例

```
"D:\Photos\2024\夏休み"
→ normalize: "/photos/2024/夏休み"
→ SHA-256: "3f8ac1b2d4e5..."
→ ファイル: {cache_dir}/3f/3f8ac1b2d4e5....db
```

### 2.4 クリアの接続所有権と削除単位（backlog §2.12、2026-10-19）

調査基点は `be318299f`。利用者の2026-08-20実測は、クリア＋再起動後に対応DBのない
WAL 1,410件（1,394.8 MB）とSHM 1,410件（44.1 MB）が残ったというもの。
この実測環境そのものは今回再現していない。

旧実装（`ca57742a1` の親）の `cache_manager.rs` は削除前に
`App::evict_all_catalog_cache` を呼んでいたが、これはApp側LRUのArcを外すだけで、
thumbnail worker / queued request / Remoteの保持接続のcloseを保証しなかった。
`cache_maintenance::spawn` の全件・期限指定は `catalog::delete_all_cache` /
`delete_old_cache`、フォルダ指定はworker内の `remove_file(db_path)` で、いずれもDB本体だけを削除していた。
したがって「全所有者のcloseを保証せずDB本体の削除を試みる」コード経路は確認できるが、
実測の各孤児をどの接続が残したかまでは確定できない。

現行では音声画像対応（`ca57742a1`、`921f1e457`）で、全catalogを
`CatalogAccess` の登録接続・epoch・SQL leaseに統合済み。接続を閉じずにDBを消す
catalog経路は調査基点では見つからなかった。残る問題は削除単位がDB本体のみで、
旧版の孤児を列挙せず、SQLiteのclose後に残った補助ファイルも処理しないこと。

| 入口・対象 | 接続の実所有者 | 削除順序（現行の共通境界を維持） |
| --- | --- | --- |
| 設定→サムネイルキャッシュ管理→すべて削除 | Appのcatalog LRU / 色取得、thumbnail・重I/O・details-metaのworker/LRU、PDF catch-up・cache creator、Remote container等の `CatalogDb`。実接続は登録済み `CatalogConnection` が所有 | UIで `begin_delete(All)` → `cache-maint` workerでSQL/open/close leaseをdrain → 全登録接続をRetiredへ移しclose → DBと兄弟WAL/SHMを削除 → 孤児も削除 → token解放 → UI結果 |
| 同画面の指定日数以上古いキャッシュ削除 | 同上（期限外の接続も従来どおり退役） | `begin_delete(OlderThan)` → 同じdrain/close → DB本体のmtimeで選択したDBと兄弟WAL/SHMだけ削除 → token解放。DBのない孤児は年齢が分からないため対象外 |
| 同画面の現在フォルダ削除 | 同上＋worker内で開く排他的な短命保守接続 | `begin_delete(Folder)` → 同じdrain/close → 対象DBの一般行と対象driveのaudio scopeをSQL削除 → 保守接続close → 他driveのaudio行/absenceがなければDBと兄弟WAL/SHMを削除。DBが元からなければ対象hashの孤児だけ削除 |
| `catalog::delete_all_cache` / `delete_old_cache` の公開helper | 同じcache-dirの `CatalogAccess` | helper自体がbegin/drain/close/tokenを所有し、上記と同じファイル削除へ接続。製品の呼出元は管理worker内の専用under-delete経路。公開helperの直接利用はテスト |
| 起動時・容量上限でのcatalog DB削除 | 該当なし | catalog DBを物理削除する自動経路はない。`delete_missing` / audio scope pruneは行単位SQLで、DBファイルは消さない |

削除単位は `.db` とその名前へ `-wal` / `-shm` を付けた3ファイル。
必ず所有者によるcloseを先に済ませ、DB削除が失敗したときは兄弟を消さない。
全削除の孤児回収も同じ排他期間・同じworker・既存の1階層のhashディレクトリ走査で行い、
対応DBの不存在を確認できた `*.db-wal` / `*.db-shm` だけを消す。
走査ではreparse pointを辿らない。失敗は既存reportのログと管理画面エラーへ返し、再試行しない。
削除DB件数は補助ファイルを数えず、従来の意味を維持する。永続schema変更・移行はない。

簡素化の判断: 既存のcache-dir単位排他でopen/SQL/closeを止められるため、専用worker、
新しい待機状態、UI全体のmodal化、起動時孤児掃除は追加しない。まれな失敗は割り切り
（ログ＋既存エラー通知）。

他キャッシュの横断確認:

| 対象 | 削除と所有権の確認結果 |
| --- | --- |
| 類似索引（`similar_db.rs` / `similar_index.rs`） | 行のSQL DELETEと `similar.base` 等の派生配列ファイル更新。SQLite DBの物理削除は見つからず、同型ではない |
| ZIP変換（`archive_cache.rs`） | 保守lock下でconverted ZIPと対応SQL行を削除。保持SQLite DBは消さない |
| EPUB/PDF変換（`epub_cache.rs`） | 世代leaseで保護した変換PDF等のpayloadとSQL行を削除。保持SQLite DBは消さない |
| PDFページ数・動画長さ等のメタ情報 | catalogのtableなので上記close/削除境界の対象 |
| 検索メタ索引（`search_index_db.rs` / `indexer_manager.rs`） | SQLite tableのresetと派生Tantivy directoryの再構築。SQLite DB本体を消す同型経路は見つからず |
| 編集preview・Auto比率 | SQL DELETE＋preview画像等のpayload削除。SQLite DB本体は保持 |
| 動画タイル（`video/tile_thumb_cache.rs` / `cache_maintenance.rs`） | 通常はmutex下のSQL DELETE/VACUUM。`Some(cache).clear_all()` 失敗時の `erase_db_files()` fallbackはApp/workerのArcが接続を保持したままDB/WAL/SHMを削除するため同型の欠陥。今回のcatalog修正の範囲外として報告。修正にはtile workerを含む接続所有境界の設計が必要。`None` のopen失敗時fallbackとは区別する |

なお、`catalog::cache_stats` の容量は従来からDB本体だけの集計で、WAL/SHMは含めない。
容量表示の仕様変更は今回含めない。

検証記録（`be318299f`＋本節の未コミット差分、通常feature / test profile）:

- 新規4テストは全件成功（nextest run `680ab91a-9d7b-44f3-9d62-a9c740be50d3`）。
  全件/期限/フォルダの実WAL DB、SQLiteのPERSIST_WALでclose後も残る補助ファイル、
  旧版の孤児と削除scope、別cache-dirの開いたDB、Windowsの外部接続で削除できないDBを検査。
- catalog全体＋管理worker＋Remote/drive CacheOnlyの関連94テスト（新規4件を含む）が成功
  （nextest run `3f91c43a-dfd8-4c89-9820-bf519fdbc026`、失敗0、実行1.766秒）。
  コマンドと結果のログは `target/section212-focused.log` / `target/section212-regression.log`。

```powershell
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(/catalog_clear_/)' --no-fail-fast
cargo nextest run --build-jobs 1 -p mimageviewer --lib -E 'test(/^catalog::tests::/) | test(/^cache_maintenance::tests::/) | test(/drive_cache_only_pins_do_not_resurrect_old_map_after_clear_or_cancel/) | test(/review2_cache_only/) | test(/keep_admission_across_delete_before_decode/) | test(/source_only_remote_zip_and_pdf_pages_do_not_require_catalog_during_delete/)' --no-fail-fast
```

- `cargo fmt`（引数なし、workspace全体）/ `cargo fmt --check` / `git diff --check` が成功。
  `python scripts/check_ui_glyphs.py` は危険glyph 0件。アプリの見た目・UI文言は変更していない。
- 初回コンパイルはworktreeのLibRaw不足とTurboJPEGの並列MSBuild失敗で未実行だった。
  LibRaw 0.22.2を既存worktreeからreparse pointのない実体として復元し、
  `MSBUILDDISABLENODEREUSE=1` と `--build-jobs 1` で上記の成功結果を得た。
  前提準備の失敗は製品テストの失敗数へ含めない。
- 本体lib全件は依頼どおりClaudeCodeが最後に実行するため未実施。
  実アプリ起動、独立レビュー・検収は未実施（別担当）。
- `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` が成功（exit 0）。
  通常feature / dev-runtime profileでcore・Remote・EPUB converterを生成し、
  VCRT配置検査も成功（runtime=4、PE=3）。ログは `target/section212-build-dev.log`、
  出力は `target/dev-runtime/mimageviewer-core.exe` ほか同ディレクトリのcompanion。
  環境は `MSBUILDDISABLENODEREUSE=1`、native依存の準備はbuild jobs 1、
  生成済み依存を再利用した最終script実行は `CARGO_BUILD_JOBS=4`。
  Ninjaのcompiler検出で停止した試行は中断し、標準MSBuildへ戻した成功結果のみを採用した。
  通常profileの成果物をエージェントから起動していない。

---

## 3. ファイル形式の選定

### SQLite（rusqlite）を採用

| 形式 | 読み込み速度 | 実装コスト | 備考 |
|------|-----------|----------|------|
| **SQLite（フォルダ1DB）** | 速い | 低 | 差分更新が容易。今回採用 |
| グローバル1DB | 速い | 低 | 全キャッシュが1ファイル。巨大化するが SQLite は問題なし |
| フラットファイル（JPEG + メタ） | 最速 | 中 | SQLite不依存だが実装コスト高め |

採用理由：
- 差分更新（ファイル追加・変更・削除の部分更新）が容易
- WALモードで並列読み書きが安全
- フォルダ単位でキャッシュを削除しやすい
- `rusqlite` (bundled) で外部依存なし

---

## 4. データベーススキーマ

```sql
-- カタログのバージョン管理
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- key='version' value='2'
-- key='folder_path' value='/photos/2024/夏休み'（正規化済みパス）
-- key='pdf_layout_dims_version' value='2'

-- サムネイルエントリ
CREATE TABLE thumbnails (
    filename    TEXT    NOT NULL PRIMARY KEY,  -- ファイル名（パスなし）
    mtime       INTEGER NOT NULL,              -- ファイル更新日時（Unix秒）
    file_size   INTEGER NOT NULL,              -- ファイルサイズ（バイト）
    width       INTEGER NOT NULL,              -- サムネイル幅（ピクセル）
    height      INTEGER NOT NULL,              -- サムネイル高さ（ピクセル）
    thumb_data  BLOB    NOT NULL,              -- WebP圧縮サムネイルデータ
    source_width  INTEGER,                     -- 元画像 / PDF raster の幅（ピクセル）
    source_height INTEGER,                     -- 元画像 / PDF raster の高さ（ピクセル）
    layout_width  INTEGER,                     -- PDF page box 幅（1/1000 point、他はNULL）
    layout_height INTEGER                      -- PDF page box 高さ（1/1000 point、他はNULL）
);
```

`source_*` は画像種別にかかわらずピクセル座標系で、編集・注釈・クリック判定等が参照する。
PDF の raster 丸めに依存しないページ矩形比だけを `layout_*` に分離し、レイアウト consumer は
こちらを優先する。v2.13.0 までのリリース済み行には `layout_*` が無く、未リリースの開発版 marker
v1 は PDF の `source_*` へ page-box 単位を書いていた。どちらも WebP から正確には補えないため、
`pdf_layout_dims_version=2` への移行時に PDF catalog の全 page 行と通常 catalog の `pdfthumb:` 行を
一度だけ削除・再生成し、無関係な画像 / ZIP 行は保持する。

### 動画・音声メタデータ (`video_meta`)

フォルダ単位の既存 catalog に次のテーブルを追加する。`pdf_meta` / `container_page_meta` と同じ
追加方式であり、既存テーブル・行・schema version は変更せず、既存行の移行は不要。

```sql
CREATE TABLE video_meta (
    filename TEXT NOT NULL PRIMARY KEY,
    mtime INTEGER NOT NULL,
    file_size INTEGER NOT NULL,
    readable INTEGER NOT NULL,
    duration_secs REAL,
    width INTEGER,
    height INTEGER,
    codec TEXT
);
```

長さ・解像度・コーデックを 1 回の probe からまとめて保存する。音声の幅・高さは NULL。
`readable=0` は確定した読取失敗の negative cache で、値列は NULL。mtime / file_size の完全一致時
だけ成功・失敗の両方を再利用する。source identity は `Option<(mtime, size)>` として保持し、
未知と既知のサイズ 0 を区別する。空のファイルも確定失敗の保存対象で、未知は DB 再利用・書込の対象外。
FFmpeg の内容不正・利用可能な stream 不在・非対応 codec / container だけを確定失敗とする。
アクセス拒否・共有違反・ファイル不在・一般 I/O エラー、取消、10 秒 timeout は `Interrupted` で
保存しない。メモリ内の `RetryLater` は試行時の `items_generation` だけで終端として扱い、
同じ一覧で連続再試行しない。次回フォルダを開くと世代が変わり、DB miss として再取得できる。

実ファイルの mtime / size は catalog の mutex・SQLite 書込ロックを取る前に再確認し、
要求時と完全一致するときだけ保存へ進む。OS の属性取得自体は中断できないため、その前後で
取消を確認し、取消後は保存しない。`BEGIN IMMEDIATE` の中は SQL 書込だけとし、ネットワーク
共有の属性取得が停滞しても既存 UI 書込を妨げない。mtime の大小で新旧を推測しない。
属性確認から commit までにファイルが変わり、新しい worker が先に保存した場合、遅い旧結果が
その行を置き換える余地は残る。この行は現在の mtime / size と一致せず lookup が miss になるため、
代償は次回の probe 1 回である。cache hit / probe の結果も worker が公開前に source identity を
確認し、古い結果を `Interrupted` として捨てる。

既存 details-meta worker が有界 catalog LRU と I/O semaphore を使って lookup → miss 時だけ probe →
確定結果の書込を行う。UI は `DetailsLazyMeta` だけを読む。詳細列・ツールチップ・選択情報も
同じ DB 値を使い、従来の値と整形は保つ。検索結果・コレクション・レーティング一覧・スマート
フォルダでも、ページ数と同じ実ファイルの親 + basename で元 catalog を参照する。
親 / キー / stamp を得られない項目や DB 障害では従来どおり probe し、表示を失敗させない。
`--perf-log` の `details_meta/media_cache_hit` / `media_probe` で再利用と再取得を区別できる。

状態の簡素化として、新 worker・要求フラグ・retry owner は追加せず、既存の staged session、
取消、一覧世代、メモリキャッシュを再利用する。取得中の modal 化は通常のスクロールを妨げるため採用しない。
サムネイルの範囲変更は既存 scroll idle gate 後に現在 viewer の範囲 snapshot を差し替え、旧 stage を
取り消す。画面外の選択情報は別機構にせず同じ stage に加え、選択変更は待たずに反映する。

### 無効化ロジック（キャッシュ整合性）

```
for each file in directory:
    row = SELECT mtime, file_size FROM thumbnails WHERE filename = ?
    if row not found:
        → 新規ファイル: サムネイル生成してINSERT
    elif row.mtime != file.mtime OR row.file_size != file.file_size:
        → 更新されたファイル: サムネイル再生成してUPDATE
    else:
        → キャッシュ有効: thumb_data をそのまま使用

for each row in thumbnails:
    if file not in directory:
        → 削除されたファイル: DELETE
```

---

## 5. サムネイルデータの圧縮形式

カタログ内のサムネイルは **WebP（lossy, 品質75）** で保存する。

| 形式 | 圧縮率 | デコード速度 | 画質 | サイズ目安（512px） |
|------|--------|------------|------|------------------|
| JPEG q=80 | 高 | 速い | 十分 | ~20-50KB |
| **WebP lossy q=75** | 高 | やや遅い | やや良い | ~15-40KB |
| QOI | 低 | 非常に速い | 可逆 | ~100-200KB |
| PNG | 低 | 普通 | 可逆 | ~200-500KB |

→ WebP は JPEG より圧縮効率が高く、同サイズで画質が良い。
  `webp` クレートでエンコード、`image` クレートでデコード。

サムネイルサイズは **固定 512px**（長辺）とする。

---

## 6. 読み込みフロー（カタログあり）

```
フォルダを開く
  ↓
① ファイル一覧を取得（瞬時）
  ↓
② normalize(folder_path) → SHA-256 → DB パスを計算
  ↓
③ {cache_dir}/{xx}/{hash}.db を開く（なければ作成）
  ↓
④ 全キャッシュエントリを一括SELECT（1クエリ）
  ↓
⑤ 各ファイルのキャッシュ有効性を確認
  ↓
  ├─ 有効: WebP バイト列 → image::load_from_memory() → GPU テクスチャ
  │         （並列処理。デコード数ms以内）
  │
  └─ 無効/未キャッシュ: 元ファイルからサムネイル生成
                         → カタログに書き込み
```

Folder の上位代表キャッシュが miss して再帰探索する場合は例外がある。子フォルダの非画像 pin
(PDF / ZIP / 動画等) は元ソースから生成せず、子が直上一覧で生成済みの完全一致 pin WebP が
直上 catalog に存在するときだけ読み取り専用で再利用する。DB / 行 / 有効な WebP が無ければ
catalog を新規作成せず、従来の画像自動選定へ戻る。再利用結果を現在の上位 catalog に保存する
かどうかは通常の `CachePolicy` / `force_cache` 判定に従う。

### 期待される速度改善

| | 初回（キャッシュなし） | 2回目以降（キャッシュあり） |
|--|---------------------|------------------------|
| 12枚（4列×3行） | 0.5〜2秒 | 0.1〜0.3秒 |
| 100枚 | 数十秒 | 0.5〜1秒 |

---

## 7. 実装コンポーネント

### 新規ファイル

```
src/
  catalog.rs      # CatalogDb 構造体、パスハッシュ計算、read/write/validate ロジック
```

### 依存クレートの追加

```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
sha2 = "0.10"   # SHA-256 でパスをハッシュ化
webp = "0.3"    # サムネイル WebP エンコード
```

### CatalogDb インターフェース（案）

```rust
pub struct CatalogDb { /* SQLite接続 */ }

impl CatalogDb {
    /// cache_dir 配下の適切な場所にDBを開く（なければ作成）
    /// folder_path はドライブ文字付きの実際のパス
    pub fn open(cache_dir: &Path, folder_path: &Path) -> Result<Self>;

    /// フォルダパスを正規化してSHA-256ハッシュ化し、DBパスを返す
    pub fn db_path(cache_dir: &Path, folder_path: &Path) -> PathBuf;

    /// フォルダ内ファイルと照合し、有効なキャッシュと要更新リストを返す
    pub fn validate(
        &self,
        files: &[FileInfo],
    ) -> (Vec<CachedThumb>, Vec<FileInfo>);

    /// サムネイルを保存（INSERT OR REPLACE）
    pub fn save(&self, entry: &ThumbEntry) -> Result<()>;

    /// 存在しなくなったファイルのエントリを削除
    pub fn purge_deleted(&self, existing_filenames: &[&str]) -> Result<()>;
}

pub struct FileInfo {
    pub filename: String,
    pub mtime: i64,
    pub file_size: i64,
}

pub struct CachedThumb {
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub webp_data: Vec<u8>,
}
```

### パスハッシュ計算の実装例

```rust
use sha2::{Sha256, Digest};

fn normalize_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    // ドライブ文字（"C:"）を除去、小文字化、バックスラッシュをスラッシュに
    let no_drive = if s.len() >= 2 && s.chars().nth(1) == Some(':') {
        &s[2..]
    } else {
        &s
    };
    no_drive.to_lowercase().replace('\\', "/")
}

fn path_to_db_path(cache_dir: &Path, folder_path: &Path) -> PathBuf {
    let normalized = normalize_path(folder_path);
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    let prefix = &hash[..2];
    cache_dir.join(prefix).join(format!("{}.db", hash))
}
```

---

## 8. app.rs への統合方針

`load_folder` の変更点：

```rust
// 現在
// → 全画像を rayon で並列デコード・リサイズ

// カタログ追加後
// 1. CatalogDb::open(cache_dir, folder)
// 2. CatalogDb::validate(files) → (cached, needs_reload)
// 3. cached → JPEG → GPU テクスチャ（高速。並列でOK）
// 4. needs_reload → 元ファイルデコード → カタログ保存 → GPU テクスチャ
```

---

## 9. 設定項目（追加）

| 設定名 | 型 | デフォルト | 説明 |
|--------|-----|---------|------|
| `cache_dir` | String | `%APPDATA%\mimageviewer\cache` | サムネイルキャッシュの格納先 |

---

## 10. 注意事項・制限

- カタログファイルはユーザーが削除して再生成できる（手動キャッシュクリア）
- `cache_dir` が存在しない場合は自動作成する（サブディレクトリ含む）
- カタログのバージョンが変わった場合（スキーマ変更時）はそのDBを削除して再生成
- SQLite の WAL モードを使用することでロック競合を最小化
- 同一パス（ドライブ文字違い）のキャッシュ衝突は mtime/file_size チェックで実害なし
