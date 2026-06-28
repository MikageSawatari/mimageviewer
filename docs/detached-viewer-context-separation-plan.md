# Detached viewer context separation plan

PDF / ZIP / 画像フォルダを別ウィンドウで開くとき、メインウィンドウを親の本一覧に残したまま、
別ウィンドウ側だけがページ列を持って閲覧できるようにするための設計方針。

この文書は `detached-viewer-implementation-plan.md` の複数画像ウィンドウ安定化方針を前提に、
「本を別々のウィンドウで開く」要望を満たすための本対応を切り出す。

## 1. 解決したい問題

現状の ZIP / PDF は仮想フォルダを開くと `App.items` をページ一覧 (`ZipImage` / `PdfPage`) へ
差し替える。`fullscreen_idx`、先読み、AI、編集、見開き、スライドショーなども同じ `items`
の index を前提にしている。

そのため、別ウィンドウで PDF / ZIP を開く場合でもメインウィンドウがページ一覧へ遷移してしまう。
これは「本一覧から複数の本を別々の別ウィンドウで開く」という基本要件と矛盾する。

軽い回避として「列挙後にメイン一覧へ戻す」案は採らない。別ウィンドウ側の `fullscreen_idx` が
メイン一覧の index を指すようになり、ページ送り、先読み、編集、AI キャッシュ、読書位置が
別コンテキストの index に化けるため、根本的に不安定になる。

## 2. 目標仕様

- 「画像を開くとき、毎回新しいウィンドウで開く」が ON のとき、メイン本一覧で PDF / ZIP /
  変換アーカイブ / 画像フォルダなどのコンテナを本として開いても、メインウィンドウはその一覧の
  ままにする。
- 開いた本は新しい detached active viewer として表示し、OS フォーカスをその別ウィンドウへ移す。
- さらに別の本をメイン一覧から開くと、既存 active viewer は同じ OS ウィンドウのまま paused
  window となり、最後の表示画像と viewer context を保持する。新しい active viewer がその本を開く。
- passive / paused window を再度アクティブ化した場合、現在 active viewer を paused 化し、
  AI / 先読み / 編集中 worker など active 専用処理を停止したうえで、その window が保持していた
  viewer context を active viewer として復帰する。PDF / ZIP を再列挙し直さない。
- active viewer だけがページ送り、見開き、スライドショー、先読み、AI アップスケール、編集を動かす。
  paused window は最後に見た画像を表示するだけで、処理対象ではない。ただし現在ページ、
  zoom / pan、表示中 texture、ページ列など復帰に必要な context は保持する。
- detached independent viewer とメインウィンドウの選択同期は行わない。メイン側の Backspace /
  フォルダ移動 / 検索 / ソート変更は、active detached viewer を閉じたり passive 化したりしない。
- 通常モードで未ピンの linked detached viewer だけは、既存仕様どおりメイン一覧と同期する。
  ピン留めされた viewer、または always-new 由来の viewer は independent context に切り替わる。

## 3. 中核方針

### 3.1 Main grid context と active viewer context を分離する

`App.items` を常に 1 つの正にする設計をやめ、少なくとも次の 2 つの文脈を分ける。

- `MainGridContext`
  - メインウィンドウに表示する一覧。
  - 親の本一覧、通常フォルダ、検索結果、タグビュー、読書履歴などを保持する。
  - detached independent viewer の操作では変更しない。
- `ActiveViewerContext`
  - 現在操作対象の別ウィンドウ viewer が読むページ列。
  - PDF / ZIP / 画像フォルダ / 通常フォルダ snapshot など、開いた時点の閲覧対象を保持する。
  - `fullscreen_idx` はこの context の `items` index を指す。

当面は active viewer を 1 つだけ持つ。複数ウィンドウは active viewer + passive display-only
snapshots の組み合わせで表現する。これにより、AI / 編集 / 先読みの対象は常に 1 context に限定できる。

### 3.2 既存 fullscreen 実装は段階的に context mount で再利用する

`ui_fullscreen.rs` と `App::start_fs_load` 周辺は `self.items[self.fullscreen_idx]` 前提の参照が多く、
一気に全関数へ `context` 引数を通すと変更規模が過大になる。

初期実装では、active detached viewer を描画・入力処理・先読み処理する短い区間だけ
`ActiveViewerContext` の item/caches を既存の `App` フィールドへ一時 mount し、
処理後に main context へ戻す方式を採る。

ただし mount 対象は明示的な `ViewerContextBundle` に集約し、構造体単位で swap する。
場当たり的に `items` だけ、または個別フィールドを手で swap しない。
Phase 1 では、まず main だけを `ViewerContextBundle` 化して挙動を変えず、以後の mount は
`mem::swap(&mut app.viewer_ctx, &mut active_ctx.bundle)` のような丸ごと swap に限定する。

`ViewerContextBundle` の抽出基準は機械的にする。原則として以下は context 所有にする。

- `items` と並列の `Vec`
- `HashMap<usize, _>`
- `HashSet<usize>`
- `*_generation`, `*_pages`, `*_cache`, `*_pending` のうち item index に依存するもの
- `fullscreen_idx` から派生する表示 / 編集 / 入力一時状態

少なくとも以下を同じ context bundle に含める。

- `items`, `thumbnails`, `image_metas`, `visible_indices`, `selected`, `scroll_to_selected`,
  `items_generation`
- thumbnail / grid-load 状態: `requested`, `pending_finalize`, `texture_backlog`, `keep_range`,
  `keep_set`, `details_hover_thumb_idx`, `thumb_pixels`, `thumb_adjust_tex`
- `current_folder`, `address`, `archive_source_override`, `zip_nav`
- `fullscreen_idx`, `viewer_presentation`, `last_viewer_sync_stamp`
- `fs_cache`, `fs_margin_bbox_cache`, `fs_pending`, `fs_upload_backlog`, `fs_early_dims`
- `input_generation`, `edit_result_cache`
- `adjustment_cache`, `ai_classify_cache`
- `erase_result_cache`, `erase_preview_cache`, `erase_base_tex_cache`, `erase_mask_generation`,
  `erase_inpaint_pending`, `erase_base_cache`
- `conceal_cache`, `conceal_base_cache`, `conceal_pages`, `conceal_mask_generation`
- `comic_cache`, `comic_pages`, `comic_bake_pending`
- `final_ai_cache`, `final_ai_pending`, `final_ai_failed`, `final_composite_cache`
- `adjustment_page_params`
- `local_adjust_page_layers`, `local_adjust_pages`, `local_adjust_selected_layers`,
  `local_adjust_generation`, `local_adjust_cache`, `local_adjust_pending`
- `export_crop_page_settings`, `export_crop_pages`
- `view_trim_page_overrides`, `view_trim_dirty_page_overrides`, `view_trim_page_apply_root_idx`
- `rotation_cache`, `rating_cache`, `checked`, `search_filter`, `tag_prewarm_queued`
- 動画 / native viewer の idx-keyed 一時状態: `normalize_ui_states`,
  `normalize_auto_scan_suppressed`, `last_loop_pos`
- `fs_vertical_cache_keep_set`
- `fs_zoom`, `fs_pan`, `fs_zoom_active`, `fs_zoom_aiming`, `fs_zoom_factor`,
  `fs_zoom_pdf_rerender_idx`, `fs_zoom_pdf_rerender_zoom`, `fs_pan_drag_start`,
  `fs_free_rotation`, `analysis_zoom`, `analysis_pan`, `analysis_pan_drag_start`
- `spread_mode` / view trim など、本単位で読む表示設定の現在値

`local_adjust_*` のようなワイルドカード表現は実装チェックリストでは使わない。上記のように実在
フィールド名へ展開し、grep で idx-keyed 状態を監査した結果をレビュー対象にする。

`MainGridContext` と `ActiveViewerContext` の index 空間が混ざると事故になるため、
idx-keyed の状態は context 外へ置かない。保持 LRU のように path key / metadata key で安全に共有できる
ものだけ context 外の共通キャッシュとして残す。

mount は panic safe にする。原則は Drop で必ず swap back する `MountedViewerContextGuard` を使う。
closure helper を使う場合でも、body が panic したときに context が mount されたまま残らないよう
`catch_unwind` などで unmount を保証する。plain closure で mount -> body -> unmount と書くだけの
実装は禁止する。

worker / pending 結果の routing は必須要件とする。`items_generation` を context 単位の
`context_generation` / `context_id` へ拡張し、サムネイル、fs load、PDF / ZIP enumerate、AI、
comic bake など idx を持つ非同期結果には発行時の context id / generation を焼き込む。
取り込み時に現在の適用先 context と一致しない結果は破棄する。これを「検討」扱いにしない。

### 3.3 detached 用の container open 経路を新設する

別ウィンドウでコンテナを本として開く経路では、既存の `load_pdf_as_folder` / `load_zip_as_folder` を直接呼ばない。
それらは main `items` をページ一覧へ差し替える main navigation API であり、本要件と相性が悪い。

代わりに `DetachedBookOpenPending` のような detached 専用 pending を作る。

- 入力:
  - open 元の main grid item (`PdfFile`, `ZipFile`, `ConvertibleArchive`, `Folder` などのコンテナ)
  - 明示 open / resume / focus の意図
  - 生成先 detached window id
- 処理:
  - PDF はページ列挙を detached pending として行う。
  - ZIP はエントリ列挙と `ZipNavState` 構築を detached pending として行う。
  - 変換アーカイブは既存変換 flow と接続するが、変換完了後の open 先は main `load_folder` ではなく
    detached pending に戻す。
  - 画像フォルダ / 通常フォルダ snapshot は、必要な item list を viewer context として構築する。
- 完了:
  - `ActiveViewerContext` を作成し、その context 内で `open_fullscreen(initial_idx)` 相当を実行する。
  - main `items` / `current_folder` / `selected` / `scroll_offset_y` は変更しない。

既存の `pending_auto_fs_open` と `fs_nav_after_pdf_enumerate` は main navigation 用として残す。
detached independent open では使わない。

ただし PDF / ZIP の列挙ロジック自体を fork copy しない。PDF worker pool、ZIP enumerate worker、
cancel / epoch / password / archive conversion などのコアは共有し、結果の sink だけを
main `start_loading_items` または active viewer context へ分岐する。

通常画像 (`Image` / 既に main context に存在する `ZipImage` / `PdfPage`) を開く経路は、
元々 main `items` を差し替えないため detached 専用 container open の対象外にする。
既存の passive snapshot + active viewer 退避モデルを使う。新規 detached pending は、
main がページ一覧へ遷移してしまうコンテナ open 問題に絞る。

### 3.4 passive window は stable viewport と paused context を持つ

passive / paused window は active 処理対象ではないが、OS ウィンドウを閉じず、active 時と同じ
stable `ViewportId` を維持する。active / passive で viewport 名前空間を分けない。
次の情報を持つ。

- 表示用 snapshot texture / 画像サイズ / タイトル
  - 連結スクロール中は中央 1 枚だけではなく、pause 時点の可視ページ群の texture と
    正規化済み表示矩形を保持する。passive window は worker を動かさず、この frozen
    page list を描くだけにして、非アクティブ化で画面内の前後ページが消えないようにする。
- `Pinned` / `Unpinned` など UI 状態
- 再アクティブ化用 `ViewerContextBundle`
  - 現在ページ、ページ列、`fullscreen_idx`
  - zoom / pan / 見開き / 表示モードなど表示状態
  - 360 度パノラマの ON/OFF 状態と案内トースト済み状態
  - 現在表示中の `fs_cache` / texture
  - context generation / window id
- fallback 用 `ViewerContextDescriptor`
  - container path
  - archive source override があれば元アーカイブ path
  - ZIP の場合は zip root と現在 prefix / entry key
  - PDF の場合は page number
  - 通常フォルダ / 画像の場合は source folder と page key
  - spread/view trim の本キー

paused window をアクティブ化したときは、保持していた `ViewerContextBundle` を active context へ
戻す。descriptor からの再列挙は、古い snapshot や表示 texture を持てない fallback のみに限定する。
以前の active context で動いていた先読み / AI / 編集 worker / slideshow timer は paused 化時に停止する。
pending は単に捨てず、cancel flag を立ててから context から外す。AI worker の結果チャネルは全
context 共有なので、main context が active detached context 宛ての final AI 結果を先に drain した
場合は backlog に退避し、active context を mount したタイミングで取り込む。
ただし表示中の 1 枚、連結スクロールの可視範囲、zoom / pan、現在ページ、ページ列は保持する。
これにより、再アクティブ化時の画像消失、ズーム初期化、OS ウィンドウ close/create による外枠
ちらつきを避ける。

## 4. 操作別の状態遷移

### 4.1 always-new モードで本一覧から PDF / ZIP を開く

1. main grid の選択状態は保持する。
2. 現 active detached viewer があれば passive snapshot 化する。
3. 新しい detached active viewport を loading 表示で作成し、フォーカスする。
4. detached pending で PDF / ZIP / 変換アーカイブを列挙する。
5. 列挙完了後、active viewer context に `PdfPage` / `ZipImage` の page list を入れる。
6. `book_open_resume` に従って初期ページを決め、active context 内で開く。
7. main grid は親の本一覧のまま残る。

### 4.2 main grid で Backspace を押す

detached independent active viewer が存在しても、main grid の Backspace は main context だけに作用する。
active viewer を閉じない。passive snapshot も作らない。

main grid が通常の PDF / ZIP ページ一覧を表示している場合は、従来どおり親へ戻る。ただしそれは
main navigation としてページ一覧を開いた場合だけであり、detached independent viewer の context
とは無関係にする。

### 4.3 active detached viewer で Backspace / Esc / Enter / 右クリック

active viewer 内のキーは active context へ作用する。

- ページ送り、見開き、スライドショーは active context の `visible_indices` を使う。
- Backspace は active context の階層を 1 段戻す。PDF / ZIP の page list から親本一覧へ戻す動作は
  detached viewer 内では「active viewer を閉じる」または「detached viewer 内で container root 表示へ
  戻す」のどちらかに仕様決定が必要。初期実装では close に寄せるのが安全。
- Esc / Enter / 右クリックは viewer session close。main grid は変えない。

### 4.4 passive window の再アクティブ化

1. 現 active viewer があれば passive snapshot 化する。
2. 現 active context の先読み / pending worker / slideshow など active 専用処理を停止する。
3. 現 active context の bundle と表示 snapshot を clicked window とは別の paused window として保持する。
   連結スクロール中は、pause 時点の可視ページ群を frozen page list として snapshot に含める。
4. clicked passive window が保持する bundle を active context に戻す。同じ stable viewport を使うため
   OS window は閉じない。
5. clicked window が古い descriptor-only snapshot の場合だけ detached pending を開始し、元ページへ
   できるだけ復帰する。

## 5. 実装段階

### Phase 0: 仕様固定と既存 workaround の扱い整理

- この文書をレビューして、main grid と detached viewer の同期境界を固定する。
- 「ページ一覧 BS で detached を閉じる」暫定修正は、本対応後は不要または main navigation 限定になる。
  実装時に残す場合も independent active viewer へ作用しないよう条件を変える。
- ネスト ZIP の BS で passive snapshot を作る既存経路も、independent detached viewer では main context
  変更扱いにしない。
- detached open routing は 1 箇所の決定関数へ集約する。always-new / pinned independent /
  linked / main navigation の分岐を各入力 handler へ散らさない。
- 本対応はインメモリ context 分離であり、既存 DB スキーマの変更は不要とする。再起動を跨いだ
  detached window 復元は今回スコープ外。将来 descriptor を永続化する場合も新規データとして扱う。

### 実装メモ (2026-06)

- `src/app.rs` に `ViewerContextBundle` と `ActiveDetachedViewerContext` を追加した。
  既存 fullscreen 実装の広い `self.items[self.fullscreen_idx]` 前提をすぐに全解体せず、
  active detached viewer を処理する短い区間だけ `App` 上へ bundle を mount する。
- mount は `with_active_detached_viewer_context` に集約し、`catch_unwind` 後に必ず
  `swap_viewer_context_bundle` で main context へ戻す。呼び出し側で個別フィールド swap はしない。
- always-new 設定かつ「開いたらフルスクリーン」対象の `PdfFile` / `ZipFile` は、
  `open_grid_container_in_detached_book_context` で main context を退避し、空の active context 上で
  既存 `load_pdf_as_folder` / `load_zip_as_folder` を実行してから main context を復元する。
  これにより、メインウィンドウは親の本一覧に残り、PDF / ZIP の enumerate pending とページ列は
  active detached context 側へ入る。
- active context の PDF / ZIP enumerate、fullscreen work、AI 先読み、local adjustment 起動、
  fullscreen viewport 描画は `update_active_detached_viewer_context` 内で mount して処理する。
- active context を mount して `load_pdf_as_folder` / `load_zip_as_folder` /
  `start_loading_items` を再利用している間は、メイン一覧用の永続履歴を更新しない。
  具体的には `settings.last_folder`、quick folder target / recent、folder nav back/forward は
  親の本一覧 context に属する状態として扱う。これを保存すると、always-new detached mode で
  再起動時に親の PDF/ZIP 一覧ではなく最後に見ていた PDF/ZIP のページ一覧へ復元されてしまう。
- worker 結果混入を避けるため、detached book context の `items_generation` は
  `DETACHED_VIEWER_CONTEXT_GENERATION_BASE` 以降の高ビット帯へ割り当てる。main context の
  `poll_thumbnails` が active context 由来の同 index / 同 generation 結果を受け入れないための
  実装上の context generation 分離である。
- active detached book context ごとに `fs_viewport_generation` を同じ context serial から割り当てる。
  これは worker / viewport lifecycle の世代情報として残す。OS window の同一性は別途
  `detached_viewer_window_id` で持ち、detached active viewport と paused passive viewport は同じ
  `detached_image_window_viewport_id(window_id)` を使う。
- active context を次の本へ切り替えるときは、現在表示中の `PdfPage` / `ZipImage` から
  display 用 snapshot を作り、同時に `ViewerContextBundle` を paused bundle として
  `DetachedImageWindowSnapshot` へ持たせる。表示中の 1 枚と zoom / pan は保持し、先読み /
  pending worker / slideshow timer は停止する。
- passive window は生成直後に即 reactivation しないよう、いったん focus が外れるまで
  `activation_armed=false` とする。armed 後も OS focus-in だけでは active 化せず、window 内の
  明示 pointer 操作で現在 active context を paused 化してから保持 bundle を active context へ
  戻す。descriptor 再列挙は paused bundle を持たない古い snapshot の fallback としてだけ使う。
- passive window の位置 / サイズ / 最大化指定は生成初回だけ `ViewportBuilder` に渡し、それ以降は
  OS が管理する live geometry を読み取って `placement` へ保存する。毎フレーム placement を再適用すると、
  window drag 中に古い座標へ引き戻す frame が生じるため行わない。
- detached window close 後に active detached context が残っていない場合は、main/root viewport へ
  focus を 1 回だけ誘導し、OS が残存 passive window へ focus を順番に渡す見た目のちらつきを抑える。
- detached book open の入口は、グリッドの Enter / ダブルクリック / ゲームパッド accept から
  `open_grid_container_in_detached_book_context` に集約した。読書履歴 / タグ / レーティング等の
  ビューでも `PdfFile` / `ZipFile` として表示される項目は同じ経路を通る。
- 現時点の detached book open 対象は `PdfFile` / `ZipFile`。画像フォルダは開いてみるまで
  「画像のみ」か確定できないため、この経路で先取りすると通常フォルダ移動を壊す。変換アーカイブも
  未変換時の dialog / conversion sink を active context に戻す追加設計が必要なため、既存の
  main navigation 経路を維持する。

### Phase 1: context bundle の抽出

- `App` の item / fullscreen / edit / cache のうち idx-keyed なものを `ViewerContextBundle` にまとめる。
- 最初は main context だけを bundle 化し、挙動を変えずにテストを通す。
- Phase 1 の受け入れ条件として、`HashMap<usize, _>` / `HashSet<usize>` / items 並列 `Vec` /
  idx-keyed cache / pending / generation を grep で全列挙し、bundle 所有か共有可能な path-keyed
  状態かを表にしてレビューする。分類は 2 つではなく、少なくとも次の 3 バケツに分ける。
  1. `App.items` の idx に依存する状態: `ViewerContextBundle` 所有。
  2. path / metadata key など context に依存しない共有状態: bundle 外で共有。
  3. ダイアログ内の行 index など `App.items` 由来ではない `usize` 状態: bundle にも共有にも入れず
     除外理由を監査表へ残す。
- mount API は panic-safe な Drop guard を基本にする。borrow 制約で guard 方式が成立しない場合は、
  unmount を `catch_unwind` 等で保証する helper に限定し、plain closure 実装は避ける。
- mount / unmount は `ViewerContextBundle` 構造体単位の swap だけで行う。個別フィールド swap は禁止。
- worker result routing のため、context id / context generation を bundle に持たせる。

### Phase 2: detached book open pending の追加

- main grid の `PdfFile` / `ZipFile` / `ConvertibleArchive` open で、always-new または independent
  detached が必要な場合は detached pending へ分岐する。
- PDF / ZIP の列挙結果を main `start_loading_items` へ流さず、active viewer context へ流す。
- loading 中 detached viewport を出して focus する。
- PDF password prompt / archive conversion dialog との接続を設計どおり実装する。
- PDF / ZIP / 変換アーカイブの列挙コアは共有し、結果 sink だけを main / detached active context で
  分ける。fork copy した列挙実装を作らない。
- detached pending の late result は target window id と context generation で破棄する。

### Phase 3: active viewer 操作の context mount 化

- detached independent viewer の `render_fullscreen_viewport`、入力、ページ送り、先読み、AI、編集を
  active context mount 中に実行する。
- main grid の update / draw は main context で実行し、active viewer の `fullscreen_idx` や
  `items_generation` に触らない。
- close / activation / passive 化の不変条件をテストで固定する。
- サムネイル、fs load、AI、PDF render、comic bake などの result poll は、適用先 context を明示して
  行う。描画時だけ active context を mount して poll は main context で受ける、という状態を作らない。
- active viewer の先読み、`promote_to_high_normal`、PDF pool priority / epoch の visible keys は
  active context のページ近傍を使う。main grid の可視範囲で active viewer の先読みを決めない。

### Phase 4: passive reactivation

- passive snapshot に stable window id と paused `ViewerContextBundle` を持たせる。
- passive window click / focus から、保持 bundle を active context へ戻す経路を実装する。
- 現 active context の pending worker cancel / slideshow 停止 / paused 化を 1 箇所に集約する。
- 表示 texture を持てない古い snapshot / loading 中 context だけ descriptor fallback を使う。
  通常の再アクティブ化で PDF / ZIP を再列挙しない。

### Phase 5: polish and compatibility

- 通常 linked detached viewer は既存の main sync を維持する。
- Ctrl+S / Ctrl+G / 読書履歴 / タグビュー / レーティングビューからの container open を同じ方針に通す。
- スライドショー、folder nav、見開き、view trim、読書位置、代表サムネピン、PDF password の edge case を
  拡張テストする。

## 6. テスト方針

最低限、以下の App-level test を追加する。

- always-new で `PdfFile` を開いても main `items` が親一覧のまま残る。
- detached PDF context は `PdfPage` list と `fullscreen_idx` を持ち、focus request が立つ。
- main grid の Backspace は detached independent active viewer を閉じない。
- 2 冊目を開くと、1 冊目 active は passive snapshot になり、2 冊目が active context になる。
- passive window を再アクティブ化すると、元 active context が paused 化され、clicked window が
  保持していた bundle が再列挙なしで active になる。zoom / pan と stable window id が維持される。
- 通常モードの未ピン linked detached viewer は main 選択同期を維持する。
- 通常モードで pin した viewer は independent context に切り替わり、再アクティブ化しても main sync しない。
- ZIP / PDF password / 変換アーカイブで detached pending が main `load_folder` に漏れない。
- worker result に古い context generation が付いている場合、同じ idx でも active / main の cache へ
  誤適用されない。

可能なら smoke / manual checklist に次を追加する。

- 本一覧に PDF / ZIP が複数ある状態で、順に開いて各別ウィンドウが残る。
- main 本一覧で BS / 検索 / ソート / スクロールをしても、別ウィンドウが消えない。
- passive window をクリックして、その本のページ送りができる。クリック時に画像が空表示へ落ちず、
  zoom / pan が維持される。
- active viewer だけで AI upscale / 先読み / 編集が動き、別 passive window は動かない。
- main 本一覧の visible range と active viewer の visible range が異なる状態で、先読み対象が
  active viewer 側になっている。

## 7. レビューしてほしい点

- context mount 方式で段階移行する方針が、`self.items[self.fullscreen_idx]` 前提の既存実装に対して
  過不足ないか。特に Drop guard による panic-safe mount で問題ないか。
- context bundle に含めるべき idx-keyed 状態の漏れがないか。
- detached 専用 pending を新設し、既存 `load_pdf_as_folder` / `load_zip_as_folder` を main navigation
  専用に残す境界は妥当か。
- passive window が page list / 表示 texture / zoom pan を保持しつつ、active 専用 worker だけ停止する
  方針で、メモリ使用量とちらつき抑制のバランスが妥当か。
- main grid の Backspace / フォルダ移動が independent detached viewer に作用しない仕様で、
  既存 linked detached viewer 仕様との矛盾がないか。
- Phase 1 の bundle 抽出を先に行うことで、機能変更前に安全な refactor として分割できるか。
- context id / generation を worker result routing の必須不変条件にする方針で、既存
  `items_generation` pattern から自然に移行できるか。

## 8. 実装時の注意

- `items` だけを swap しない。idx-keyed state を同時に移さないと、別ページの補正 / AI / mask が
  誤適用される。
- `cancel_token` / thumbnail worker / fs worker / PDF / ZIP enumerate / AI / comic bake の結果には
  context id / generation を持たせる。active viewer context と main context の idx 衝突で結果を
  誤適用しない不変条件を作る。
- PDF / ZIP enumerate の late result は context id と target window id で破棄判定する。
- retained AI / retained PDF page cache のような path-keyed cache は共有してよいが、live cache は
  active context の所有物にする。
- close-to-tray / app exit では active viewer context と passive windows を明示的に破棄する。
- UI thread で ZIP/PDF 列挙や catalog cold open を増やさない。既存 async 方針を維持する。
- 永続 DB の schema migration は行わない。context 分離に必要な状態はメモリ上に閉じる。
