# 閲覧履歴 機能設計

画像の本と動画・音声ファイルを、ユーザーが最近閲覧した順に集めて表示する
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

最近閲覧した画像フォルダ / ZIP / PDF / 変換アーカイブに加え、動画・音声ファイルを
最終閲覧順に一覧化する。画像の本は親コンテナ、動画・音声はファイルを記録単位とする。
既存の「最近開いたフォルダ履歴」はフォルダバーの移動履歴であり、閲覧履歴とは目的が
違うため分離する。

UI 名称は **閲覧履歴** とする。DB テーブル名、kind の既存文字列、
StartupFolderMode::ReadingHistory、address の内部キーなど、永続化・内部識別子は
互換性維持のため変更しない。

## 2. 要件

### 2.1 対象

記録対象は、ユーザー操作でフルスクリーン表示した画像ページ、およびユーザー操作で
再生を開始した動画・音声とする。画像ページは親コンテナ、動画・音声はファイル単位で記録する。

| 開いた項目 | 履歴に記録する単位 | 備考 |
| --- | --- | --- |
| GridItem::Image(path) | 親フォルダ | 通常フォルダ。画像ページだけで読む文脈のときに記録する |
| GridItem::ZipImage | ZIP / CBZ 本体 | 変換アーカイブ閲覧中は元 RAR / 7z / LZH を記録する |
| GridItem::PdfPage | PDF 本体 | パスワード付き PDF は開けた後だけ記録される |
| GridItem::Video(path) | 動画ファイル | 履歴自身に最終位置と尺を保存する |
| GridItem::Audio(path) | 音声ファイル | 履歴自身に最終位置と尺を保存する |

通常フォルダの画像は、画像シークバーが「ページ移動」として成立するのと同じ条件を対象にする。
具体的には、現在の表示順に含まれるナビゲーション対象がすべて画像ページのときだけ、
フォルダを本として記録する。動画・音声を開いた場合はこの画像文脈判定を通さず、
対象ファイルそのものを記録する。

一覧からのオープン、手動のファイル移動、履歴 / ブックマークからのオープンは記録する。
連続再生の EOF 遷移、スライドショーの自動送り、
SlideshowEndAction::NextFolder による遷移は記録しない。
遷移起点は HistoryTrigger の必須引数として記録地点まで伝え、現在の再生状態から推測しない。

製本ルート / 本棚の本も、同じ画像条件を満たすなら閲覧履歴に載せる。ZIP / PDF や
ネスト ZIP / 章分け CBZ は外側コンテナを記録する。深い階層ではページ位置を NULL にし、
再オープンは既存の open / resume 設定に委ねる。

動画・音声の履歴位置は resume DB ではなく閲覧履歴自身の列を使う。完走時に既存の
resume エントリが削除されても、閲覧した事実と履歴位置表示は残る。再オープン位置は
従来どおり resume の規則に従うため、完走した動画は先頭から開く。

### 2.2 検索結果との関係

検索モードごとの扱い:

| モード | 記録 | 理由 |
| --- | --- | --- |
| 通常閲覧 | 対象 | 実フォルダ / ZIP / PDF の読書文脈が明確 |
| Ctrl+S コンテナ検索 | 対象 | フォルダ / ZIP / PDF を探して開く導線なので、閲覧履歴と一致する |
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
閲覧履歴対象に含める。保存するパスは変換後キャッシュ ZIP ではなく、必ず元アーカイブ
パスにする。

既存の `archive_source_override` / `effective_folder()` 規約に従う:

- 記録時: `archive_source_override` があればそれを優先して履歴に保存する
- 表示時: `ConvertibleArchive { path, format }` として履歴ビューに出す
- 再オープン時: `load_folder_or_convert_archive...` 経由で開く
  - 有効な変換キャッシュがあれば即開く
  - キャッシュが無ければ既存の変換確認 / パスワード / 進捗 UI に合流する
  - `archive_file_handling == Ignore` の場合は既存と同じく開かずトーストで知らせる

### 2.4 履歴件数と設定

- 既定で閲覧履歴の記録は ON
- 保持件数は既定 1000
- 最大 1000
- 設定で記録を OFF にできる
- 設定で保持件数を変更できる
- 保持件数を下げた場合は、設定保存時または次回記録時に古い項目から削除する
- 記録 OFF は新規記録を止めるだけで、既存履歴は削除しない
- 既存履歴は環境設定から全削除できる
- 閲覧履歴ビューの右クリックメニューから 1 件ずつ削除できる

設定候補:

```rust
pub reading_history_enabled: bool,      // default true
pub reading_history_limit: usize,       // default 1000, clamp 1..=1000
```

`reading_history_limit = 0` は使わず、記録停止は `reading_history_enabled = false` で表す。
これにより UI 表示と DB pruning の意味が単純になる。

### 2.5 起動時表示

起動時に開く場所へ「閲覧履歴」を追加する。

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
閲覧履歴を表示する。履歴が空なら空状態を表示する。DB が開けない場合もビュー自体は
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

閲覧履歴は、検索結果やドライブ一覧と同じく、実在フォルダではない専用の場所として
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

フォルダバーの履歴 ←/→ では、この synthetic path 自体を back/forward stack に保持する。
場所 / ファイルメニューから開くときは、専用ビューをインストールする前に直前の実フォルダを
back stack へ記録するため、メニュー起動直後から ← で元のフォルダへ戻れる。
履歴から pop したときは実ディレクトリとしてロードせず `enter_reading_history()` へ
ディスパッチし、DB から専用ビューを再構築する。これにより実フォルダへ移動した後も
←/→ で閲覧履歴へ戻れ、`__reading_history__` を開いた空表示には落ちない。

閲覧履歴ビューの items は既存の GridItem を再利用する。folder / zip / pdf / archive は
従来のコンテナ項目、video / audio はそれぞれ Video / Audio として構築する。
専用 `GridItem::ReadingHistoryContainer` は追加しない方針を第一候補にする。理由は、
既存のサムネイル、open、rating、右クリックメニューの分岐を再利用でき、
match arm の追加漏れを減らせるため。代表サムネ固定は現在フォルダを親コンテナとして
記録する操作なので、合成パスの閲覧履歴ビューでは MVP 対象外にする。最終閲覧日時などの履歴メタ情報は
`App.reading_history_rows` のような side table に保持し、idx ではなく正規化キーで参照する。
表示順の再構築や削除で idx がずれてもメタ情報を取り違えないようにする。

履歴ビューは最終閲覧日時の降順を固定し、ソート UI は追加しない。種別絞り込みは
ブックマークと同じ MediaFilter / BookKindFilter を使い、「すべて / 動画 / 音声 / 本」と
本の内訳を同じラベル・順序で表示する。スマートフィルタ / ★フィルタは無効化する。

### 3.2 表示内容

実装済みの表示内容:

- サムネイルまたはコンテナ / メディアアイコン
- 表示名
- サムネイル表示で選択中セルの下に出す最終閲覧日時 / 閲覧位置
- ホバー tooltip の場所 / 最終閲覧日時 / 閲覧位置
- 詳細表示の最終閲覧列 / 閲覧位置列
- ブックマークと同型の種別絞り込み

DB には最終閲覧日時と、本の last_page / page_count、動画・音声の
media_position_ms / media_duration_ms を意味を分けた列として保存する。
存在しない項目は、初回表示時に同期 `exists()` を大量に呼ばない。表示直後に worker で
自動整理して件数を変えると、選択行やスクロール位置が動いて体験が悪くなるため、ユーザーが
項目を開こうとした 1 件だけ確認する。パスの親ドライブ / 共有が利用可能で、対象だけが
確実に存在しない場合は、移動せず「ファイルが見つからない」旨のトーストを出し、その行を
閲覧履歴から削除する。外付けドライブ未接続、ネットワーク共有にアクセスできない、権限エラー
など存在しないと断定できない場合は、移動せず「アクセスできません」と通知し、履歴行は残す。
外付けドライブのドライブレターが変わった場合は、古い行が開けなくなり、新しい行が別 key
として追加される。これは `normalize_keep_drive` を採用する代償として許容する。

### 3.3 操作

- 入口: 製本とは無関係なので製本メニューには置かない。ファイルメニューの
  「閲覧履歴を開く」と、アドレスバー「場所▼」の「閲覧履歴」(ドライブ一覧の下) から開く。
  既存の「最近開いたフォルダ履歴」とは別物だと分かる配置にする。
- ダブルクリック / Enter / Gamepad A: 通常のコンテナ open と同じ。ただし閲覧履歴ビューでは
  `guard_reading_history_open` が先に対象コンテナの存在を 1 件だけ確認する。確実に削除済みなら
  トーストを出してその行を履歴から削除し、コンテナの中には入らない。外付けドライブ未接続など
  存在しないと断定できない場合は、履歴を残したままトーストだけ出す。`note_reading_history_open` が
  戻り先予約 (`reading_history_return_from`) を更新する: 閲覧履歴ビューで本 (コンテナ) を
  開いたら本の container パスを焼き付け、閲覧履歴ビュー以外で別コンテナを開いたら捨てる。
  画像 / ページ (非コンテナ) のオープンでは変えない (本の中でページを読むだけなので保持)
- 本を「親へ戻る」操作で閉じる (Backspace / アドレスバーの親へ戻る / 自動フルスクリーン時の
  Esc 直帰): 実ディレクトリではなく閲覧履歴ビューへ戻る。判定は `reading_history_back_nav()`
  (= `effective_folder()` が `reading_history_return_from` と一致する間だけ ReadingHistory を
  返す)。`grid_parent_nav_target` / `resolve_grid_parent_nav` / `resolve_return_to_parent_nav`
  の 3 経路で同じ判定を通す
- **戻り先予約の寿命管理**:
  - ネスト ZIP の深い階層 (`zip_nav` が root でない) では `reading_history_back_nav` は
    None を返す (`effective_folder()` は深さに関わらず root ZIP のままなので、明示的に弾く)。
    root に戻ってから閲覧履歴へ抜ける = Esc 直帰経路でも深い階層から一気に戻らない
  - 別の本 / フォルダへ明示ナビで出たら予約を捨てる: `load_folder_with_scan` が予約した本
    以外の実フォルダへ移ったときクリア、`enter_drive_list` でもクリア、コンテナの
    再オープン時は `note_reading_history_open` がクリア
  - **変換アーカイブの例外**: 閲覧履歴の RAR/7z/LZH を開くと `open_archive_via_cache` /
    変換完了処理が `load_folder(cache_zip)` を呼ぶため、`cache_zip != 元アーカイブ` で予約が
    `load_folder_with_scan` のクリアに巻き込まれる。`archive_source_override` を元パスへ戻すのと
    同様に、ロード前に「予約が元アーカイブと一致したか」を退避し、ロード成功後に
    `reading_history_return_from = 元アーカイブ` を書き戻す (= 閉じると閲覧履歴へ戻れる)。
    閲覧履歴以外から開いた変換アーカイブでは退避フラグが立たないので誤保持しない
- 閲覧履歴の合成パスは履歴 back/forward スタックに積まない (`record_folder_nav_transition` /
  quick folder target capture で `__search_results__` と同様に除外)。閲覧履歴へ戻るのは上記
  「親へ戻る」専用経路だけにし、Alt+← で実体のない合成パスを開こうとする事故を防ぐ
- Backspace: 閲覧履歴へ来る前の場所に戻る。起動直後はドライブ一覧またはデスクトップへ
- 右クリック:
  - 履歴から削除
  - この本のフォルダに移動 (本を含む実フォルダへ移動して前後の本を探す。
    `parent_folder_for_nav` + `JumpFromSearch` を流用し、戻った先で本を選択状態にする)
  - パスをコピー
  - Explorer で場所を開く
  - 変換アーカイブの場合は既存の変換関連操作に合流
- 環境設定:
  - 記録 ON/OFF
  - 保持件数
  - 履歴を全削除
  - 起動時に閲覧履歴を表示

操作可否は `items_are_drive_list` ではなく、検索結果ビューに近い扱いにする。履歴の各項目は
実パスを持つため、rating、代表サムネ固定、右クリックメニュー、Explorer で場所を開く操作は
原則として有効にする。

履歴から開くときの `auto_fullscreen_zip_pdf` / `book_open_resume` などは既存設定に従う。
履歴ビュー専用の開き方は追加しない。

---

## 4. 永続化設計

### 4.1 DB

DB は次の既存パスを維持する:

    %APPDATA%/mimageviewer/reading_history.db

reading_history テーブルの既存列と既存 kind 文字列は変更しない。動画・音声対応では
リリース済み DB を ALTER TABLE ADD COLUMN で移行し、次の専用列を追加する。

    media_position_ms INTEGER
    media_duration_ms INTEGER

本の last_page / page_count に秒数を格納して意味を混在させない。kind には新規の
video / audio を加えるが、folder / zip / pdf / archive の文字列はそのまま維持する。
パス key も従来どおり path_key::normalize_keep_drive(path) を使う。

一覧読み出しでは、未知 kind を Folder にフォールバックせず、その行だけ読み飛ばす。
これにより今後の新 kind を古い実装が誤った項目種別として扱う事故を防ぐ。ただし、
すでに配布済みの旧版バイナリ自体の挙動は今回の変更では修正できない。

動画・音声の位置更新は既存行に対する UPDATE のみとし、再生進捗の保存だけで新しい履歴行や
最終閲覧順を作らない。履歴行の作成と最終閲覧日時の更新はユーザー操作による open 時だけ行う。

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

- 起動時に閲覧履歴ビューを表示する
- ユーザーが閲覧履歴ビューへ移動する
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

記録地点には必須引数 HistoryTrigger を渡す。

    enum HistoryTrigger {
        UserChosen,
        AutoAdvance,
    }

UserChosen は一覧・履歴・ブックマークからの open と、上下キー / ホイール /
Ctrl+上下による手動ファイル移動に使う。AutoAdvance は次の 5 起点から型付きで伝播する。

1. advance_slideshow
2. try_start_slideshow_next_folder
3. apply_video_continuous_eof_target
4. apply_video_audio_mode_continuous_eof_target
5. apply_music_continuous_eof_target

record_reading_history は AutoAdvance なら記録せず、UserChosen のときだけ項目種別を解決する。
画像は従来の純粋な画像ページ文脈を確認して親コンテナを upsert する。動画・音声は
is_readable_page_idx の画像限定ガードを通さず、対象ファイルを video / audio として upsert する。

HistoryTrigger は open_fullscreen、フォルダ読み込み後の reopen、遅延 ZIP/PDF/変換アーカイブ、
native video source swap、snapshot navigation、連結表示の scroll transition を含む遷移データに保持する。
App に自動進行中フラグを置かず、slideshow_active 等の現在状態からも推測しない。
引数に既定値を設けないため、新しい遷移経路は記録方針を明示しない限りコンパイルできない。

同一フォルダ内の画像自動送りは同じ key の upsert だったため件数は元から増えなかった。
今回、NextFolder で別フォルダへ移った画像も AutoAdvance として記録対象外にし、
自動送りのたびにフォルダ履歴が 1 件ずつ増える既存の穴を塞ぐ。

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
8. `address = "閲覧履歴"`
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
相当の通常経路が安全。ただし MVP では `StartupFolderMode::Previous` から閲覧履歴 synthetic
path を復元しない方針なので、通常経路を使う場合も `settings.last_folder` への保存は
skip する。

提案:

- `StartupFolderMode::ReadingHistory` では直接 `enter_reading_history()`
- ユーザーが明示的に閲覧履歴へ移動した場合も、MVP では `last_folder` に synthetic path を保存しない
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

外部ファイラや SendTo からパスが渡された場合は、従来どおり明示パスを優先し、閲覧履歴起動は
使わない。

### 7.2 Backspace / 履歴ボタン

閲覧履歴は合成ビューなので、検索結果やドライブ一覧に近い扱いにする。

- 閲覧履歴へ入る前の場所があれば Backspace / 履歴戻るで戻る
- 起動直後など戻り先がなければドライブ一覧またはデスクトップへ戻る
- 閲覧履歴内の項目を開いた場合は、通常のフォルダ履歴へ遷移を積む
- 閲覧履歴自体を「最近開いたフォルダ履歴」には入れない

### 7.3 last_folder

MVP では `settings.last_folder` に閲覧履歴の synthetic path を入れない。

候補 A: `Previous` で閲覧履歴も復元する

- 利点: 前回閲覧履歴を見て閉じたら次回も同じ場所から始まる
- 欠点: `last_folder` が実パス前提の処理に漏れると開けない

候補 B: `ReadingHistory` モードの時だけ起動する

- 利点: 実パス前提の既存処理への影響が小さい
- 欠点: 前回表示場所としての一貫性は少し落ちる

MVP は候補 B を推奨する。`StartupFolderMode::ReadingHistory` が明示設定されているときだけ
閲覧履歴で起動し、`Previous` 用の `last_folder` には実コンテナを保存し続ける。

---

## 8. 実装内容

1. ReadingHistoryKind に video / audio を追加し、メディア位置専用列を migration する。
2. record_reading_history と全 open / reopen 経路へ必須の HistoryTrigger を通す。
3. 5 つの自動進行起点を AutoAdvance、手動起点を UserChosen とする。
4. 閲覧履歴ビューで MediaFilter / BookKindFilter を有効化する。
5. UI・マニュアル・製品ページ・設計文書の表示名称を「閲覧履歴」に統一する。
6. 未知 kind の行は一覧読み出し時にスキップする。

---

## 9. テスト観点

Unit / App-level:

- 同じ key の upsert は重複せず、保持上限 1000 件の規則が変わらない
- リリース済み schema へ media_position_ms / media_duration_ms が追加される
- 動画・音声の進捗更新は既存行だけを更新し、新規履歴を作らない
- 未知 kind は Folder に変換されず行ごと読み飛ばされる
- 動画・音声は UserChosen で記録され、AutoAdvance では記録されない
- slideshow / NextFolder / video EOF / video-audio EOF / music EOF の 5 起点が AutoAdvance
- 手動の一覧 open / ファイル移動は UserChosen
- 完走時に resume が削除されても閲覧履歴行は残る
- 動画 / 音声 / 本と本の内訳のフィルタ結果がブックマークと同じ意味になる

Manual / smoke:

- 画像フォルダ、ZIP、PDF、変換アーカイブを手動で読むと本として履歴へ出る
- 動画・音声を手動で開くとファイル単位で履歴へ出る
- 上下キー / ホイール / Ctrl+上下で手動移動した項目は履歴へ出る
- 連続再生 EOF、スライドショー自動送り、NextFolder では履歴件数が増えない
- 完走した動画は履歴に残り、次回は既存 resume 規則により先頭から開く
- 種別絞り込みで動画 / 音声 / 本と本の内訳を分けられる
- 起動時表示、履歴からの open、履歴からの削除が従来どおり動く

UI snapshot:

- 環境設定ページに閲覧履歴設定が表示される
- 閲覧履歴ビューの空状態 / 通常状態
- 閲覧履歴ビューの動画 / 音声 / 本フィルタ

---

## 10. §1.40 で確定した判断

- UI 名称は「閲覧履歴」。既存の永続値と内部識別子は変更しない。
- 動画・音声はファイル単位で履歴へ加え、位置と尺は専用列へ保存する。
- 自動進行の 5 経路は HistoryTrigger::AutoAdvance を必須引数で伝播し、記録しない。
- 種別絞り込みはブックマークの MediaFilter / BookKindFilter を流用する。
- resume の削除条件や「続きから」の既存挙動は変更しない。
- 保持上限は従来どおり 1000 件。
