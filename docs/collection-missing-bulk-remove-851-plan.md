# §1.851 見つからない項目をまとめて外す — 設計

- 状態: 設計 (2026-11-22、独立レビュー待ち)。実装は統括の合図の後。
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
**そのパスのドライブ / 共有のルートが存在する**項目だけ。

- 未対応 (`Unsupported`)、読み取れない (`AccessError`) は入れない。
- ルートが存在しない項目は入れない。Windows では、外したドライブ上のパス (`ERROR_PATH_NOT_FOUND`) や、
  つながらない共有も `NotFound` に分類され得る (Rust 標準の対応表は未確認、安全側に倒す)。
  マニュアルの「一時的に読めない項目も登録行を残す」と食い違わないよう、これらは一時的に接続されて
  いないものとして扱う。
- ルートは `Path::components()` の先頭の `Prefix` + `RootDir`:
  - `Prefix::Disk` (`D:`) → `D:\`
  - `Prefix::UNC(server, share)` → `\\server\share\`
  - `VerbatimDisk` / `VerbatimUNC` も同様に組み立てる。
  - `Verbatim` / `DeviceNS`、Prefix の無いパス、`RootDir` の無いパスは**対象外** (安全側)。
- ルートの存在確認は `std::fs::metadata(root)` が成功して `is_dir()` のときだけ「存在する」。エラーは
  すべて「存在しない」。
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
    /// 全登録 (表示の絞り込み前) のうち、Missing かつドライブ / 共有のルートが存在する項目。
    /// 並びは effective order。表示の絞り込み (project_collection_display_snapshot 等) では変えない。
    pub(crate) removable_missing: Arc<[CollectionRemovableMissingEntry]>,
}
```

- ルートの存在確認は、分類ループの中で Missing の項目にだけ行い、ルートごとに 1 回だけ (準備 1 回の中の
  `HashMap<PathBuf, bool>`)。つながらない共有でルートの確認が時間待ちになっても、ルートごとに 1 回で済む。
  各確認の前に `keep_running()` を見て取り消しに従う。
- 本番の `CollectionPreparedSnapshot` の構築は prepare.rs の 1 か所だけ。表示用の射影
  (`prepare_collection_grid_install` の書庫射影、`project_collection_display_snapshot`) は `entries` だけを
  差し替えるので、このフィールドはそのまま残る。ナビゲーション・履歴・Remote の準備も同じ関数を通るので、
  どの経路で準備した snapshot も同じ値を持つ。
- Remote もこの関数を使うため計算はするが、使わない (Missing の項目のルートを 1 回ずつ見るだけ)。Remote の
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

表示:

| 状態 | 表示 | 有効 | ホバー |
| --- | --- | --- | --- |
| 現在の一覧がコレクション直下で、準備済み・最新 (`collection_grid_content_target()` が Ok)、N ≥ 1 | 見つからない項目をすべて外す… (N件) | 有効 | — |
| 同上で N = 0 | 見つからない項目をすべて外す… (0件) | 無効 | 見つからない項目はありません |
| コレクション直下でない / 準備中 / 更新中 (`collection_grid_content_target()` が Err) | 見つからない項目をすべて外す… (件数なし) | 無効 | Err の理由文 (既存のインポート・エクスポートと同じ文言) |
| `collection_ui.can_edit()` が false、または別の操作中 | 件数は出す | 無効 | コレクションを現在編集できません |

- 件数は `session.prepared().removable_missing.len()` を読むだけ。メニューを開いてもディスクを読まない。
- 件数が出せない間 (準備中・更新中) は件数を付けず無効にし、理由をホバーに出す。準備が終わると次のフレームから
  件数付きで出る。

### 4.2 見つからない項目の右クリック

報告者は見つからない項目を 1 件ずつ右クリックしているので、そこからも同じ操作へ届くようにする。

- コレクション直下の placeholder セルを、チェックなしで右クリックしたときのメニューに、「コレクションから外す」の
  直後へ「見つからない項目をすべて外す… (N件)」を足す。表示・有効条件は §4.1 と同じ。
- チェックありのメニュー、実在する項目のメニュー、フルスクリーン、コレクションの子 (実フォルダ) には出さない。
- `context_menu_model` に `MenuCommand::RemoveMissingFromCollection` を足し、`ContextMenuInput` に
  `collection_missing_removal: CollectionMissingRemovalAvailability` (`NotOffered` / `Ready { count }` /
  `Unavailable { count: Option<usize>, reason }`) を足す。値は右クリックの時点で
  `NativeGridContextMenuTarget` に取り込み、選んだときはその時点の対象 (stamp + revision + entry_ids) を使う
  (既存の `collection_root_delete` と同じ取り込み方)。native 描画と egui 描画は同じモデルを見るので両方に出る。

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
- 注記: ドライブや共有フォルダにつながっていない項目、読み取れない項目、未対応の項目は外しません。
- ボタン: 外す / キャンセル (既存と同じ。Enter / Escape も既存の modal と同じ扱い)

1093×614 logical points でもボタンまで見えること (最大 5 行のパスで高さを抑える)。

### 4.4 送信と結果

- 選んだ時点の `CollectionGridContentTarget` (stamp + expected_revision) と `removable_missing` の entry_id 群から
  `CollectionGridRemoveTarget` を作り、`request_collection_grid_remove` へ渡す (新しい経路は作らない)。
  sample_paths は同じ `removable_missing` の先頭から取る。
- 結果の適用、他の context への反映、Conflict の扱いは既存の「外す」と同じ (§5)。
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

- `prepare.rs`: Missing でルートあり → 対象 / ルートなし (存在しないドライブ文字・存在しない UNC) → 対象外 /
  Unsupported・AccessError → 対象外 / 並びが effective order / ルートの確認がルートごとに 1 回 (確認関数を
  注入できる形にして数える) / 取り消しで止まる。
- ルートの組み立て: Disk・UNC・VerbatimDisk・VerbatimUNC・Verbatim・DeviceNS・相対パス。
- 射影: `project_collection_display_snapshot` と書庫射影の後も `removable_missing` が残る (隠れた種類の
  Missing も含む)。
- メニューの状態: §4.1 の 4 行それぞれ (件数・有効・ホバー)。
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

小〜中。`prepare.rs` (フィールド + ルート判定 + テスト)、`keymap.rs` (メニュー 1 項目)、`ui_main.rs` (メニュー描画)、
`context_menu_model.rs` + `ui_dialogs/context_menu.rs` (右クリック 1 項目)、`ui_dialogs/collections.rs`
(確認ダイアログの種類)、テストのフィクスチャで `CollectionPreparedSnapshot` を直接組み立てている数か所。
永続データのスキーマは変えない (マイグレーション不要)。
