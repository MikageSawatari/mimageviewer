# リモート閲覧: お気に入り横断のコンテナ検索 (本体 Ctrl+S 相当)

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. この増分の範囲

リモートに**検索**を入れる最初の増分。本体 Ctrl+S =「お気に入りを横断して
フォルダ・ZIP・PDF を名前で探す」だけをやる。

**この増分に入れるもの**

- ホームに「検索」タブを 1 つ増やし、語句と種類を指定して検索する
- 結果は通常の一覧画面 (グリッド) として出し、タップでそのフォルダ / ZIP / PDF を開く

**この増分に入れないもの** (後続で別途やる。先回りして作らない)

- 現在地の絞り込み (本体 Ctrl+F)
- タグビュー (本体 Ctrl+T)
- メタデータ検索 (本体 Ctrl+G)
- 閲覧中のタグ付け

## 2. 調査済みの事実 (再調査不要)

### 2.1 本体側の検索範囲

- Ctrl+S の実体は `App::execute_favsearch`
  ([src/app.rs:18837](../src/app.rs))。`search_index_db::SearchIndexDb::search(query,
  &favorite_roots, kind, mode)` を worker thread で呼ぶだけ。
- `SearchIndexDb::search` ([src/search_index_db.rs:324](../src/search_index_db.rs)) の性質:
  - `WHERE favorite_root IN (...)` — 正規化済みお気に入り root 文字列で絞る
  - **動画は常に除外される**。`kind = None` (すべて) でも `kind <> VideoFile` が付く。
    つまり返るのは **Folder / ZipFile / PdfFile の 3 種だけ**
  - `ORDER BY display_name COLLATE NOCASE`、`LIMIT 5000`
  - 語句は `search_query::parse` の token 列。`-除外` と OR/AND モードに対応。本増分は
    **既定の AND のみ**を使う (OR トグルは出さない)
- 索引に行があるのは「コンテナ索引」(`FavoriteEntry::auto_index_structure`) を有効にした
  お気に入りだけ。本体は root filter に**全お気に入り**を渡し、索引の中身側で範囲が決まる。
  → リモートも**全お気に入りの root を渡す**だけで、本体と範囲が一致する。
- 本体 UI での名前は **「コンテナ索引」**
  ([src/ui_dialogs/fav_add.rs:94](../src/ui_dialogs/fav_add.rs) の
  「コンテナ索引 (フォルダ・ZIP・PDF を Ctrl+S で名前検索)」)。利用者向け文言はこの語を使う。

### 2.2 リモート側の受け皿は既にある

- `CollectionEngine` ([src/remote_ipc/collections.rs](../src/remote_ipc/collections.rs)) が
  「候補 (絶対パス) の列 → お気に入り相対の `RemoteEntry` 列」を既に持っている。
  `CandidateEntry` → `to_remote_entries` → `bound_remote_entries` (上限 1000 + `truncated`)。
- お気に入り allowlist の正本は `CollectionEngine.favorite_roots` (`RemoteFavoriteRoots`)。
  **`self.settings.favorites` は起動時 snapshot なので allowlist に使わない。**
  必ず `favorite_roots.current()` を使う ([src/remote_ipc/live_favorites.rs](../src/remote_ipc/live_favorites.rs))。
- `Collection` は `WorkLane::Heavy` で処理される
  ([src/remote_ipc/pipe.rs:1119](../src/remote_ipc/pipe.rs) の `work_lane` の `_ =>`)。
  新しい検索メッセージも同じ扱いでよい (UI thread には乗らない)。
- remote-web 側は IPC 応答に**もう一度**同じ allowlist をかける
  (`Library::retain_allowed_remote_entries`、[crates/remote-web/src/store.rs:234](../crates/remote-web/src/store.rs))。
  検索結果にも必ず通すこと。
- HTTP 応答のログは path のみで**クエリ文字列は落ちている**
  ([crates/remote-web/src/diagnostics.rs:121](../crates/remote-web/src/diagnostics.rs) の `request_path`)。

### 2.3 ブラウザ側

- ホームのタブは `createHomeTabs` ([crates/remote-web/web/app.js:2312](../crates/remote-web/web/app.js))、
  route は `parseRoute` (同 1444)。
- 一覧画面は collection でも folder でも同じ `renderGridScreen` を通る。**スマートフォルダは
  すでにフォルダ / ZIP / PDF を返して正しく開けている**ので、検索結果も同じ経路に乗せれば
  タップして開くまで作り直す必要はない。
- `.home-tabs` は `grid-template-columns: repeat(3, minmax(0, 1fr))` で**3 個決め打ち**
  ([crates/remote-web/web/styles.css:464](../crates/remote-web/web/styles.css))。タブが増えるので直す。
- 文字入力の既定動作は document 側の tap 所有者が残している
  ([crates/remote-web/web/document-double-tap.mjs](../crates/remote-web/web/document-double-tap.mjs) の
  `DEFAULT_TAP_EXCLUSIONS` の `text_input`)。`type="search"` もこの除外に入るので、追加設定は不要。

## 3. 先に直すもの — お気に入り 1 つの不在で全件消える

`map_existing_to_favorite` ([src/remote_ipc/path_guard.rs:39](../src/remote_ipc/path_guard.rs)) が

```rust
for favorite in favorites {
    let root = std::fs::canonicalize(&favorite.path).ok()?;   // ← 関数ごと None で返る
```

になっている。**お気に入りが 1 つでも参照できない** (取り外した外付け / offline の NAS /
削除済みフォルダ) と、その favorite に無関係な候補まで含めて**全件が捨てられる**。
閲覧履歴・レーティング・ブックマークも今この経路を通っている。

検索は本質的に全お気に入りをまたぐので、この欠陥が常時効いてしまう。**この増分で直す。**

- 参照できない root は**その root だけ飛ばす** (`continue` 相当)。他の root の判定は続ける
- ついでに **root の canonicalize を候補ごとに繰り返さない**。候補 1000 件 × お気に入り 5 個で
  5000 回 syscall を呼ぶ形になっている。root 側は 1 回だけ解決して持ち回る
- 既存の呼び出し側 (`to_remote_entries`) のシグネチャは変えなくてよいが、
  root 解決を 1 回にまとめられる形にすること
- 回帰テスト: お気に入り 2 つのうち片方の root を消し、**残った root 配下の候補が返る**こと

## 4. 構造の決め

- **範囲の決定権は本体側 1 か所**。remote-web もブラウザも「どのお気に入りを検索するか」を
  組み立てない。ブラウザが送るのは**語句と種類だけ**。
- **索引の状態は推測させない**。「0 件」と「そもそも索引が無い」は利用者にとって別のこと
  なので、応答に**型として**載せる。ブラウザ側が件数 0 から理由を推測しない。
- **結果は一覧の一種**として返す。`CollectionPayload` を再定義せず**そのまま内包**し、
  検索固有の値 (索引の状態) だけを足す。entries / sort_state / entry_limit / truncated を
  別名で作り直さない。
- **検索は送信で走る**。1 文字ごとに走らせない。回線越しの IPC を打ち続けることになるうえ、
  本体の索引は接頭辞一致ではないので途中の語句に意味が無い。
- **語句をログに残さない**。path のみを記録する現在の方針 (§2.2) を検索でも守る。
  `with_log_details` に語句を入れない。長さと件数までにする。

## 5. プロトコル

`crates/remote-ipc` に検索専用のメッセージを足し、`PROTOCOL_VERSION` を 1 つ上げる。

```rust
pub struct FavoriteSearchRequest {
    pub query: String,
    pub kind: FavoriteSearchKind,
}

pub enum FavoriteSearchKind { All, Folder, Zip, Pdf }   // 動画は索引に無い (§2.1)

pub struct FavoriteSearchPayload {
    pub listing: CollectionPayload,          // 既存型をそのまま内包する
    pub index_state: FavoriteSearchIndexState,
}

pub enum FavoriteSearchIndexState {
    Ready,        // コンテナ索引を有効にしたお気に入りがあり、索引を読めた
    Disabled,     // どのお気に入りにもコンテナ索引が無い = 何を入れても 0 件になる
    Unavailable,  // 索引をまだ開けない (未作成 / 読み取り失敗)
}

pub enum FavoriteSearchResponse { Success(FavoriteSearchPayload), Error(CollectionError) }
```

- `ClientMessage::FavoriteSearch` / `ServerMessage::FavoriteSearch` を追加する。
  `work_lane` は既定の `Heavy` のままでよい。`request_kind` / `operation_description` /
  `service_stopped_response` / `queue_busy_response` など、**メッセージ種別を列挙している
  箇所を全部埋める** (漏れるとその経路だけ無反応になる)。
- 語句の上限は 200 文字。超過と空文字は `CollectionErrorCode::BadRequest`。
- `docs/web-remote-plan.md` §13.5 の現行版数の記述を新しい版数に更新する。

## 6. 本体側 (core)

`src/remote_ipc/` に検索を追加する。置き場所は `collections.rs` に足すか別ファイルにするかは
任せる。判断根拠をコメントに残すこと。

- **索引は読み取り専用で開く。** `SearchIndexDb::open()` は無ければ**作成**し read-write で
  開く ([src/search_index_db.rs:92](../src/search_index_db.rs))。リモート worker がこれを
  呼ぶと、本体が持つ接続と別に書き込み接続をもう 1 本作り、索引が無い環境では空ファイルを
  作ってしまう。`TagsDb::open_readonly` ([src/tags_db.rs:81](../src/tags_db.rs)) と同じ形で
  **read-only の入口を足す**。ファイルが無ければ `Unavailable` を返す (作らない)。
- `index_state` の決め方:
  - お気に入りに `auto_index_structure` が 1 つも無い → `Disabled`
  - 索引を開けない → `Unavailable`
  - それ以外 → `Ready`
- 検索の流れ: `favorite_roots.current()` の全 root を `SearchIndexDb::search` に渡す →
  `IndexEntry` を `CandidateEntry` に写す → 既存の `to_remote_entries` /
  `bound_remote_entries` を通す。
  - `IndexKind` → `RemoteEntryKind` は Folder / Zip / Pdf の 3 対応
  - 並び順は索引側で名前順に確定しているので、`sort_state` は
    `FIXED_LIST_SORT_LOCK_REASON` で固定にする (他の固定一覧と同じ)
  - `title` は語句をそのまま出さず、「検索結果」とする。件数はブラウザ側が
    `entries.length` から出せる
- **上限の扱い**: 索引は最大 5000 行返す。お気に入りへの写像で落ちる分があるので、
  写像は**上限 + 1 件に達した時点で止める** (全件 canonicalize しない)。
  `truncated` は「写像後が上限を超えた」か「索引が 5000 行返した」のどちらかで立てる。
- 索引に残っているだけで**実体が消えているパスは結果に出ない** (`map_existing_to_favorite` が
  canonicalize に失敗して落とす)。これは正しい挙動なので、**索引を消したり書き換えたりしない。**

## 7. remote-web 側

- `GET /api/search/favorites?q=...&kind=...` を追加する。
  - 認証と session owner の扱いは `/api/collection` と同じにする (`with_session_activity` /
    owner の渡し方を含め、既存 collection 経路に揃える)
  - 入場制御は `IpcClass::Heavy`
  - 応答は `Cache-Control: no-store`
  - `retain_allowed_remote_entries` を必ず通す
  - **ログに語句を入れない。** `with_log_details` に載せてよいのは
    語句の長さ・種類・件数・IPC 所要時間・索引状態まで
- Service Worker の扱いは変えない。**検索応答をキャッシュしない。**

## 8. ブラウザ側

### 8.1 ホームの「検索」タブ

- タブを 1 つ増やす (`お気に入り / スマートフォルダ / 場所 / 検索`)。
  `.home-tabs` の 3 列決め打ちを直し、**幅 360px でも全タブが押せる**こと。
  タグビューでもう 1 つ増える予定なので、個数を決め打ちにしない。
- タブの中身:
  - 何を探せるかの 1 行説明。**「お気に入りの中から、フォルダ・ZIP・PDF を名前で探します」**。
    画像 1 枚 1 枚は対象外であることが読み取れる文にする (対象外なのに探して 0 件を見る、を防ぐ)
  - 語句の入力欄 + 種類の選択 (すべて / フォルダ / ZIP / PDF) + 送信
  - `form` の submit で走らせる (携帯のキーボードに実行キーが出る)
  - **入力欄の実効 font-size は 16px 以上**にする。これを下回ると iOS が focus 時に
    ページ全体を勝手に拡大する
- 語句と種類は**このセッションの間だけ**覚えておき、結果から戻ったときに復元する。
  端末に保存しない。

### 8.2 結果画面

- route を 1 つ足す (例 `#search/<encoded>`)。既存の `#collection/...` の正規表現に
  相乗りさせない。語句は `encodeURIComponent` 済みで載せる。
- 描画は**既存の一覧画面をそのまま使う**。タイル生成・サムネイル・仮想スクロール・
  戻り位置の記憶に手を入れない。
- 戻り先は検索タブ。
- 状態別の表示:
  - `Ready` かつ 0 件 → 「一致するフォルダ・ZIP・PDF はありませんでした。」
  - `Disabled` → お気に入りに**コンテナ索引**が設定されていないこと、mIV 本体の
    お気に入り編集で設定できることを伝える。語は本体の UI に合わせる (§2.1)
  - `Unavailable` → 索引をまだ利用できないこと。再試行を促す表現にとどめ、**自動で再試行しない**
  - `truncated` → 既存の件数上限の注意書きを使い回す

## 9. やってはいけないこと

- 索引に無いものを補うために**ファイルシステムを歩く**こと。この検索は索引だけを見る
- ブラウザ側でお気に入りの一覧から検索範囲を組み立てること (§4)
- 語句を localStorage / Service Worker / ログのいずれかに残すこと
- `search_index.db` を書き込みで開く / 無いときに作る / 内容を掃除すること
- 1 文字ごとの自動検索、および結果が空だったときの自動再検索
- 既存の一覧描画・サムネイル経路・sort bar の作り直し
- `document-double-tap.mjs` の除外一覧を、今回の入力欄のために広げること
  (`type="search"` は既に除外に入っている)

## 10. テスト

Rust:

- `map_existing_to_favorite`: お気に入り 2 つで片方の root が無いとき、残りの root 配下の
  候補が返る (§3 の回帰)
- 索引が無い環境で `Unavailable` を返し、**`search_index.db` を作らない**こと
- コンテナ索引が 1 つも無いとき `Disabled` を返すこと
- お気に入りの外にあるパスが索引に入っていても結果に出ないこと。応答 JSON に絶対パスが
  含まれないこと (`collections.rs` の既存テスト
  `favorite_allowlist_drops_outside_entries_and_returns_only_relative_paths` と同じ形)
- 語句の長さ超過 / 空が `BadRequest` になること
- 上限を超える結果で `truncated` が立ち、件数が上限に収まること

web (`crates/remote-web/web/*.test.mjs`):

- route の往復 (語句・種類が encode / decode で保たれる)
- 索引状態ごとの表示分岐 (Ready 0 件 / Disabled / Unavailable が別の文言になる)
- 送信でだけ検索が走ること (入力の変化だけでは走らない)
- 既存の web テスト **235 件**を維持すること

## 11. 確認と報告

- Rust: `cargo test -p mimageviewer --lib` は本体 `src/` に触れるので**全件**流す。
  `crates/remote-ipc` / `crates/remote-web` のテストも流す
- web テスト一式、`python scripts/check_ui_glyphs.py`、`git diff --check`
- **ビルドとコミットは行わない。** 変更ファイルと追加テストの一覧を報告する
- **新しいファイルを追加した場合は明記すること。** 配信の許可リストがビルド時に生成される
  ため、web/ にファイルを足した増分は本体の再ビルドが要る
- 本体 `src/` に触れた箇所は全部と理由を報告する
- 既存の未追跡 brief (`docs/briefs/*.md`) には触れない
