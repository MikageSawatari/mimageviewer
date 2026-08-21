# stage-archive-open — 複数ウィンドウモードで RAR / 7z / LZH を開くと
# メイングリッドまで書庫一覧へ切り替わる (backlog §1.99)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) を読むこと。**
症状と根本原因の正本: [next-release-backlog.md](next-release-backlog.md) §1.99。
所有権設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5。

ブランチ: `detached-rework`。コミットメッセージに `(detached-rework)` を含める。

---

## 1. 直すこと

複数ウィンドウモード (`Settings::detached_viewer_open_images_in_window` = true) で
グリッドの RAR / CBR / 7z / LZH をダブルクリックまたは Enter で開くと、
独立ウィンドウが開くだけでなく**メインウィンドウのグリッドも書庫の中身へ遷移する**。
ZIP / PDF では起きない。

**守るべき不変条件**: 複数ウィンドウモードから本を開く操作は、表示先の独立ウィンドウだけを
更新し、メイングリッドの場所・一覧 generation・検索 / 絞り込み・選択状態を変更しない。

## 2. なぜそうなっているか (確認済み)

- ダブルクリック ([ui_main.rs:13017](../src/ui_main.rs:13017)) と Enter
  ([app.rs:34249](../src/app.rs:34249)) は、どちらも先に共通 gate
  `open_grid_container_in_detached_book_context` を通る。
- その gate が使う typed open plan `DetachedGridItemOpenPlan`
  ([app.rs:1857](../src/app.rs:1857)) は `FolderCandidate` と `Descriptor` の 2 種で、
  `Descriptor` を作る `detached_book_context_descriptor_for_grid_idx`
  ([app.rs:37315](../src/app.rs:37315)) が扱うのは **`PdfFile` と `ZipFile` だけ**。
- したがって `GridItem::ConvertibleArchive` では gate が false を返し、
  通常ナビゲーションの arm ([ui_main.rs:13100](../src/ui_main.rs:13100)) へ落ちて
  `load_folder_or_convert_archive_with_auto_fullscreen` ([app.rs:18298](../src/app.rs:18298)) が
  `OpenRequestOwner::Navigation` 固定で走る。
- RAR は同期的に開けない。直読み可否を worker で probe した**後**、
  `pending_direct_nav` なら `load_zip_as_folder_with_input_seq`
  ([archive_convert.rs:729](../src/ui_dialogs/archive_convert.rs:729))、
  変換完了なら `load_folder_with_scan_owned`
  ([archive_convert.rs:869](../src/ui_dialogs/archive_convert.rs:869)) が
  **メイン一覧を書き換える**。

**「RAR を ZIP と同じ descriptor に足す」では直らない。** RAR は非ソリッド・入れ子なしのものだけが
直読みで、それ以外は変換対象という分岐があり ([virtual-folders.md](virtual-folders.md))、
どちらになるかは probe 完了まで決まらないため。

## 3. 手本にする既存実装 (これと同型にする)

ブックマークからの OtherArchive 開きは**既に正しく解けている**。写経元にすること。

| 段 | ブックマーク経路 | 位置 |
| --- | --- | --- |
| 要求 identity | `OpenRequestOwner::Bookmark(BookmarkOpenRequestOwner)` | [app.rs:308](../src/app.rs:308) |
| 完了時の宛先意図 | `ArchiveConvertCompletionPolicy::Bookmark(owner)` | [archive_convert.rs:47](../src/ui_dialogs/archive_convert.rs:47) |
| 直読み完了の着地 | `open_converted_bookmark_in_detached_context` | [archive_convert.rs:700](../src/ui_dialogs/archive_convert.rs:700) |
| 変換完了の着地 | 同上 | [archive_convert.rs:857](../src/ui_dialogs/archive_convert.rs:857) |
| 着地の中身 | 現アクティブを park → `ViewerContextDescriptor::Zip { path: 実体, entry_name: None, archive_source_override: Some(元アーカイブ) }` で新しい detached context を作る | [app.rs:39993](../src/app.rs:39993) |
| stale 判定 | `bookmark_open_owner_is_current` / `discard_stale_archive_bookmark_request` | [archive_convert.rs:647](../src/ui_dialogs/archive_convert.rs:647) |

`DetachedGridItemOpenPlan::FolderCandidate` ([app.rs:1857](../src/app.rs:1857)) も同型の先例で、
「分類が終わるまで main 所有、終わってから detached へ渡す」形になっている
(裁定は [app.rs:35075](../src/app.rs:35075)、detached 側での拒否は [app.rs:35198](../src/app.rs:35198))。

## 4. やること

### 4.1 typed open plan に candidate を足す

`DetachedGridItemOpenPlan` に variant を追加する。

```rust
enum DetachedGridItemOpenPlan {
    FolderCandidate { path: PathBuf },
    /// 直読みできるか変換が要るかが probe 完了まで決まらない書庫。probe / 変換の間は
    /// main 所有のままにし、実体が決まってから detached context を作る。
    ConvertibleArchiveCandidate { path: PathBuf },
    Descriptor(ViewerContextDescriptor),
}
```

`detached_grid_item_open_plan` ([app.rs:39909](../src/app.rs:39909)) が
`GridItem::ConvertibleArchive` に対してこの variant を返す。
`open_grid_item_in_detached_book_context_with_auto_fullscreen`
([app.rs:39952](../src/app.rs:39952)) の `match` に arm を足し、
`FolderCandidate` と同じく **その場では park も descriptor 生成もせず**、
detached 宛ての archive open を開始して `true` を返す。

⚠ **`should_auto_fullscreen_grid_container` ([app.rs:35247](../src/app.rs:35247)) は
`ConvertibleArchive` に対して false を返す**。この関数の戻り値を変えると
`park_active_detached_context_for_new_grid_open` など他の呼び出し元の意味も変わるので、
**この関数は変えない**。ConvertibleArchive の auto-fullscreen 判定は、通常ナビ arm が今使っている
`settings.effective_auto_fullscreen_zip_pdf()` ([ui_main.rs:13103](../src/ui_main.rs:13103)) と
同じ式を plan 側で使う。

### 4.2 要求 identity と完了時の宛先意図

- `OpenRequestOwner` に detached grid 用の variant を足す。
  `BookmarkOpenRequestOwner` と同様に **request id を持たせる** こと。
  window_id は stamp にしてはならない (**フォルダ再オープンで意図的に再利用される**、
  [app.rs:37568](../src/app.rs:37568))。
- `ArchiveConvertCompletionPolicy` ([archive_convert.rs:47](../src/ui_dialogs/archive_convert.rs:47))
  に対応する variant を足し、`request_rar_open_owned`
  ([archive_convert.rs:425](../src/ui_dialogs/archive_convert.rs:425)) と
  `request_archive_convert_owned` ([archive_convert.rs:369](../src/ui_dialogs/archive_convert.rs:369))
  の `match owner` を埋める。
- `claim_open_request_owner` ([app.rs:18409](../src/app.rs:18409)) に arm を足す。
  通常ナビゲーションが後から来たら、この要求は **stale として明示的に取り消す**
  (silent fallback にしない。理由付きでログを残す)。

### 4.3 3 つの完了経路すべてを detached へ着地させる

**1 つでも取りこぼすとメイングリッドが動く。** 対象:

1. **直読み確定** — `pending_direct_nav` 消費
   ([archive_convert.rs:676](../src/ui_dialogs/archive_convert.rs:676))。
   ブックマークと同じ位置に detached 分岐を置き、`load_zip_as_folder_with_input_seq` へ
   落とさずに `return` する。
2. **変換完了** — `pending_nav` 消費
   ([archive_convert.rs:795](../src/ui_dialogs/archive_convert.rs:795))。
   同じく `load_folder_with_scan_owned` の**前**で分岐する。
3. **変換キャッシュ命中 (同期)** — `try_archive_cache_lookup` が当たったときの
   `open_archive_via_cache_owned` ([app.rs:18357](../src/app.rs:18357))。
   probe を挟まずその場で開くので、ここは detached plan 側で先に処理してもよい。
   **どちらで処理するにせよ、メイン一覧を触らないこと。**

着地関数は `open_converted_bookmark_in_detached_context` と同型で新設する
(ブックマーク版を書き換えて共用にするか、並置するかは実装判断。
**ブックマーク経路の挙動は変えないこと**)。descriptor は

```rust
ViewerContextDescriptor::Zip {
    path: <実体 (直読みなら RAR 自身、変換なら キャッシュ ZIP)>,
    entry_name: None,
    archive_source_override: Some(<グリッドで選ばれた元アーカイブのパス>),
}
```

`archive_source_override` を元アーカイブにするのは、DB identity と表示上の場所を
利用者視点の元ファイルに保つため (ブックマーク版のコメントと同じ理由)。

### 4.4 失敗時

着地に失敗したら **要求を取り消して理由をログに残し、トーストを出す**。
黙ってメインナビゲーションへ落とさない (それが今のバグそのもの)。
ブックマーク版の失敗処理 ([archive_convert.rs:713](../src/ui_dialogs/archive_convert.rs:713)) に倣う。

## 5. 触ってよい / いけない範囲

触ってよい:

- `src/app.rs` の `DetachedGridItemOpenPlan` / `detached_grid_item_open_plan` /
  `open_grid_item_in_detached_book_context_with_auto_fullscreen` / `OpenRequestOwner` /
  `claim_open_request_owner` / 新しい着地関数
- `src/ui_dialogs/archive_convert.rs` の completion policy と 3 つの完了経路
- `src/ui_main.rs` / `src/app.rs` の `ConvertibleArchive` arm (detached gate が true を返したら
  従来処理へ落とさない、の一点だけ)
- `src/app/tests.rs` (テスト追加)

触ってはいけない:

- **visibility 述語、`show_viewport_*` の builder、viewport ID、HWND registry、
  activation watcher、placement** — 本ステージでは一切不要 (§1.99 の見込みどおり)
- `find_visible_thread_window_matching_rect*` (憲法 1)
- ブックマーク経路の既存挙動
- フル機能ウィンドウモードの従来の一覧遷移

新しい detached 用 bool / Option を App へ足さない (憲法 3)。要求 state が要るなら
既存の open request 機構 (`OpenRequestOwner` / completion policy) に載せる。
時間窓 (debounce / grace / settle) で競合を吸収しない (憲法 5)。

## 6. 完了条件

1. `cargo test -p mimageviewer --lib` が緑。既存 detached テスト 104 本を削除・弱体化しない (憲法 8)。
2. 新規テスト (最低これだけ):
   - 複数ウィンドウモードで `ConvertibleArchive` を開くと、plan が
     `ConvertibleArchiveCandidate` になり、メインの `items_generation` /
     `current_folder` / `selected` が変わらない
   - 直読み確定の完了が detached へ着地し、メイン一覧を変えない
   - 変換完了の完了が detached へ着地し、メイン一覧を変えない
   - 変換キャッシュ命中の同期経路でもメイン一覧を変えない
   - 完了までの間に通常ナビゲーションが来たら、要求が stale として**明示的に取り消される**
   - フル機能ウィンドウモードでは従来どおりメイン一覧が遷移する (退行防止)
   - ZIP / PDF の既存経路が変わらない (対照)
3. `cargo fmt` 済み。
4. 完了報告に、上記 3 つの完了経路それぞれについて「どこで分岐し、何を return したか」を
   file:line で書く。

## 7. 実機 smoke (利用者が実施)

複数ウィンドウモードで、次を順に開いてメイングリッドが動かないことを確認する。

1. 直読みできる RAR (非ソリッド・入れ子なし)
2. 変換が要る RAR (ソリッド or 入れ子あり)
3. 既に変換キャッシュがある RAR
4. 7z / LZH
5. 対照として ZIP / PDF
6. フル機能ウィンドウモードに切り替えて 1〜5 (従来どおりメイン一覧が遷移すること)

各独立ウィンドウに正しいページが出ること、閉じてもメイングリッドが元のままであることを見る。
