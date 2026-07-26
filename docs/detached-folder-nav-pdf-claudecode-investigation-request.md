# ClaudeCode 調査依頼: detached PDF の Ctrl+↓ でウィンドウが閉じる

作成日: 2026-07-26  
対象: mImageViewer v2.8.0 / `master`  
作成時 HEAD: `bd0fadb0`

## 0. 依頼の目的

複数ウィンドウモードの independent detached 静止画 viewer で、PDF を表示中に
Ctrl+↓を押すと、同じ物理フォルダに次の PDF が存在するにもかかわらず detached
window が閉じる回帰があります。

まずコード、既存ログ、必要なら追加計装を使い、入力から folder navigation、
PDF enumerate、同一 window での reopen までの状態遷移をフレーム単位で調査してください。
見えている症状だけを抑える修正は行わず、どの所有権境界またはライフサイクル遷移が
「内部 reopen」を「ユーザーによる terminal close」と誤認しているかを確定してください。

この依頼の第1段階は **調査と修正設計まで** です。原因が確定する前にコードを変更・コミット
しないでください。既存ログで不足する場合は、必要な追加ログの場所と項目、または最小の
計装パッチを提示してください。

## 1. 実機で確認された現象

### 再現手順

1. 複数ウィンドウモードを有効にする。
2. 同じ物理フォルダに、現在の並び順で前後関係になる PDF を2つ以上置く。
3. 先頭側の PDF を detached window で開く。
4. その detached window にフォーカスを置いて Ctrl+↓を押す。

### 実際の動作

- 操作中の detached window が閉じる。
- 次の PDF は同じ window に表示されない。

### 期待する動作

- Ctrl+↓は `effective_folder()` を起点とする物理 filesystem DFS 順で次の本へ進む。
- 次の PDF を同じ detached window、同じ stable `window_id` / ViewportId、同じ位置・サイズで開く。
- PDF enumerate 中も window/session/runtime を生存させ、可能なら直前ページの holdover を表示する。
- main window の一覧、検索・絞り込み、選択、スクロール、履歴には一切影響しない。
- 物理順の末尾で次が存在しない場合も、現在の window を閉じず境界ヒントを表示する。

Ctrl+↓だけでなく、同じ親直下を移動する Ctrl+PageDown でも同じ問題が起きるかを切り分けて
ください。ただし最優先の再現条件は Ctrl+↓による PDF → PDF です。

## 2. 直近の改修と現在の仕様

この機能は次のコミット列で追加・修正されました。

- `c754a133` Enable physical folder navigation in detached viewers
- `77b92d26` Isolate detached image folder navigation context
- `ac10b5cb` Constrain detached page navigation to physical scope
- `7a2ed600` Build detached image lists from physical sources
- `171a8737` Fix detached image scan lifecycle
- `4c2a9e7c` Fix detached container open lifecycle
- `1b71bc66` Fix mixed folder open ownership (detached-rework folder-nav)

中心となる仕様書は `docs/detached-rework-stage-folder-nav.md` です。実装上の不変条件は
次のとおりです。

- independent detached still viewer は `ViewerNavigationScope::DetachedPhysical` を所有する。
- folder-nav request、pending、result、PDF/ZIP enumerate、deferred reopen は
  `ViewerContextBundle` の所有物である。
- active bundle を `App` に mount している間だけ poll / apply / reopen する。
- main の検索結果やフィルタを detached の物理一覧・移動順へ混入させない。
- internal folder-nav reopen と terminal close を区別する。
- internal reopen では session/runtime/window identity を終了・削除しない。

関連設計も先に確認してください。

- `AGENTS.md`
- `docs/detached-rework-plan.md`
- `docs/detached-viewer-lifecycle-redesign-proposal.md`
- `docs/detached-viewer-keepalive-design.md`
- `docs/detached-viewer-context-separation-plan.md`
- `docs/fullscreen-navigation-consistency.md`
- `docs/async-architecture.md`

特に `docs/detached-viewer-lifecycle-redesign-proposal.md` の BA-4、BA-5、BA-7 と、
過去の「PDF/ZIP + Ctrl+↓」の因果連鎖を確認してください。ただし、旧問題は
window 再生成や小窓フラッシュであり、今回の「terminal close」と同一原因だと決めつけないで
ください。

## 3. 現在の主要コード経路

行番号は今後ずれる可能性があるため、シンボル名を主に追ってください。

### 入力と folder-nav

- `src/ui_fullscreen.rs`
  - `handle_fullscreen_ctrl_nav_context`
  - `handle_fullscreen_sibling_nav_context`
  - `capture_fs_nav_holdover`
  - `fs_nav_deferred_reopen_wait_active`
  - `keep_fullscreen_viewport_alive`
- `src/app.rs`
  - `start_folder_nav`
  - `poll_folder_nav`
  - `apply_folder_nav_result`
  - `load_folder_nav_target`
  - `reopen_fullscreen_after_folder_nav_load`
  - `close_fullscreen_for_folder_nav_reopen`

### active detached context と PDF

- `src/app.rs`
  - `update_active_detached_viewer_context`
  - `poll_pdf_enumerate`
  - `open_deferred_fullscreen_after_enumerate`
  - `finalize_closed_active_detached_viewport`
  - `detached_active_window_alive_wanted`
  - `begin_active_detached_session_close`
  - `finish_active_detached_session_close`
  - `remove_detached_window_runtime`
- `src/ui_fullscreen.rs`
  - `keep_fullscreen_viewport_alive`
  - `render_active_detached_viewport_backstop`

### 既存テスト

- `src/app/tests.rs`
  - `active_detached_update_polls_only_its_bundle_folder_nav_result`
  - `mounted_detached_physical_context_owns_sibling_nav_without_touching_main_state`
  - `detached_book_pdf_open_keeps_main_grid_on_parent_list`
  - `detached_folder_nav_close_preserves_viewport_host_for_reopen`
  - `detached_folder_nav_reopen_reuses_window_even_if_grid_intent_returns`
  - `folder_nav_close_preserves_detached_window_id_for_reuse`
  - `folder_nav_reopen_reuses_active_detached_window_id`
  - PDF password、required target、terminal-before-viewport 関連テスト

## 4. 最優先で検証してほしい原因候補

Codex 側の静的確認では、次の経路が最も疑わしく見えます。ただし、これは結論ではなく、
ログまたは再現テストで確認すべき仮説です。

1. `update_active_detached_viewer_context()` はフレーム冒頭で、現在 window が shown なら
   `close_viewport_id` を保存する。
2. 同じ update 内で `poll_folder_nav()` の PDF target 結果を
   `apply_folder_nav_result()` に渡す。
3. `apply_folder_nav_result()` は internal reopen として
   `close_fullscreen_for_folder_nav_reopen()` を呼ぶ。
4. 次の PDF は非同期 enumerate なので、`fullscreen_idx=None` のまま
   `pdf_enumerate_pending` と `fs_nav_after_pdf_enumerate` が残る。
5. `keep_fullscreen_viewport_alive()` は、この deferred gap で同じ viewport を holdover 描画する。
6. その直後、`update_active_detached_viewer_context()` の
   `detached_viewport_finalized` 判定は、PDF pending/deferred reopenや内部遷移を条件に含めず、
   `fullscreen_idx.is_none()` とフレーム冒頭の `close_viewport_id` だけで
   `finalize_closed_active_detached_viewport()` を呼び得る。
7. この finalize は `ViewportCommand::Close` を送り、`fs_viewport_shown=false` にし、
   active detached session を finish し、window runtime も削除する。

`finalize_closed_active_detached_viewport()` 内のコメントは「should_drop 経路でのみ呼ばれる」
と説明していますが、現在の呼び出し順では finalize 判定が `should_drop` 計算より前に独立して
います。このコメントと実際の制御フローの矛盾を確認してください。

確認したい中心命題は次です。

> `fullscreen_idx=None` は「viewer を閉じる意思」ではなく、PDF/ZIPのinternal reopen中にも現れる。
> それにもかかわらず、active detached の終端処理が field presence だけで terminal close を
> 推論していないか。

この仮説が正しい場合も、単に `pdf_enumerate_pending` を finalize 条件へ追加するだけの局所ガードが
正しいとは限りません。ZIP、通常画像scan、archive conversion、protected PDF、required target
failure、明示的なEsc/×、pause/activateを含む lifecycle 全体で、internal transition と terminal
close の所有者を1つの typed state / requestとして表現できているか評価してください。

## 5. 必須の調査手順

### 5.1 差分と責任境界

1. `c754a133^` から現在までの該当コード差分を追う。
2. detached bundle で folder-nav を poll/apply するようになった時点と、
   active context finalize 経路との接続点を特定する。
3. v2.7.0相当で同じ active context update/finalize がどの入力から呼ばれていたかを比較する。
4. 回帰を「新しいfolder-navが既存の壊れた終端前提へ到達可能にした」のか、
   「後続修正が直接終端条件を壊した」のかに分ける。

### 5.2 1回の Ctrl+↓を状態遷移として追跡

少なくとも次の値を、入力時、folder-nav worker開始/完了、apply前後、close/reopen前後、
PDF enumerate開始/完了、update末尾、viewport close command送信時に追ってください。

- `frame_counter`, `input_seq`
- detached context serialまたはownerを一意に識別できる値
- `window_id`, ViewportId
- `ViewerNavigationScope`
- `current_folder`, `effective_folder`, folder-nav result path
- `FolderNavMode`, forward、`hit_image_folder`、scan result
- `fullscreen_idx`
- `fs_viewport_shown`, `fs_viewport_presentation`
- `folder_nav_pending`
- `pdf_enumerate_pending`, `zip_enumerate_pending`
- `fs_nav_after_pdf_enumerate`
- `fs_nav_locked_gen`
- `detached_viewer_folder_nav_reuse_window_once`
- active detached sessionと`DetachedWindowState`
- `should_drop`を構成する各条件
- `finalize_closed_active_detached_viewport`、session finish、runtime remove、
  `ViewportCommand::Close`の実行理由

既存のdetached debug loggingは環境変数 `MIV_DETACHED_WINDOW_DEBUG` で有効になります。
既存ログで相関情報が足りない場合は、`window_id + context owner + input_seq/request id` を全イベントへ
通す追加計装を提案してください。

### 5.3 最小マトリクス

最初に次を比較してください。

| 起点 | 操作 | 移動先 | 確認点 |
| --- | --- | --- | --- |
| PDF | Ctrl+↓ | PDF | 今回の必須再現 |
| PDF | Ctrl+PageDown | PDF | sibling経路との差 |
| ZIP | Ctrl+↓ | PDF | PDF enumerate着地共通性 |
| PDF | Ctrl+↓ | ZIP | ZIP enumerateとの対称性 |
| 通常画像フォルダ | Ctrl+↓ | PDF | PDF着地側だけの問題か |
| PDF | Ctrl+↓ | 通常画像フォルダ | PDF退出側の問題か |
| 末尾PDF | Ctrl+↓ | なし | windowを閉じないこと |

まず無暗号・正常PDF、既定の並び順、ローカルディスクで再現してください。protected PDF、
壊れた/空コンテナ、network folderは根因確認後のsibling lifecycle監査対象です。

通常プロファイルの開発版・release版をClaudeCode側から起動しないでください。GUI再現が必要なら
`AGENTS.md`に従い、`scripts/prepare-portable-smoke.ps1`で作った
`target/portable-smoke`だけを使用してください。

## 6. 既存テストで見逃した理由の確認

既存テストは次を個別には固定しています。

- detached bundleだけがfolder-nav resultをpollする。
- mainの一覧・フィルタを変更しない。
- internal closeでwindow identity/reuse flagを保持する。
- PDF enumerate/deferred reopenをbundleが所有する。
- viewport生成前の失敗ではsession/runtimeを正常終了する。

しかし、次のproduction sequenceを同じテストで複数フレーム通していない可能性があります。

```text
active detached PDF表示中
  → Ctrl+↓ handler
  → folder-nav resultをactive bundleのupdateでapply
  → close_fullscreen_for_folder_nav_reopen
  → fullscreen_idx=None + PDF enumerate pending
  → keep-alive/holdover render
  → active update末尾のfinalize/should_drop判定
  → 1フレーム以上pending
  → PDF enumerate完了
  → 同一windowでreopen
```

`active_detached_update_polls_only_its_bundle_folder_nav_result` は境界 `path=None` を使っており、
PDF着地後のclose→async gap→reopenを通しません。window identityのテストもhelperを直接呼ぶ
単体的なものが中心で、`update_active_detached_viewer_context()` の同一フレーム終端処理との
合成を検出できない可能性があります。

原因確定後は、production updateを跨ぐ回帰テスト案を示してください。最低限、
次を固定する必要があります。

1. active detachedで最初のPDFが表示済み、session/runtime/windowが生存している。
2. folder-nav resultとして2つ目のPDFを返す。
3. PDF enumerateを意図的に1フレーム以上pendingにする。
4. gap中もactive context/session/runtimeと同じwindow IDが生存し、
   `ViewportCommand::Close`相当のterminal finalizeへ入らない。
5. enumerate完了後、2つ目のPDFが同じwindow IDで開く。
6. mainのfolder/items/visible_indices/filter/selection/scroll/historyが不変である。
7. Ctrl+↓の境界では現在PDFとwindowを維持する。

egui commandの直接観測が難しい場合も、finalize呼び出し、session state、runtime presence、
`fs_viewport_shown`を検査できるstate transition testにしてください。

## 7. 修正設計を評価するときの制約

`AGENTS.md`とdetached reworkの憲法に従ってください。

- 症状だけを隠すrepaint、delay、retry、時間窓、geometry heuristicを追加しない。
- App-globalなdetached用bool / `Option`をさらに追加して状態を表さない。
- `fullscreen_idx`、pending field、shown flagの有無だけをterminal intentの代用にしない。
- main context、active A、passive Bのrequest/result/session/runtimeを交差させない。
- Esc/×による明示close、internal reopen、失敗によるterminal closeを同じ曖昧な分岐にしない。
- ZIP/PDF/通常画像scan/変換待ちのsibling経路を監査する。
- main一覧の保持、物理スコープ、同一window再利用というユーザー仕様を後退させない。
- 正しい修正が大きい場合は、局所patchを先に入れず、必要なtyped stateまたはreducer境界を説明する。

BA分類を使う場合、今回の主候補はBA-5（immediate viewportの描画漏れ・gap）と
BA-7（複数fieldから暗黙に終端を推論）です。window identityを作り直す場合に限りBA-4も関係します。

## 8. 期待する調査結果

次の順で報告してください。

1. **再現結果**  
   PDF→PDFのCtrl+↓で確認した事実、Ctrl+PageDownやZIPとの比較。
2. **確定したイベント時系列**  
   inputからwindow closeまでをframe / owner / window ID付きで示す。
3. **根本原因**  
   最初に破られた不変条件、該当ファイル・シンボル・行、導入コミット。
4. **最優先仮説の判定**  
   active update末尾のpremature finalizeが原因か、別原因か。
5. **既存テストが見逃した理由**  
   helper単体、frame gap、owner mount、OS command観測など具体的に説明。
6. **sibling経路監査**  
   ZIP、通常画像、conversion、password、error/empty、Esc/×への波及。
7. **推奨する修正設計**  
   最小だが根本的な所有権/状態遷移修正。必要なら段階案。
8. **追加すべき回帰テスト**  
   テスト名、初期状態、複数frameの刺激、assertする不変条件。
9. **追加ログが必要な場合**  
   正確な挿入箇所、correlation key、期待される正常/異常ログ。

判断を保留する点は明確に分け、コードから確認できた事実、ログで確認した事実、推測を混同しないで
ください。

