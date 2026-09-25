# レーティング操作・一覧ソート・タグジャンプ 実装計画

最終更新: 2026-09-15

状態: §1.238 Phase A は実装、focused 自動回帰、独立 completion review を完了。2026-09-14 に
設計 lead と独立 reviewer が typed jump の構造へ合意し、2026-09-15 に完成差分が承認された。
Phase B の名前 / 番号降順と表示名統一も製品実装、focused回帰、独立completion reviewを完了した。
§1.237 の製品実装は別 chunk とし、特に `GridDisplayOrderState` は追加の設計 review とscope決定まで
着手しない。

## 0. 正本、範囲、完了条件

要件の正本は [次版バックログ §1.238 / §1.237](next-release-backlog.md#1238-タグビューのフォルダに移動が動作しない--397-2026-09-14)
である。この chunk は次の順に進める。

1. §1.238: タグビュー等の検索結果から「フォルダに移動」したとき、特殊一覧を終了し、
   対象の実フォルダを一度だけ開いて対象項目を選択する。
2. 一覧へ名前降順・番号降順を追加し、名前 / 番号 / 日付の表示名をフォルダツリーと揃える。
3. §1.237: 評価を1段階上げる / 下げる操作と、通常一覧の評価昇順 / 降順を追加する。

§1.239、§1.240 は [保留バックログ](backlog-on-hold.md#今回のリリース後へ延期2026-09-14-利用者決定)
どおり次回以降へ延期する。コレクション Phase 4 / 5 もこの chunk では実装しない。
本・ZIP・PDF の固定ページ順、フォルダ代表サムネイルの候補順、評価別ビュー固有の設定時刻順は
変更しない。

完了条件は次のとおり。

- タグ / Ctrl+G / Ctrl+S の同族経路で、特殊一覧の復元と実フォルダ遷移が競合しない。
- 実ファイルを対象にした移動は親フォルダを開き、そのファイルを安定した実パスで選択する。
- 一覧用 `SortOrder` の名前 / 番号に昇順・降順があり、表示名がツリーと一致する。
- 未評価 0 と明示的な ★3 の意味を保ったまま、指定した2方式で評価順を作れる。
- 評価変更時に同期 DB I/O や一覧 `items` の危険な並べ替えを行わず、選択、再生、
  コンテキスト別キャッシュを保ったまま表示順だけを更新する。
- 既存の直接評価、Undo / Redo、XMP、複数ウィンドウ通知、未評価 / ★1〜★5 一覧が同じ
  所有境界を通る。

## 1. 調査で確認した現状

### 1.1 タグビューからの「フォルダに移動」（実装前の根本原因）

`MenuCommand::JumpToFolder` は [context_menu.rs](../src/ui_dialogs/context_menu.rs) で
`ContextMenuAction::JumpFromSearch(PathBuf)` を返す。ファイル項目は親フォルダだけを保持し、
移動後に選ぶ実ファイルの identity を action に載せていない。`JumpToBookFolder` だけは action を
返す前に、App 全体の `select_after_load: Option<String>` へ basename を先書きしている。

`apply_jump_from_search_to` は各検索 mode の legacy `saved_folder` を `None` にしてから
`close_global_search` / `close_favsearch` / `close_tag_view` を呼ぶ。しかし `close_tag_view` は
`dismiss_tag_view_without_restore` のあと、返された `TopLevelGridRestore` を必ず復元する。
`dismiss_tag_view_without_restore` は legacy fallback より canonical な
`top_level_grid_view.take_return_to()` を優先するため、`tag_view.saved_folder = None` では復元を
止められない。

その結果、「検索前の一覧を復元する遷移」と「対象の実フォルダを開く遷移」が同じ操作から連続して
発行され、前者の cancel / load / state reset が後者を破棄または上書きできる。待ち時間不足ではなく、
終了と復元と遷移要求の owner が分かれていることが原因である。既存テストは legacy
`saved_folder` だけを模擬し、`TopLevelGridView` の canonical `return_to` を設定していないため、
この競合を検出していない。

同じ `JumpFromSearch` / `apply_jump_from_search_to` を Ctrl+G、Ctrl+S、タグビューが共有する。
このためタグだけに guard を足さず、同族 route を一つの遷移 request に統合する。

### 1.2 一覧表示順

一覧の `SortOrder` は現在 `FileName`、`Numeric`、日付2方向、サイズ2方向である。`FileName` と
`Numeric` は既存保存値でもあるため、名称変更や serde rename をせず、これらを昇順として残す。
フォルダツリーには名前 / 番号 / 日付の6候補が既にあり、長い表示は
`名前（昇順）`、`名前（降順）`、`番号（昇順）`、`番号（降順）`、
`日付（古い順）`、`日付（新しい順）`、短縮表示は `名前↑/↓`、`番号↑/↓`、
`日付↑/↓` である。一覧は `ファイル名順`、`番号順（区切り無視）` 等で一致していない。

`ListingSortMetadata` はファイル名 key と同じ長さの typed metadata として mtime と file size を
持つ。サイズ順追加時に、描画中の I/O を避けて producer 側で key を作る基盤ができている。
一方、通常フォルダの評価 cache prewarm は一覧 install 後に UI thread から一括 DB 取得しており、
タグ一覧は現在の `sort_order` で materialize しない。評価順には producer ごとの取得境界を揃える
必要がある。

`visible_indices` は raw `items` index の昇順 membership として `binary_search` / `partition_point`
から使われる。詳細表示だけは `details_order` が表示順を持つが、サムネイル描画は
`visible_indices[vis_pos]` を直接読む。評価変更後に `items` を並べ替えると、index を key にした
thumbnail、metadata、rating、選択、fullscreen、再生状態を一斉に remap する必要が生じる。
`visible_indices` をそのまま表示順にすると、既存の membership 前提を壊す。どちらも安全な局所修正に
ならない。

### 1.3 評価変更

既存の `apply_rating_to_selection(stars)` は、可視 checked があれば checked、なければ selected を
対象にし、各成功変更を一つの Undo にまとめる。`set_rating_result` から
`write_user_rating_shared` を通り、SQLite、App-global path generation、context-local cache、
フォルダ集計、XMP、検索結果、評価別ビュー、スマート絞り込みを更新する。fullscreen 画像 / 本、
native video も同じ低位 write 境界へ到達するが、入口と変更後処理は別々である。

複数 viewer context は rating DB 自体を共有し、`record_rating_session_write` と
`sync_current_context_rating_session_writes` で各 context の cache を収束させる。この境界を保ち、
評価 sort の再配置だけを各 context の表示順 owner へ通知する。

## 2. タグジャンプの構造修正

### 2.1 一つの typed request が終了、行先、選択を所有する

`JumpFromSearch(PathBuf)` を、少なくとも次の情報を持つ typed request に置き換える。

```rust
struct JumpToFolderRequest {
    destination: JumpToFolderDestination,
    selection: JumpSelection,
}

enum JumpToFolderDestination {
    PhysicalDirectory(PathBuf),
    ArchiveContainer(PathBuf),
}

enum JumpSelection {
    None,
    ExactPath(PathBuf),
}
```

実ファイル / 動画 / archive 本体等は `destination = physical_path.parent()`、
`selection = ExactPath(physical_path)` とする。`GridItem::Folder` と Folder 型 `SearchContainer` は対象自体を
物理フォルダとして開くため `destination = PhysicalDirectory(target path)`、`selection = None` とする。
Zip 型 `SearchContainer` は `ArchiveContainer(target path)` とし、directory scan へ誤投入せず既存の
archive open / conversion lifecycle へ渡す。
`JumpToBookFolder` は container の親を開き、container の exact path を選ぶ。同名や大小文字差に
依存する basename だけをこの request の identity にしない。

context menu は action を返すだけとし、`select_after_load` や検索 mode を先に変更しない。
現在ある「context nav が frame 内の優先度裁定に勝ったあとだけ副作用を適用する」という境界を維持する。

### 2.2 検索終了時に return context を消費し、復元しない

request が勝ったときは、active な Ctrl+G / Ctrl+S / タグ mode ごとに
`dismiss_*_without_restore` を呼ぶ。ここで pending worker を cancel し、canonical
`TopLevelGridRestore` を必ず take するが、`restore_view_return_context` は呼ばない。
通常の閉じる操作と Backspace は従来どおり `close_*` を通り、typed origin を復元する。

検索前 folder を Back 履歴に積めるのは、消費した return context が
`TopLevelGridRestore::Folder` のときだけとする。Collection、SmartFolder、Rating、Bookmarks、
SubfolderExpansion 等を legacy `saved_folder` へ平坦化して物理 folder 履歴に積まない。
legacy `saved_folder` は古い状態の fallback として dismiss 内だけに残し、新しい request の owner に
しない。行先と検索前 folder が同一なら履歴を追加しない。

この操作から実 folder load を一度だけ発行する。`close_*` による origin load と destination load の
二重発行、sleep、retry、追加 repaint は禁止する。

### 2.3 selection は folder-open lifecycle に含める

exact selection を App-global の新しい `Option` として置かない。既存の typed
`FolderOpenScanPurpose` に `JumpToPhysicalFolder { selection }` を追加するか、それと同じ寿命を持つ
typed navigation request に含め、path、scan generation、selection が一緒に replace / cancel /
complete されるようにする。`RequiredFullscreenTarget` が exact target と scan を一緒に所有する構造を
先例とする。

scan 成功後、通常の一覧 install が終わった時点で `folder_tree::path_eq` 相当の Windows path 比較に
より exact path を一度だけ検索し、見つかれば現行の raw index と `scroll_to_selected` で選択と scroll
intent を更新する。Phase A は未承認の新しい表示順 owner に依存させない。見つからなければ request を
消費して残さず、開けた親フォルダには留まり、対象消失エラーを通知する。folder scan 自体の失敗、archive
変換確認の cancel / error でも stale selection を次の unrelated load に持ち越さない。

既存 `select_after_load` は BS、親へ戻る、作成 / rename 後の basename hint として残せるが、
検索結果から exact target へ飛ぶ request の正本にはしない。

### 2.4 同族 route inventory

実装時は次を一緒に確認する。

- タグ結果、Ctrl+G global search、Ctrl+S favorite search の `JumpToFolder`。
- `JumpToBookFolder` と Folder / SearchContainer の destination 解決。
- タグ / global / favorite の drill 中からの jump と、pending worker cancel。
- 通常のタグ Backspace、タグを閉じる操作、別 top-level view への直接切替。
- 物理 folder history と quick-folder history の suppress / push 所有。
- missing target、folder load error、archive conversion cancel、同 frame の別 navigation 勝者。

## 3. 一覧 SortOrder と表示名

### 3.1 enum と比較

`SortOrder` に `FileNameDesc`、`NumericDesc`、`RatingAsc`、`RatingDesc` を追加する。
既存 `FileName` と `Numeric` は保存互換のため昇順のまま残す。全 consumer の exhaustive match を
更新し、未知値 fallback だけで新 variant を隠さない。

一覧の長い表示名は次とする。

| variant | 表示名 | 短縮表示 |
| --- | --- | --- |
| `FileName` | 名前（昇順） | 名前↑ |
| `FileNameDesc` | 名前（降順） | 名前↓ |
| `Numeric` | 番号（昇順） | 番号↑ |
| `NumericDesc` | 番号（降順） | 番号↓ |
| `DateAsc` | 日付（古い順） | 日付↑ |
| `DateDesc` | 日付（新しい順） | 日付↓ |
| `SizeAsc` | サイズ順（小さい順） | サイズ↑ |
| `SizeDesc` | サイズ順（大きい順） | サイズ↓ |
| `RatingAsc` | 評価（昇順） | 評価↑ |
| `RatingDesc` | 評価（降順） | 評価↓ |

名前 / 番号 / 日付はフォルダツリーの表記と完全に揃える。サイズは既存表示との互換性を優先し、
長い表示名の「サイズ順」を維持する。番号順の description には両方向とも「記号・空白などの
区切りを無視して連番を比較する」という現行説明を残す。

降順は主 key だけを反転する。日付、サイズ、番号、評価の同値 key はファイル名昇順を tie-break にし、
filesystem 列挙順へ依存させない。名前降順は名前 key 自体を反転する。番号降順の delimiter 除去後に
同値になる名前も、最終的にはファイル名昇順で確定する。

### 3.2 固定順と代表画像の境界

次は新候補の影響を受けない。

- `folder_thumb_options()` は現行の `[FileName, Numeric, DateAsc, DateDesc]` の4候補を維持する。
  設定 sanitize も `is_size()` の除外ではなく、この4候補の明示 allowlist にする。名前 / 番号降順、
  サイズ、評価を folder representative 選択へ追加しない。
- ZIP / PDF page、本の読み順、reading history 等の view 固有の固定順を解除しない。
- Stack の代表セル、SearchContainer の集約セルを rating 0 と見なして再配置しない。
- `RatingViewSort::RatedAtAsc/Desc` は評価別ビュー固有の設定時刻順として維持する。
  `RatingViewSort::Normal(RatingAsc/Desc)` は同じ星数しかないため名前昇順 tie になる。

### 3.3 toolbar 保存 migration

サイズ順追加時の `toolbar_sort_size_options_migrated` は再利用しない。Phase B 用の独立 marker を追加する。
load 時に marker が false で、保存済み toolbar が現在の canonical 6候補と完全一致するときだけ、
名前 / 番号降順を含む canonical 8候補へ拡張する。Rating 公開時は別 marker を使い、canonical 8だけを
Rating を含む canonical 10へ拡張する。順序違い、部分集合、空 vector を含む custom toolbar は一切補完
しない。各 phase の新規 `Settings::default()` は対応 marker を true とする。一度 migration 後に利用者が
新候補を隠しても、次回 load で復活させない。

`SortOrder::all()` を保存文字列へ変換する Remote、favorite view、collection sort facts 等も更新する。
コレクション Phase 3 が触れている共有ファイルは引き継ぎ後にだけ編集し、Phase 4 / 5 の追加仕様は
混ぜない。

## 4. 評価順の意味と設定

### 4.1 未評価位置

設定型を次の2値にする。

```rust
enum RatingSortUnratedPosition {
    BetweenThreeAndTwo,
    BelowAll,
}
```

既定は `BetweenThreeAndTwo` とする。未評価を操作上だけ ★3 を基準にする §1.237 の説明と近く、
0 と明示的な3の区別も保てるためである。環境設定の評価 page に選択肢を置き、降順の実例をそのまま
表示する。

- ★3と★2の間: `★5, ★4, ★3, 未評価, ★2, ★1`
- 全評価より下: `★5, ★4, ★3, ★2, ★1, 未評価`

昇順は選んだ列の完全な逆順とする。DB の保存値は従来どおり 0〜5 で、0 を3へ書き換えない。
比較は純粋関数にし、2 mode × 2 direction × 0〜5 の全組合せを table test で固定する。同じ rating 内は
常にファイル名昇順を tie-break にする。

### 4.2 ratable と non-ratable を混同しない

`ListingSortMetadata` に rating key を追加する場合は `Option<u8>` 等の typed availability を使う。
`Some(0)` は評価可能だが未評価、`Some(1..=5)` は評価済み、`None` は評価非対応である。
0 を unavailable sentinel にしない。

通常の混在一覧で rating sort を許す場合、`None` は両方向とも rating domain の後ろへ置き、相互は
名前昇順とする。昇順が完全反転する要件は評価可能な0〜5 domain に適用する。view 固有の固定セルは
その view の category / fixed-order policy を優先し、rating sort のために評価可能扱いへ変えない。

### 4.3 sort spec の snapshot

未評価位置を比較関数の暗黙 default にしない。worker / prepared result が少なくとも
`sort_order` と `rating_unrated_position` を同じ snapshot として持ち、完了時に現在設定と一致する
owning context だけが結果を適用する。既存 `CurrentViewOrderSnapshot` もこの設定を含める。
比較関数は `SortOrder` と snapshot 済み policy を受け、UI thread の live settings を途中参照しない。

## 5. rating metadata の producer 境界

描画、cell tooltip、比較 closure から rating DB を読まない。`ListingSortMetadata` と同じ aligned key、
または同等の prepared row を各 producer が一括生成する。

| 一覧 producer | rating key の取得元 / 方針 |
| --- | --- |
| 通常の物理 folder | directory scan / listing preparation worker で ratable path を集め、`rating_db.get_many` を1回行う。UI thread は prepared result を install するだけにする |
| Ctrl+G global search | 既に `GlobalHit.stars` にある値を使う |
| タグ一覧 | tag/stat worker 内で対象 path を集め、一括 rating 取得と sort を行う。現在抜けている `settings.sort_order` 適用もここで直す |
| smart folder / subfolder expansion | 既存 worker の rating batch map を同じ materialization に載せる |
| rating view | row が持つ stars を使い、追加 DB 読みをしない |
| bookmarks / reading history | row snapshot または既存 worker での一括 rating 取得を使う。view 固有固定順は維持する |
| collection | Phase 3 の current normal-sort contract を維持する範囲だけ rating fact を追加する。Phase 4 / 5 の閲覧仕様を先取りしない |
| Remote | server と local が同じ sort spec / label / comparator を使う |

通常 folder の現行 `prewarm_rating_cache` を UI thread の新しい同期 point-get 群へ拡張しない。
rating sort に必要な全件 snapshot は preparation worker 側へ移す。表示付近の XMP hydration は別目的なので
残せるが、結果を rating cache へ publish したとき、後述の同じ再配置 finalizer を呼ぶ。

## 6. 表示順の単一 owner と安定した再配置

この節は方向性の記録であり、下記の4段階設計を確定するまで製品実装を承認しない。初期案の
`{ generation, 3 vectors, revision }` だけでは producer 固有の固定順、rating facts 完備性、worker priority、
snapshot / context restore を再現できないことが独立 review で判明した。

### 6.1 typed context-owned state

`visible_indices`、thumbnail の実表示順、`details_order` の関係を新しい必須型へ集約する。

```rust
struct GridDisplayOrderState {
    items_generation: u64,
    authority: EffectiveGridOrderAuthority, // Fixed / Manual / Sortable
    facts_generation: u64,
    facts: Vec<GridOrderFact>, // group/category, filename tie, Option<u8> rating
    visible_membership: Vec<usize>, // raw idx 昇順。binary_search 用
    thumbnail_order: Vec<usize>,    // filter 済みの通常一覧表示順
    details_order: Vec<usize>,      // filter 済みの詳細列表示順
    revision: u64,
}
```

これは必要条件を示す概念形であり、field 構成は C0 の設計 review で確定する。
`ViewerContextBundle` が context ごとに swap / park / restore する。既存 field の横へ pending bool や
別 `Option<Vec<_>>` を足さず、現行 `visible_indices` と `details_order` をこの owner の中へ移す。
items install、filter rebuild、normal sort、details header sort は型の method を通し、三つの vector と
revision を同時に更新する。不正な generation や index は debug assertion と unit test で検出する。

`current_grid_order()` は thumbnail mode で `thumbnail_order`、details mode で `details_order` を返す。
サムネイル描画、total rows、scroll hint、keep / prefetch 範囲、pointer hit、range selection、keyboard / gamepad
移動、fullscreen / slideshow の読み順は、raw `visible_membership` ではなく同じ current display order を使う。
filter membership 判定と selected の可視性判定だけが `visible_membership` の binary search を使う。

### 6.2 初期 materialize と rating 変更

既存 producer が既に requested sort で `items` を materialize している場合、初期
`thumbnail_order` は `0..items.len()` のうち可視な idx でよい。rating sort 中は prepared rating key で
この index vector を並べる。評価変更後は `items` と index-keyed cache を動かさず、変更を反映した
`thumbnail_order` / `details_order` だけを再構築する。

これにより `selected`、checked、`fullscreen_idx`、thumbnail、metadata、native video の current item
identity は保持される。sort だけで fullscreen や再生を閉じたり再始動したりしない。評価 filter の境界を
跨いで current item 自体が非表示になった場合だけ、既存の nearest-visible policy を使う。

main と detached は display order state と rating cache を別々に持つ。App-global session write を各 context
が consume したとき、その context の active sort が rating なら1回だけ order を再構築する。detached の
predicate、viewport、mount/unmount lifecycle は変更しない。実装がそこへ到達する必要が出た場合は、
[detached rework plan §2](detached-rework-plan.md#2-構造の正本constitution) と §11 の構造合意・記録 gate を
先に通す。

### 6.3 rating 更新後の共通 finalizer

直接★指定、増減、Undo / Redo、XMP import/hydration、別 context の session write が rating cache を更新したら、
一つの finalizer が次を行う。

1. active rating filter / virtual-view membership を再評価する。
2. active normal sort が rating なら display order state を再構築する。
3. selected が可視なら identity を保持し、新しい表示位置に対して scroll intent を必要時だけ更新する。
4. global search hit / rating view row、details revision、prefetch/navigation cache を既存規則で invalidate する。

smart folder の resident membership と他 metadata snapshot は従来どおり reopen まで固定する。ただし
active normal `RatingAsc/Desc` の表示 key はユーザー rating write の session publication を受けて再配置する。
これを smart query の再評価や Phase 4 / 5 の collection refresh へ広げない。

## 7. 評価を1段階上げる / 下げる

> §1.237 part A の確定範囲: アイテムとコンテナの両方に StepUp / StepDown を追加する。
> 評価ソートは別作業へ延期する。以下の当初案の「コンテナ評価の増減は追加しない」は
> この決定で置き換える。現行の直接指定は対象ごとに `set_rating_result` を呼び、成功行を
> 一つの Undo entry と finalizer に集める。下記の atomic batch は後続の構造案であり、
> part A はこの既存の成功行契約を共用する。

### 7.1 純粋な step 規則

```text
increase: 0 -> 4, 1 -> 2, 2 -> 3, 3 -> 4, 4 -> 5, 5 -> no change
decrease: 0 -> 2, 1 -> no change, 2 -> 1, 3 -> 2, 4 -> 3, 5 -> 4
```

`step_rating(current, direction) -> Option<u8>` を純粋関数にする。`None` は端で変化なしを表し、DB write と
Undo record を作らない。0 は操作計算時だけ3を基準にする。未評価を先に3として保存せず、増加は4、減少は2を
一度で保存する。clear は既存の直接操作を使う。

### 7.2 KeyAction と入口

`KeyAction::RatingItemStepUp` / `RatingItemStepDown` と
`RatingContainerStepUp` / `RatingContainerStepDown` を Rating context、`ALL_ACTIONS`、表示名、INI docs、
customization UI、parse / roundtrip tests に追加する。新操作の初期 chord は `ChordList::EMPTY`、文書表記は
`none` とする。

次の入口を同じ typed `RatingEdit::Step(direction)` へ変換する。

- Grid: checked 可視項目があればそれら、なければ selected。
- 画像 / 本 / 音楽 fullscreen: 現在の ratable item。
- native video window: current video item。native key result は stars の特殊値にせず、
  `Assign { container, stars }` と `Step(direction)` を区別する enum にする。

ring picker や context menu へ増減項目を追加する要件はないため、この chunk では増やさない。

### 7.3 既存 rating pipeline との統合

操作開始時に current context の session writes を同期し、その後 cache snapshot から各 target の before を
求める。複数選択は target ごとに異なる before / after を持てる。描画時や target loop 内の point DB read は
行わない。

直接★指定と増減の双方を、`ResolvedRatingChange { idx, key, before, after, meta, xmp_target }` 相当を commit する
共通経路へ寄せる。対象解決、`write_user_rating_shared` / batch transaction、path generation publication、
XMP、folder counts、search snapshots、rating view membership、Undo、表示順 finalizer を共有する。
same-value 直接指定と clamp 済み delta は変更集合から除く。成功した複数変更は既存どおり一つの Undo action とし、
失敗時の UI / Undo 整合性は現在の atomic batch contract を優先する。

単一 target が clamp、または一括 target がすべて clamp の場合は「これ以上上げられません / 下げられません」と
通知する。変更と clamp が混在した一括操作は成功変更数だけをまとめて通知し、対象ごとの toast は出さない。

Undo / Redo は before / after の異なる行だけを戻し、同じ session publication と finalizer を使う。
rename / move 後は既存 rating key migration の結果を再 snapshot し、旧 path key で表示順を固定しない。

## 8. 実装順と ownership

コレクション Phase 3 の dirty diff と共有ファイル writer を混ぜない。引き継ぎ後、次の coherent phase を
直列に進め、各 phase を独立 review できる状態にする。

### Phase A: §1.238 typed jump

- `ContextMenuAction` を destination + exact selection の request にする。
- 検索 mode の dismiss-without-restore を一つの終了境界から呼ぶ。
- typed folder-open purpose が selection を成功 / cancel / error まで所有する。
- handler-level test で canonical `TopLevelGridRestore` を設定し、タグ終了と destination 発行を同時に検証する。

### Phase B: 名前 / 番号降順と表示名

2026-09-15 に実装、focused回帰、独立completion reviewを完了した。

- `SortOrder`、comparator、labels、descriptions、serde / DB / Remote mapping を更新した。
- 新しい toolbar migration marker と custom-hidden 回帰を追加した。
- representative / fixed-order allowlist を明示的に現行値へ固定した。

### Phase C: rating metadata と表示順 owner（再設計待ち）

- **C0:** `Fixed / Manual / Sortable` を区別する effective-order owner へ現行 consumer を意味変更なしで
  移す。membership / display access、snapshot、Smart Folder prepared grid、viewer-context swap、prefetch、
  監査ツールを同じ ownership 単位にする。
- **C1:** producer 固有の group / category rank、filename tie、`Option<u8>` rating、facts 完備世代を持つ
  aligned row を非公開で接続する。collection の Manual / definition-owned Standard、本・ZIP・PDF・
  reading history の固定順を global sort から独立させ、全 producer が揃うまで Rating variant を公開しない。
- **C2:** display position と order revision を使う thumbnail worker priority、旧 queue の再投影 / 排水、
  小変更の incremental 再配置、rating change / session sync 後の共通 finalizerを実装する。toolbar rating
  sort と Details の rating header sort は別 finalizer として判定する。
- **C3:** `RatingAsc / Desc`、未評価位置設定、Remote / UI、canonical 8→10 migration を公開する。

Phase C は display、context ownership、multi-window に広く触れる。親の scope 決定と各段階の設計 lead / 独立
reviewer 合意前に製品編集を始めない。別の局所案へ変える場合も、`items` / cache permutation や追加 sentinel
に退行しないことを再レビューする。

### Phase D: rating step actions

- pure step、KeyAction、Grid / fullscreen / native video route を追加する。
- 直接指定と step を resolved-change commit / finalizer へ合流させる。
- Undo、XMP、multi-context、filter / order 回帰を追加する。

## 9. 回帰試験

### 9.1 タグジャンプ

- normal image / video、Folder、SearchContainer、book container の destination と exact selection。
- タグ top-level / drill 中、Ctrl+G、Ctrl+S の同じ action。
- canonical return origin が Folder / Collection / SmartFolder / Rating / Bookmark の各場合。
- 通常のタグ Backspace / close は origin を復元し、jump は復元せず destination を一度だけ開く。
- target が scan 後に消える、destination scan 失敗、archive conversion cancel、別 navigation が同 frame に勝つ。
- 名前 / 番号 / 日付 / サイズ sort 下でも同じ対象を選び、選択位置まで scroll する。

### 9.2 sort と設定

- 名前 / 番号の昇順・降順。delimiter を無視した numeric 同値、大小文字、同日時 / 同サイズの name tie。
- 既存 `FileName` / `Numeric` 保存値の load、全 variant の Settings / favorite / Remote / collection mapping。
- Phase B で canonical 6だけが8へ、Phase C3 で canonical 8だけが10へ移行し、custom順、部分集合、
  空 toolbar、新候補を後から隠した状態を維持する。
- folder representative 候補が4つのまま。本 / ZIP / PDF page と view 固有固定順が不変。

### 9.3 rating sort

- 0〜5を混在させた2 mode × 昇順 / 降順と、同率 name tie。
- `None` と `Some(0)` の区別、非対応 item の末尾規則。
- thumbnail と details の順序一致。details の ★ header sort も同じ未評価 policy を使う。
- folder、tag、global search、smart/subfolder、bookmarks、rating view、現行 collection、Remote の producer。
- draw / comparator の DB 呼出しゼロを fake DB counter または prepared-result test で固定する。
- rating write 後に raw item idx、selected、checked、fullscreen idx、native video path が変わらず、表示位置だけ変わる。
- main / detached の片方の変更が session sync 後に各 context の order へ一度ずつ反映され、片方の close で
  sibling order / cache を reset しない。

### 9.4 rating step と既存機能

- step table 全値、★5 increase / ★1 decrease の DB write・Undoなし。
- 未評価単一 / checked 複数 / 各値混在、直接★指定との target parity。
- Grid、画像 / 本 / 音楽 fullscreen、native video、別ウィンドウ。
- 一つの Undo / Redo で全成功行が戻り、clamp 行は含まれない。
- rename / move 後の key、XMP write / import、folder count、global search hit、smart filter。
- 未評価、★1〜★5の既存仮想一覧の membership と件数が不変。

## 10. 文書、検証、引き継ぎ

実装時は少なくとも [keymap spec](keymap-spec.md)、[key customization plan](key-customization-impl-plan.md)、
[rating list view plan](rating-list-view-plan.md)、[list size sort plan](list-size-sort-plan.md)、
[folder tree sort plan](folder-tree-sort-plan.md)、[top-level grid view](top-level-grid-view.md)、
[UI responsiveness](ui-responsiveness.md) を最終形へ更新する。ユーザー向け manual のソート、評価操作、
shortcut customization も同時に更新する。

検証は phase ごとの純粋関数 / handler test から始め、共有表示順と multi-context まで到達した時点で
repository 指定の focused tests、`cargo fmt --check`、full gate、`build-dev.ps1` を行う。通常 profile の
実行ファイルは agent が起動しない。live GUI が必要なら、シナリオ、時間、desktop/input 使用、使い捨て
portable data を先に提示し、別途明示承認を得る。

初版の調査・設計時点では製品コード、テストを編集せず Cargo / GUI を実行しなかった。その後、構造合意済みの
Phase A だけを実装し、focused test、`cargo check -p mimageviewer --bin mimageviewer-core`、
`cargo fmt --check`、UI glyph check、`scripts/test-full.ps1` を完走した。`scripts/build-dev.ps1` で
通常プロファイルの検証用 core / remote service も生成したが、GUI は起動していない。

### 10.1 Phase B 実装 checkpoint（2026-09-15）

- 既存の `FileName` / `Numeric` serde値を昇順のまま維持し、一覧専用の
  `FileNameDesc` / `NumericDesc` を追加した。番号降順は自然順の主keyだけを反転し、区切り除去後に
  同値となる項目の名前昇順tieを維持する。
- 名前 / 番号 / 日付の長短ラベルを `FolderTreeSortOrder` と一致させた。番号のdescriptionには
  記号・空白などの区切りを無視する説明を両方向に残した。
- toolbarは独立markerで旧canonical6だけをcanonical8へ一度だけ拡張する。さらに古いcanonical4は
  既存size migrationで6へ進めてから8へ進める。custom順、部分集合、空vector、移行済み候補を
  隠した状態は補完しないことをJSON / settings DB回帰で固定した。
- フォルダ代表サムネイルは `folder_thumb_options()` の明示4候補だけを許可し、一覧専用の降順、
  サイズ順を設定・cache key・workerへ流さない。本 / ZIP / PDFの固定順定数とconsumerは変更していない。
- 通常一覧、Ctrl+G flat / drill、Smart Folder、サブフォルダ展開、Stack、rating view、bookmark、
  collection、Remote physical folderが同じ比較契約を使う直接回帰を追加した。Phase Cのrating sortや
  表示順ownerは編集していない。
- 製品コードと各consumerの完成差分を独立reviewし、blocking defectなしを確認した。サイズの長い表示名は
  今回の統一対象から外れる既存表示「サイズ順（小さい順 / 大きい順）」を維持すると明文化した。
- focused回帰は比較契約、全consumer、toolbar / settings DB migration、serde / collection / Remote mapping、
  folder representative allowlist、一覧integrationを通過した。可視ラベル変更に合わせて環境設定のsnapshotを
  更新し、`scripts/test-full.ps1` は再実行でPASSした。
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --check`、UI glyph check、
  `cargo run -p viewer_context_audit --quiet`、`git diff --check` を完了した。
- `scripts/build-dev.ps1 -PreserveRuntime` で通常profileの検証用core / remote serviceを生成した。
  core SHA-256 は `1CE7557215DCE8EDF760E5E1A0D93738E86FEF9AE5B5B2ADB39D6C9867AA8E1A`、
  remote service SHA-256 は `192E2F800704A833C46831C2A42C47F24558B17A19C80F3DC23BEC21CE7D057D`。
  agentはGUIを起動せず、動作中processも停止していない。
