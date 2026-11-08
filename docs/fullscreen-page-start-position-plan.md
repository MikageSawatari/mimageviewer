# ページ切替後の初期表示位置「中央 / 読み始め」(§1.726)

- 状態: **実装途中・幾何の設計前提と現行コードの差異を確認して停止** (2026-11-08、§9)。独立実装レビュー未実施。設計 r3 (設計担当 ClaudeCode) の独立設計レビュー (GPT-6.1 Sol / xhigh) r1 の P2 5 件、r2 の P2 2 件を反映 (§8)。
- 要望: 5ch >>739 (2026-11-07)。ズーム/フィット設定で画像が画面からはみ出すと、ページを送った先が
  中央から表示されるので、上から読みたい。拡大操作の中心点やマウスカーソルの移動を変える要望ではない。
- 利用者決定 (backlog §1.726): 設定を追加する。既定は従来どおり「中央」。

## 1. 仕様

設定「ページ切替後の表示位置」: `中央` (既定) / `読み始め`。

- `中央`: 現行と完全に同じ。新しい状態を作らない (§3 の要求を一切作らない)。
- `読み始め`: 表示単位 (単ページ / 見開き / 分割の片側) を切り替えた直後の位置を、はみ出す軸だけ読み始めへ寄せる。
  - 縦にはみ出す (表示単位の高さ > 表示領域の高さ): 上端を表示領域の上端に合わせる。
  - 横にはみ出す: 実効の読書方向が右→左なら右端、左→右なら左端を表示領域の端に合わせる。
  - はみ出さない軸は従来の配置 (pan 0。見開き端の単ページを片側へ置く既存配置もそのまま) を保つ。
- 倍率 (`fs_zoom`) は変えない。変えるのは切替直後の `fs_pan` の初期値だけ。

### 1.1 適用する切替・しない切替 (入口の棚卸し)

**適用する** = 現行で「ページを開くときに zoom=1 / pan=0 へ戻している」唯一の共通点
`App::open_fullscreen_with_metadata_boundary` (src/app.rs、`self.fs_zoom = 1.0; self.fs_pan = ZERO` の箇所) を通る切替。
現行の「切替で位置を既定へ戻す」点をそのまま使い、「既定の位置」を設定で選べるようにする
(既存機能と同じ経路・同じタイミングに揃える。CLAUDE.md「設計の簡素化」)。この共通点を通るのは:

- 通常のページ送り (キー・ホイール・クリック・ゲームパッド・タッチ。`FsPageNav` の Delta / Wrap / Boundary / Target)
- シークバー・ページ一覧からのジャンプ (`FsPageLoadContract::LatestSeek`)
- 前後の本・フォルダへの移動、履歴、スライドショーの自動送り
- 一覧からの初回 open (ページを「開いた直後」も切替直後と同じ扱いにする。一覧から開いて中央、送ると上端、という食い違いを作らない)
- 同じページの開き直し (見開き位置の正規化など)。現行でも位置が中央へ戻る経路なので、戻り先が設定値になるだけ

**加えて適用する**: 分割表示 (1 枚を半分ずつ読む) で、受理したページ送り (`FsPageNav::Split` 等) が同じ画像の片側→片側へ移る場合。
表示単位の切替だが open を通らず、現行は pan を戻さない。`読み始め` のときだけ、倍率は保ったまま初期位置を求める。
`中央` のときは現行どおり何もしない (pan を保つ)。
要求を作るのは**受理した移動の側**であり、slice の setter (`set_fullscreen_page_slice_for_display_unit`) ではない。
setter は寸法判明・分割モード変更・回転による整合 (`reconcile_fullscreen_page_slice`) からも呼ばれるので、そこで要求を作ると
手動パン後にも初期位置へ戻ってしまう。整合処理は要求を作らず、消えた要求を復活させない。

**適用しない** (現行どおり):

- 連結読み (縦・横)。ページ送りは scroll で、`fs_pan` は交差軸の位置しか持たない
- 動画・音声 (native presenter)。360 度表示、立体表示 (depth)、Z ズーム、分析モード
- ダブルクリックの「表示リセット」、fit 方式・倍率制限の変更 (`reset_fullscreen_fit_transform`)、
  読書方向・連結方向の変更 (`reset_continuous_reading_transform`)。切替ではなく表示方式の変更なので中央のまま。
  (後で要望があれば同じ要求を使って広げられる)
- 編集ツールの単ページ化 / 見開き復帰 (`enter/leave_page_edit_single_view`、`leave_local_page_edit_single_view`)。
  編集操作に伴う一時的な表示単位変更なので中央のまま
- detached viewer の再開で、保存済みの zoom/pan を復元する経路 (src/app.rs の resume 直後の復元)。復元を優先し要求を捨てる
- 共通設定変更時に駐車中の context の pan を 0 にする経路 (viewer_context_registry.rs)
- mIV Remote の Web 表示 (本体の `fs_pan` を使わない別の表示)

## 2. 幾何: どの時点の何に合わせるか

初期位置は、**その表示単位を初めて live に描く frame の、canonical な表示幾何**から求める。

- 表示領域 = `fullscreen_page_layout` の viewport (固定バー・シーク列・ロックした情報パネルを除いた `image_rect`)。
- 表示単位の矩形 = pan 0 で解いた各ページの `paint_rect` (表示トリム後) の和。見開きでは白い companion 枠を描く場合はその枠も含める
  (表示単位全体で「はみ出すか」を判定し、片側配置の単ページを勝手に端へ動かさない)。描かない Vacant 枠は含めない。
- pan の求め方 (pan は viewport 中央基準の screen point オフセット。正の y で画像が下へ動く):
  - 縦: `rect.height() > viewport.height()` なら `pan.y = viewport.top - rect.top`、そうでなければ要求作成時の pan.y のまま
  - 横: `rect.width() > viewport.width()` なら RTL は `pan.x = viewport.right - rect.right`、LTR は `pan.x = viewport.left - rect.left`、そうでなければそのまま
  - 物理ピクセル整列は既存の transform resolver に任せる (pan を独自に丸めない)
- 実効の読書方向: `spread_mode.reading_direction().unwrap_or(self.reading_direction)` (シークバーと同じ源)。
  単ページ表示でも本ごとの読書方向に従う。
- 幾何の入力は既存の `fs_material_fit_input` / `display_basis()` と同じもの (元画像寸法・PDF の page box)。
  通過 rendition (サムネイル) で描く frame でもページ矩形は元寸法から決まるので、その frame で確定してよい。
  高画質化・元画像の読み込み後に矩形は変わらないので、位置も動かない。
- **一度確定したら再計算しない** (§3)。確定後に自動トリム結果の到着・RAW 現像・窓サイズ変更などで幾何が変わった場合は、
  現行と同じく `fs_pan` (screen point) を保つ。初期位置へ戻し続けない。

## 3. 状態: 1 つの one-shot 要求

viewer context ごとの 1 フィールド (仮名 `fs_initial_placement_request: Option<FsInitialPlacementRequest>`)。
名前を `*_pending` にしない (`context_async_owner_registry_audits_bundle_pending_fields` の命名規則)。

```rust
struct FsInitialPlacementRequest {
    items_generation: u64,      // 要求を作った context の items 世代
    anchor_idx: usize,          // 開いた (切り替えた) ページ
    base_zoom: f32,             // 要求作成時の fs_zoom (open では 1.0)
    base_pan: egui::Vec2,       // 要求作成時の fs_pan (open では ZERO)
}
```

- **作る**: 設定が `読み始め` のときだけ、§1.1 の適用入口 (受理済みの open と、受理済みの分割片側移動) で作る (前の要求は上書き)。`中央` では作らない。
  要求の identity は `items_generation` + `anchor_idx` とし、slice は identity に含めない (未消費の間に整合処理が slice を直すことがあるため。
  分割片側移動の要求は、その移動後の slice を描く frame で解く)。
- **解く (唯一の consumer)**: 単ページ / 見開きの paint で、placement を解く**直前**に 1 回。条件:
  1. 要求の items 世代・anchor が今の frame の表示単位と一致する
  2. `fs_zoom == base_zoom` かつ `fs_pan == base_pan` (作成後に誰かが位置・倍率を動かしていない。補助条件で、主は下の明示消去)
  3. この frame で選んだ表示単位が current である (holdover を選んでいない。`selected_display_unit.is_current()` を両描画経路へ渡す。
     current を描いた上に holdover を重ねる経路があるので、`CurrentItem` かどうかでは判定しない。紙めくり条件を含む `live_eligible` は代用しない)
  4. **実際に選んだ素材と今の fit / view から、この表示単位の幾何を解決できた** (`MaterialDisplayBasis` の single / spread / specified の契約に従う)

  1・2 が不成立なら要求を捨てる。3・4 が不成立なら要求を残し、次の frame で再評価する
  (holdover 中、寸法が後着する PDF・RAW 等。条件がそろった最初の frame で一度だけ確定する)。
  除外モード (下の「消す」) は通常 paint へ入らない場合があるので、その判定は paint の外で行う。
  成立したら `base_zoom`・pan 0 で幾何を解き、§2 で pan を求めて `fs_pan` に入れ、要求を消し、同じ frame の placement をその pan で解く。
  **中央で 1 frame 描いてから跳ぶ見え方を作らない。**
- **消す**: 解いたとき、条件 1・2 不成立のとき、フルスクリーンを閉じたとき、items 世代の交代、
  および利用者の位置・倍率入力の入口 (`set_fs_pan_from_input`、ホイール拡大、キー拡大、ピンチ、ナビゲータ、ダブルクリック表示リセット、
  fit 方式変更のリセット、連結リセット、編集ツールの単ページ化 / 復帰、detached 再開の zoom/pan 復元、
  共通設定変更時に駐車中 context の pan を 0 にする reset (viewer_context_registry.rs の 3 か所。その bundle の要求を消す))。
- **paint の外の退役判定 (判定ロジックは 1 つの helper)**: `render_fullscreen_viewport_with_root_input` で、キー・pointer 処理の後、
  `paint_viewer_presentation_interlude` より前に呼ぶ (冒頭だけだと同じ frame の Z・比較開始を見落とす)。native media の早期 return
  (動画・音声) の前でも同じ helper を呼ぶ。呼び出しは複数でもよいが、判定は helper 1 つだけに書く。要求があり、
  設定が `読み始め` でない (設定を `中央` へ戻した)、`fs_zoom_owner() != Image` (Z・360・depth・分析)、paged でない (連結)、
  比較・オーバーレイ編集・表示トリム編集中、または current が page renderer の描かない項目 (動画・音声) のとき、要求を捨てる。
  設定変更時に全 context を走査しない (各 context が自分の frame で退役する)。
  条件 2 があるので、消し忘れた書き手があっても「人が動かした位置を初期位置で上書きする」ことは起きない。
  明示の消去は、後でたまたま値が base に戻った場合の取り違えを防ぐため。
- context 固有: `ViewerContextBundle` へ `fs_zoom` / `fs_pan` と同じく入れ、swap / 既定値 / destructure の全箇所を揃える。
  main / F12 detached / 独立 viewer のそれぞれで、自分の context の要求だけを作り・解き・消す。兄弟 context の pan を変えない。

### 3.1 簡素化の検討 (CLAUDE.md「設計の簡素化」)

- **open 時に pan を直接計算する案は採らない**: open の時点では表示領域 (HUD・パネル予約後) も寸法も確定していない。
  確定する唯一の場所は paint の幾何なので、要求を 1 つ持って paint で 1 回だけ解く。
- **「人が触るまで毎 frame 読み始めへ寄せ続ける」案は採らない**: 消費者が毎 frame になり、窓サイズ変更・トリム到着などで位置が動き続ける。
  backlog の「勝手に初期位置へ戻し続けない」に反する。one-shot にする。
- 新しい bool を増やさず、要求の有無 (`Option`) 1 つで表す。解く場所は 1 か所。
- まれな失敗 (幾何が永久に確定しない等) は割り切り: 要求が残るだけで位置は現行の中央。次の切替・入力で上書き・消去される。

## 4. 既存操作との関係

- 手動パン・ドラッグの制約 (`clamp_fullscreen_pan_from_layout`、48px 以上を見せる) は入力時だけ働く。
  初期位置は表示単位の端を表示領域の端に合わせるので、この制約の範囲内。
- キー拡大の中心点、マウス位置基準のホイール拡大、Z ズームの照準、ナビゲータ、ルーペは、確定後の `fs_pan` をそのまま使う。
  計算式は変えない。
- **紙めくり演出 (仕様上の制限)**: 既存規則 (`page_turn_view_eligible`) は zoom / pan が既定でないと演出しない。
  はみ出す表示単位でも全体テクスチャなら現行の `中央` (pan 0) では演出できる場合があるが、`読み始め` で pan が 0 でなくなると、
  そこからのページ送りでは演出しない。この制限は新しい選択肢 `読み始め` だけの性質で、既定の `中央` の挙動は変わらない。
  演出規則は変えない (pan 付きの projection 対応は範囲外)。マニュアルに 1 行書く。
  あわせて、演出開始時は pan 0 だったが着地先の表示単位が読み始めへ動く場合、**着地先を中央で描いてから跳ぶ frame を作らない**こと
  (演出が target の通常 layout を使うなら要求の解決で演出が取り消されて通常表示になる、等。実装でどちらになるかを確かめ、テストに残す)。
- **holdover は前の表示単位を描いたときの placement で描く**: open は pan を 0 へ戻すが、holdover は現行 `self.fs_zoom_pan()` (現在値) を使うため、
  `読み始め` で pan が 0 でなかった前ページが、待ちの間だけ中央へ跳ぶ。capture 時に保存済みの
  `FsDisplayUnitLayoutProjection::Painted(placement)` (`capture_fs_display_unit_with_rendition`) を、白 companion 経路と同じく
  **別の表示単位への移動中に残す previous** (navigation sequence の previous overlay) の単ページ・見開きにも使い、新ページの transform と分離する。
  - 適用は `Normal` / `SingletonSpread` placement に限る。`Painted(Z)` は placement だけでは再生できない (Z は fit を Page 基準にし倍率制限を外すが、
    holdover のページ情報は通常の実効 fit から capture される) ので、Z の holdover は現行経路のまま。
  - 同じ表示単位の再構築待ち (`FinalEffectSourceReload` の holdover) は現行どおり現在の transform で描く (待機中も手動パン・ズームが効く)。
  - どちらの holdover かは、`FsHoldover` の種類が分かる選択側から描画関数 (`draw_fs_display_unit_holdover_with_geometry`) へ明示的に渡す。
    ページ番号やログ文字列から推測しない。App の zoom/pan を一時的に書き換えて描く方式にしない。既存の `PaintedWhite` 経路は維持する。
  - 見開きでは captured placement を使う間、現在カーソルによる Z の再計算へ入れない。これは `中央` で手動パン後に送った場合の
  同じ跳び (現行の挙動) も直すが、「前の表示単位をそのまま描き残す」という holdover の意味どおりの修正であり、新しい挙動ではない。

## 5. 設定・UI・保存

- `Settings::fullscreen_page_start_position: FullscreenPageStartPosition` (`Center` 既定 / `ReadingStart`、`#[serde(other)] Unknown`→Center へ normalized)。
  **未リリースの新設定**なので migration・schema version は不要 (serde default で欠落時は中央)。
- 共通設定のみ。お気に入り別表示状態 (`FavoriteViewState`) には入れない。設定の転送 (export / import) は fit 方式と同じ分類で対象にする。
- 環境設定「見開き / 連結」の fit 方式 (`spread/fit`) の近くに ComboBox 1 行。検索 anchor を search_index に追加。
  説明は 1 行: 画面からはみ出すとき、ページを切り替えた直後にどこから表示するか。
- 新しいキー操作は足さない (設定のみ)。HUD の fit popup にも出さない。
- 文書: マニュアル (全画面表示の該当ページ)、display-pipeline.md §2.4 付近、spec.md、本書。UI スナップショットを更新。

## 6. 検証 (自動テスト)

幾何の純関数 (表示単位の矩形・viewport・方向 → pan) と、App の要求の生成・消費の状態遷移を分けてテストする。

- 純関数: 幅フィットで縦だけはみ出す / 縦フィットで横だけ (LTR・RTL) / 原寸・指定スケールで両軸 (LTR・RTL) / 全体が収まる (pan 不変) /
  見開き・端の単ページ (はみ出さない軸の片側配置が保たれる) / 回転 90 度・トリム / viewport 原点が 0 でない (HUD 予約)
- holdover: 「前ページを読み始めに置いた (pan≠0) → 未準備のページへ送る → 待ちの間、前ページの位置が動かない」。見開きも。
- 分割: 寸法判明・回転・分割モード変更による slice 整合では要求を作らない (手動パン後に整合が走っても pan 不変)。
- 退役: Z・360・depth・連結・比較・動画へ移った frame、設定を `中央` へ戻した frame で要求が消える。駐車中 context の reset で要求が消える。
- 紙めくり: 着地先が読み始めへ動くとき、中央の着地先を描く frame が無い。
- 状態遷移: `中央` では要求を作らない (現行 pan=0)。`読み始め` で open→最初の live frame で確定→次の frame で再計算しない。
  holdover frame では解かない。寸法未確定の間は保持し、確定 frame で一度だけ。手動パン・ホイール拡大・Z・表示リセット後は解かない。
  高画質差し替え後も位置不変。分割の片側移動で確定、`中央` では pan 保持。連結読み・動画・360・depth では作らない / 捨てる。
  detached 再開で復元した pan を上書きしない。兄弟 context の pan と要求を変えない。
- 設定: 既定値・serde 欠落・転送分類・設定数テストの更新。環境設定の UI スナップショット。

## 7. detached viewer 凍結ルールとの関係

新規 context (新しい detached / 独立 viewer の作成) は要求なしで始まる。既存 viewer の bundle の移譲・swap では、
`fs_zoom` / `fs_pan` と同じく要求も一緒に移す。

`ViewerContextBundle` へ一般の表示状態フィールドを 1 つ足すだけで、detached 述語・viewport 経路・placement・HWND には触れない。
detached 専用の bool / Option ではなく、既存の `fs_pan` と同じ context 固有の表示状態 (憲法 §2-3 の対象外と判断)。
独立設計レビューで「症状パッチではなく構造的な追加である」ことに合意済み (§8)。
変更範囲を detached-rework-plan.md §11 に記録した。実装は §9 の前提差異で停止中で、独立実装レビューでの確認は未完了。

## 8. 独立設計レビュー r1 の対応 (GPT-6.1 Sol / xhigh、2026-11-08)

P1 なし、P2 5 件。すべてコードで確かめて採用した。

1. holdover の旧ページが中央へ跳ぶ → §4。capture 済み placement で holdover を描く。
2. 分割 setter での要求作成は整合処理からも走る → §1.1・§3。受理した移動側でだけ作り、identity から slice を外す。
3. holdover 中の保持 / 破棄の矛盾 → §3。identity 不一致・CAS 不成立は破棄、holdover 選択中・幾何未解決は保持。`selected_display_unit.is_current()` で判定。
4. CAS では駐車中 reset・設定変更を検出できない → §3。駐車 reset で消す。paint の外に 1 か所の退役判定を置く。
5. 紙めくりは「元から対象外」と言えない → §4。`読み始め` 選択時だけの演出制限として仕様に書き、既定の挙動は変えない。
   中央→跳びの frame を作らないことを実装で確かめる。

r2 の再レビュー: P1 なし、P2 2 件。どちらもレビュー担当が示した範囲どおりに採用した (r3)。

6. captured placement を一律に使うと、同じ表示単位の再構築待ち (`FinalEffectSourceReload`) で手動操作が止まる → §4。navigation の previous だけに限定し、種類は選択側から明示的に渡す。
7. `Painted(Z)` は placement だけでは再生できない → §4。`Normal` / `SingletonSpread` に限定し、Z は現行経路。

あわせて、要求構造体から未使用の `page_slice` を外し、退役 helper の呼び出し点を明記した。
レビュー担当は、`ViewerContextBundle` への追加と holdover の描画入力の所有分離が症状パッチではなく構造的な変更であることに同意した (§7)。

## 9. 実装途中の記録と設計前提の差異 (2026-11-08)

設定・転送・環境設定 UI、context 所有の要求、受理済み分割移動、要求の消去入口、単ページ・見開きの初期配置、
navigation previous の placement 再生までの未コミット差分を作成した。完成・受入済みとは扱わない。
実装位置は `src/ui_fullscreen/initial_placement.rs` (`FsInitialPlacementRequest`、`reading_start_pan`、
`request_fs_initial_placement` / `clear_fs_initial_placement_request` / `retire_fs_initial_placement_request` /
`resolve_fs_initial_placement`)、`src/ui_fullscreen.rs` (`accept_fullscreen_page_slice_navigation`、
`draw_fs_image_with_initial_placement`、`draw_fs_spread_with_placement`、
`draw_fs_display_unit_holdover_for_selection`、`FsHoldoverTransform`)。

**§2 の「最初の current frame の矩形は元寸法で決まり、高画質化後も変わらない」という前提が、通常サムネイルの単ページでは成立しない。**
`ThumbnailState::raster_thumbnail_display_basis` (`src/grid_item.rs`) は元寸法が既知でも Single にサムネイル自身の sampler 基準を渡す。
通常完成素材の `normal_raster_material_basis` (`src/app.rs`) も Single に完成 sampler 基準を渡す。
`prepare_fullscreen_state_with_permission` は通常の非通過・非カラー化待ちで、完成素材が無いとき直接サムネイルを選択する。
たとえば原寸表示で 80×120 のサムネイルから 800×1200 の完成画像へ替わると、最初の矩形が画面に収まっていても完成画像ははみ出す。
今回の one-shot consumer は最初のサムネイルで pan 0 として消費するため、その後の完成画像は中央のままとなる。
これはコードからの確認であり、製品起動・自動実行による再現は未確認。既存の `display_basis_tests` も通常画像の原寸は選択 texture pixel 基準という契約を明記している。
論点を限定した別担当 (GPT-6.1 Sol / xhigh) のソース確認でも、通常の初回 open で到達可能と確認した。
非 RAW の thumbnail fallback は許可され、初回 open は full load の完了を待たず、previous の無い frame は `Current` になる。
1 pixel/point・viewport 600×400・thumbnail 150×200・full 1500×2000 なら、先の収まる矩形で要求が消え、後の画像上端は中央配置の -800 になる。
この確認は設計前提だけを対象にしており、独立実装レビューの完了ではない。現時点の実描画テスト案は完成素材だけを渡しており、この thumbnail 到着順をまだ検査していない。

既存の表示基準を変える、要求の消費を遅らせる、または適用仕様を見直す判断が必要になるため、実装担当は独自の補償設計を加えず停止した。
設計担当がこの境界を決定してから実装と §6 の検証を再開する。
追加した回帰テスト案は `src/ui_fullscreen/initial_placement/tests.rs`。テスト実行・snapshot 更新は未完了。
`cargo check -p mimageviewer --bin mimageviewer-core` は本体 Rust 検査前に依存物の準備で失敗し、
絞り込み nextest は依存物の初回コンパイル中に停止した。ビルド失敗を製品コードのテスト失敗とは扱わない。

検証実績:

- `cargo fmt` / `cargo fmt --check` / `git diff --check`: 成功。
- `python scripts/check_ui_glyphs.py`: 危険グリフ 0。
- `python scripts/check_site_cards.py`: 53 件、超過 0。
- `cargo check -p mimageviewer --bin mimageviewer-core`: VLM worker 不足・TurboJPEG native build の出力先アクセス拒否で、本体 Rust の型検査前に失敗。
- `powershell -NoProfile -File scripts/vlm-worker-identity.ps1 -RestoreBackupRepo .`: 既存バックアップを実ファイルで復元し identity 一致を確認。worker は起動していない。
- `cargo nextest run -p mimageviewer --lib --build-jobs 2 --test-threads 2 -E 'test(~fullscreen_page_start_position) | test(~page_start_position) | test(~settings_transfer::tests::) | test(~preferences::search_index::tests::)'`: TurboJPEG native build で失敗。
- 上記を `--build-jobs 1` にして再実行: TurboJPEG を通過、依存物の初回コンパイル中に設計前提の差異が確定したため停止。選択テストの実行前。ログは `target/1726-settings-focused.log`。
- snapshot は既存 fixture に設定行を足したところまでで、PNG 更新・比較・目視確認は未実施。
- full gate・build-dev・製品起動・コミットは利用者指定に従い未実施。

