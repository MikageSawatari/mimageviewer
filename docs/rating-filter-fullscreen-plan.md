# 評価変更で読書ページが絞り込みから外れる場合 (§1.371)

2026-10-11 / Stage 1 設計案。調査対象: `next-tray` = master `ed0be21fa03e0fe2392317607a59e06e5326a4b5`。
製品コード未変更、実機再現未実施。観測は [バックログ §1.371](next-release-backlog.md) の利用者報告、以下は現ソースの照合結果である。
設計担当・独立レビュアーの合意前に実装しない。

## 1. 契約と現コードの差分

- 利用者決定 B: 評価変更で現在ページが表示条件から外れたら、読む順の次の残存ページ、なければ前へ移る。残存ページがゼロなら**現在のページ一覧**へ戻る。親フォルダへの自動退出ではない。評価トーストを残し、Ctrl+Z は評価を戻して元画像へ戻る。フィルターを解除しない。
- 不変条件: 評価・表示集合の公開が終わったとき、filter に従う reader の `fullscreen_idx` はその context の読書対象集合に属するか `None`。`selected` は保存対象の代用品にしない。見開きでは anchor と相方、連結では anchor と表示 unit を新しい集合で解決する。
- `set_rating_result_inner` → `write_user_rating_shared_deferred` は SQLite 成功後にだけ session generation を進める (`src/app.rs:70364,70622`)。`publish_current_context_rating_writes` は cache・検索・Rating/Smart membership・visible order を公開するが、全画面ページを調停しない (`:70498`)。
- `apply_rating_edit_to_fullscreen_item` はその公開後さらに `rebuild_visible_indices()` を呼ぶ (`:71346`)。コンテナ setter (`:71413`) と XMP hydration (`:71730` 以降) にも重複再計算がある。
- `fullscreen_seek_info` は `get_still_image_indices()` 内の現在ページ位置を `?` で探す (`src/ui_fullscreen.rs:21234`)。除外されたページなら None。strip geometry (`:20910`)、ページ番号 (`:21045`)、seek 描画 (`:21653`) が同じ情報に依存する。キャッシュ失効では直らず、集合外の anchor を作る公開経路が根因。
- **バックログとの差異**: 一覧の現実装 `redirect_selected_to_visible` は raw items index の `prev.or(next)`、前優先 (`src/app.rs:69695` 付近)。決定 B の次優先でも Details の表示順でもない。評価由来の選択移動を今回の共通規則へ揃える。履歴復元等の一般的な選択補正まで変更しない。
- **追加前提**: `open_fullscreen...` (`src/app.rs:65671`) と `close_fullscreen_now` (`:74891`) は `clear_meta_undo()` を呼ぶ。通常ナビをそのまま呼ぶだけでは自動移動後・最後の一枚退出後の Ctrl+Z を失う。`UndoEntry::Rating` は現在、値とキーのみで表示復帰先を持たない (`src/undo_stack.rs:133` 付近)。
- `sync_current_context_rating_session_writes` はcache同期だけで seen generation を進める (`src/app.rs:70462`)。getter/setterの事前同期からも呼ばれるため、「cache同期済み」を「集合とreaderまで公開済み」の代わりにしない。新しいseenフラグを足さず、既存seenは共通publication完了時にだけ進める。読み取り側のkey overlayはpublicationを消費しない。
- 本調査では導入時期のタグ・履歴は再調査していない。現在の状態不整合を確認したのであり、利用者の実機報告を新たに再現したとは主張しない。

## 2. 同型の入口・context の棚卸し

| 入口 / producer | 現経路と今回の扱い |
| --- | --- |
| F1–F5 / F6、カスタム評価キー、StepUp/Down | `[Rating]` の `RatingItem*` → `consume_rating_action` → `apply_rating_edit_to_fullscreen_item` (`ui_fullscreen.rs:30603`)。一覧は `apply_rating_edit_to_selection` (`app.rs:53287,72082`)。同じ保存・公開へ統合し、上限 no-op は保存・Undo・移動なし。新 Action / 固定キーを足さない。 |
| Shift+F1–F6、カスタム container/step キー | `apply_rating_edit_to_current_container` → `write_container_rating_for_target` (`app.rs:71316,71413`)。子ページの評価ではない。通常は子ページ集合不変なので移動なし。同じキーを持つ親一覧の行だけ membership が変わる。合成一覧は `current_container_rating_target` が拒否する既存契約を維持。 |
| 情報パネル・マウスの★、音楽パネル | `ui_metadata_panel.rs:2718` の `set_rating` / `set_current_folder_rating`、`ui_music_panels.rs:1232` の `set_rating`。表示ページの idx を保存対象として捕捉する。キーボードだけ直してこの入口を残さない。 |
| メイン toolbar/右クリックの評価メニュー | `ui_main.rs:11227` の selection/container setter。同じ共通近傍規則。画像全画面 HUD の独立した★メニューは見当たらず、★操作は情報パネル・キー・リング。 |
| native 動画キー / HUD の★ | `app/native_video.rs:12556` の共通 rating helper、`:6751` の `NativeVideoOutputEvent::SetRating` → `set_rating`。静止画 strip とは consumer が違うが、表示集合から外れる anchor の問題は同型。動画/音声を勝手に非対象にせず、既存 media open/close/presenter owner を使う回帰対象。 |
| マウス/パッドのリング設定 picker | `app/gamepad_input.rs:4071` の固定 key/owner に対する live preview → deferred setter → 一括 publication。確定時 `commit_live_picker_undo` (`:4878`) が before/最終成功値を一件にまとめる。preview 自体が DB 保存済みなので commit まで移動を延期しない。公開一回・Undo 一回を維持。 |
| Undo / Redo | `undo_ops.rs:184,233,321` → `write_user_ratings_shared` の transaction 成功後に同じ publication。Undo は後述の復帰要求、Redo は再び次/前/一覧の規則を使う。 |
| Remote rating | `remote_ipc/ui.rs:3040` の `persist_remote_rating` → `write_user_rating_shared`。RemoteActive の前に全ローカル reader を閉じる barrier (`:1116`) があるため、「Remote とローカル全画面が同時に動く」は通常の状態に含めない。値/一覧公開とログを共有し、Remote のページ位置・immutable listing をPC都合で動かさない。ローカル Undo を新設しない。返却時は既存 reload。 |
| XMP hydration / metadata import | `app.rs:71730` の session ledger publication、`app/metadata_import_refresh.rs` と `apply_metadata_import_terminal_refresh` の context 指定 snapshot 採用。同じ集合不変条件を確認する外部 producer。ユーザーの★操作とは区別し、新しい Undo を作らない。hydration の stale 明示clear保護は維持。 |

| 表示文脈 / consumer | 適用範囲 |
| --- | --- |
| 通常画像、本フォルダ、ZIP/CBZ・直接 RAR・入れ子 ZIP、PDF、EPUB | filter に従う Main scope に適用。`rating_path_key` / `page_path_key` を使い、ZIP の entry、PDF page 番号、EPUB の元論理パスを維持。変換書庫の既存 source/cache 対応を変えない。Shift+F は page key とは別の container key。 |
| Ctrl+F、Ctrl+S、Ctrl+G 合成/ドリル、tag、Rating、Smart Folder、bookmark/history 一覧、stack flat reader | その context の実効 membership と `current_reader_order` を使う。Rating の固定★、Smart の rating mask による離脱も対象。Smart は元 slot を残すので即時判定可能 (`smart_folder.rs:1291`)。検索/Rating の worker による後続 items 差し替えでも同じ owner に接続する。Stack 集約セルは評価非対象、flat の実ページは対象。 |
| Collection root | `ViewerNavigationScope::CollectionRoot` は installed collection order が正本で App-global filter を無視 (`app.rs:67957`)。星・DBを同期するが、評価を理由に reader/collection 順序を動かさない。物理 child は実際の child scope の規則。 |
| F12 / always-new / active・parked detached | presentation 名では分岐しない。`DetachedPhysical` は global filter を無視 (`app.rs:67966`) し、全物理ページを維持する。main の行が消えても独立窓は移動・閉鎖しない。Main scope を保持した reader なら同じ規則。`swap_viewer_context_bundle` の publication (`viewer_context_registry.rs:2213,2792`) は mounted owner の集合だけを調停し、別窓の idx・worker・cancel token を流用しない。 |
| ★固定 snapshot / DirectMedia | 既存の snapshot membership と DirectMedia reader order が正本。今回の理由で snapshot を解除・原一覧をreloadしない。実効 reader 集合が変わらなければ移動なし。既存 snapshot index-swap owner (`app/snapshot_ops.rs:410` 付近) を二重に走らせない。 |

既存の `effective_rating_filter()` の container-open suppression は保持する。今回の評価変更・Undo のために新しい suppression を発動したり、`settings.rating_filter` を全ONに書き換えたりしない。main の明示 filter 操作は既存どおり suppression を落とす (`ui_main.rs:11211,12659`)。

## 3. 単一 owner と単一決定点

**owner は既存 viewer context の表示集合公開処理**。App は mounted projection、`ViewerContextBundle` は退避した同じ状態の所有者である。保存先/Undo は stable key、表示ページ/選択/キャッシュ/世代は context-local のままにする。
`publish_current_context_rating_writes` を rating mutation の唯一の入口とし、**集合を採用し終える境界に一つの `reconcile_rating_membership` (仮称)** を置く。入力 helper・HUD 描画・seek helper・worker の request 時点では移動を決めない。

1. 同期 publication の冒頭で、context ID、一覧 scope、旧 grid/reader order、旧 selected と reader anchor の identity を在メモリで捕捉する。cache が更新済みでも旧表示順はまだ旧集合。保存対象と Undo の before/origin はsession値の同期後・保存前に捕捉する。操作全体のローカルな typed 値で渡し、App に pending bool/idx を追加しない。
2. SQLite 成功 → session ledger → owner の cache と membership を更新 → visible/Details/reader order と navigation cache を一度だけ採用する。成功した batch は一括で判定。`set_rating`、container、fullscreen helper、hydration の末尾で重ねて rebuild しない。rating sort snapshot の survivor 相対順は維持する ([評価順設計 §5](rating-snapshot-order-plan.md#5-評価変更後の処理))。
3. 採用した新集合と旧順から純粋な共通 helper で `Keep / Move(identity) / ReturnToGrid` を決める。旧順の anchor より後ろにある最初の残存候補、なければ前を逆順に探す。次とは**読書順の ordinal が増える方向**で、RTL でも raw index 比較や左右キー設定による反転をしない。新しい items なら identity を新 slot へ解決してから着地する。
4. grid の候補は grid order の選択可能行、静止画 reader の候補は still-page order (フォルダ・container・動画を自動的な「次の画像」にしない)。動画/音声 reader は既存の media navigation eligibility。アルゴリズムだけ共有し、候補 domain を混同しない。reader が動いた context では selected を着地点へ揃える。保存したのが別ページで現在 anchor が残るなら reader は Keep、selected は独立に補正する。
5. `Move` は既存 fullscreen landing/open、連結 seek、presentation/ロード契約を使う。strip・seek・ページ番号・見開き unit・分割slice・resume は新 anchor と集合を参照する。集合の失効による topology 変更も既存 unit owner へ通知する。draw 中に古い `fs_idx` を保持している呼出しは transition outcome を受け、旧 anchor の残りの描画/操作を続けない。
6. 静止画の候補がゼロなら既存 close の teardown で reader を閉じ、同じ filtered page grid を見せる。`handle_fullscreen_close_request` の auto-open → parent 退出規則をそのまま使わない。context のリソースを正しく閉じる処理は省略せず、空集合を穴埋めする fallback は作らない。

rating filter の明示変更 (toolbar/縮小facet/menu、設定の採用・復元) も「設定採用 → 集合採用」の同じ決定点を通す。★保存でないので新たな rating Undo/toast は作らない。後の非評価 filter によって元ページが対象外なら Undo はその条件を解除しない。
明示filter変更では、旧visible列だけでは新たに許可されたページの近傍位置を決められない。既存のinstalled items/Details sort ownerから、hidden行を含む同じ並びの候補列を**その採用処理の一時値**として解決し、旧anchorを基点に新membershipを探す。raw idxへfallbackしない。新たな行を末尾追加するmembership-only採用は、その既存のappend規則を使う。「旧visibleの全行が消えたが、新たに合う行がある」場合をゼロ枚と誤判定しない。常設の全画面専用リストは持たない。

**非同期の採用**: Ctrl+G `publish_search_rating_writes` (`global_search_ui.rs:2932`) と Rating membership refresh は後から items を再構成する。request 時の raw idx で予約せず、既存の context/items/write-generation 採用検査の成功時に、その時点の旧順/anchor と新集合を共通決定点へ渡す。同じページがすでに移動済みなら Keep。stale/cancel/error は採用も移動もない。
items を交換する経路は旧 idx を新 items に持ち越さず、現在ページの identity で既存の再入場・ロードを行う。専用の fs cache 全移植機構を新設しない。交換による再入場が必要でも表示集合・復帰先・Undo を同じ採用処理で確定し、次フレームの pending reopen に分割しない。検索の streaming/再評価完了で別画像へ飛ばないことを必須テストにする。

## 4. Undo と成功ログ

- `UndoEntry::Rating` に**一つの typed origin/復帰情報**を持たせる (仮称 `RatingOrigin::Grid / Viewer { context_id, listing_identity, page_key, presentation }`)。既存の stable `RatingChange` と同じ操作を所有し、別の「最後のratingページ」フィールドや pending reopen を併設しない。grid/Remote/container-only の変更から無条件に画像を開かない。
- 自動 membership 移動・ゼロ枚退出は通常の手動ナビとは違う内部 cause として既存 open/close に渡す。その cause のときだけ、操作が所有する rating Undo/Redo を保持する。過去ページの idx-only adjustment/layer Undo を丸ごと保存・復元しない。drag の確定、タグ保留の終了、全リソース teardown は通常の境界処理を実施する。手動の別場所 open/close、scope の変更・context 破棄は既存の clear 方針を維持。
- Ctrl+Z は transaction 成功後の同じ集合採用に `RestoreOrigin` を渡し、元ページがその context/scope で再び対象になったら近傍より優先して元画像へ戻る。最後の一枚で一覧へ戻った後も、同じ一覧の Undo から元 presentation に戻る。raw idx、selected、現在前面窓で保存先を引き直さない。
- 検索/Ratingのmembership再構成で元keyのslot自体が消えている場合、`RestoreOrigin` は**既存の再構成request owner内のtyped採用intent**として運ぶ。元keyが戻る採用で同じ決定点が一度着地し、requestのscope/context変更・取消はそのintentも終了する。DB待ち、App-globalな別pending reopen、古い結果からの再起動は足さない。保存/Undo成功と、worker待ちの表示復帰を同じ瞬間だと偽らない。
- 別窓で Undo した場合も DB は元 key に戻るが、無関係な前面窓を元ページにしない。生存する origin context にだけ復帰を配送する。別scope/消失した origin は書き戻しだけとし、旧窓を新設・旧一覧を開き直さない。現在の非評価条件が元画像を除外している場合も filter を維持し、復帰不可を通知する。Redo は after 値を公開し、現在 origin context の旧順で次/前/一覧を再決定する。
- リングは保存済み preview をその都度調停し、target key と origin context を固定したまま他ページに誤配送しない。他の picker 行も元 `RingPickerAnchor` を解決して操作し、移動後の現在idxにretargetしない。最後の一枚のcloseはpickerの終端: 既存 `published_live_picker_rating_changes` → `commit_live_picker_undo` の方法で成功済み分を一度だけ記録してから閉じる (新たな保存・プレビューの巻戻しなし)。通常commit/切断/モーダルの既存一件Undo契約も維持する。
- `[RATING]` 成功ログは setter/キー処理ではなく**SQLite 成功→ledger 公開の境界**に集約する。source、origin/projected context ID、items generation、target idx (未投影は none)、rating key、before/after、settings/effective filter、旧/新 reader 所属、`Keep/Move/Grid` と着地点を記録する。DB成功と context への後続反映は同じ write generation で相関する別種の行にし、mount を二度目の保存と記録しない。旧値は捕捉済み cache/session/Undo 記録を使い、ログのための同期I/Oを足さない。未投影Remote等で旧値が得られない場合は `before=unknown` と情報源を明示し、未評価0と偽らない。通常全画面の旧値・前後所属は必ず記録する。
- toast は共通transition後の表示面へ出す。ゼロ枚退出では MainWindow、残存ページへ移るなら origin viewer。既存の★文字列を保ち、移動処理で消さない。DB失敗は成功世代・成功toast・Undo・移動を公開しない。XMP後続失敗は既存の通知で、DBの評価を巻き戻さない。
- **まれな失敗は割り切り (ログ + 通知)**。再試行・完了待ち・多段復旧・DBの削除再作成を設計しない。通常の連続入力/Undo/worker採用順はこの単一ownerで扱う。

## 5. 採用しない案・検証と引き継ぎ

- 独立した全画面専用「フィルター外ページ列」、勝手な filter 解除/suppression、seek の None を全itemsへfallback、追加repaint/遅延: 一覧とreaderの二正本を残すため不採用。
- 保存/移動をpending boolや多段rollbackで組む案: 現在のDB書込みは同期で成功境界があり、membershipも在メモリ。公開と着地を一つにすれば通常入力の組合せを増やさずに済む。モーダル化は評価の連打・一覧編集を不便にし、既存機能を削るため採用しない。DB非同期化は本項に混ぜない。
- 設定変更で常に窓を閉じてreaderを作り直す案: 残存ページへ進む決定B、zoom/連結/窓identityを壊すため不採用。ゼロ枚だけ既存closeを使う。itemsが本当に交換される既存検索/Rating採用では既存再入場を再利用し、専用live cache移植を作らない。
- Undoスタック全体を `take` してopen/close後に戻す案: 他ページのidx-only編集、tag保留、別contextを復活させるため不採用。rating entry自身のoriginと内部transition causeに限定する。

実装時の必須テスト (Stage 1では追加・実行していない):

1. 純粋近傍helper: `[a,b,c]` の中/先頭/末尾、複数同時離脱、ゼロ、Details降順・非単調raw idx、RTL/LTR、filter変更で旧visible全離脱/新entrantのみ残存。次優先→前、残存anchorはKeep、gridとreaderの候補domainの差。
2. handler: F1–F6とcustom/step、selected≠fullscreen、全表示と `[true,false,false,true,true,true]`。DB key/値、cache、generation、可視所属、移動先、publication一回、成功ログ・★toast、bound/no-opを検査。既存 `rating_step_*` / metadata page-star テストを再利用。
3. Image/ZIP/PDF/EPUB・入れ子ZIP/変換書庫のpageとShift+F containerの分離。子ページへの誤書込み・XMP対象の拡大なし。最後のstill pageとcontainer-only残存では一覧へ戻る。
4. Ctrl+Z/Redo: 次/前への移動後、最後の一枚退出後、連続二操作、Details並び替え後のstable key、非評価filter変更、別窓からのUndo。元画像への復帰、filter不変、entry一件、旧idx編集Undoを復活させない。
5. Ring live preview→移動→別行→commit/切断/モーダル/ゼロ枚close。固定key/owner、一件Undo、保存の重複なし、非rating行を別画像に適用しない。情報パネルclickとnative HUD/キーも同じ境界を通る。
6. Ctrl+G/Rating/Smartの即時離脱＋後続worker採用、採用前Undo/再評価、streaming、open/switch/close/cancel/error/stale completion。現在identityの保持/次優先、cache/queueの世代整合、移動二重実行なし。評価sort snapshotのsurvivor順維持。Collection root・DirectMedia/★固定は順序不変。
7. F12/always-new active/parkedの同key同期と異なるitems番号、main gridのfilter編集。getterの先行cache同期で未公開membershipを取りこぼさない。DetachedPhysicalの全ページ、窓binding・zoom・worker tokenと未変更siblingの保持。read-only swap/open/closeで新しいmutation/全reloadを作らない。既存 `detached_physical_visible_set_ignores_main_local_filter` と `detached_descriptor_open_preserves_main_filters_and_shows_every_physical_page` を維持。
8. Remote acquire→rating→release: reader閉鎖契約、共有DB/ledger、返却reload、PC/Remote位置の分離。XMP stale hydration/metadata import snapshot採用でも集合外anchorを残さない。既存DB不在/Undo失敗テストを利用し、rare-failureの組合せテスト機構を新設しない。
9. 単ページ/見開き・表紙補助/横長分割/縦横連結: HUD page number・seek情報・stripが新集合に対応。実際のHUD/stripを使うheadless snapshotが可能な範囲で追加し、空一覧とtoastの表示面も確認。AI/補正/ロード待ちの既存presentation契約を維持。

検証ownerは実装担当。焦点 `cargo test -p mimageviewer --lib <filter>` → core check → `cargo fmt --check`、文字列変更ならglyph lint、snapshot確認、共有変更の完了時は `scripts/test-full.ps1`。実装完了後に `scripts/build-dev.ps1` を実行し、通常profileの確認用binaryを利用者へ渡す。Stage 1は文書のみのためbuild不要、mIV起動なし。

Stage 2 の候補所有ファイル: `app.rs` (save/publication/visible/open/close)、`ui_fullscreen.rs` (landing・入力後のanchor再解決)、`undo_stack.rs` / `undo_ops.rs` (originとrating限定保持)、`app/gamepad_input.rs`、`ui_metadata_panel.rs`、`app/native_video.rs`、検索/Rating/Smartの採用経路と関連テスト。新しいnative placement/focus機構やRemote protocol変更は不要。実装と同時に display pipeline / keymap / rating snapshot order の記述を更新する。

**実装前レビューの確認点**: 前優先の現gridとの差異を今回のrating由来経路だけで次優先へ揃えること、ringの非rating行を含む固定anchorの保持、後続items交換とorigin復帰の同一採用、rating限定Undo保持を独立レビューする。これらが既存の再入場・ownerで収まらず機能制限や大きなlive移植を要するなら、その境界を設計担当へ返し、症状パッチへ縮めない。
F12の述語/viewport/close経路に実装が届く場合は [detached憲法 §2](detached-rework-plan.md) に従い、設計担当と独立レビュアーの構造合意後、同plan §11へ対象・理由を記録する。今回のStage 1ではその合意や実装を済ませたとは扱わない。

## 利用者決定 (2026-10-11)

- ★による移動は一覧・フルスクリーンとも「次を優先、無ければ前」に揃える (本設計の案を採用。一覧の既存の前優先から変わる。履歴などの一般補正は変えない)。
- ★の自動移動と最後の 1 枚での一覧復帰では★の取り消しを残し、Ctrl+Z で元の画像へ戻す (本設計の typed origin 案を採用)。
