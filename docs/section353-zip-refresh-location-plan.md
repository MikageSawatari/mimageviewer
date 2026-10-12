# §1.353 書庫の全再読込で現在階層を保つ

2026-10-12、`next-list` / `ea060b66e` 基点のコード調査。**R2 設計 ACCEPT、実装・targeted/check成功、full gate未完了**。
実機の観測者はバックログ記載の利用者。エージェントはアプリを起動していない。
正本項目: [next-release-backlog.md §1.353](next-release-backlog.md#1353-zip-の中の階層で最新の情報に更新ctrlf5-するとzip-の最上位の階層に戻る--利用者の実機確認-2026-10-08)。

`001a0dabf` の独立設計レビューは **REVISE**。本改訂は P2 (子一覧の配送漏れ) と
P3 (prefix が存在する場合の collapse／親 stack 変更) を反映し、`1c313a2ec` の R2 設計レビューは **ACCEPT**。2026-10-12 の利用者指示で実装へ進む。

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
| 検索・Collection・Smart・Rating の子 | 現行の surface 単独 dispatch では取りこぼす。次節の root／物理子判定を追加し、子の既存 owner と帰路を保つ |

実機で PDF／変換書庫／nested ZIP を比較したという証拠ではなく、ソース上の経路監査である。

### 共通更新入口での配送 (P2)

Action handler (app.rs:52973 / :53340)、メニュー (ui_main.rs:7396)、sort control が呼ぶ
`reload_top_level_grid` を共通 resolver の入口とする。`surface` は親一覧の owner も表すので、
surface が Collection／Smart／Search であることだけで root 更新と決めない。
追加する resolver は保存状態ではなく、採用済み typed location／session position と
現在の rows の由来から、その場で更新先を導出する。UI で実パスの存在確認はしない。

| 採用済み表示 | resolver の配送先と保持する owner |
| --- | --- |
| 通常物理 Folder／ZIP／PDF | 共通 `RefreshList` の物理更新。ZIP は現在実効 prefix を付ける |
| Collection session `Root` | 従来の `open_collection_grid` で actor／root projection を更新 |
| Collection session `PhysicalSource` | `collection_grid_physical_reload_owner` を捕捉、intent を `Refresh` にして既存 staged physical load。entry ID／revision／親 restore anchor を維持。root を開かない |
| Smart `Root` | 従来の `refresh_smart_folder_staged` で root 更新 |
| Smart `Container`／`Scoped` | 現在 logical source と `smart_physical_target_kind` を使い、`begin_smart_physical_navigation_with_navigation` に同じ場所の navigation と `RefreshList` を渡す。definition、entry、back stack／source lease を維持。root 再走査へ戻さない |
| Favorite Search 結果 root (`on_results_grid`)／Tag 結果 root | 従来の再クエリ。親の入力 query と種別を維持 |
| Favorite Search `nav_stack` の物理子、Tag／Global Search から実際に開いた物理子 | 表示 producer が physical load なら、元検索の帰路を持つ共通物理更新。検索 active flag だけで結果 root と判定しない |
| Global Search の Flat／Aggregated／検索ヒット `DrilledInto` | 検索 hit の投影であり、物理 ZIP 全件一覧と混同しない。再クエリ要求へ採用済み `DrillState` (container root／current path／is_zip) と facet scope を固定して、同じヒット階層へ採用する。新結果で drill 階層が消えた場合だけ直近の存在するヒット階層へ戻す |
| Rating root／`RatingPhysical` | root は既存 sort 保持 rebuild、physical は既存 `RatingPhysicalLoadIntent::Refresh` と `RefreshList`。stars／nav chain／親一覧復元を維持 |
| Snapshot、その他の合成 root | 固定一覧 no-op 等の既存仕様を維持。`current_folder` が書庫に見えるだけで物理更新しない |

コード上の根拠: Collection は `commit_collection_grid_source_open` (:1750 付近) で
session position だけを変え、surface は Collection のまま。Smart の root refresh
(:7687) は Root target を作る一方、physical child API (:2401) は別の要求を持つ。
Favorite Search は `nav_stack`、Global Search は `DrillState` と実際の items producer を
併せて判定する。`items_are_global_search_view` 等は描画互換の派生情報で、採用位置の代用にしない。

物理配送を選んだ後の `reload_current_view_in_place` にも、親の surface を見て再び
root へ転送する分岐を残さない。共通 resolver の typed 結果から physical 更新へ直接渡す。
staged prepared producer、classification、ZIP materializer まで一つの `RefreshList` を伝え、
子の要求を受理できなければ理由を通知して旧子一覧を保つ。親更新への silent fallback はしない。
folder pane 更新は、同じ入口で現在物理子を対象に一回提出する。

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

所有する非同期要求と場所採用に関わるため、実装前の独立レビューで構造を承認済み。
`ResumeList` の流用は、materializer が現状未対応であり、主一覧保存の意味も異なるので採らない。
まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。新しい retry／rollback は追加しない。

### 存在する prefix の collapse／親 stack 変更 (P3)

requested node が残っている場合と、node が消失して祖先へ戻る場合を別の採用理由にする。
`A/` が残っていて新 tree で `A/sub/` に collapse する場合は、利用者の階層移動ではない。
active facet は更新前の値を保持し、その値を新 actual route へ束縛する。
旧 saved frame は新しい実効親 stack にも存在する同じ route の frame だけを保持する。
消えた／表示されなくなった中間 route の frame は落とすが、手動消費済み frame を再生成しない。
新たな parent に保存 frame がなければ Backspace は既存の無条件 facet へ戻る。
旧 tree の frame を別 prefix へ付け替えて条件を復活させない。

現在 actual prefix が同じで親 stack だけ変わった場合も active facet は不変、frame は上記
route 対応で再整合し、Backspace は新 tree の実効親を使う。node 消失時だけ §2.4–5 の
祖先 fallback／親 frame 復元を使う。この理由の区別は準備結果に載せ、採用後に推測しない。

## 3. 実装範囲・検収

実装対象: `src/app.rs`、`src/app/startup_list_restore.rs`、`src/zip_tree.rs`、
`src/app/top_level_grid_view.rs`、`src/app/collection_grid.rs`、`src/app/smart_folder.rs`、
`src/global_search_ui.rs`、検索物理子を扱う `src/ui_main.rs`、`src/app/tests.rs`。
各 staged producer の要求生成・準備・採用へ `RefreshList` を明示的に接続する。
detached 述語／viewport は変更しない。
実装時に `docs/virtual-folders.md` の現在位置と fallback の仕様も更新する。

回帰範囲:

- 純粋 state: 同階層、root、nested ZIP、wrapper collapse、深い階層消失→直近祖先、
  不正 prefix 拒否、Backspace stack。
- 実 handler: `GridReload` を F5 / Ctrl+F5 に明示割り当て、および更新メニューから、通常／
  Collection／Smart／Favorite Search／Global Search／Tag／Rating の root と物理子へ配送。
  ZIP の A/sub を保ち、子更新で親 root producer の呼出し 0 回・子採用 1 回を検査。
- 主一覧: facet 手動復元済み／未復元、同じ再読込の反復、祖先 fallback、外側履歴不変。
- 変換 source/cache alias と直接 RAR、PDF / EPUB のカーソルと帰路。
- 検索／Collection／Rating 子からの更新、遅延結果・取消・別場所への移動。
- sibling context の資源を変更しないこと。
- `A/` は存在するが子削除によって `A/sub/` に collapse する場合: jpg facet を保持、
  手動消費済み frame は復活せず、新しい Backspace の実効親とその保存 facet を同時に検査。
- `A/sub/` は同じだが root／A の wrapper 構成が変わる場合: active facet 不変、
  残存 route の frame のみ保持、消えた親 frame 不在、新 stack の Backspace を検査。
- Global Search のヒット drill と実 physical ZIP 子を別 fixture にして、再クエリの対象・
  query／drill／facet／帰路の保持を実 Action／メニュー入口で検査。

検証所有者: 実装担当。targeted → fmt → 通常／portable core check → 項目最後に full gate 一回。
アプリ起動・コミットなし。検証実数は末尾へ記録する。

### R2 追加回帰と実装レビュー

Global Search は完了・非 truncated・エラーなし・rating prepare の最終 batch ACK 後だけ祖先へ戻す。最初の batch に現在階層のヒットがなく後続 batch で現れる handler 回帰で drill／facet を保持する。コンテナ全体が消えた場合は既存の drill 解除で Flat／Aggregated へ戻す。

実装レビューで、新しい実効親の無条件 facet frame を場所変更時に除去する問題を修正した。frame は帰路として保持し、badge／手動復元は有効条件だけを対象とする。Smart の更新は論理 source と既存 backing を同じ既存 owner に渡し、変換元の metadata 再検証・再変換を行わず mounted 子を再列挙する。実 Folder の更新も既存 PhysicalHistory worker を使う。

非モーダル更新中の Ctrl+F は最新の提出済み条件と入力 draft を成功採用直前に捕捉し、同じ intent の既存 hydration 終端から新しい rows に一回再実行する。更新受理時の古い条件を利用者の再提出・close の上へ戻さない。

独立実装レビュー (`gpt-6.1-sol` / `xhigh`、2026-10-12) は修正後 **ACCEPT（コードレビュー）**。未解消の重要所見なし。targeted/checkは成功、full gateはrelease入力不足で未完了。PDF ページの Ctrl+F 禁止、warm verifier と起動復元記録の従来の意味は維持する。

### 検証記録 (2026-10-12)

追加回帰12件すべて成功。関連 targeted は28件成功（追加facet回帰2件の再実行を含む、計40実行・重複除外38件）。既存の ZIP 再入場3件、通常／Collectionの変換cache更新2件、Collection／Ratingの実ZIP更新・外側履歴2件、PDF2件、EPUB2件、facet15件、detached cacheのmain非干渉1件、park/remount1件を確認した。F5／Ctrl+F5はGridReloadへの明示割当で検証。既定キーは変更していない。

cold target の最初のnative buildがMSBuild段階で失敗したため、同じ pinned libjpeg-turbo 3.1.0 をworktreeの `target/native-verification/turbojpeg/` へ実コピーしてstatic/SIMDで構築した。依存版・Cargo設定は変更していない。検証コマンドだけ `TURBOJPEG_SOURCE=explicit`、`TURBOJPEG_LIB_DIR`／`TURBOJPEG_INCLUDE_DIR` をそのinstallへ設定し、MSVCのcanonical include経路を補う `CXXFLAGS=/I G:\home\mimageviewer-nextlist\target\native-verification\turbojpeg\install\include` を使用。Cargo同時ジョブは2。初回テスト実行のDLL不足は既存手順どおりFFmpeg／VCRT DLLの実コピーで解消した。

試験fixtureのキー受付前・別場所採用前の未完了待ちと、直接深い階層へ跳んだ旧親stackの期待を修正し、全12件を再実行して成功を確認した。実製品の同期待ち／入力制限は追加していない。

`cargo fmt --check`、通常／portable の `cargo check -p mimageviewer --bin mimageviewer-core`、UI glyph検査（問題0件）、`git diff --check` は成功。full gate は指定の一回を実行し、以下の前提不足で停止。

変更ファイル: `src/app.rs`、`src/app/startup_list_restore.rs`、`src/app/top_level_grid_view.rs`、`src/app/collection_grid.rs`、`src/app/smart_folder.rs`、`src/app/facet_navigation.rs`、`src/app/tests.rs`、`src/global_search_ui.rs`、`src/zip_tree.rs`（Rust9）。文書は本書、`virtual-folders.md`、`section375-operation-reconcile-plan.md`、`section373-book-append-undo-plan.md`、`README.md`、`next-release-backlog.md`（6）。§1.375／§1.373の実装は行わず、R2指摘を設計へ反映して再レビューを待つ。

`powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/test-full.ps1 -SuppressCrashDialogs` は **一回だけ実行、exit 101、テスト実行0件**。workspaceのlauncher build scriptが `target/release/mimageviewer-core.exe`、`mimageviewer-remote.exe`、`mimageviewer-epub-pdf.exe` の欠落を検出した。coreの型／回帰の失敗ではなく、全体gateのビルド前提が未成立。vendor3 crateの後続gateも未実行。指定に従いfullを再実行していない。ログ: [section1353-full.log](../target/section1353-full.log)。

cold worktreeにrelease入力が必要という前提不足でここを安全な引継ぎ区切りにする。確認用 `build-dev.ps1` binaryは全体gate前提未成立のため未作成。アプリ起動・コミットなし。coordinatorがrelease入力3exeを準備し、未完了gateと確認用buildを引き継ぐ。失敗をPASS扱いにしない。
