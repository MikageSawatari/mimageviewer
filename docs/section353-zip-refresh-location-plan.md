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
   外側履歴の cursor／stack は進めない。ここで確定する実位置は表示一覧の prefix／rows／route
   の整合であり、新しい起動復元先の採用ではない。§1.335 の既存契約に従い、更新は保存済み
   target と cursor を保持し、同じ明示済み target の通常の departure／exit だけが最新 cursor を捕捉する。
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

### P2 facet 帰路の追加修正 (2026-10-12)

新たな実効親へ `reproject` が追加した無条件 frame は、Backspace 用の帰路である。
badge の手動復元は最新の有効条件 frame 一件だけを消費し、間にある無条件 frame を
除去しない。手動復元後も新しい親への Backspace は無条件となり、消費した元親の条件を
再生成しない。

消失階層から祖先へ戻る採用は専用の純粋 reducer を使う。新 actual の Book scope と、
Smart／Collection の物理子 scope が表していた旧実効 Book root の alias を、同じ採用
payload へ渡す。alias は採用前の旧 `zip_nav` の実効 stack 先頭と論理元書庫から捕捉し、
同じ論理 source と typed physical child scope が一致する場合だけ作る。新 stack の先頭も
渡し、scope key が同じでも意味上の Book prefix が変わったことを区別する。
旧 Book scope と alias の意味上の Book scope のうち、新 actual と同じ論理書庫・segment
境界での祖先に当たる最深のものを選び、その scope が
所有する帰路 frame だけを復元する。旧／新 route の位置番号や common prefix を使った
連続 pop はしない。例えば旧 root→A→A/sub、更新後の実効 stack が [A] なら、A の
保存条件を復元し、表示されなくなった root の条件で上書きしない。actual が A/new や
B/pages に collapse したときも、旧 A／旧 root の scope から条件を選ぶ。
選んだ frame が手動消費済みなら active 条件を保持し、frame を再生成しない。
旧表示自体が root→A に collapse しており、新 B に対応する旧祖先 Book scope がない
場合は無条件とする。旧 root A → 新 root B／空 root へ A の条件を転用しない。
外側の Smart／Collection root の frame へ遡って上書きせず、残る外側の帰路は保持する。

条件決定後に新実効 route へ frame を再投影し、消えた wrapper の frame は復元せず
除去する。未消費 alias frame も、旧実効 Book root が新しい実効親 stack に残る場合だけ
保持し、新 parent の対応 scope へ再投影する。旧 []→A→A/sub、root PNG／A GIF の
更新先が A/new だけなら GIF を復元して obsolete root PNG frame を落とす。新 stack に
[] が残る場合は root PNG の帰路を保持する。消費済み条件は scope 表現が変わっても
再生成しない。Ancestor は旧 node 消失による場所変更なので、synthetic scope key が
同じ場合も場所条件を除く。外側履歴不変は既存契約を維持する。新しい pending／
完了待ち／モーダル制限は不要で、既存の成功採用境界内でこの条件決定だけを行う。

requested node が残る `Preserved` 採用にも、同じ同 source 確認済み alias と new actual
Book scope を渡して意味上の親 frame を再投影する。root []→A→A/sub の root PNG／A GIF
が保存されていて、更新後が A/sub 一階層だけなら両 wrapper frame を除く。
この採用理由では active facet の復元・無条件化・場所条件 clear は一切行わない。
残る外側 synthetic root の帰路と、手動消費済み frame の不在は維持する。

Smart／Collection の typed physical child は、空／非空を問わず ZIP の実効 stack 先頭を
表す。Book scope は二段目以降だけにし、初期 Open・履歴・Refresh・階層移動・同階層の
sort／pin 再投影で同じ route を作る。Open は既存 Navigation mode を維持する。
旧 Book(A) が新実効 root になった場合は、その保存 frame を physical child へ転写する。
逆に旧 physical child の実効 root が新しい途中 Book 親になれば、その Book scope へ転写する。
消費済み frame は両方向とも再生成しない。sort／pin 再投影で場所条件を消さない。

この producer 統一と双方向転写は独立 follow-up review (`gpt-6.1-sol` / `xhigh`) で
ACCEPT（コードレビュー）。純粋な親転写の手動消費有無、Smart／Collection 各16ケース、
初期 collapsed root の Backspace→再入場と、同階層の場所条件保持を回帰へ追加した。

回帰は手動復元→新親 Backspace、祖先 fallback と root collapse の同時発生、手動消費済み
A frame、collapse 先の新枝、外側 frame／履歴不変を含む。検証は全 writer の作業が安定した
後に担当を集約して行い、この節の追記時点では追加回帰をまだ実行していない。

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

### `3bfbf1989` 後の中間検証（追加6件の時点、2026-10-12）

手動復元で無条件の帰路frameを消さず、階層消失＋root collapseでは実際の祖先scopeのframeを
復元する。独立follow-up reviewで判明したSmart／Collectionのsynthetic ZIP rootも、旧routeの
同一scopeを優先してその保存条件だけを復元する。手動消費済みならactive条件を保持する。
新規回帰6件（pure3・実reload3）と既存の手動復元→Backspace回帰拡張を追加した。
独立コードレビュー `gpt-6.1-sol` / `xhigh` は追加P2修正後 **ACCEPT**。

最終targeted成功: `section1353`18件、`facet_navigation`18件、未接続の照合部品17件、
`folder_scan`45件、`folder_mtime`6件。計104実行・重複除外98件成功、失敗0件。
照合部品の初回fixture失敗3件と修正は§1.375の記録へ分離した。
`cargo fmt --check`、UI glyph検査（問題0件）、通常／portable core check、`git diff --check` は成功。
同じpinned TurboJPEGのworktree内explicit installとCargoジョブ2を使い、依存版・Cargo設定は変更なし。
検証環境: `target/nextlist-verify-env.ps1`。
ログ: [ZIP更新18件](../target/nextlist-section1353.log)、
[facet18件](../target/nextlist-facet_navigation.log)、
[通常check](../target/nextlist-check.log)、[portable check](../target/nextlist-check-portable.log)。

`build-release.ps1 -PreserveRuntime` は一回実行し **exit1、Rustビルド前で停止**。
起動中mImageViewer（APPDATAのruntimeを含む）を保護する既存ガードが停止を要求した。
アプリ停止・キャッシュ削除・保護ガードの迂回は行っていない。
このworktreeのrelease入力core／remote／EPUB workerは未生成のため、依存するtest-fullは
**今回0回・未実行**。既知の入力不足で再び0テストのfullを回して成功扱いにはしない。
先に起動中アプリの終了という前提を整え、release build後に指定のfull gate一回を引き継ぐ。
ログ: [release前提停止](../target/nextlist-build-release.log)。

`build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は確認用通常profileの構築中。
このworktreeのdev-runtime出力だけを検査し、既存アプリに作用しない。
TurboJPEGはshared registryをビルドせずworktree専用installを使う。
§1.373の未レビュー接続コードは退役し、§1.375もApp handler／context bundleへ未接続。
確認用binaryの機能確認対象は§1.353である。アプリ起動・コミットなし。

### 最新ソースの検証（2026-10-12）

追加回帰は9件（pure6、実reload3）。Smart／Collection の実reloadは各16ケース。
手動復元→Backspace、新実効親、階層消失＋root collapse、旧 Book 親→新 physical root、
逆方向、消費済み frame、初期 Open／履歴／同階層 sort・pin の canonical root と場所条件を検査する。
独立 follow-up コードレビューは最終修正後 ACCEPT、未解消の重要所見なし。
追加ケースの最初の実行は20件中18件成功・2件失敗で、root帰路の不一致をコードと試験で確定した。
期待値を維持して producer を修正した。ログ: [不一致の回帰](../target/nextlist-section1353-root-route-failure.log)。

最新targeted: ZIP21件、facet21件、未接続の照合部品17件、folder_scan45件、folder_mtime6件。
計110実行・重複除外101件成功、失敗0件。部品・走査・mtimeの計68実行は、その対象コードに
追加変更がないため有効な成功結果を再利用した。fmt、通常／portable core check、UI glyph検査
（問題0件）、git diff --checkも成功。
ログ: [ZIP21件](../target/nextlist-section1353-final.log)、
[facet21件](../target/nextlist-facet_navigation-final.log)、
[通常check](../target/nextlist-check-final.log)、[portable check](../target/nextlist-check-portable-final.log)。

release wrapperの保護停止後、CLAUDE.md「実機検証用バイナリの準備」に記載された代替の
core→remote→EPUB worker→launcher直接cargo buildを実行中。worktree内のexact出力に
起動中processがないことを確認し、VST3 identity／EffeTune source・stage／VCRT gateを維持する。
既存アプリを停止せず、APPDATAにも作用しない。依存版は変更せず、同じexplicit TurboJPEGと
Cargoジョブ2を使う。release四段完了後にtest-full一回、最新sourceのbuild-devを更新する。
§1.375第一段Delete／§1.373 Undoは各設計書の前提矛盾で未接続のまま。新Undoと新Busyは製品に残していない。
release直接buildの初回はcore binaryのLLVM段階で `out of memory / Allocation failed`、exit101。
Rustの型エラー・製品テストの失敗ではない。ログを `target/nextlist-release-oom.log` に保存した。
通常プロセスを停止せず、Cargoジョブだけ2→1へ下げ、同じopt-level=3／ThinLTO／依存版で
一回再実行中。Windowsメモリ照会では物理127.3GiB中50.1GiB空き、commit空き22.4GiBだった。
system設定・pagefile・Cargo.toml・release最適化は変更していない。

### release 入力の完了と全体 gate の開始（2026-10-12）

直接 release build の単一ジョブ再実行は exit0。core 29分34秒、remote 5分53秒、EPUB worker
2分49秒、launcher 1分01秒で四段とも成功した。最終 VCRT 検査は runtime4／PE4 成功。
標準 opt-level=3／ThinLTO、VST3 identity と EffeTune stage を保持し、既存アプリの停止・
APPDATAへの変更・アプリ起動は行っていない。
ログ: [release四段](../target/nextlist-release-inputs.log)。

`test-full.ps1 -SuppressCrashDialogs` は今回一回を開始済み（2026-10-12 20:33:33 JST）。
開始印 `target/nextlist-test-full-started.txt` が重複実行を禁止する。全体テストと後続の
最新sourceの `build-dev.ps1` は一担当が所有し、結果・終了コードを同worktreeのログへ記録する。
現在はworkspaceのテスト用コンパイル中。最終実数は完了後に追記する。

### 今回の変更範囲と利用者確認

- Rust（意味のある差分8ファイル）: [app.rs](../src/app.rs)、
  [facet_navigation.rs](../src/app/facet_navigation.rs)、[tests.rs](../src/app/tests.rs)、
  [folder_scan.rs](../src/app/folder_scan.rs)、
  [similar_navigation_tests.rs](../src/app/similar_navigation_tests.rs)、
  [folder_reconcile.rs](../src/app/folder_reconcile.rs)。最後の照合部品は製品へ未接続。
  [startup_list_restore.rs](../src/app/startup_list_restore.rs) は更新を復元先の明示採用から除外する修正。
  [ui_main.rs](../src/ui_main.rs) は実更新メニュー回帰の非同期採用待機だけ。
- 文書7ファイル: 本書、[§1.375](section375-operation-reconcile-plan.md)、
  [§1.373](section373-book-append-undo-plan.md)、[virtual-folders.md](virtual-folders.md)、
  [README.md](README.md)、[next-release-backlog.md](next-release-backlog.md)、
  [startup-restore-target-plan.md](startup-restore-target-plan.md)。
- manual: [shortcuts.html](../htdocs/mimageviewer/manual/shortcuts.html)。ZIP更新時の階層保持を記載。

確認用 binary の最新構築が成功した後、利用者が起動するコマンド:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

通常 `%APPDATA%\mimageviewer` profile を使い、実際の設定・データを更新し得る。
single-instance mutex を共有するため、インストール済み／トレイ常駐の mImageViewer を先に閉じる。
agent は起動しない。確認対象は通常 ZIP と Smart／Collection 子の深い階層で、更新メニュー／
割当済み `GridReload` による位置と条件の保持、手動復元→Backspace、階層消失と root collapse、
初期入場／履歴／sort・pin 後の帰路。現基点の F5 は既定★5、`GridReload` は既定キーなし。
§1.375 の Delete と§1.373 の新 Undo は、この binary の完了機能として扱わない。

### full 一回の結果と原因修正（2026-10-12）

指定の full は一回を完走し、終了コード101。全62結果行の合計は13,339成功・48失敗・63無視
（内部subprocessの2結果行も含む）。本体libraryは12,126成功・44失敗・56無視、LibRawは0成功・4失敗。
workspaceは全対象を終えたが、後続のegui／egui-wgpu／eframeは未到達。自動のdev構築も止まった。
ログ: [full一回](../target/nextlist-test-full.log)。再実行しない。

失敗の41件は `vendor/raw-samples` 未準備が原因。既存nextfix／nexttile worktreeの非reparseな
実directoryにmanifest全27ファイル（計360,971,087 bytes）のsize／SHA-256一致を確認した。
同じ実ファイルをこのworktreeへコピーし、再照合後にRAWのtargeted検証を補完する。
full実行中のサンプル追加や環境変更は行っていない。

非RAWの7件のうち、export／Stack Escape／viewer後watch反映／locked facetの更新メニューの
4件は、既存PhysicalHistory workerの採用前にitems／入力受付を検査していたfixture。
元の期待・呼出回数を維持し、既存採用ownerの終端をtest内だけで待つ。製品へ同期待ちを足さない。

残る起動復元3件は製品の回帰で、`finish_main_list_open` がRefreshListをExplicitListと同じ
復元記録Adoptへ入れていた根因を修正。RefreshListのcursor復元・settings.saveは保ち、
新target採用だけから除外する。既存§1.335の3回帰の期待は変更していない。
独立追加確認はこの最小修正をACCEPT。本文の不変条件5と§1.335文書も同じ保存契約へ訂正した。
新owner／状態／回復機構は追加しない。

補完順は失敗関連targeted・ZIP／startup／reload回帰、fmt、通常／portable core check、
未到達vendor3ゲートの初回、最新sourceのdev構築。担当を一つに集約し、各結果を別ログへ記録する。
release入力四段はfull前に構築した版であり、最後の起動復元修正の利用者確認用は最新devとする。
補完検証の最終実数は完了後に記録する。

### 補完targeted・checkの確定結果（2026-10-12）

RAW sample全27件の実コピーとsize／SHA-256照合は成功。
補完targetedはLibRaw4件、本体RAW354件（既存無視1件）、startup73件、reload72件、ZIP21件、
export／Stack Escape／watch復帰／facet更新メニュー各1件、名前にrawを含まないCR2回帰1件、
計529成功・失敗0。
変更がないfacet21件、照合部品17件、folder scan45件、mtime6件の89実行を再利用し、
最新の有効なtargetedは**618成功実行・重複除外596件・失敗0**となる。

起動復元3件の期待は変更せず、全startup回帰を成功させた。四つのfixtureも元のitems／
入力受付／一回だけreloadのassertを維持して成功。UIからprivate pollを呼んだ補完初回の
コンパイル失敗は、`#[cfg(test)]`だけの既存ownerへの委譲入口で修正し、製品visibilityは維持。
その限定follow-upは最終ACCEPT。失敗compileのログを保存し、テスト実行失敗と混同しない。

fmt、通常／portable core check、glyph検査（問題0）は最新sourceで全成功。
ログは `target/nextlist-supplement-*.log`、各終了コードは同名`-exit.txt`に保存。
参照: [RAW354件](../target/nextlist-supplement-raw-r2.log)、
[startup73件](../target/nextlist-supplement-startup.log)、
[reload72件](../target/nextlist-supplement-reload.log)、
[ZIP21件](../target/nextlist-supplement-section1353.log)、
[通常check](../target/nextlist-supplement-normal-check.log)、
[portable check](../target/nextlist-supplement-portable-check.log)。

未到達vendorの初回補完はegui25件、egui-wgpu11件、eframe18件の全54件成功、各exit0。
最新sourceの `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は通常profileで成功、exit0。
portable featureは使わず、起動・既存アプリ停止・APPDATA変更は行っていない。
元のfull一回はexit101のまま保存しており、上記補完をfull再実行／full PASSと表記しない。

### 最終引継ぎ（2026-10-12）

初回full失敗48件と補完成功のcrate＋qualified test nameを照合し、全48件を被覆、未被覆0件。
名前に`raw`を含まない `thumb_loader::tests::remote_cr2_preview_is_available_from_zip_bytes` は
exact filter一件で追加成功した。sourceは変更せず、dev／fmt／check等の有効結果を再利用した。
補完targeted＋初回vendorの合計は583成功・失敗0・既存無視1、重複除外577件。
変更がない関連89実行も合わせた有効結果は672成功実行・重複除外650件
（targeted618／596件、vendor54件）。

最新devはcore16分46秒、remote0.68秒、EPUB worker0.56秒で成功。worker二件は変更がなく
有効cacheを使用。core／remote／EPUBの3exeと最終VCRT runtime4／PE3成功を確認した。
coreは2026-10-12 22:19:31 JST生成、324,525,568 bytes、SHA-256
`A0F2D626577EFF9395442DDD0071567F510CDD4718E5F0995363292C390AD4BB`。
ログ: [最新dev](../target/nextlist-supplement-build-dev.log)、
[CR2 exact一件](../target/nextlist-supplement-missing-cr2.log)、
[VCRT report](../target/vcrt-pe-reports/dev-runtime.json)。

限定最終コードレビューはACCEPT。コミット・アプリ起動・既存アプリ停止・ユーザーの
APPDATA profileへの変更は行っていない。§1.375は初期採用provenance、§1.373は現在の本への
appendと観測先着について、各設計書の判断点で停止したまま。両項目の製品修正は未完了。
最終 `git diff --check` は成功。文書7件のUTF-8を確認し、既存BOMを保持した。
HEADは `3bfbf1989` のまま。変更範囲は上記Rust8・文書7・manual1、未コミット。
