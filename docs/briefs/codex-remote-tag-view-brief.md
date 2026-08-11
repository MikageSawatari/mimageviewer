# リモート閲覧: タグビュー (本体 Ctrl+T 相当)

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

直前の増分「お気に入り横断のコンテナ検索」(commit `67436106`) の続き。同じ形をなぞる。

## 1. この増分の範囲

ホームに「タグ」タブを 1 つ増やし、**タグを見つけて、そのタグが付いた項目を開ける**ようにする。

**入れるもの**

- タグの一覧 (ピン留め / 最近 / よく使う + 名前順の全件) を出し、**名前で絞り込める**
- タグを選ぶ / 語句を入れて実行すると、そのタグが付いた項目が一覧で出る
- 種類 (すべて / フォルダ / 画像 / 動画 / 音声 / ZIP / PDF / アーカイブ) で絞れる

**入れないもの**

- **閲覧中のタグ付け・タグ編集**。この増分は読み取りだけ。tags.db へ書かない
- 現在地の絞り込み (本体 Ctrl+F) — **利用者判断で見送り**。フォルダが分かっていれば足りる
- メタデータ検索 (本体 Ctrl+G) — 単ファイルが大量に並ぶため本体側の設計から見直す。今回は対象外

## 2. 調査済みの事実 (再調査不要)

### 2.1 本体のタグビュー

[src/tag_view.rs](../src/tag_view.rs) が worker 本体を持っている。

- `run_tag_view_search(data_dir, query, kind_filter, cancel)` の中身:
  - 語句が空 → `summaries` (全タグ) だけ返す。項目は返さない
  - 語句あり → **完全一致 → (複数語かつ完全一致無しなら) 語の AND → 接頭辞** の順で
    `item_key` を引く。この「何に一致するか」の規則が正本
  - 各 `item_key` を `classify_tag_view_path` で実パス分類する。`fs::metadata` を 1 件ずつ叩き、
    実体が無いキーは**結果から隠すだけ** (tags.db は絶対に変更しない)
  - `TAG_VIEW_RESULT_LIMIT = 10_000`
- `classify_file_kind` は**拡張子が未知のファイルを `Folder` に倒す**。
  実フォルダかどうかは手前の `meta.is_dir()` で分かれている (§4 で扱う)
- タグ名の正規化は `tags_db::normalize_tag_display_name` / `normalize_tag_key` (NFKC + 小文字)。
  `#` は剥がされる。表示は `format_display_tag` が `#` を付け直す

### 2.2 本体のタグ一覧の組み立て

`App::tag_view_menu_sections` ([src/ui_main.rs:11818](../src/ui_main.rs)) が
**ピン留め / 最近 / よく使う**の 3 区画を作っている。

- ピン留め = `settings.tags` (`TagDef`) のうち `show_shortcut`。名前順
- 最近 = `last_applied_at` の新しい順、上位 20
- よく使う = `count` の多い順、上位 20
- 3 区画は重複を除外する (先に出たタグは後の区画に出さない)
- 表示名は `settings.tags` の名前で上書きし、無ければ `TagSummary::tag` を使う

`self.tag_view.summaries` と `self.settings.tags` だけから決まる**純粋な規則**で、
App の他の状態には依存していない。

### 2.3 tags.db

- `TagsDb::open_readonly(path)` が既にある ([src/tags_db.rs:81](../src/tags_db.rs))。
  **`open_at` は作成 + schema 初期化を行う**ので、リモートからは呼ばない
- `tag_summaries()` は全タグを名前順で返す (`tag`, `tag_key`, `count`, `last_applied_at`)
- **`count` は mIV 全体の件数**で、お気に入りの外に付けたタグも数に入る

### 2.4 リモート側 (直前の増分でできている形)

- `CollectionEngine` ([src/remote_ipc/collections.rs](../src/remote_ipc/collections.rs)) の
  `favorite_search` が手本。`favorite_roots.current()` → 索引 → `CandidateEntry` →
  `to_remote_entries_bounded` → `bound_remote_entries` (上限 1000)
- 状態を型で返す形も既にある (`FavoriteSearchIndexState`)
- `PROTOCOL_VERSION` は **33**。この増分で 34 に上げる
- HTTP は `/api/search/favorites` が手本。`IpcClass::Heavy`、`Cache-Control: no-store`、
  `retain_allowed_remote_entries` を必ず通す、**語句をログに残さない**
- ブラウザは `#search/<kind>/<query>` の route と `showFavoriteSearch` が手本。
  結果画面は既存の一覧描画をそのまま使う

## 3. 構造の決め

- **一覧の組み立て規則は 1 か所。** §2.2 の 3 区画の規則を、`App` メソッドから
  `(summaries, settings.tags)` だけを引数に取る**自由関数へ切り出し**、本体 UI と
  リモートの両方がそれを呼ぶ。ブラウザ側で並べ替え・上位 N・重複除外をやり直さない。
- **「何に一致するか」も 1 か所。** §2.1 の item_key 選択規則 (完全一致 → 語の AND →
  接頭辞) を関数へ切り出し、本体の tag view worker とリモートの両方が呼ぶ。
- **タグの絞り込みは端末の中で行う。** 1 文字ごとに本体へ問い合わせない。名前順の全タグを
  1 回受け取り、ブラウザ内で部分一致で絞る。項目の検索だけが本体への要求になる。
- **件数の意味を偽らない。** `count` は mIV 全体の数で、この端末から開けるのはお気に入りの
  中だけ。件数を作り直して合わせようとせず、**一覧の先頭に 1 行だけその旨を書く**。
  (お気に入り前提の数を別に数えると、junction 越しの項目などで結局ずれる)
- **読み取りだけ。** tags.db を書き込みで開かない・作らない・掃除しない。

## 4. 種類の対応 — 未知のファイルをフォルダにしない

`classify_file_kind` は拡張子が未知の**ファイル**を `TagViewItemKind::Folder` に倒す
(§2.1)。この値をそのまま `RemoteEntryKind::Folder` に写すと、ファイルがフォルダのセルとして
並び、開こうとして失敗する。

- **実フォルダ (`meta.is_dir()`) だけを `Folder` にする。** 拡張子が未知のファイルは
  `RemoteEntryKind::Other` にする
- 本体側の分類はこの増分では変えない (本体 UI の既存挙動を動かさない)
- 未知拡張子のファイルが `Other` になり、フォルダとして出ないことをテストで固定する

## 5. プロトコル

`crates/remote-ipc` に 2 つ足し、`PROTOCOL_VERSION` を 34 にする。

```rust
// (1) タグ一覧
pub struct TagBrowseRequest;

pub struct RemoteTagChoice {
    pub name: String,     // 表示名 (# は付けない)
    pub count: usize,     // mIV 全体の件数 (§3)
}

pub struct TagBrowsePayload {
    pub pinned: Vec<RemoteTagChoice>,
    pub recent: Vec<RemoteTagChoice>,
    pub popular: Vec<RemoteTagChoice>,
    /// 名前順の全タグ。端末内の絞り込み用。上限を超えたら truncated。
    pub all: Vec<RemoteTagChoice>,
    pub all_truncated: bool,
    pub state: TagIndexState,
}

pub enum TagIndexState {
    Ready,
    Empty,        // tags.db はあるがタグが 1 つも無い
    Unavailable,  // tags.db が無い / 開けない
}

// (2) タグが付いた項目
pub struct TagItemsRequest {
    pub tag: String,
    pub kind: TagItemKind,
}

pub enum TagItemKind { All, Folder, Image, Video, Audio, Zip, Pdf, Archive }

pub struct TagItemsPayload {
    pub listing: CollectionPayload,   // 既存型をそのまま内包する
    pub state: TagIndexState,
}
```

- 応答はどちらも `Success(..) | Error(CollectionError)` の形にする
- `ClientMessage` / `ServerMessage` に 2 つずつ足し、**メッセージ種別を列挙している箇所を
  全部埋める** (`request_kind` / `message_owner` / `operation_description` /
  `response_outcome` / `service_stopped_response` / `queue_busy_response` / `id()`)。
  `work_lane` は既定の `Heavy` のままでよい
- タグ語句の上限は 200 文字。空・超過は `CollectionErrorCode::BadRequest`
- `all` の上限は 2000 件
- `docs/web-remote-plan.md` §13.5 の現行版数を 34 に更新する

## 6. 本体側 (core)

- tags.db が無ければ `Unavailable` を返す。**作らない。** `TagsDb::open_readonly` を使う
- タグ一覧: `tag_summaries()` → §3 の切り出した関数で 3 区画 → `all` は名前順で 2000 件まで
- 項目検索:
  - §3 の切り出した item_key 選択規則でキーを引く
  - キーを 1 件ずつ実パス分類し、`favorite_roots.current()` の allowlist へ写す
  - **上限 + 1 件に達したら分類を止める。** 全 10,000 件を `fs::metadata` で舐めない。
    キーは `item_key COLLATE NOCASE` 順で来るので、先頭から取る打ち切りは決定的になる
  - `truncated` は「写像後が上限を超えた」か「キー取得が上限に達した」で立てる
  - 実体が無いキー・お気に入りの外のキーは結果から落とすだけ。**tags.db を変更しない**
- 種類フィルタは §4 の対応で `TagItemKind` へ写す

## 7. remote-web 側

- `GET /api/tags` (一覧) と `GET /api/tags/items?tag=...&kind=...` を足す
- 認証・session owner・`IpcClass::Heavy`・`Cache-Control: no-store`・
  `retain_allowed_remote_entries` は `/api/search/favorites` と同じにする
- **タグ名をログに残さない。** `with_log_details` に載せてよいのは
  タグ名の長さ・種類・件数・状態・IPC 所要時間まで
- Service Worker は変えない。**タグの応答をキャッシュしない**

## 8. ブラウザ側

### 8.1 「タグ」タブ

- ホームのタブを 1 つ増やす (`お気に入り / スマートフォルダ / 場所 / 検索 / タグ`)。
  タブの列は既に可変 (`repeat(auto-fit, ...)`)。**幅 360px で 5 つとも押せる**ことを確認する
- 中身:
  - 件数の意味を書いた 1 行 (§3)
  - **タグ名の絞り込み欄**。入力すると `all` を端末内で部分一致で絞り、一覧を差し替える。
    本体へは問い合わせない
  - 絞り込み欄が空のときは **ピン留め / 最近 / よく使う**の 3 区画を出す。
    入力中は絞り込んだ平坦な一覧にする
  - 種類の選択 (すべて / フォルダ / 画像 / 動画 / 音声 / ZIP / PDF / アーカイブ)
  - タグを押すと項目一覧へ進む。**絞り込み欄の語句をそのまま実行する経路も残す**
    (一覧に出ていないタグを直接叩けるように。`all_truncated` のときはこれが頼りになる)
  - 入力欄の実効 font-size は 16px 以上 (iOS の focus 時拡大よけ)
- 絞り込みの語句と種類は**このセッションの間だけ**覚え、戻ったときに復元する。端末に保存しない

### 8.2 項目一覧

- route を 1 つ足す (例 `#tag/<kind>/<encoded tag>`)。既存の route の正規表現に相乗りさせない
- 描画は既存の一覧画面をそのまま使う。タイル生成・サムネイル・仮想スクロール・
  戻り位置の記憶に手を入れない
- 題は `favoriteSearchResultTitle` と同じ考えで、**押した / 入力したタグを画面に残す**
  (`「#タグ」の項目` の形。長い名前は表示だけ丸める)
- 戻り先はタグタブ
- 状態別:
  - `Ready` かつ 0 件 → 「このタグの項目は見つかりませんでした。」
  - `Empty` → タグがまだ 1 つも無いこと
  - `Unavailable` → タグをまだ利用できないこと。**自動で再試行しない**
  - `truncated` → 既存の件数上限の注意書きを使い回す
- 画像・動画・音声もタグの対象なので、**セルから通常どおり開けること** (コンテナ検索と違い
  1 枚の画像や 1 本の動画が並ぶ)。`state.images` の作り方は集約ビューと同じ導出にする

## 9. やってはいけないこと

- tags.db への書き込み・作成・掃除。閲覧中のタグ付け UI
- 1 文字ごとに本体へタグ一覧や項目を問い合わせること
- ブラウザ側で 3 区画の並べ替え・上位 N・重複除外をやり直すこと (§3)
- 件数をお気に入り前提で数え直して「合っているように見せる」こと (§3)
- タグ名を localStorage / Service Worker / ログのいずれかに残すこと
- 実体が無いキーを見つけたときに tags.db から消すこと
- 既存の一覧描画・サムネイル経路・sort bar の作り直し
- 本体 UI (`ui_main.rs` のタグビュー) の見た目・挙動を変えること。
  §3 の切り出しは**呼び出し先を移すだけ**で、結果が変わってはいけない

## 10. テスト

Rust:

- 切り出した 3 区画の関数が、切り出し前と同じ結果を返すこと (ピン留めの表示名上書き、
  上位 20、区画をまたぐ重複除外)
- 切り出した item_key 選択規則が、完全一致 → 語の AND → 接頭辞 の順で効くこと
- tags.db が無い環境で `Unavailable` を返し、**tags.db を作らない**こと
- タグが 0 件のとき `Empty` を返すこと
- お気に入りの外にタグが付いていても項目一覧に出ないこと。応答 JSON に絶対パスが
  含まれないこと
- 実体が無いキーが結果から落ち、**tags.db が変化しない**こと
- 未知拡張子のファイルが `Folder` ではなく `Other` になること (§4)
- 上限を超える結果で `truncated` が立ち、件数が上限に収まること
- タグ名の空 / 200 文字超が `BadRequest` になること

web (`crates/remote-web/web/*.test.mjs`):

- route の往復 (タグ名・種類が encode / decode で保たれる)
- 端末内の絞り込みが部分一致で効き、絞り込み中は平坦な一覧になること
- 状態ごとの表示分岐 (Ready 0 件 / Empty / Unavailable が別の文言になる)
- 題に押した / 入力したタグが出ること
- 既存の web テスト **240 件**を維持すること

## 11. 確認と報告

- Rust: 本体 `src/` に触れるので `cargo test -p mimageviewer --lib` を**全件**流す。
  `crates/remote-ipc` / `crates/remote-web` のテストも流す
- web テスト一式、`python scripts/check_ui_glyphs.py`、`cargo fmt --all -- --check`、
  `git diff --check`
- **ビルドとコミットは行わない。** 変更ファイルと追加テストの一覧を報告する
- **新しいファイルを追加した場合は明記すること** (配信の許可リストがビルド時に生成される)
- 本体 `src/` に触れた箇所は全部と理由を報告する
- **`htdocs/` と `README.md` は触らない。** mIV Remote は未公開なので、公開ページへは
  出荷が決まってからまとめて書く
- 既存の未追跡 brief (`docs/briefs/*.md`) には触れない
