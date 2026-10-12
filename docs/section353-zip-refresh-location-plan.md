# §1.353 書庫の全再読込で現在階層を保つ

2026-10-12、`next-list` / `ea060b66e` 基点のコード調査。**設計レビュー待ち・未実装**。
実機の観測者はバックログ記載の利用者。エージェントはアプリを起動していない。
正本項目: [next-release-backlog.md §1.353](next-release-backlog.md#1353-zip-の中の階層で最新の情報に更新ctrlf5-するとzip-の最上位の階層に戻る--利用者の実機確認-2026-10-08)。

## 1. 原因と入口

`GridReload` → `reload_top_level_grid` (app/top_level_grid_view.rs:1278) →
`reload_current_folder_preserving_override` → `reload_current_view_in_place(Full)`
(app.rs:25395)。後者はカーソル名と変換書庫の alias を退避するが、
`current_folder` (= 外側の書庫／cache ZIP) と `PreservePresentation` だけを再読込へ渡す。
ZIP の現在 `zip_nav.current()` は要求に入らない。

`materialize_zip_grid` (app.rs:35840 付近) は `RestoreList` の `zip_prefix` だけを
`ZipNavState::restore_prefix` へ渡し、それ以外は `ZipNavState::new(tree)` を作る。
新しいツリーの root／自動 collapse 結果を最初から materialize するため、階層が失われる。
facet の採用処理はその root に route を合わせている。facet だけを保持する修正では、
行と route が食い違う。

既存回帰 `section1339_zip_same_location_reload_reconciles_prepared_prefix_and_manual_restore`
(app/tests.rs:23184) は、更新後に root へ戻ることを明示的に期待している。
この期待は今回の要求で更新する。ただし実際の root への fallback 時の facet 復元と、
手動消費済み frame を再生成しない検証は残す。

### F5 / Ctrl+F5 と関連経路

この基点の既定では、F5 は `RatingItem5`、`GridReload` は既定キーなし
(keymap.rs:6478 / :6562、keymap-spec.md)。Ctrl+F5 専用の更新処理はない。
利用者の保存済み割り当ては読んでいないため、そのキーがどの Action を呼ぶかは未確認。
F5 を `GridReload` に割り当てた場合と Ctrl+F5 を同 Action に割り当てた場合は同じ全再読込。
キー既定値を今回変更しない。テストは割り当てを明示して Action 配送を検証する。

| 対象／操作 | コード上の経路と適用範囲 |
| --- | --- |
| ZIP / CBZ の内部階層 | 上記の prefix 欠落が直接該当 |
| ZIP in ZIP の内部階層 | outer ZIP の `ZipTree` と完全 entry chain を共有。同じ欠落。内側 ZIP を実パスとして開き直さない |
| 変換済み書庫 | cache ZIP の `zip_nav` と source alias を使用。同じ欠落。既存 alias／source proof／cache routing は維持 |
| 直接閲覧 RAR / CBR | ZIP 系ツリー materialization を使用。prefix の同じ契約を適用 |
| PDF / EPUB | `PdfPage` 一覧に ZIP の内部 prefix はない。既存カーソル名、文書列挙、EPUB lease を維持。階層復元の対象外 |
| ピン更新 (`Pins`) | `refresh_current_zip_level_preserving_position` が既存ツリーの同階層を再投影。全再列挙の代用にはしない |
| 履歴再入場 | 既存仕様どおり root／collapse。全再読込と区別する |
| 検索・Collection・Rating の子 | reload owner と親への帰路を保つ。内部更新を合成 root の再クエリに置き換えない |

実機で PDF／変換書庫／nested ZIP を比較したという証拠ではなく、ソース上の経路監査である。

## 2. 修正設計と不変条件

単純化: 既存 `StartupListIntent` と採用境界へ payload を載せる。新しい pending prefix、
bool、完了後の「root を表示してから階層へ入り直す」処理、旧 tree の保持は導入しない。
全再読込をモーダルにする必要はない。既存要求の取消・source proof・stale 結果拒否を使用する。

1. 全再読込の受理時、既存意図 enum に `RefreshList` を追加し、論理 source と実効 ZIP prefix、
   カーソルヒントを一つの typed target に固定する。物理ロード先は既存 backing のまま。
   `PreservePresentation` 全体を別の意味にせず、起動 `RestoreList`、tray `ResumeList` と区別する。
2. classification、変換 dialog、staged physical、非同期 ZIP 列挙、同期採用経路は
   この同じ intent を既存 owner のまま伝える。`InternalHydration` も内側 intent を投影する。
3. 新ツリーで requested prefix が存在すれば、その prefix を復元してから一回 materialize する。
   `ZipNavState::restore_prefix` の既存 strict validation と実効親 stack 構築を再利用する。
4. prefix が消えた場合だけ segment 単位で祖先を調べ、残った最も深い node から既存 collapse
   規則に従う。文字列 prefix 比較や entry の大文字小文字を潰す比較は使わない。
   自動 collapse で消えたラッパーと Backspace の親 stack も新ツリーに基づく。
5. 採用時に actual prefix、rows、facet route を一緒に確定する。同じ actual route の更新は
   active facet と手動復元済み frame を保持。祖先 fallback 時は既存帰路の親 frame を復元。
   外側履歴の cursor／stack は進めない。主一覧の起動復元記録は成功した実位置と対応させる。
6. 列挙失敗・取消・別要求への切替・stale は既存終端に従い、旧一覧の prefix／facet／帰路へ
   遅延結果を適用しない。兄弟 viewer の tree／receiver／cache を変更しない。

所有する非同期要求と場所採用に関わるため、ユーザー指定に従い**この設計で実装前に止める**。
`ResumeList` の流用は、materializer が現状未対応であり、主一覧保存の意味も異なるので採らない。
まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。新しい retry／rollback は追加しない。

## 3. 実装範囲・検収

予定: `src/app.rs`、`src/app/startup_list_restore.rs`、`src/zip_tree.rs`、
`src/app/tests.rs`。staged producer が intent を脱落させる場合は、その所有ファイルを追加して
レビューへ戻す。detached 述語／viewport は変更しない。
実装時に `docs/virtual-folders.md` の現在位置と fallback の仕様も更新する。

予定回帰 (未実行):

- 純粋 state: 同階層、root、nested ZIP、wrapper collapse、深い階層消失→直近祖先、
  不正 prefix 拒否、Backspace stack。
- 実 handler: `GridReload` を F5 / Ctrl+F5 に明示割り当て、現在 A/sub と facet を保持。
- 主一覧: facet 手動復元済み／未復元、同じ再読込の反復、祖先 fallback、外側履歴不変。
- 変換 source/cache alias と直接 RAR、PDF / EPUB のカーソルと帰路。
- 検索／Collection／Rating 子からの更新、遅延結果・取消・別場所への移動。
- sibling context の資源を変更しないこと。

検証所有者: 実装担当。targeted → fmt → 通常／portable core check → 項目最後に full gate 一回。
アプリ起動・コミットなし。今回は文書のみ、テスト追加 0 件・実行 0 件・build 0 件。
