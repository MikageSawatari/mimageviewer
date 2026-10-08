# v4.0.0 出荷前レビュー D: コレクションの mIV Remote 統合

対象: `docs/collection-remote-plan.md` Phase 5 の製品実装 (core `src/remote_ipc/persistent_collections.rs`、
protocol `crates/remote-ipc`、service `crates/remote-web/src/{http,ipc_client,store}.rs`、
Web UI `crates/remote-web/web/app.js`)。

**本レビューはソース読解のみで、実機・ブラウザ・テスト実行を一切行っていない。**
「再現した」「動作を確認した」に相当する観測はこちらには存在しない。以下はすべて
「コードを読んだ限りの推定」であり、根拠に `file:line` を添える。実機確認が要る項目は
その旨を明記する。

## 要約

| 重要度 | 件数 |
| --- | --- |
| P1 (認証・閲覧範囲の逸脱 / Remote からのデータ変更 / クラッシュ) | **0** |
| P2 (誤動作・不整合) | **4** (D-1 〜 D-4) |
| P3 | **10** (D-5 〜 D-14) |

read-only 強制・認証境界・二重 path 検証については、protocol 上も HTTP 上も
逸脱を見つけられなかった (§「問題なしと判断した項目」参照)。P2 は性能・PC との表示
不整合・稀な経路の誤着地に集中している。

---

## P2

### D-1 / P2 / catalog 要求が core の単一 Home worker を最長 9 秒占有し、`/api/home` と `/api/list` を巻き添えにする

**根拠**

- `work_lane()` が `PersistentCollectionCatalog` を `Home` レーンへ割り当てる:
  `src/remote_ipc/pipe.rs:1891-1893` (`Home | FolderList | PersistentCollectionCatalog => WorkLane::Home`)
- Home レーンの worker は **1 本だけ**: `src/remote_ipc/pipe.rs:780-790`、起動ログも
  `home_workers=1` (`src/remote_ipc/pipe.rs:900`)
- catalog の予算は 9 秒: `EXACT_REQUEST_BUDGET = Duration::from_secs(9)`
  (`src/remote_ipc/persistent_collections.rs:40`)、`catalog()` は
  `deadline = Instant::now() + EXACT_REQUEST_BUDGET` を使い
  (`src/remote_ipc/persistent_collections.rs:251`)、`wait_actor_reply` の
  `default(remaining)` で最長 deadline まで actor 応答を待つ
  (`src/remote_ipc/persistent_collections.rs:760-768`)
- 加えて `catalog()` の再試行は外側 16 回 × `exact_catalog()` 内側 16 回
  (`persistent_collections.rs:252, 676`) で、revision churn 中は deadline まで回り続ける
- Home キューは容量 8 の `sync_channel` (`src/remote_ipc/pipe.rs:71, 767`)。溢れると
  `TrySendError::Full` で Busy 応答になる (`src/remote_ipc/pipe.rs:3111`)
- remote-web 側で `/api/home` と `/api/list` はどちらも同じ core メッセージを出す
  (`crates/remote-web/src/http.rs:2605` の `home`、`http.rs:3268` の `api_list` →
  `ipc_client.rs:828` の `folder_list`)

**失敗シナリオ (推定)**

PC で大きな import / 手動並べ替えを実行して collection actor のキューが詰まっている間に、
端末がセッションを取得する。session acquire は favorites / home / saved_collections を
同時に投げる (`crates/remote-web/web/app.js:2309` `refreshAfterSessionAcquire`)。
catalog が Home worker を最大 9 秒占有し、その間 `/api/home` は着手されない。
続けてフォルダを開こうとすると `/api/list` も同じ 1 本の後ろに並び、キューが 8 件を
超えると Busy (503) が返る。**コレクションを一度も開いていない利用者の通常の
フォルダ閲覧が止まる**。

`docs/collection-remote-plan.md` §4.2 は「collection actor が Starting / Busy / Failed でも、
お気に入り・場所・smart folder の Home を表示できるようにする」を要件にしている。
payload は確かに分離されているが、**実行レーンを分離していないため要件の意図
(Home が collection の状態に引きずられない) は満たされていない**。

**修正方向**

- catalog を Home レーンから外す (Heavy か専用レーン)、または
- Home レーンで動く要求だけ短い予算 (例 1.5 秒) にして `Starting` / `Busy` を早期に返し、
  端末側の再読込に委ねる。`EXACT_REQUEST_BUDGET` を要求種別ごとに分ける形が素直。

---

### D-2 / P2 / next / prev / EOF のたびにコレクション全体を再 prepare する (エントリあたり stat 2 回 + canonicalize 1 回 + JSON 直列化 2 回)

**根拠**

- `navigate()` は毎回 `exact_prepared()` を呼ぶ (`src/remote_ipc/persistent_collections.rs:437`)
- `exact_prepared()` → `prepare_collection_snapshot_while()` が **全 entry に対して**
  `inspect_collection_source()` (= `metadata()` 相当) を実行
  (`src/remote_ipc/persistent_collections.rs:716`、`src/collection_store/prepare.rs:383-387`)
- 続く `view_facts()` が `stream_bounded_wire_entries(exact.prepared.entries…, wire_entry, …)` で
  **全 entry** を変換する (`src/remote_ipc/persistent_collections.rs:198-213`)。
  `wire_entry()` は entry ごとに
  - `inspect_collection_source()` を **もう一度** (`persistent_collections.rs:1088`)
  - `path_guard::resolve_existing()` = `std::fs::canonicalize` (`persistent_collections.rs:1115`、
    `src/remote_ipc/path_guard.rs:80`)
  を行う
- 同じ entry を 2 回 JSON 直列化する: `wire_entry_budget_cost()` 内の
  `serialized_json_len(wire)` (`persistent_collections.rs:791`) と、view token digest の
  `serde_json::to_vec(wire)` (`persistent_collections.rs:210`)
- spread が Single でない場合はさらに `cached_prepared_collection_landscape_flags_while()` が
  親フォルダごとに catalog DB を開き、未カタログ画像を実読みする
  (`persistent_collections.rs:179-184`、`src/remote_ipc/collections.rs:869-1000`)
- prepared snapshot の memo は存在しない。`collection_revision` / `view_token` を鍵にした
  キャッシュは `src/collection_store/prepare.rs` にも engine 側にもない

**失敗シナリオ (推定)**

数千〜数万件のコレクションを端末で開き、ページ送りを続ける。1 ステップごとに
全 entry の stat / canonicalize / 直列化が走り、heavy worker を長時間占有する。
`long_field_wire_retention_is_single_pass_and_budget_bounded`
(`persistent_collections.rs:1863-1903`) は 100,000 件の変換が 9 秒未満で終わることを
固定しているが、これは **1 ページ送りあたり** の下限側コスト (実 I/O なしの合成データ) で
あり、実ファイルに対する stat / canonicalize は含まない。UI thread ではないので本体は
固まらないが、Remote のページ送り応答と heavy worker の空きを圧迫する。

なお `image_display_unit_for_target()` は着地対象のページだけを再検証する設計に
なっており (`persistent_collections.rs:939-1001`)、そこは正しい。問題は
**その手前の `view_facts()` が常に全件を舐める**ことにある。

**修正方向**

`(collection_id, collection_revision, GridDisplayOrder, spread request)` を鍵にした
prepared / view-facts の短命 memo を engine 内に持ち、revision / watch が動いたときだけ
再構築する。少なくとも `wire_entry` の `inspect_collection_source()` 再実行は
prepare の結果 (`entry.availability`) と統合して 1 回にできる。

---

### D-3 / P2 / コレクション root の動画サムネイルだけ「同名画像 sidecar」が効かない

**根拠**

- 永続 entry は常に `thumbnail_address: None` を返す
  (`src/remote_ipc/persistent_collections.rs:1117-1123`)
- core のサムネイル生成は sidecar 解決に `source_address` (= 端末が返してくる
  `thumbnail_address`) を要求する: `resolve_video_sidecar(resolved, source_address)`
  (`src/remote_ipc/thumbnail.rs:450-453, 499-548`)。`source_address` が無ければ
  pin → Shell の順に落ちる
- 通常フォルダ一覧は sidecar を埋める
  (`src/remote_ipc/container.rs:3461-3467`)、集約ビューも複数フォルダ横断で
  `RemoteThumbnailSources::for_remote_entries` を通す
  (`src/remote_ipc/collections.rs:1229-1230`)
- 端末側は `entry.thumbnail_address ?? entryAddress(entry)` を使う
  (`crates/remote-web/web/app.js:7034`)

**失敗シナリオ (推定)**

同じ動画ファイルが、通常フォルダ経由では同名画像のサムネイルで出るのに、
コレクション root では Shell 生成のフレームで出る。
`docs/collection-spec-proposal.md:37-38` は PC のコレクション root について
「動画は pin、同名画像、Shell の既存優先順位を維持する」を要件にしているので、
PC と Remote で表示が食い違う。

**修正方向**

集約ビューと同じ `RemoteThumbnailSources::for_remote_entries(&settings, …)` を
`wire_entry` 生成の前段に通し、Video entry に `thumbnail_address` を埋める。
コレクションは集約ビューと同じ「複数フォルダ横断」形状なので、
`for_remote_entries` がそのまま使える (`src/remote_ipc/mod.rs:105-127`)。

---

### D-4 / P2 / deep link 着地の ordinal 基準が `navigable_media`、その後のシーク要求は `still_image` 基準 — 基準がずれたまま `locate_ordinal` を送る

**根拠**

- deep link 解決は `target_kind: "navigable_media"` で navigate する
  (`crates/remote-web/web/app.js:5014-5027`)
- core はその `target_kind` で `target_count` / `target_ordinal` を作る
  (`src/remote_ipc/persistent_collections.rs:530-535, 594-597`、
  `remote_eligible_entries()` は `resolved_kind_matches_navigation` で
  NavigableMedia = Image|Video|Audio)
- 端末はその位置をそのまま `sparsePosition` に格納する
  (`app.js:5046-5050` → `installPersistentSparseImageTarget(target.group, targetPosition)`
  → `app.js:7344-7349` で `context.sparsePosition = position`)
- シークバーは `sparsePosition` を最優先で使う
  (`app.js:5398-5407` `currentPersistentImagePosition`、`app.js:5409-5424` `viewerSeekSnapshot`)
- シーク確定は `target_kind: "still_image"` + `locate_ordinal = groupIndex` で送る
  (`app.js:7756-7768` `commitSeekGroup` → `enqueuePersistentCollectionNavigation(1, "still_image", …, groupIndex)`)
- core 側 `current_ordinal_candidates()` は `still_image` 射影の n 番目を取る
  (`src/remote_ipc/persistent_collections.rs:1488-1504`)

**失敗シナリオ (推定)**

`[画像, 動画, 画像, 動画, 画像]` のコレクションで、**root prefix に載っていない entry**
(= truncation された大規模コレクション、または relink 後) へ deep link
`#saved-collection/<id>/entry/<entry-id>` で入る。
`state.entries.findIndex` が外れるので `locatePersistentCollectionRouteTarget` 経路に入り
(`app.js:4945-4962`)、`navigable_media` 基準で `target_count = 5` / `ordinal = 2` が入る。
シークバーは max=4 を表示するが、そこからのシークは画像 3 件の射影で解釈されるので、
ordinal 3 / 4 は候補ゼロ → `Boundary { TargetUnavailable }` →
「次の項目を開けませんでした」のトーストになり、ordinal 0〜2 も意図と別の画像に着地する。

1 ステップ進めば `still_image` 基準の `sparsePosition` に置き換わるので
(`app.js:7488-7495`)、**着地直後の 1 回だけ**ずれる。

**副次 (表示のみ)**: 動画・音声 deep link も `navigable_media` 基準の
`n / total` をタイトルに出し (`app.js:5083-5088`)、次の EOF / 手動送りで
動画のみ基準に切り替わって数字が飛ぶ。

**修正方向**

deep link 解決の navigate を、着地した媒体種別で 1 回だけ ordinal を取り直すか、
`locate_entry_id` 解決時は着地 kind に対応する `target_kind`
(`still_image` / `video` / `audio`) で再問い合わせして位置を確定する。
あるいは `sparsePosition` に「どの射影の ordinal か」を持たせ、
射影が一致しないときはシークバーを出さない。

---

## P3

### D-5 / P3 / `image_count` / `target_count` は remote-web の二重検証より前の値で、ブロックされた行が差し引かれない

`validate_persistent_snapshot_addresses()` は core が `Available` としたエントリを
remote-web 側の `validate_remote_file_kind` で落とし、`BlockedByRemotePolicy` へ書き換え、
page group からも外す (`crates/remote-web/src/http.rs:2833-2864`)。しかし
`payload.image_count` / `entry_limit` / `truncated` は書き換えない。
端末の `imageCount` はその値を使う (`app.js:5100`) 一方、ordinal は検証後の
`rootBinding.images` から引く (`app.js:5402-5406`)。両者の基準がずれるので、
core と remote-web の判定が食い違った件数だけシークバーの総数が過大になる。
navigate の `target_count` (`persistent_collections.rs:612`) も同様。

食い違いが起きる条件は、core の `inspect_collection_source` と remote-web の
`classify_entry` (`crates/remote-web/src/store.rs:299-327`) の kind 判定差、または
prepare と HTTP 応答の間でのファイル消失。**発生頻度は低いと推定するが、
起きたときは無言でずれる。**

### D-6 / P3 / `grid-unavailable` に CSS が存在しない

`renderGridTile` は unavailable 行に `tile.disabled = true` と
`tile.classList.add("grid-unavailable")` を付ける (`app.js:6831-6835`) が、
`crates/remote-web/web/styles.css` に `.grid-unavailable` も `.grid-tile:disabled` も
**1 つも定義がない** (`styles.css` 全体を grep して 0 件、`index.html` / `styles.css` に
collection 由来の差分自体が無い)。`.grid-tile` は `cursor: pointer` を無条件に付ける
(`styles.css:940`)。結果、「見つかりません」行は `!` グリフと `type-badge` の文字だけで
区別され、淡色化も枠色変更も無い。実機での見え方は未確認。

### D-7 / P3 / catalog タブが `order` を使わず、件数表示も明示的な再読込も無い

- catalog payload は `order` (Manual / Standard) を運ぶ
  (`crates/remote-ipc/src/lib.rs:1355-1360`) が、タブは `homeCard(icon, name)` しか作らず
  `row.order` を捨てている (`app.js:5171-5182`)。entry 件数も出ない
- 成功時のタブには再読込ボタンが無い (エラー時のみ `app.js:5146`)。
  `applyRemoteStateGeneration` も catalog を触らない (`app.js:2346-2366`) ので、
  PC で作成・改名しても session 再取得かアプリ再読込 (`CommandName.RELOAD_APP`) までは
  反映されない。**favorites / places と同じ更新規則**なので不整合ではないが、
  PC で編集 → 端末で確認という本機能の使い方では気付きにくい

### D-8 / P3 / ホームタブが 6 つになり、375px 幅でラベルが収まらない可能性

`createHomeTabs` が `collections` を加えて 6 タブになった (`app.js:3805-3812`)。
`.home-tabs` は `grid-template-columns: repeat(auto-fit, minmax(56px, 1fr))`
(`styles.css:514-522`)。375px 端末では 1 列あたり約 56px に対し、
`font-size: clamp(0.76rem, 3.2vw, 0.92rem)` = 約 12px で
「スマートフォルダ」(8 文字) / 「コレクション」(6 文字) は折り返す。
`.home-tab` に `overflow` / `word-break` 指定は無い (`styles.css:524-535`)。
**実機での見え方は未確認。** 5 タブ時点でも同種の圧迫はあったが、
6 タブでさらに狭くなる。

### D-9 / P3 / ZIP / PDF を collection root から開いた後の「上へ」が物理親フォルダへ戻る

`loadContainer` は `rootOpenReturnHash({ hasCollection, atFavoriteRoot: isFavoriteRoot(effectiveAddress.path), … })`
を使う (`app.js:4561-4566`)。`rootOpenReturnHash` は
`hasCollection && atFavoriteRoot` のときだけ collection hash を返す (`app.js:4690-4697`)。
コレクションに登録された ZIP はお気に入り root ではないので `fallbackHash`
= `containerParentHash(address)` = ZIP の物理親フォルダ (`app.js:7106-7120`) になり、
`state.gridReturnHash` へ入る (`app.js:4635`)。
`CommandName.PARENT_FOLDER` はこの値をそのまま使う (`app.js:3052-3062`)。

ブラウザの Back (履歴) は `navigate(containerHash(address), { returnHash: state.gridHash, … })`
(`app.js:3313-3317`) のおかげで collection root へ戻るので、**「戻る」は正しく、
「上へ」だけが逸れる**。`docs/collection-remote-plan.md` §7 の
「戻る時は saved collection route を最新 load し、entry ID → source identity で
root を reanchor する」は Back 経路でのみ成立している。
集約ビュー (bookshelf など) から ZIP を開いたときも同じ式を通るので、
**永続コレクション固有の退行ではなく既存経路の継承**。

### D-10 / P3 / `BlockedByRemotePolicy` 行は、Remote が住所を発行しない path のファイル名を端末へ出す

`wire_entry` は `ResolveError::NetworkPath | InvalidPath` を
`BlockedByRemotePolicy` にして address を返さない (`persistent_collections.rs:1124-1129`) が、
`name` は常に `entry_name(&entry.source_path)` = leaf ファイル名を返す
(`persistent_collections.rs:1111, 1151, 1615-1620`)。
端末は「Remote非公開」バッジと共にその名前を表示する
(`app.js:6851-6855, 6981-6988`)。

通常フォルダ一覧では `\\server\share\...` 系の path はそもそも列挙に乗らないので、
**コレクション root は「Remote が開けない path のファイル名を出す唯一の面**になる。
Remote の公開範囲は「本体が開ける範囲と同じ」(`docs/web-remote-plan.md` §3.1) なので
権限逸脱ではなく、行を残すこと自体はプラン §4.3 の要件だが、
「名前まで出す」が意図どおりかを一度確認したほうがよい
(代替: `last_known_kind` だけを出して名前を伏せる)。

### D-11 / P3 / App が collection actor を先に落とせない保証が「型」ではなく「呼び出し側の規律」になっている

`docs/collection-remote-plan.md` §5.2 は
「runtime event receiver は runtime の shutdown 権限から分離し、App は event を poll できるが
actor を先に shutdown できない**型にする**」と書いている。
製品経路は確かに `install_process_owned_collection_runtime(client, events)` を使い
(`src/lib.rs:1401`)、actor の join 権限は `run_native` の外に残る
(`src/lib.rs:1502-1515`)。

しかし `CollectionUiState` には `runtime: Option<CollectionStoreRuntime>` が残っており
(`src/ui_dialogs/collections.rs:477`)、`install_collection_runtime()` (`collections.rs:1353`) で
App が runtime を所有でき、`shutdown_for_exit()` が
`runtime.shutdown_and_join()` を呼ぶ (`collections.rs:569-571`)。
これは `App::on_exit` 内の `shutdown_collection_runtime_for_exit()`
(`src/app.rs:74566`) から、**Remote server 停止・producer drain より前**に走る。

`install_collection_runtime` の呼び出しは現状すべてテスト
(`src/app/collection_grid.rs:1639` / `src/app/collection_navigation.rs:2897` /
`src/ui_main.rs:22354`、いずれも `#[cfg(test)]` ブロック内) なので**実害は無い**。
ただし「型で禁じる」という合意条件は満たしていない。テスト専用 API を
別型 (テスト用 owner) に分けるか、`#[cfg(test)]` を付けるのが素直。

### D-12 / P3 / 公開文書にコレクションの記述が無い (リリース Phase 1 で漏れやすい)

`htdocs/mimageviewer/privacy.html` / `index.html` / `manual/remote.html` に
「コレクション」の語が 1 件も無い (3 ファイルを grep して 0 件)。

- `privacy.html:134-145`「端末内に保存されるデータ」の列挙に、新しい
  `collection.db` (コレクション名 + 参照先の絶対パス) が入っていない
- `index.html:1217-1223`「💾 データはあなたの PC の中だけ」の列挙も同様
- `manual/remote.html` に、端末側のコレクション一覧・閲覧 (読み取り専用) の説明が無い

**偽になっている主張は見つからなかった**: `index.html:1200-1205`
「通信するのは 3 つの場面だけ」は、コレクション閲覧が既存のリモート閲覧トグルに
乗るだけなので成立したままで、新しい通信先も増えていない。
したがって CLAUDE.md の「全称・否定表現が偽になる」類型には該当しない。
**列挙漏れ**なので、リリース Phase 1 の htdocs 更新で拾えば足りる
(コレクション機能全体のレビュー担当と重複する可能性がある)。

### D-13 / P3 / 回帰テストの穴 (詳細は §7 の対応表)

主な未固定点:

- **read-only の「不在」を固定するテストが無い。** `RemoteWriteRequest` /
  `RemoteGridScope` に永続 variant が無いことは構造的事実だが、将来の追加を弾く
  assertion は無い
- **既存 `CollectionKind` の golden JSON fixture が無い。**
  `crates/remote-ipc/src/lib.rs:3898-3930` と `3752-3764` は round-trip 検査で、
  variant 名が変わっても通る。プラン §13.1 の
  「existing `CollectionKind` / `CollectionPayload` JSON fixture が変わらない」は未達
- **protocol 55 ↔ 56 の相互拒否テストが無い。** `crates/remote-ipc/src/lib.rs:3050-3064`
  は `PROTOCOL_VERSION + 1` の拒否と同版の受理を見るが、55 を名指しした組合せは無い
- **pipe レベルで、非 owner session からの永続 collection 要求が拒否されることの
  テストが無い。** `message_owner()` (`src/remote_ipc/pipe.rs:1913-1959`) が
  catch-all 無しの網羅 match なので構造的には漏れないが、回帰では固定されていない
- `wire_snapshot` / `catalog` の truncation を実際に切る budget 注入テストが core 側に無い
  (`complete_display_unit_prefix` の単体テストはある: `persistent_collections.rs:1842-1860`)

### D-14 / P3 / navigate 応答の `Boundary` で端末が `exact_view_token` / `exact_revision` を取り込まない

`Boundary` payload は `exact_revision` / `exact_view_token` を持つ
(`crates/remote-ipc/src/lib.rs:1573-1578`、`persistent_collections.rs:648-655`) が、
端末の `performPersistentCollectionNavigation` は boundary 分岐で
`current.collectionRevision` / `current.viewToken` を更新せずに抜ける
(`app.js:7453-7460`)。境界に当たった後もコレクションが更新され続けると、
次の要求は毎回 `replacement_needed = true` になり (`persistent_collections.rs:598-599`)、
不要な full snapshot が付いてくる。**正しさは壊れない (D-2 のコストを増やすだけ)。**

---

## 監査項目ごとの結果

### 1. read-only の強制 — 問題なし

- protocol に永続コレクションの create / rename / reorder / remove / import variant は
  **存在しない**。`ClientMessage` に増えたのは
  `PersistentCollectionCatalog` / `Snapshot` / `Navigate` の 3 つだけ
  (`crates/remote-ipc/src/lib.rs:2508-2522`)
- `RemoteWriteRequest` に永続コレクション用 variant は無い
  (`crates/remote-ipc/src/lib.rs:575-...`)。書き込みは既存の spread / reading progress /
  rating / bookmark / trim / sort のみ
- `RemoteGridScope` は `Address` と集約 `Collection { collection: CollectionKind }` のまま
  (`lib.rs:549-568`)。永続コレクションの並びを書き換える経路は無い
- 端末側も `state.gridSortScope = null` + `locked_reason` を設定して sort select を
  disabled にし (`app.js:5112-5113, 4883-4900, 6687`)、`changeGridSortOrder` は
  `locked_reason` があれば即 return する (`app.js:6713`)
- Remote の command 一覧に削除・除外系は無い
  (`crates/remote-web/web/command-core.mjs:1-58`)
- **既存 aggregate の wire 互換**: `CollectionKind` は 6 variant のまま変更なし
  (`lib.rs:1279-1286`)、`CollectionPayload` / `/api/collection` / `#collection/...` も不変。
  hash route も `#collection/` と `#saved-collection/` で衝突しない
  (`parseCollectionRoute` は `#collection/` で startsWith 判定、`app.js:2795-2797`)

### 2. 認証・公開範囲 — 問題なし (D-10 のみ要確認)

- 3 route とも `route_requires_remote_session` の既定 deny 側に入る。
  除外は `/api/auth/*` / `/api/session/acquire` / `/api/app-version` / `/api/telemetry` だけで、
  saved-collection は含まれない (`crates/remote-web/src/http.rs:4705-4712`)
- route 表でも `remote_owner.expect("route guard checked session")` を通る
  (`http.rs:720-732`)。PIN / Bearer の `AuthDecision::Unauthorized` guard (`http.rs:646`) は
  saved-collection route より前段
- テストが 2 本ある:
  `every_video_stream_ai_archive_and_saved_collection_route_is_below_the_fail_closed_auth_guard`
  (`http.rs:5960-5975`) と
  `saved_collection_routes_require_a_session_and_finalize_as_no_store` (`http.rs:6053-6090`)
- 3 応答とも `Cache-Control: no-store` (`http.rs:2722, 2775, 2821`)。
  service worker は `request.mode !== "navigate"` を素通しするので API を一切キャッシュしない
  (`crates/remote-web/web/service-worker.js:45-58`)
- **二重 path 検証**: core は `path_guard::resolve_existing` + `inspect_collection_source` の
  実 kind 一致を通してからしか address を出さない (`persistent_collections.rs:1086-1135`)。
  remote-web は返ってきた address を `Library::validate_remote_file_kind` で
  canonicalize + `std::fs::metadata` + 拡張子分類まで再検査する
  (`http.rs:2833-2864, 2868-2911`、`crates/remote-web/src/store.rs:284-328`)。
  落ちた行は `BlockedByRemotePolicy` へ落とされ、page group からも外れる
- **collection membership は許可証になっていない**。child を開くと
  `/api/list` / `/api/container` / archive job といった既存 route へ合流し
  (`app.js:3308-3317`)、それぞれの既存検証を通る
- **source identity token は path へ復号されない**。core は 64 桁 lower-hex を検証し
  (`persistent_collections.rs:1195-1205`)、現 prepared entry を同じ token へ写像して
  一致を探すだけ (`persistent_collections.rs:1226-1235`)。token は
  `SHA-256("mimageviewer:persistent-collection-source:v1\0" + namespace + "\0" + normalized_path)`
  (`persistent_collections.rs:1248-1255`)
- `Missing` / `Unsupported` / `AccessError` / `BlockedByRemotePolicy` は address を返さず
  `last_known_kind` だけを載せる (`crates/remote-ipc/src/lib.rs:1394-1410`)。
  `AccessError(String)` の OS 文言は wire に出ない (`persistent_collections.rs:1143-1147`)
- **export 相当の情報は API に出ない**。絶対パス一覧を返す endpoint は追加されていない

### 3. 端末 UI の整合性 — D-3 / D-6 / D-7 / D-8 / D-9 以外は妥当

- 置き場所は **ホームの独立タブ「コレクション」** (`app.js:3805-3812`)。
  場所 (drive_list / reading_history / bookshelf / bookmarks / rating) や
  スマートフォルダの一覧には混ざらない (`renderPlacesTab`, `app.js:4108-4165`)。
  PC が専用「コレクション」ツールバーを持つ方針
  (`docs/collection-spec-proposal.md:20`) と揃っている
- 一覧形式: サムネイル無し・名前のみのカード (`app.js:5171-5182`)。truncation は
  「件数が多いため先頭 N 件を表示しています。」(`app.js:5202-5206`)。
  空は「コレクションはまだありません。mIV 本体で作成・編集できます。」(`app.js:5166-5173`) で
  read-only を明示している
- 並び表示: root のヘッダ sort select は `Manual` → 「手動順」、
  `Standard` → 本体の `sort.label()` をそのまま 1 択で出し、
  `locked_reason`「並べ替えは mIV 本体のコレクション設定で変更できます」で無効化
  (`app.js:4883-4900`、`persistent_collections.rs:1534-1546`)
- missing 表示の文言は PC と一致: 「見つかりません」
  (`app.js:6981-6988` vs `docs/collection-spec-proposal.md:44`)。
  他は「未対応」「アクセス不可」「Remote非公開」
- **PC 側文言との不一致は見つからなかった。** PC の「コレクションから外す」に
  相当する操作は Remote に存在しないので語の衝突も無い
- next / prev / EOF / First / Last / ordinal seek はすべて core の
  `resolve_prepared_collection_navigation` (Phase 4 の pure resolver) と
  `endpoint_candidates` / `current_ordinal_candidates` に集約されている
  (`persistent_collections.rs:476-523`)。端末は cached index から target を合成しない
- 媒体抽出は `PersistentCollectionNavigationKind` で分離
  (`still_image` / `video` / `audio` / `navigable_media`、`persistent_collections.rs:1324-1346`)。
  container (Folder / Zip / Pdf / ConvertibleArchive) は媒体候補に入らない
  (`resolved_kind_matches_target`, `persistent_collections.rs:1391-1413`)
- tail: `Stop` / `Loop` を wrap 設定から渡す (`app.js:7584-7588`)。
  **スライドショーと Ctrl+↑↓ は追加されていない** (プラン §8.1 どおり)
- child (Folder / ZIP / PDF / 変換アーカイブ) は既存 route へ合流し、
  child 内の順序・sort UI は通常のものへ戻る (`app.js:3308-3317`、
  `applyPersistentCollectionSnapshot` の外なので `gridSortScope` は address になる)
- 戻り先は `savedCollectionChildRouteState()` の marker + `returnIdentity` で解き、
  最新 snapshot を読み直して entry ID → source identity の順で再 anchor する
  (`app.js:1534-1552, 4931-4945`、`persistentCollectionEntryIndexByIdentity` は
  entry_id 優先 → source_identity fallback、`app.js:4988-4997`)。
  prefix 外なら「一覧上限外の項目です。」を出して先頭に偽装しない (`app.js:4967-4970`)

### 4. session / route owner と race — 問題なし

- browser owner は `Inactive / Root / DirectViewer / Child` の tagged owner
  (`app.js:1383, 1522-1532, 1596-1610`)。集約の `state.collection` へ
  `persistent` を混ぜて既存 helper に紛れ込ませてはいない:
  `refreshableCollectionRoute()` は `saved_collection` を除外する (`app.js:6160-6171`)
- 応答適用前に **route sequence / viewer instance / viewer sequence /
  session cache epoch / collection ID** を再照合する
  (`persistentCollectionResponseIsCurrent`, `app.js:7388-7402`;
  `persistentCollectionRouteRequestIsCurrent`, `app.js:4972-4985`)
- 連打は `PersistentCollectionNavigationQueue` が
  bounded signed queue (±32) で 1 step ずつ処理し、逆方向 / 別 intent は
  abort + replace する (`app.js:1460-1510`)。most-recent-wins へ潰していない
- **PC 側の削除 / rename / reorder 直後**: core は subscribe → load → prepare の
  linearization を守り、`notice_invalidates()` が
  「catalog_revision が進んだうえで対象 ID が消えた」を delete race として
  再 load → `NotFound` へ収束させる (`persistent_collections.rs:771-787`)。
  `CollectionRevisionNotice` は毎回全 collection の revision map を持つので
  (`src/collection_store/model.rs:268-285`)、latest-wins slot でも取りこぼさない。
  catalog も subscribe-before-list + 新 revision gate を持つ
  (`persistent_collections.rs:671-696`)
- churn が続いた場合は `MAX_EXACT_RESTARTS` (16) と 9 秒 deadline で
  `Busy` (HTTP 503) に終端し、stale を返さない
  (`persistent_collections.rs:332, 394, 667`)
- **worker 終端の ownership check**: `execute_work` は handler 完了後に
  `session_operation.ownership_response()` を見て、Active でなければ応答を
  Session 応答へ差し替える (`src/remote_ipc/pipe.rs:1764-1773`)。
  superseded session へ古い collection 応答は届かない
- **response budget**: entry 変換を streaming で行い、retain 予算
  (`MAX_RESPONSE_FRAME_BYTES - 1 MiB`) を超えた時点で prefix を打ち切る
  (`persistent_collections.rs:84-116, 194-214`)。
  `complete_display_unit_prefix()` が見開き単位を割らないよう prefix を切り下げ
  (`persistent_collections.rs:809-830`)、最後に完成 `ServerMessage` envelope を
  実 serialize して 64 MiB 未満を再確認する
  (`persistent_collections.rs:846-855, 920-929`)。catalog も同様に pop-until-fit
  (`persistent_collections.rs:287-314`)
- **truncated prefix 外の target**: `persistentSparseTargetMatchesRootBinding` は
  root prefix に同 identity が **無ければ true を返す** (`app.js:7366-7372`)。
  membership を current 条件にしていない (プラン §6.3 / §15-3 の要件どおり)
- **session 交代**: acquire / 再取得で catalog / root binding / pending navigation を
  破棄し (`app.js:1896-1906`)、history state の
  `savedCollectionSessionEpoch` が一致しない過去エントリは
  `historyStateWithoutPersistentCollectionSession` で無効化される
  (`app.js:2197-2214`)
- **cache**: API は `no-store`、service worker は API を扱わない (項目 2 参照)

### 5. 終了順・ライフサイクル — 問題なし (D-11 は型の話)

- 順序は `src/lib.rs:1502-1515`:
  `begin_app_exit()` (public/session admission 停止) → remote service manager drop →
  `retire_app_without_ui_owner()` → `RemoteIpcServer` drop (listener / worker join) →
  `producer.close_and_drain()` → `runtime.shutdown_and_join()`。
  プラン §5.2 の 4→8 と一致
- App 側 final は frame 非依存の
  `retire_remote_resources_for_final_exit()` (`src/remote_ipc/ui.rs:745-768`) で
  `RemoteAppDrainLease` を組み立てて即 drop し、
  pending UI reply / bookmark write / video Opening・Starting を terminal 化して
  ACK する (`ui.rs:599-640`)。**App は server join を待たない** ので自己 deadlock しない
- `CollectionRemoteProducerControl` は単一 close channel で全 lease を wake し
  (`begin_close` で `close_tx.take()` → 全 receiver が Disconnected、
  `src/collection_store/runtime.rs:272-285`)、Closing 以後の `begin_request` は
  `None` (`runtime.rs:258-270`)、最後の lease Drop まで
  `close_and_drain` が返らない (`runtime.rs:292-310, 336-347`)。
  回帰も入っている
  (`remote_producer_close_wakes_every_lease_and_drains_before_actor_shutdown`,
  `runtime.rs:351-...`)
- **Remote startup 部分失敗**: `RemoteIpcServer::start` が Err なら渡した producer clone だけが
  drop され、`collection_remote_producer` は lib.rs 側に残る (`src/lib.rs:1307-1316`)。
  actor と PC 側コレクションは生存する
- **actor 不在時**: `persistent_collection_engine` が `None` なら 3 route とも
  typed `Unavailable` を返し、aggregate / folder / page / AI / video は生きたまま
  (`src/remote_ipc/pipe.rs:1039-1081, 1242-1256`)
- **PIN 変更 / 全端末ログアウト**: core は子プロセス
  (`mimageviewer-remote.exe`) を再起動するだけで、producer と actor は core 側の
  `RemoteIpcServer` に残る。producer の再注入は不要 (再起動対象外)
- collection の actor 待ちは `select_biased!` で
  session cancel wake / producer close / actor reply / deadline を待つ
  (`persistent_collections.rs:752-769`)。`try_lock + sleep` も固定 poll sleep も無い

### 6. AI / streaming / diagnostics — 問題なし

- 新しい media reader は追加されていない。画像は既存 `renderImageViewer` →
  Page / PageDemand / Remote AI、動画・音声は `renderVideoViewer` →
  `/api/video/start` (`app.js:5083-5088, 7495-7505`)。
  `api_video_start` は address を `validate_remote_file_streamable` で再検証してから
  既存 stream owner へ渡す (`crates/remote-web/src/http.rs:1541-1553`)
- collection ID は file access key にも stream cache key にも混ざっていない
  (sparse entry は `{name, kind, address, persistent_identity}` を持つだけ、
  `app.js:7306-7318`)
- **telemetry**: 永続コレクション専用イベントは追加されていない。
  fetch エラーの `resource` は `new URL(...).pathname` だけを残すので
  `?id=<collection uuid>` はログに出ない
  (`safeResourcePath`, `app.js:16263-16270`)
- **server 側 diagnostics**: catalog は
  `kind / collection_count / catalog_revision / ipc_status / ipc_ms` だけを
  `with_log_details` に載せる (`http.rs:2723-2732`)。
  コレクション名・entry 名・path・source identity は入っていない。
  snapshot / navigate は details 無し (request log はクエリ全体を落とす、
  `docs/web-remote-plan.md` §3.1)
- core 側の request kind 文字列も `persistent_collection_{catalog,snapshot,navigate}`
  のみ (`src/remote_ipc/pipe.rs:1850-1852`)、
  operation description も汎用文言 (`pipe.rs:1975-1983`)

### 7. テストの網羅 (監査項目 1〜5 との対応表)

| 監査項目 | 固定しているテスト | 判定 |
| --- | --- | --- |
| 1. read-only (write variant 不在) | — (構造のみ) | **穴** (D-13) |
| 1. sort write が永続 scope へ行かない | — (JS 側 guard のテスト無し) | **穴** (D-13) |
| 1. 既存 `CollectionKind` wire 互換 | `crates/remote-ipc/src/lib.rs:3898` `collection_message_round_trip_keeps_spread_request_and_address_groups`、`lib.rs:3752` の `SetSortOrder` round-trip | round-trip のみ / golden fixture 無し (D-13) |
| 1. protocol version | `crates/remote-ipc/src/lib.rs:3050-3064` (`VERSION+1` 拒否 / 同版受理)、`lib.rs:3181, 3357` (`== 56`) | 55↔56 名指しは無し (D-13) |
| 2. 認証・session 必須 | `crates/remote-web/src/http.rs:5960` fail-closed guard、`http.rs:6053` session 必須 + `no-store` | **十分** |
| 2. 二重 path 検証 (snapshot) | `http.rs:5609` `persistent_post_validation_reanchors_a_spread_to_its_surviving_page` | **十分** |
| 2. 二重 path 検証 (navigate target) | `http.rs:5689` `persistent_navigation_post_validation_shrinks_or_rejects_a_spread` | **十分** |
| 2. 実 kind の再検査 (stale kind 不公開) | `src/remote_ipc/persistent_collections.rs:1906` `wire_entry_does_not_publish_a_stale_prepared_kind` | **十分** |
| 2. blocked 行を count / First / ordinal から除く | `persistent_collections.rs:1751` `blocked_wire_entry_is_excluded_from_count_first_and_ordinal_zero` | **十分** |
| 2. source identity の書式検証 | — (`navigation_identity` の 64 hex 検証に単体テスト無し) | 穴 (小) |
| 3. entry ID → source identity → head | `persistent_collections.rs:1668` `current_locator_prefers_url_entry_id_then_matching_history_source`、`app-runtime.test.mjs:312` | **十分** |
| 3. ordinal 射影 | `persistent_collections.rs:1714` `current_locator_uses_the_remote_eligible_ordinal_projection` | **十分** |
| 3. unavailable 行が address を持たない | `app-runtime.test.mjs:251` | **十分** |
| 3. route 解析 / hash に path を含めない | `app-runtime.test.mjs:237-247` | **十分** |
| 4. route owner 遷移 (child / sibling) | `app-runtime.test.mjs:282`、`persistentCollectionRouteOwnerTransition` | **十分** |
| 4. session epoch で history を無効化 | `app-runtime.test.mjs:472, 510` | **十分** |
| 4. 応答 current 判定 (route/viewer/epoch) | `app-runtime.test.mjs:527-540, 571-605` | **十分** |
| 4. sparse target は prefix membership を要求しない | `app-runtime.test.mjs:607-...` `persistent sparse targets must exactly match identities already present in the root prefix` | **十分** |
| 4. catalog owner が旧 session 結果を捨てる | `app-runtime.test.mjs:542` | **十分** |
| 4. EOF terminal が 1 回だけ settle | `app-runtime.test.mjs:408` | **十分** |
| 4. response budget (display unit を割らない) | `persistent_collections.rs:1842` `response_prefix_never_cuts_an_interleaved_spread_unit` | **十分** |
| 4. budget streaming が単一パス | `persistent_collections.rs:1863` | **十分** |
| 4. budget 注入で実際に truncate する (core) | — | 穴 (D-13) |
| 4. revision / delete race | — (core 側の `notice_invalidates` 単体テストが見当たらない) | 穴 |
| 5. producer close wake + last-drop drain | `src/collection_store/runtime.rs:351` `remote_producer_close_wakes_every_lease_and_drains_before_actor_shutdown` | **十分** |
| 5. App drain lease の Drop-only 終端 | `src/remote_ipc/ui.rs` 周辺 (プラン §16 が「各 1/1」と記録) | 記録あり / 本レビューでは名前を特定できず |
| 5. pipe: 非 owner session の拒否 | — (`message_owner` が catch-all 無しの網羅 match で構造保証) | 穴 (D-13) |

### 8. 公開文書との突き合わせ

- `htdocs/mimageviewer/index.html:1200-1205`「🌐 通信するのは 3 つの場面だけ」 —
  **偽にならない**。コレクション閲覧は既存のリモート閲覧トグルの内側で動き、
  新しい通信先も外部送信も増えていない
- `htdocs/mimageviewer/privacy.html:165-175`「ネットワーク通信」 — **偽にならない**。同上
- `htdocs/mimageviewer/privacy.html:134-145`「端末内に保存されるデータ」 /
  `index.html:1217-1223`「💾 データはあなたの PC の中だけ」 —
  **列挙にコレクションが無い** (D-12)。全称・否定表現ではなく列挙なので
  「偽」ではないが、追記が要る
- `htdocs/mimageviewer/manual/remote.html` — **コレクション閲覧の記述が無い** (D-12)

---

## 補足: 実機確認を勧める項目

エージェント側に実行時の観測は無いので、以下は利用者側での確認を提案する。

1. **D-1**: PC で大きな import を走らせながら端末でセッションを取得し、
   ホームとフォルダ一覧が待たされないか
2. **D-4**: 画像と動画が混在する大きめのコレクションで
   `#saved-collection/<id>/entry/<entry-id>` を直接開き、シークバーの総数と
   ドラッグ後の着地が合っているか
3. **D-3**: 同名画像 sidecar を持つ動画をコレクションに登録し、
   通常フォルダ経由とコレクション root でサムネイルが一致するか
4. **D-6 / D-8**: 375px 幅の端末で、ホームタブ 6 個の折り返しと
   「見つかりません」タイルの見え方
