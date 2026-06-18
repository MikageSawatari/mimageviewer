# 読書履歴 機能設計

フォルダ / ZIP / PDF など「画像ページを読む単位」を最近読んだ順に集めて表示する
専用ビューの要件と実装設計メモ。ClaudeCode レビュー後、MVP を実装済み
(2026-06、`src/reading_history_db.rs` / `App::enter_reading_history`)。

関連ドキュメント:

- [architecture-overview.md](architecture-overview.md) — App 状態層、永続化ストア、仮想フォルダの全体像
- [virtual-folders.md](virtual-folders.md) — ZIP / PDF / 変換アーカイブの current_folder / effective_folder 規約
- [ui-responsiveness.md](ui-responsiveness.md) — UI スレッド同期 I/O 回避
- [search-architecture.md](search-architecture.md) — Ctrl+S / Ctrl+G の責務分離
- [spec.md](spec.md) — 実装時に反映するユーザー向け仕様

---

## 1. 目的

最近読んだフォルダ / ZIP / PDF を一覧化し、読みかけや最近読んだ本へ戻りやすくする。
既存の「最近開いたフォルダ履歴」はフォルダバーの移動履歴であり、読書履歴とは目的が
違うため分離する。

UI 名称は **読書履歴** とする。「本履歴」も短いが、既存機能の「製本」「本棚」と
混同しやすいため、画面上の場所名は「読書履歴」、説明文では「最近読んだ本」を使う。

---

## 2. 要件

### 2.1 対象

記録対象は、フルスクリーンで `Image` / `ZipImage` / `PdfPage` を開いたときの
親コンテナとする。動画は対象外。

| 開いたページ | 履歴に記録する単位 | 備考 |
| --- | --- | --- |
| `GridItem::Image(path)` | 親フォルダ | 通常フォルダ。画像ページだけで読む文脈のときに記録する |
| `GridItem::ZipImage { zip_path, .. }` | ZIP / CBZ 本体 | 変換アーカイブ閲覧中は元 RAR / 7z / LZH を記録する |
| `GridItem::PdfPage { pdf_path, .. }` | PDF 本体 | パスワード付き PDF は開けた後だけ記録される |

通常フォルダは、画像シークバーが「ページ移動」として成立するのと同じ条件を対象にする。
具体的には、現在の表示順に含まれるナビゲーション対象がすべて `has_page_data`
(`Image` / `ZipImage` / `PdfPage`) のときだけ記録する。フォルダ、動画、ZIP/PDF 本体、
変換アーカイブ、検索コンテナなどが混在する通常フォルダは対象外。

製本ルート / 本棚の本も、同じ条件を満たすなら読書履歴に載せる。検索インデックスでは
チャーン回避のため除外される場合があるが、本棚はコレクション、読書履歴は時系列という
役割の違いがあるため、読んだ事実は履歴として扱う。

ZIP / PDF は開いた中身がページ一覧であるため対象にする。ネスト ZIP や章分け CBZ の
深い階層を読んでいる場合も、読書履歴では外側コンテナ (`zip_path`、または変換元
アーカイブ) を記録する。既存の `book_resume` と異なり、読書履歴は「本へ戻る」一覧であり、
深い階層のページ位置復元までは MVP で担わない。`zip_nav.at_root() == false` の場合は
`last_page` / `page_count` を NULL にして、再オープンは既存の open / resume 設定に委ねる。

### 2.2 検索結果との関係

検索モードごとの扱い:

| モード | 記録 | 理由 |
| --- | --- | --- |
| 通常閲覧 | 対象 | 実フォルダ / ZIP / PDF の読書文脈が明確 |
| Ctrl+S コンテナ検索 | 対象 | フォルダ / ZIP / PDF を探して開く導線なので、読書履歴と一致する |
| Ctrl+G アイテム検索 | 対象外 | ページ / ファイル単位の検索で、親コンテナを読んだとみなすか曖昧 |
| タグビュー | 対象外 | Ctrl+G と同じく合成ビューの意味が強い |
| Ctrl+F 現在地フィルタ | 対象 | 現在の実コンテナ内の一時絞り込みなので、通常閲覧の延長 |

Ctrl+S の結果一覧そのものにはフォルダ / ZIP / PDF のコンテナセルしかないため、そこで
フルスクリーンページを開くことはない。Ctrl+S からフォルダ / ZIP / PDF を開いた後、
その中の `Image` / `ZipImage` / `PdfPage` をフルスクリーン表示した時点で記録する。

実装ガードとしては、`global_search.active` または `items_are_global_search_view` が
真なら記録しない。`tag_view.active` または `items_are_tag_view` も同様に記録しない。
`favsearch.active` は Ctrl+S で開いた実コンテナ内でも真のままになり得るため、これだけで
除外してはいけない。Ctrl+S の結果グリッド自体はコンテナセルだけで `is_readable_page_idx`
が false になる想定なので、基本的には `favsearch` 専用ガードは不要。

Ctrl+F は現在の表示順に対する一時フィルタなので、判定もフィルタ後の
`current_grid_order()` に基づく。たとえば画像と動画が混在するフォルダでも、Ctrl+F で
画像ページだけに絞って読んだ場合は記録対象になり得る。これは Ctrl+F を通常閲覧の延長と
みなす仕様として許容する。

### 2.3 変換アーカイブ

RAR / 7z / LZH / CBR / CB7 などの変換アーカイブは、ユーザー視点では 1 つの本なので
読書履歴対象に含める。保存するパスは変換後キャッシュ ZIP ではなく、必ず元アーカイブ
パスにする。

既存の `archive_source_override` / `effective_folder()` 規約に従う:

- 記録時: `archive_source_override` があればそれを優先して履歴に保存する
- 表示時: `ConvertibleArchive { path, format }` として履歴ビューに出す
- 再オープン時: `load_folder_or_convert_archive...` 経由で開く
  - 有効な変換キャッシュがあれば即開く
  - キャッシュが無ければ既存の変換確認 / パスワード / 進捗 UI に合流する
  - `archive_file_handling == Ignore` の場合は既存と同じく開かずトーストで知らせる

### 2.4 履歴件数と設定

- 既定で読書履歴の記録は ON
- 保持件数は既定 1000
- 最大 1000
- 設定で記録を OFF にできる
- 設定で保持件数を変更できる
- 保持件数を下げた場合は、設定保存時または次回記録時に古い項目から削除する
- 記録 OFF は新規記録を止めるだけで、既存履歴は削除しない
- 既存履歴は環境設定から全削除できる
- 読書履歴ビューの右クリックメニューから 1 件ずつ削除できる

設定候補:

```rust
pub reading_history_enabled: bool,      // default true
pub reading_history_limit: usize,       // default 1000, clamp 1..=1000
```

`reading_history_limit = 0` は使わず、記録停止は `reading_history_enabled = false` で表す。
これにより UI 表示と DB pruning の意味が単純になる。

### 2.5 起動時表示

起動時に開く場所へ「読書履歴」を追加する。

```rust
pub enum StartupFolderMode {
    Previous,
    Desktop,
    Specific,
    Drives,
    ReadingHistory,
}
```

起動時モードが `ReadingHistory` の場合は、実フォルダではなく専用の合成ビューとして
読書履歴を表示する。履歴が空なら空状態を表示する。DB が開けない場合もビュー自体は
空で表示し、トーストとログで知らせる。

起動先の解決は `known_folders::startup_folder()` の戻り値だけで分岐しない。既存の
`Drives` も `None` を返すため、`ReadingHistory` も `None` 扱いにすると判別できなくなる。
呼び出し側は `StartupFolderMode` の enum variant を見て、`Drives` と `ReadingHistory` を
それぞれ明示分岐する。

`StartupFolderMode` に variant を追加するため、設定の deserialize は未知の値で設定全体が
壊れないようにする。旧版へ戻した場合や将来 variant が増えた場合は、未知値を `Previous`
など安全な既定値へフォールバックする。

履歴ビューから項目を開く時のページ位置は、既存設定に従う。MVP では履歴専用の
「続きから / 先頭から」設定は追加しない。

将来追加するなら次のような設定を足せるよう、実装では履歴オープン経路を 1 箇所に集約する:

```rust
pub enum ReadingHistoryOpenMode {
    FollowDefault,
    Resume,
    FirstPage,
}
```

---

## 3. UI 仕様

### 3.1 専用ビュー

読書履歴は、検索結果やドライブ一覧と同じく、実在フォルダではない専用の場所として
表示する。

実装候補:

```rust
fn reading_history_synthetic_path() -> PathBuf {
    crate::data_dir::get().join("__reading_history__")
}
```

裸の相対名ではなく、`search_results_synthetic_path()` と同じく data_dir 配下の絶対パスにする。
履歴ビューは実パスを持つ項目を並べる合成ビューなので、ドライブ一覧より検索結果ビューに
近い扱いに寄せる。

App 状態には `items_are_reading_history_view: bool` を追加する。

読書履歴ビューの `items` は、既存の `GridItem` を再利用する:

| DB kind | GridItem |
| --- | --- |
| `folder` | `GridItem::Folder(path)` |
| `zip` | `GridItem::ZipFile(path)` |
| `pdf` | `GridItem::PdfFile(path)` |
| `archive` | `GridItem::ConvertibleArchive { path, format }` |

専用 `GridItem::ReadingHistoryContainer` は追加しない方針を第一候補にする。理由は、
既存のサムネイル、open、rating、右クリックメニューの分岐を再利用でき、
match arm の追加漏れを減らせるため。代表サムネ固定は現在フォルダを親コンテナとして
記録する操作なので、合成パスの読書履歴ビューでは MVP 対象外にする。最終閲覧日時などの履歴メタ情報は
`App.reading_history_rows` のような side table に保持し、idx ではなく正規化キーで参照する。
表示順の再構築や削除で idx がずれてもメタ情報を取り違えないようにする。

履歴ビューでは最近読んだ順を保つ必要があるため、ツールバーのソートや詳細ヘッダソートは
無効化する。MVP では表示モード自体は既存設定を壊さないが、詳細表示でも
`current_grid_order()` / `details_order` は DB の最近順を保つ。列ヘッダクリックによる並べ替えは
読書履歴ビューでは無効にする。スマートフィルタ / ★フィルタも MVP では無効化する。
履歴内検索は将来機能とする。

### 3.2 表示内容

実装済みの表示内容:

- サムネイルまたはコンテナアイコン
- 表示名
- ホバー tooltip に場所 (フルパス) / 最終閲覧日時 / 既読位置 (複数フォルダ・ドライブの
  同名本を場所で判別できるよう、場所をフルパスで先頭に出す)
- 詳細表示モードでは `更新日時` 列を `最終閲覧` 列、`状態` 列を `既読位置` 列に読み替えて
  別々の列に表示する (1 列に押し込めない)

DB には最終閲覧日時と、取れる場合は `last_page` / `page_count` を保存する。MVP では
表示は通常のグリッド/詳細表示を再利用しつつ、`App.reading_history_rows` side table から
最終閲覧日時や `12 / 120` のような補助表示を引く。

存在しない項目は、初回表示時に同期 `exists()` を大量に呼ばない。MVP では開こうとして
失敗したときにトーストを出し、右クリックやメニューから削除できるようにする。余裕があれば
「存在しない項目を整理」ボタンで worker に検査させ、まとめて削除する。外付けドライブの
ドライブレターが変わった場合は、古い行が開けなくなり、新しい行が別 key として追加される。
これは `normalize_keep_drive` を採用する代償として許容し、整理機能で回収する。

### 3.3 操作

- 入口: 製本とは無関係なので製本メニューには置かない。ファイルメニューの
  「読書履歴を開く」と、アドレスバー「場所▼」の「読書履歴」(ドライブ一覧の下) から開く。
  既存の「最近開いたフォルダ履歴」とは別物だと分かる配置にする。
- ダブルクリック / Enter: 通常のコンテナ open と同じ
- Backspace: 読書履歴へ来る前の場所に戻る。起動直後はドライブ一覧またはデスクトップへ
- 右クリック:
  - 開く
  - 履歴から削除
  - パスをコピー
  - Explorer で場所を開く
  - 変換アーカイブの場合は既存の変換関連操作に合流
- 環境設定:
  - 記録 ON/OFF
  - 保持件数
  - 履歴を全削除
  - 起動時に読書履歴を表示

操作可否は `items_are_drive_list` ではなく、検索結果ビューに近い扱いにする。履歴の各項目は
実パスを持つため、rating、代表サムネ固定、右クリックメニュー、Explorer で場所を開く操作は
原則として有効にする。

履歴から開くときの `auto_fullscreen_zip_pdf` / `book_open_resume` などは既存設定に従う。
履歴ビュー専用の開き方は追加しない。

---

## 4. 永続化設計

### 4.1 DB

新規 DB を推奨:

```text
%APPDATA%/mimageviewer/reading_history.db
```

`book_resume.db` に同居させる案もあるが、読書履歴は一覧表示・削除・prune・存在確認など
責務が異なるため、別 DB の方が管理しやすい。

schema 案:

```sql
CREATE TABLE IF NOT EXISTS reading_history (
    key TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    archive_format TEXT,
    title TEXT NOT NULL,
    last_read_at_ms INTEGER NOT NULL,
    last_page INTEGER,
    page_count INTEGER,
    file_size INTEGER,
    mtime_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_reading_history_last_read_at
    ON reading_history(last_read_at_ms DESC);
```

`kind` は `folder` / `zip` / `pdf` / `archive`。`archive_format` は `rar` / `7z` /
`lzh` / `zip` など、`ArchiveFormat` へ戻せる文字列を保存する。通常 ZIP / PDF /
フォルダでは NULL。

`key` は `path_key::normalize_keep_drive(path)` を第一候補にする。読書履歴は実際に開く
パスのリストなので、`book_resume.db` のようなドライブ文字除外よりも、別ドライブの
同名パスを衝突させないことを優先する。将来、外付けドライブのドライブレター変化に
追従したい場合は、別途ボリューム ID / ファイル ID の採用を検討する。keep-drive の代償として、
外付けドライブのドライブレターが変わると旧行と新行が別履歴として並ぶ可能性がある。
MVP では「存在しない項目を整理」で回収する。

`path` はユーザー視点の実パスをそのまま保存する。変換アーカイブでは元アーカイブパスであり、
キャッシュ ZIP パスを保存してはいけない。

`book_resume.db` とは key を共有しないし、共有する必要もない。既存の `record_book_resume`
は `current_folder` を保存するため、変換アーカイブではキャッシュ ZIP 側の key になる。
一方、読書履歴は元アーカイブを保存し、開くときに `load_folder_or_convert_archive...` 経由で
既存の変換 / resume 経路へ合流する。したがって履歴用の path 解決 helper は
`book_resume` の解決を流用せず、別に持つ。

`last_page` / `page_count` は補助情報。履歴から開く挙動は既存の `book_resume.db` に従うため、
この値は UI 表示と将来の直接ジャンプ用であり、MVP の open source of truth にはしない。

### 4.2 Reader / Writer

`book_resume_db.rs` と同じく、読み取り用ハンドルと書き込み worker を分ける。SQLite の
journal / busy timeout / pragma も `book_resume_db` と同じ方針にして、多重起動や既存 DB 群との
挙動差を作らない。

候補:

```rust
pub struct ReadingHistoryDb { conn: rusqlite::Connection }
pub struct ReadingHistoryWriter { tx: Sender<ReadingHistoryCommand>, handle: JoinHandle<()> }

pub enum ReadingHistoryCommand {
    Upsert(ReadingHistoryRecord),
    RemoveKey(String),
    Clear,
    Prune { limit: usize },
}
```

書き込みはページ移動のたびに発生し得るため、UI スレッドで SQLite INSERT / UPDATE を
行わない。`App::open_fullscreen` / フルスクリーン内ページ移動は writer へ command を
送るだけにする。履歴 entry の file size / mtime 補完も `ReadingHistoryDb::upsert` の直前、
つまり通常記録経路では `reading-history-writer` 側で行い、UI スレッドで `Path::metadata()` を
呼ばない。

読み取りは以下のタイミングだけなので UI スレッド同期でも許容しやすい:

- 起動時に読書履歴ビューを表示する
- ユーザーが読書履歴ビューへ移動する
- 環境設定で件数を表示する

ただし DB open が cold で遅い可能性があるため、`App::new_from_settings` で一度 open し、
ビュー表示時に毎回 open しない。DB が開けなければ履歴機能は no-op + トースト / ログにする。

### 4.3 Prune

上限は 1000 件だが、ページ送りのたびに upsert が発生し得るため、毎回 prune しない。
writer 側で upsert の結果が新規 INSERT だったとき、または保持件数の設定変更時だけ prune する。
同じ key の UPDATE では件数が増えないため prune 不要。

```sql
DELETE FROM reading_history
WHERE key NOT IN (
    SELECT key FROM reading_history
    ORDER BY last_read_at_ms DESC
    LIMIT ?1
);
```

保持件数の設定変更時は、設定保存後に `ReadingHistoryCommand::Prune { limit }` を送る。

`last_read_at_ms` / `last_page` の更新も key 単位で dedup / throttle する。ページ位置は補助表示で
あり、多少 stale でも open source of truth ではない。必要なら「同一 key は N 秒に 1 回まで」
または「フルスクリーン終了時に最終位置を flush」のような方針にする。

---

## 5. 記録ロジック

記録は `record_book_resume(idx)` と同じ発火点に置く。ただし発火点だけを揃え、
key / path 解決は読書履歴専用 helper で行う。`record_book_resume` は `current_folder` を
保存するため、変換アーカイブではキャッシュ ZIP 側の key になり得るが、読書履歴では
ユーザー視点の元アーカイブパスを保存する。

候補:

```rust
pub(crate) fn record_reading_history(&mut self, idx: usize) {
    if !self.settings.reading_history_enabled { return; }
    if self.global_search.active || self.items_are_global_search_view { return; }
    if self.tag_view.active || self.items_are_tag_view { return; }
    if !self.is_readable_page_idx(idx) { return; }
    if !self.current_context_is_pure_image_pages(idx) { return; }

    let Some(mut record) = self.reading_history_record_for_idx(idx) else { return; };
    if self.zip_nav.as_ref().is_some_and(|n| !n.at_root()) {
        record.last_page = None;
        record.page_count = None;
    }

    let key = record.key.clone();
    if self.last_reading_history_key.as_ref() == Some(&key)
        && !self.reading_history_touch_due(key.as_str())
    {
        return;
    }

    self.reading_history_writer.record(record, self.settings.reading_history_limit);
    self.last_reading_history_key = Some(key);
}
```

`current_context_is_pure_image_pages` は、フルスクリーンのページシークバーが実際にページ
ジャンプとして動く条件と揃える。既存の `fullscreen_seek_info` は `ui_fullscreen.rs`
内部にあるため、共通化する場合は新しい走査を書かず、既存の `build_image_reading_indices` /
`build_nav_indices` / `count_seek_overlay_non_image_items` を移設または共通 helper 化する。
純粋 helper は「現在の表示順から image page indices と non-image count を作る」責務にする。

`reading_history_record_for_idx` の解決規則:

1. `archive_source_override` がある場合はそれを保存し、`kind=archive` にする
2. `GridItem::Image(path)` は現在の実フォルダ (`current_folder`、必要なら item の親) を保存し、
   `kind=folder`
3. `GridItem::ZipImage { zip_path, .. }` は `zip_path` を保存し、`kind=zip`
4. `GridItem::PdfPage { pdf_path, .. }` は `pdf_path` を保存し、`kind=pdf`
5. `current_folder` が `search_results_synthetic_path()` や `reading_history_synthetic_path()`
   のような合成パスなら保存しない

Ctrl+S 経由で開いた実コンテナは `favsearch.active` が真でも記録対象にする。Ctrl+G とタグ
ビューは対象外なので、検索合成ビューから親コンテナを逆算する複雑な分岐は MVP では持たない。

`last_read_at_ms` は upsert 時の現在時刻。`last_page` は `idx` ではなく、可能なら
画像ページだけの reading position (`1-based`) を保存する。`page_count` も同じ helper から
取れる場合に保存する。

---

## 6. 履歴ビュー生成

`App::enter_reading_history()` を追加する。

処理の流れ:

1. 現在の実ビューの `folder_history` を保存する
2. フルスクリーンや pending worker を閉じる / cancel する
3. `reading_history_db.list_recent(limit)` で `Vec<ReadingHistoryEntry>` を取得する
4. entry を既存 `GridItem` へ変換する
5. `install_new_items(items, image_metas)` を呼ぶ
6. `items_are_reading_history_view = true`
7. `current_folder = Some(reading_history_synthetic_path())`
8. `address = "読書履歴"`
9. `rebuild_visible_indices()`

`install_new_items` は既定で `items_are_global_search_view` / `items_are_tag_view` /
`items_are_drive_list` を false に戻す。`items_are_reading_history_view` も同じリセットブロックに
必ず追加し、通常フォルダを開いた後に履歴ビュー状態が残留しないようにする。
`enter_reading_history()` では `install_new_items` の後で `items_are_reading_history_view = true`
にする。

同時に `use_full_path_cache_keys()` は履歴ビューでも true を返すようにする。履歴には
別フォルダの同名 ZIP / PDF / フォルダが並び得るため、basename ベースのサムネイル cache key
を使ってはいけない。この変更は上記のリセットとセットで入れる。

`start_loading_items` を通すか、Ctrl+G の `replace_search_view_items` に近い軽量経路にするかは
実装時に判断する。サムネイル worker / catalog / converted archive cache path refresh を
通常どおり動かしたいので、MVP は `start_loading_items(reading_history_synthetic_path(), ...)`
相当の通常経路が安全。ただし MVP では `StartupFolderMode::Previous` から読書履歴 synthetic
path を復元しない方針なので、通常経路を使う場合も `settings.last_folder` への保存は
skip する。

提案:

- `StartupFolderMode::ReadingHistory` では直接 `enter_reading_history()`
- ユーザーが明示的に読書履歴へ移動した場合も、MVP では `last_folder` に synthetic path を保存しない
- 将来 `StartupFolderMode::Previous` で synthetic path を復元するなら、
  `resolve_openable_path` とは別に `is_reading_history_synthetic_path` を先に判定する

---

## 7. 起動 / ナビゲーション

### 7.1 起動

`App::new_from_settings` / 初回フレームの startup open 解決に `StartupFolderMode::ReadingHistory`
を追加する。これは実パス解決 worker に投げず、UI 側で `enter_reading_history()` を呼ぶ。
`known_folders::startup_folder()` の `Option<PathBuf>` だけで判断せず、`StartupFolderMode` の
variant を先に見る。`Drives` と `ReadingHistory` がどちらも「実パスなし」になっても、
呼び出し側で混同しないようにする。

外部ファイラや SendTo からパスが渡された場合は、従来どおり明示パスを優先し、読書履歴起動は
使わない。

### 7.2 Backspace / 履歴ボタン

読書履歴は合成ビューなので、検索結果やドライブ一覧に近い扱いにする。

- 読書履歴へ入る前の場所があれば Backspace / 履歴戻るで戻る
- 起動直後など戻り先がなければドライブ一覧またはデスクトップへ戻る
- 読書履歴内の項目を開いた場合は、通常のフォルダ履歴へ遷移を積む
- 読書履歴自体を「最近開いたフォルダ履歴」には入れない

### 7.3 last_folder

MVP では `settings.last_folder` に読書履歴の synthetic path を入れない。

候補 A: `Previous` で読書履歴も復元する

- 利点: 前回読書履歴を見て閉じたら次回も同じ場所から始まる
- 欠点: `last_folder` が実パス前提の処理に漏れると開けない

候補 B: `ReadingHistory` モードの時だけ起動する

- 利点: 実パス前提の既存処理への影響が小さい
- 欠点: 前回表示場所としての一貫性は少し落ちる

MVP は候補 B を推奨する。`StartupFolderMode::ReadingHistory` が明示設定されているときだけ
読書履歴で起動し、`Previous` 用の `last_folder` には実コンテナを保存し続ける。

---

## 8. 実装ステップ案

1. `docs/spec.md` に仕様を反映する
2. `settings.rs`
   - `reading_history_enabled`
   - `reading_history_limit`
   - `StartupFolderMode::ReadingHistory`
   - sanitize / default / unknown variant fallback / tests
3. `reading_history_db.rs` を追加する
   - schema
   - `list_recent`
   - `upsert`
   - `remove`
   - `clear`
   - `prune` は新規 INSERT 時または設定変更時のみ
   - background writer
   - `book_resume_db` と同じ SQLite pragma / busy timeout 方針
4. `App` 初期化
   - DB open
   - writer spawn
   - `last_reading_history_key`
   - `items_are_reading_history_view`
   - `reading_history_rows`
5. 記録経路
   - `record_reading_history(idx)`
   - `open_fullscreen` / continuous reading reanchor / seek overlay jump から呼ぶ
   - Ctrl+G / tag view 除外
   - Ctrl+S 実コンテナ許可
   - 変換アーカイブは `archive_source_override` 優先
   - ネスト ZIP / 章階層では外側コンテナだけ記録し、ページ位置は NULL
   - `build_image_reading_indices` / `build_nav_indices` / `count_seek_overlay_non_image_items` を共通化
   - 同一 key の連続更新を throttle
6. 読書履歴ビュー
   - `enter_reading_history`
   - data_dir 配下の synthetic path
   - `install_new_items` のリセットブロックに `items_are_reading_history_view` を追加
   - `use_full_path_cache_keys()` に履歴ビューを追加
   - 最近順固定
   - empty state
   - `last_folder` には synthetic path を保存しない
7. UI
   - ファイルメニュー「読書履歴を開く」 + アドレスバー「場所▼」の「読書履歴」(ドライブ一覧の下)
   - 環境設定の記録 ON/OFF / 件数 / 全削除 / 起動時選択
   - 右クリック「履歴から削除」
8. 起動
   - `StartupFolderMode::ReadingHistory`
   - `startup_folder()` の `None` ではなく mode variant で分岐
   - DB failure / empty fallback
9. テスト
   - DB upsert / prune / remove / clear
   - 記録対象判定
   - Ctrl+G 除外、Ctrl+S 実コンテナ許可
   - 変換アーカイブが元パスで記録される
   - 件数 clamp

---

## 9. テスト観点

Unit / App-level:

- `ReadingHistoryDb` の upsert は同じ key を重複させず `last_read_at_ms` を更新する
- limit 3 の状態で 4 件 upsert すると古い 1 件だけ消える
- `reading_history_limit` は 1..=1000 に clamp される
- `reading_history_enabled=false` では writer command が送られない
- `GridItem::Video` は記録されない
- `global_search.active=true` では `Image` でも記録されない
- `favsearch.active=true` かつ実コンテナ内では記録される
- `archive_source_override=Some(original.rar)` では cache ZIP ではなく `original.rar` が保存される
- 変換アーカイブでは `book_resume` と読書履歴の key が一致しなくても、履歴から開く経路で
  既存 resume に合流できる
- ネスト ZIP / 章階層を読んだ場合は外側コンテナが記録され、`last_page` / `page_count` は NULL
- 合成パス (`search_results_synthetic_path`, `reading_history_synthetic_path`) は記録されない
- 混在フォルダ (Image + Video / Folder / ZipFile) は記録されない
- Ctrl+F で混在フォルダを画像ページだけに絞った場合は記録される
- `install_new_items` 後に `items_are_reading_history_view` が false へ戻る
- 履歴ビューでは `use_full_path_cache_keys()` が true になり、通常フォルダへ戻ると false に戻る
- Details 表示でも読書履歴の最近順が保たれ、列ヘッダソートが効かない
- `StartupFolderMode::ReadingHistory` は `Drives` と混同せず、mode variant で分岐される
- 未知の `StartupFolderMode` 値は設定全体を壊さず安全な既定値へフォールバックする

Manual / smoke:

- 通常フォルダの画像だけフォルダを読む → 読書履歴に出る
- 画像 + 動画混在フォルダを読む → 出ない
- ZIP / PDF を読む → 出る
- RAR / 7z を変換して読む → 元アーカイブとして出る
- 章フォルダを持つ CBZ を読む → 外側 CBZ として出る
- Ctrl+S で ZIP / PDF を検索して読む → 出る
- Ctrl+G で画像を検索して読む → 出ない
- Ctrl+F で画像だけに絞って読む → 絞り込み後が画像ページだけなら出る
- 読書履歴ビューから ZIP / PDF を開く → 既存の自動フルスクリーン / 続きから設定に従う
- 読書履歴ビューで rating / 開く / コピー / 履歴から削除などの右クリック操作が使える
- 読書履歴ビューでは代表サムネ固定が表示されない
- 記録 OFF にして読む → 件数が増えない
- 起動時表示を読書履歴にする → 読書履歴ビューで起動する

UI snapshot:

- 環境設定ページに読書履歴設定が表示される
- 読書履歴ビューの空状態
- 読書履歴ビューの通常状態

---

## 10. レビューしてほしい点

- 「読書履歴」という名称でよいか。「最近読んだ本」を補助ラベルにする方針で違和感がないか。
- 通常フォルダの対象判定を「現在のナビゲーション対象がすべて画像ページ」にすることで、
  ユーザー期待と実装都合のバランスが取れているか。
- Ctrl+G / タグビューを対象外、Ctrl+S を対象にする切り分けで十分か。
- ネスト ZIP / 章階層では外側コンテナだけ記録し、ページ位置は NULL にする判断でよいか。
- 製本ルート / 本棚の本も読書履歴に載せてよいか。MVP 方針は「載せる」。
- `reading_history.db` を `book_resume.db` とは別にする判断でよいか。
- 履歴ビューで既存 `GridItem` を再利用し、履歴メタ情報を side table に持つ設計でよいか。
- 履歴ビューは表示モードを強制せず、詳細表示でも最近順固定・列ソート無効にする方針でよいか。
- `StartupFolderMode::Previous` では読書履歴 synthetic path を復元せず、
  明示的な `ReadingHistory` 起動モードだけにする MVP 方針でよいか。
