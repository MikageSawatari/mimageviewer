# §1.851 見つからない項目をまとめて外す — 設計

- 状態: 設計 ACCEPT (2026-11-22、Codex gpt-6.1-sol / xhigh の独立レビュー 4 回目で ACCEPT、HEAD 8f3d50443)。実装は統括の合図の後。
- 正本: バックログ [next-release-backlog.md §1.851](next-release-backlog.md)。コレクション全体の所有境界は
  [collection-implementation-plan.md](collection-implementation-plan.md) §5・§6・§9・§20・§21。
- 対象版: v5.6.0 候補 (修正量が小さければ、利用者希望 2026-11-22)。

## 1. 問題 (コードから)

利用者の報告 (X、2026-11-22): フォルダ名を変えた後、コレクションから見つからなくなった項目を外す作業で、
1 件ずつしか削除できない気がする。

- 見つからない登録は `GridItem::CollectionPlaceholder` として表示する。
- `GridItem::is_checkable()` (`src/grid_item.rs`) は placeholder を対象外にしている。チェックの入口
  (クリック・Ctrl・Shift 範囲・Ctrl+A・ゲームパッド) はすべて `grid_item_checkable` を通るので、
  placeholder にはチェックが入らない。
- 「コレクションから外す」(`collection_root_delete_resolution`) は、チェックがあればチェックした項目、
  なければ右クリック / 選択中の 1 件を外す。placeholder を外す手段は 1 件ずつだけで、毎回確認が出る。
- 見つからない項目だけを選ぶ・絞り込む手段もない。

## 2. 方針 (統括決定 2026-11-22)

- **採る**: 「見つからない項目をすべて外す… (N件)」を足す (案 B)。確認ダイアログに件数と先頭数件の
  パスを出す (案 C)。外す処理は既存の `request_collection_grid_remove` → `ConfirmRemove` →
  `submit_collection_remove` に乗せる。
- **採らない**: placeholder をチェック可能にする案 (案 A)。`checked` を読む約 30 ファイルの一括操作
  (タグ・評価・AI 説明文・回転・外部ツール・整理・名前変更等) の意味が変わり、監査範囲が大きい。

### 2.1 状態の組み合わせを減らす検討 (CLAUDE.md の方針)

- 確認ダイアログは既存のモーダル (`ConfirmRemove`) をそのまま使う。新しい非同期の待ちや、
  確認中に一覧が変わったときの追従処理は作らない。一覧が変わった場合は、既存どおり送信時の
  `expected_revision` の不一致 (Conflict) で失敗し、既存の通知が出る。
- 対象の計算は一覧の準備 (worker) で 1 回だけ行い、準備結果に入れる。メニューは結果を読むだけで、
  開くたびの計算・ディスク読み込み・新しい待ち状態を持たない。
- まれな失敗 (actor の書き込み失敗など) は既存の「外す」と同じ扱い (ログ + 通知) で、新しい回復処理は足さない。

## 3. 「見つからない」の判定と対象

### 3.1 対象にする項目

一覧の準備で `CollectionSourcePreparation::Missing` (元パスの `metadata` が `NotFound`) と分類され、かつ
**「最も近い実在の祖先」が、リンク先までたどって実在するフォルダである**項目だけ (r1 レビュー P1 で、
ドライブ / 共有のルートだけを見る案から改めた)。

- 未対応 (`Unsupported`)、読み取れない (`AccessError`) は入れない。
- Windows では、外したドライブ・つながらない共有も `NotFound` になる。Rust 1.100 の対応表は
  `ERROR_FILE_NOT_FOUND` (2)・`ERROR_PATH_NOT_FOUND` (3)・`ERROR_INVALID_DRIVE` (15)・`ERROR_BAD_NETPATH` (53)・
  `ERROR_BAD_NET_NAME` (67) を `NotFound` にする (r1 レビューが std の実装で確認)。さらに、登録パスの途中に
  junction / ディレクトリの symlink / フォルダへのマウントがあり、その接続先が外れている場合も `NotFound` になる。
  これらは一時的に接続されていないものとして扱い、対象から外す。マニュアルの「一時的に読めない項目も
  登録行を残す」と食い違わないようにするため。
- 判定 (`missing_source_is_removable(path, cache)`、prepare worker 内):
  0. `path` が絶対パス (`Prefix` + `RootDir` を持つ) でなければ、I/O をせずに**対象外** (r2 レビュー P3。
     本番の登録は絶対の drive / UNC パスだけだが、判定側でも拒否する)。
  1. ルートから `path` 自身まで、上から順に各パス P (ルート、1 段下、…、`path`) を調べ、
     `std::fs::symlink_metadata` (リンクをたどらない) で状態を得る。
     - 成功 → `path` 自身でなければ、降りる前にその段の `metadata` (リンクをたどる) の結果を見る
       (キャッシュになければ 1 回照会して覚える)。エラーまたはフォルダでなければ、その段を N として止まり
       **対象外** (外れたリンクの下の別の項目を、2 件目以降は I/O なしで打ち切る。r3 レビュー P2)。
       フォルダなら次の段へ進む。`path` 自身まで実在した (分類の後に元が戻った) 場合は**対象外** (実装レビュー r1 P2 で訂正。旧版は N = `path` として、フォルダに戻った場合を対象にしていた)。
     - `NotFound` → 1 つ上の段を「最も近い実在の祖先」N として止まる。ルート自身が `NotFound` なら**対象外**。
     - `NotFound` 以外のエラー → **対象外**。
  2. N に対して `std::fs::metadata` (リンクをたどる) が成功し、`is_dir()` のときだけ**対象**。
- 上から調べる理由 (r2 レビュー P2): 下から調べると、つながらない共有の下の a.jpg・b.jpg… のように登録パスが
  それぞれ違うとき、葉ごとにネットワーク照会が増える。上から調べてパスごとの結果を覚えておけば、ルートや途中の
  フォルダが到達不能と分かった後は、同じ祖先を持つ別の項目は I/O なしで対象外になる。
- この判定で区別できるもの:
  - フォルダ名の変更・移動・削除 (報告の場面): N は変更前の親など実在の普通のフォルダで、対象になる。
  - 外したドライブ・つながらない共有: ルートが `NotFound` かほかのエラーになるので対象外。
  - 接続先が外れた junction / symlink / マウント: そのリンク自身は `symlink_metadata` で成功し、その下の段が
    `NotFound` になるので N はリンク自身になる。リンク先をたどる `metadata` が失敗するので対象外。
    登録した項目そのものが外れたリンクの場合は、`path` 自身の追跡が失敗するので、同じく対象外。
  - 接続先が生きている junction の下でフォルダ名を変えた場合: N は実在するので対象になる。
- 表示の分類 (「見つかりません」の表示、テキスト書き出し、Remote の状態、登録時の分類) は**変えない**。
  `inspect_collection_source` も変えない (変えると、テキスト取り込みで外したドライブ上のパスを登録できなく
  なる等、既存の挙動が変わるため)。

### 3.2 ファイルの種類の表示設定との関係

対象は**コレクションの全登録** (表示の絞り込み前) から数える。ファイルの種類の表示設定や書庫の表示設定で
一覧に出ていない種類の見つからない項目も含める。

- 理由: 「すべて外す」の意味をそのまま保つ。テキスト書き出しも画面上の絞り込みで行を省かない
  (マニュアル)。確認ダイアログに件数とパスを出すので、利用者は外す前に分かる。
- マニュアルに「表示設定で隠している種類の項目も含みます」と書く。

### 3.3 計算する場所と時点

`collection_store::prepare::prepare_collection_snapshot_while` (分類の唯一の所有者、worker で動く) で、
分類と並べ替えの後に計算し、`CollectionPreparedSnapshot` の新しいフィールドに入れる。

```rust
pub(crate) struct CollectionRemovableMissingEntry {
    pub(crate) entry_id: CollectionEntryId,
    pub(crate) source_path: PathBuf,
}

pub(crate) struct CollectionPreparedSnapshot {
    // 既存フィールドはそのまま
    /// 全登録 (表示の絞り込み前) のうち、Missing かつ §3.1 の判定で対象になった項目。
    /// 並びは effective order。表示の絞り込み (project_collection_display_snapshot 等) では変えない。
    pub(crate) removable_missing: Arc<[CollectionRemovableMissingEntry]>,
}
```

- 判定は分類ループの中で Missing の項目にだけ行う。調べたパスごとの結果 (`symlink_metadata` の結果:
  実在 / `NotFound` / その他のエラー、と N に対する `metadata` の結果) は準備 1 回の中の
  `HashMap<PathBuf, _>` に覚える。上から調べるので、到達不能と分かったルート・祖先の下の別の項目は、
  キャッシュを見るだけで I/O をしない。各 I/O の前に `keep_running()` を見て取り消しに従う。
  判定の I/O は関数を注入できる形にし、テストで実ファイルなしに回数と結果を検査できるようにする。
- 既存の分類 (`inspect_collection_source`) の照会 (項目ごとに 1 回) は今と同じで、増えるのは Missing の項目の
  判定分だけ。到達不能な共有の下に多数の登録があっても、判定で増える照会はルート (と到達できる祖先) ごとに 1 回。
- 本番の `CollectionPreparedSnapshot` の構築は prepare.rs の 1 か所だけ。表示用の射影
  (`prepare_collection_grid_install` の書庫射影、`project_collection_display_snapshot`) は `entries` だけを
  差し替えるので、このフィールドはそのまま残る。ナビゲーション・履歴・Remote の準備も同じ関数を通るので、
  どの経路で準備した snapshot も同じ値を持つ。
- Remote もこの関数を使うため計算はするが、使わない (Missing の項目について §3.1 の I/O を行うだけ)。Remote の
  通信内容・状態は変えない。
- 準備結果の再利用 (`CollectionPrepareReuseKey`) は revision が同じ間は再利用する。外部での変更 (フォルダ名を
  戻す、ドライブを抜き差しする) は、既存どおり明示的な更新や開き直しで再評価する (マニュアルの既存の記述どおり)。
  確認から送信までの間に元が復活した場合も外す。確認ダイアログにパスを出すことと、外すのは登録だけで
  元に触らないことで割り切る (再追加で戻せる)。

## 4. UI

### 4.1 コレクションメニュー

`MenuCommandId::CollectionsRemoveMissingCurrent` を足す (`parent: TopMenuId::Collections`、
ラベル「見つからない項目をすべて外す…」、`action: None`)。並びは「現在のコレクションを並べ替え…」の後。
保存済みのメニュー配置には `resolve_menu_commands_for_parent` が末尾へ自動で補う (既存の仕組み)。

受付の判定は 1 つの helper `collection_grid_missing_removal(&self) -> CollectionMissingRemovalAvailability`
にまとめ、トップメニューと右クリック (§4.2) が共有する。判定は既存の並べ替えの受付
(`collection_grid_root_order`) と同じ条件をとる (r1 レビュー P2: `collection_grid_content_target()` が Ok でも、
`prepared()` は更新中・失敗中に旧い installed を返し得るため、それだけでは最新と言えない)。

1. コレクション直下でない → `NotOffered` (トップメニューでは無効、理由「コレクション直下を開くと使用できます」)。
2. load が `Deleted` / `Failed`、`collection_grid_refresh_waits_for_viewer()`、load が `Ready` / `Empty` 以外
   (`RequestNeeded` / `Snapshot` / `Preparing` で旧 installed を保持している場合を含む)、
   `wanted_revision > accepted_revision`、`collection_grid_content_target()` が Err、
   prepared の `collection_id` が stamp と違う → `Unavailable { count: None, reason }` (件数なし)。
3. `collection_ui.can_edit()` が false、または `collection_ui.operation` が idle でない →
   `Unavailable { count: Some(N), reason: "コレクションを現在編集できません" }`。
4. N = 0 → `Unavailable { count: Some(0), reason: "見つからない項目はありません" }`。
5. それ以外 → `Ready { target: CollectionGridContentTarget, entries: Arc<[CollectionRemovableMissingEntry]> }`
   (prepared の `removable_missing` の Arc を共有するだけで、コピーしない)。

表示:

| 状態 | 表示 | 有効 | ホバー |
| --- | --- | --- | --- |
| `Ready` | 見つからない項目をすべて外す… (N件) | 有効 | — |
| `Unavailable { count: Some(0) }` | 見つからない項目をすべて外す… (0件) | 無効 | 見つからない項目はありません |
| `Unavailable { count: Some(N) }` (編集不可) | 見つからない項目をすべて外す… (N件) | 無効 | コレクションを現在編集できません |
| `Unavailable { count: None }` / `NotOffered` | 見つからない項目をすべて外す… (件数なし) | 無効 | 理由文 (並べ替えの受付と同じ区分の文言) |

- 件数は prepared の `removable_missing.len()` を読むだけ。メニューを開いてもディスクを読まない。
- 件数が出せない間 (準備中・更新中・旧い表示を保持中) は件数を付けず無効にし、理由をホバーに出す。
  準備が終わると次のフレームから件数付きで出る。

### 4.2 見つからない項目の右クリック

報告者は見つからない項目を 1 件ずつ右クリックしているので、そこからも同じ操作へ届くようにする。

- コレクション直下の placeholder セルを、チェックなしで右クリックしたときのメニューに、「コレクションから外す」の
  直後へ「見つからない項目をすべて外す… (N件)」を足す。表示・有効条件は §4.1 と同じ。
- チェックありのメニュー、実在する項目のメニュー、フルスクリーン、コレクションの子 (実フォルダ) には出さない。
- `context_menu_model` に `MenuCommand::RemoveMissingFromCollection` を足し、`ContextMenuInput` に
  `collection_missing_removal: CollectionMissingRemovalAvailability` (`NotOffered` / `Ready { count }` /
  `Unavailable { count: Option<usize>, reason }`) を足す。値は §4.1 の helper の結果を
  `NativeGridContextMenuTarget` に取り込む (既存の `collection_root_delete` と同じ取り込み方)。選んだときは
  取り込んだ `Ready` の target (stamp + revision) と entries (immutable な `Arc`) を使う。
  native 描画と egui 描画は同じモデルを見るので両方に出る。
- 取り込みの寿命は既存の契約に従う: native は開いた時点の target を保持し、egui は毎フレーム target を
  作り直す。どちらも既存の context / items 世代の検査 (`context_menu.rs` の世代確認) で、一覧が更新されたら
  メニューを閉じる。それでも古い target が届いた場合は、§4.4 の受付時と送信前の stamp 検査で actor へ
  送らない。新しい追従状態は足さない。

### 4.3 確認ダイアログ

`CollectionDialogOperation::ConfirmRemove` に表示用の型付きフィールドを足す。

```rust
pub(crate) enum CollectionRemoveConfirmKind {
    /// 既存のチェック / 選択からの「外す」。文言は今と同じ。
    Selection,
    /// 見つからない項目をすべて外す。先頭最大 5 件のパスと総数。
    Missing { sample_paths: Vec<PathBuf> },
}
```

Missing の文言:

- 見出し: 見つからない項目をすべて外す
- 本文: 見つからない N 件の参照をコレクションから外します。
- パス: 先頭最大 5 件を 1 行ずつ (長いパスは省略表示し、ホバーで全体)。6 件以上なら「ほか M 件」。
- 注記: 外すのは登録だけです。元のファイルやフォルダは削除・変更しません。
- 注記: つながっていないドライブ・共有フォルダ・リンク先にある項目、読み取れない項目、未対応の項目は外しません。
- ボタン: 外す / キャンセル
- キー: 現行の `ConfirmRemove` はボタンのクリックしか受けない (r1 レビュー P3 で前提の誤りを訂正)。
  Missing でも既存の選択からの「外す」でも、`self.dialog_enter_pressed(ctx)` で「外す」、
  `self.dialog_escape_pressed(ctx)` で「キャンセル」を受ける (closure の前でローカル変数に取る)。
  IME 変換中の Enter で確定しないこと、メニューを選んだのと同じフレームの Enter で即時確定しないことを
  テストする (既存の modal / input gate を使う)。

1093×614 logical points でもボタンまで見えること (最大 5 行のパスで高さを抑える)。

### 4.4 送信と結果

- 選んだ時点の `CollectionGridContentTarget` (stamp + expected_revision) と `removable_missing` の entry_id 群から
  `CollectionGridRemoveTarget` を作り、`request_collection_grid_remove` へ渡す (新しい経路は作らない)。
  sample_paths は同じ `removable_missing` の先頭から取る。
- **stamp の検査 (r2 レビュー P2 で前提の誤りを訂正)**: 現行の `request_collection_grid_remove` と
  `submit_collection_remove` は stamp を検査せずに actor へ送る。stamp を見るのは成功後の UI 適用だけで、
  context / surface を開き直しても DB の revision が同じなら Conflict にならない。そこで Grid 起点の「外す」
  (新しいコマンドと既存の選択からの「外す」の両方) に、次の 2 か所で origin context の検査を入れる。
  1. `request_collection_grid_remove` の受付時 (確認ダイアログを開く前)。
  2. 確認ダイアログで「外す」が確定し、`submit_collection_remove` で actor へ送る直前。
  検査は **stamp の `ViewerContextId` の context で**、不変参照だけで評価する。`with_viewer_context` は
  closure の前後で評価の反映・ページ編集の reconciliation・メタデータ表示の更新 (pending を消費して可視一覧を
  作り直す) を行うので使わない (r3 レビュー P2 で前提の誤りを訂正)。代わりに既存の
  `with_viewer_context_ref` と `ContextRef` を使い、`collection_grid_request_stamp_is_current` の判定
  (stamp の一致・Root・`installed_items_generation == items_generation`) を、`App` と `ContextRef` の両方から
  呼べる共通の純関数に切り出す。context が存在しなければ拒否する。
  古ければ actor へ送らず、ダイアログを閉じて「コレクションの一覧が変わったため、外していません。もう一度
  選んでください」と通知する。既存のインポートの確認 (`PreviewImport`) が確定前に同じ検査をしている形に揃える。
  既存の「外す」にとっては、古い要求を送らなくなるだけで、通常の操作は変わらない。
- 結果の適用、他の context への反映、Conflict の扱いは既存の「外す」と同じ (§5)。
- `apply_collection_grid_remove_success_in_current_context` は表示 entry ごとに `removed.contains()` を呼ぶため、
  大量に外すと UI スレッドで二次の計算量になる (r1 レビュー P2)。削除 ID を一度だけ `HashSet` にしてから
  走査する形に直す (既存の「外す」にも効く。挙動は変えない)。
- 0 件では送らない (メニューが無効なので届かない。届いた場合も何もしない)。

## 5. 複数ウィンドウ・Remote・所有境界

新しい書き込み経路・新しい共有状態は作らない。触れるのは「準備結果の型に読み取り専用のフィールドを 1 つ足す」
「メニュー 2 か所と確認ダイアログの表示」だけ。

- **対象の context**: メニューを操作した context が表示しているコレクション直下。トップメニューは
  `self` (その時点の投影中の context) の `collection_grid_content_target()`、右クリックは右クリック時に
  取り込んだ target を使う。インポート・エクスポート・既存の「外す」と同じ。
- **送信と適用**: stamp (`ViewerContextId` + surface generation + `CollectionId`) と expected_revision を運ぶ
  既存の `ActorTask::ContextRemove` を使う。成功は `apply_collection_grid_remove_success` が stamp の context へ
  routing し、その context の checked / cursor だけを直す。stale な stamp は捨てる。
- **他の context への反映**: actor の commit が各 context の private revision watch へ通知し、各 context が
  自分の準備を非同期でやり直す (collection-implementation-plan §5)。同じコレクションを表示している別の
  ウィンドウも、次の準備で行が消え、件数も新しい準備結果で更新される。別 context の texture・cursor・
  open/close owner には触らない。
- **Remote**: Remote は read-only (§9) のまま。この操作を Remote へ公開しない。Remote の一覧は既存の
  latest snapshot の収束で行が消える。`removable_missing` は Remote の通信に載せない。
- **MCP** (`remove_from_collection`): 変えない。
- detached viewer の述語・viewport 経路には触れない。

## 6. テスト

- `prepare.rs` (I/O を注入した判定関数): 親が実在の普通のフォルダ → 対象 / 祖先をいくつか上がって実在 → 対象 /
  ドライブ・共有ごと無い → 対象外 / 最も近い実在の祖先が外れたリンク (`symlink_metadata` 成功・`metadata`
  失敗) → 対象外 / 登録項目そのものが外れたリンク → 対象外 / 途中で `NotFound` 以外のエラー → 対象外 /
  相対パス → 対象外 / Unsupported・AccessError → 対象外 / 並びが effective order / 同じ祖先の I/O が準備 1 回で
  1 回だけ / ルートや祖先が到達不能と分かった後、同じ祖先を持つ別の項目では I/O をしない /
  同じ外れた junction の直下の別の葉を複数判定し、2 件目以降は追加 I/O が無い /
  相対パス (実在する相対の祖先があっても) → I/O なしで対象外 / 取り消しで止まる。
- 実ファイルでの判定 (一時フォルダ): フォルダ名を変えた後の登録 → 対象。接続先を消した junction の下の登録 →
  対象外 (junction は `mklink /J` 相当を std で作れないので、作れない環境ではテストを skip せず、注入版の
  テストで覆う。実ファイル版は作れる場合だけでよい)。
- 射影: `project_collection_display_snapshot` と書庫射影の後も `removable_missing` が残る (隠れた種類の
  Missing も含む)。
- stamp の検査: revision は同じで surface generation だけ古い target (一覧を開き直した後の右クリックの残り、
  別 context の stamp) を、受付時・確定時のどちらでも actor へ送らないこと。投影中でない origin context の
  stamp を正しく評価し、その context の状態を変えないこと (未反映の更新 (pending) を持つ origin と兄弟 context で、
  検査の前後に pending・可視一覧・generation が変わらない)。context が無い stamp は拒否。既存の選択からの
  「外す」も同じ。
- 受付 helper: §4.1 の 1〜5 の各分岐。特に旧 installed を保持した `RequestNeeded` / `Preparing` / `Failed`、
  `wanted_revision > accepted_revision` で件数なし・無効になること。トップメニューと右クリックが同じ結果を出すこと。
- 成功の適用: 表示から隠れた削除対象を含めて外したとき、残る checked / cursor が保たれ、兄弟 context に
  触らないこと。大量件数 (例: 1 万件) でも線形であること (HashSet 化)。
- 確認ダイアログのキー: Enter で外す、Escape で閉じる、IME 変換中の Enter とメニュー選択と同じフレームの
  Enter では確定しない。
- 右クリック: placeholder の単独メニューにだけ出る / チェックあり・実在項目・フルスクリーン・子では出ない。
- 送信: 選ぶと `ConfirmRemove { kind: Missing }` に正しい entry_ids・sample_paths・stamp・revision が入る。
  確定で `remove_entries` に全件が渡り、元ファイルが残る。revision が古いと Conflict で何も消えない。
- 複数 context: context A で外すと A の checked / cursor だけが直り、同じコレクションを表示する context B は
  revision watch で再準備する (既存の §5 の試験の形に合わせる)。
- 確認ダイアログの snapshot (Missing、6 件以上) と、1093×614 でボタンが見えること。
- keymap: `MenuCommandId::ALL`・stable name・カタログの一致 (既存の在庫テストに乗る)。

## 7. 文書

- マニュアル `htdocs/mimageviewer/manual/collections.html` の「元の場所と元ファイルの操作」に、
  見つからない項目をまとめて外す方法 (メニュー・右クリック)、対象 (見つからない項目だけ、つながっていない
  ドライブ・共有、読み取れない・未対応は外さない、表示設定で隠している種類も含む)、外すのは登録だけで元には
  触らないこと、フォルダ名を変えた後の流れ (外して、新しい場所を追加し直す) を書く。
- collection-implementation-plan.md に本節への参照を 1 行足す。docs/README.md の索引に本書を足す。
- 製品ページ・privacy は変更なし (通信・保存の追加なし)。

## 8. 修正量の見込み

小〜中。`prepare.rs` (フィールド + 祖先の判定 + テスト)、`keymap.rs` (メニュー 1 項目)、`ui_main.rs` (メニュー描画)、
`context_menu_model.rs` + `ui_dialogs/context_menu.rs` (右クリック 1 項目)、`ui_dialogs/collections.rs`
(確認ダイアログの種類とキー)、`app/collection_grid.rs` (受付 helper・成功適用の HashSet 化)、テストのフィクスチャで `CollectionPreparedSnapshot` を直接組み立てている数か所。
永続データのスキーマは変えない (マイグレーション不要)。

## 9. 設計レビューの記録

- r1 (fd4679d67) REVISE: P1 ドライブ / 共有のルートだけの判定では外れた junction 等の下の登録を除けない → §3.1 を祖先判定へ。P2 `collection_grid_content_target()` だけでは最新と言えない → §4.1 の受付 helper。P2 成功適用の二次の計算量 → HashSet 化。P3 ConfirmRemove に Enter / Escape が無い → §4.3。
- r2 (7ea149cc3) REVISE: P2 送信時に stamp を検査していない → §4.4 の受付時・送信前の origin 検査。P2 下からの探索では葉ごとに照会が増える → 上からの探索とキャッシュ。P3 相対パスの扱い → I/O なしで拒否。
- r3 (e2e163b3f) REVISE: P2 `with_viewer_context` は状態を変える → `with_viewer_context_ref` と純関数。P2 外れたリンクの下で照会が繰り返される → 降りる前に追跡結果を見て打ち切る。
- r4 (8f3d50443) ACCEPT: 追加指摘なし。
