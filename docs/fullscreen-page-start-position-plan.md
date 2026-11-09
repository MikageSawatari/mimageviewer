# ページ切替後の初期表示位置「中央 / 読み始め」(§1.726)

- 状態: **r4 再レビュー承認済み。ページ構成切替の追補は実装済み・レビュー／実機再確認待ち** (2026-11-09、§9.7・§9.8)。設計担当の §10 の決定を実装し、core check・絞り込み nextest・UI snapshot を通過した。独立設計レビュー (GPT-6.1 Sol / xhigh) r1 の P2 5 件、r2 の P2 2 件を反映した r3 の記録は §8。
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

**第 3 の適用入口 (依頼 27 追補)**: paged のページ構成を実際に変更したときも適用する。
単ページ・見開き・表紙付き見開き・横長分割の間の切替は、新しい表示単位の pan を 0 に戻して要求を作る。
切替前の倍率は保つ。手動で拡大・移動した後も、新しい構成では読み始めへ合わせる。
正規化後に同じ構成となる場合や拒否された切替では何もしない。`中央` は従来の適用経路を変えない。
読書方向だけの切替 (`7`) と、編集ツールの一時的な単ページ化・復帰は従来どおり要求を消す側に保つ。

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

要求を作る入口は、open の zoom/pan リセット、受理した分割片側移動、paged のページ構成切替の 3 つ。
構成切替は倍率を保持し pan 0 を配置基準にする。消費・仮素材から完成素材への確定は共通で、別の配置状態は追加しない。

**r4 では確定時点を §10 が上書きする。** 仮素材にもその frame の幾何を適用し、実際の完成素材が表示単位全体で揃った最初の current frame に確定する。

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
    origin_zoom: f32,           // 要求作成時の fs_zoom (open では 1.0)
    origin_pan: egui::Vec2,     // 要求作成時の fs_pan (open では ZERO)
    applied_zoom: f32,          // 最後に自動適用した倍率 (初期値は origin_zoom)
    applied_pan: egui::Vec2,    // 最後に自動適用した位置 (初期値は origin_pan)
}
```

- **作る**: 設定が `読み始め` のときだけ、§1.1 の適用入口 (受理済みの open、分割片側移動、paged のページ構成切替) で作る (前の要求は上書き)。`中央` では作らない。
  要求の identity は `items_generation` + `anchor_idx` とし、slice は identity に含めない (未消費の間に整合処理が slice を直すことがあるため。
  分割片側移動の要求は、その移動後の slice を描く frame で解く)。
- **解く (唯一の consumer)**: 単ページ / 見開きの paint で、placement を解く**直前**に 1 回。条件:
  1. 要求の items 世代・anchor が今の frame の表示単位と一致する
  2. `fs_zoom == applied_zoom` かつ `fs_pan == applied_pan` (最後の自動適用後に誰かが位置・倍率を動かしていない。補助条件で、主は下の明示消去)
  3. この frame で選んだ表示単位が current である (holdover を選んでいない。`selected_display_unit.is_current()` を両描画経路へ渡す。
     current を描いた上に holdover を重ねる経路があるので、`CurrentItem` かどうかでは判定しない。紙めくり条件を含む `live_eligible` は代用しない)
  4. **実際に選んだ素材と今の fit / view から、この表示単位の幾何を解決できた** (`MaterialDisplayBasis` の single / spread / specified の契約に従う)

  1・2 が不成立なら要求を捨てる。3・4 が不成立なら要求を残し、次の frame で再評価する
  (holdover 中、寸法が後着する PDF・RAW 等。条件がそろった最初の frame で一度だけ確定する)。
  除外モード (下の「消す」) は通常 paint へ入らない場合があるので、その判定は paint の外で行う。
  成立したら `origin_zoom`・pan 0 で幾何を解き、収まる軸は `origin_pan` を保って §2 で pan を求めて `fs_pan` に入れ、同じ frame の placement をその pan で解く。要求の保持・確定は §10 に従う。
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
  明示の消去は、後でたまたま値が applied に戻った場合の取り違えを防ぐため。
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
変更範囲を detached-rework-plan.md §11 に記録した。§10 の決定を実装・自動検証済みで、全体の独立実装レビューの P2 1 件を修正した (§9.4)。再確認は未完了。

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

### 9.1 2026-11-08 の停止記録

以下は当時の記録。途中差分は設計担当が `b9329ec7b` にコミットし、§10 の決定 (`0ac62559f`) を受けて再開した。

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

### 9.2 r4 実装・検証記録 (2026-11-09)

- 当時の状態: **実装済み・独立実装レビュー待ち**。以下は r4 実装時の内容と実行結果。以後の P2 修正は §9.4。製品起動・実機確認は未実施。

`FullscreenPageMaterialKind::{Complete, Provisional}` を `src/gpu_lanczos.rs` の通常素材・presentation 素材に保持する。
`src/app.rs` の通常サムネイル・通過素材の生成元（RAW 早期分岐・キャッシュ再利用を含む）で仮素材を明示し、
`resolve_fs_processed_material_from_caches` は未完成 final・注釈待ちの下地・通常 final 待ちの下段素材も仮素材として返す。
寸法・texture ID・ページ番号から種類を推測しない。リサンプル・capture・Native の再構成・補助素材への変換も種類を保持する。

`fs_initial_placement_pan` は要求と選択素材の幾何から frame 内の配置候補を解き、App の pan/base にはまだ公開しない。
paint はその候補で素材を準備し、実際に選べた素材で `resolve_fs_initial_placement` を呼ぶ。
仮素材なら要求の base zoom/pan を更新して保持し、単ページはそのページ、見開きは両ページが完成素材で準備できた frame に確定する。
単ページの Native が拒否された場合は元の要求から次の候補を解く。見開きも各ページで既存のサムネイル代替を一度だけ選べる
有限の選択経路を使い、実際の素材の union で配置を解き直す。通常素材の GPU 準備は選択が定まった後の一回だけにする。
片側を準備できなければ要求を保持し、既存の部分描画には確定済み pan を使う。代役も無い Native は描画せず、寸法情報だけを残して従来の部分配置を保つ。
確定前の候補 pan や素材を bind/submit しない。
この選択整理は有効な初期配置要求がある current frame に限り、既定の中央・captured・Z の既存描画経路は保つ。
Native の準備で代役へ戻った場合や片側が無い場合は確定しない。既存の source 選択順序・表示基準・紙めくり条件は変えない。
同 frame の最終 transform に反映し、余分な persistent field・rollback・待機機構は追加していない。

`src/ui_fullscreen/initial_placement/tests.rs` と `src/app/tests.rs` に、§6 と §10 の到着順・状態遷移・実描画・入力・
context・別窓復元・実際の紙めくり offer 着地の回帰を置く。素材種類の保持は `src/gpu_lanczos.rs` と既存の素材所有テストでも検査する。
設定の既定・serde・転送分類・検索・環境設定 UI のテストは `b9329ec7b` の追加分をそのまま検証する。

検証コマンドには `MSBUILDDISABLENODEREUSE=1` を付け、Cargo の並列数を 4 に制限する。
full gate・build-dev・製品起動・コミットは今回も行わない。

- UI snapshot は次の PowerShell で更新・通常比較し、どちらも 1 成功・245 除外。
  ログは `target/1726-snapshot-update.log` / `target/1726-snapshot-verify.log`。
  PNG は 480×300 の既存画像を維持し、目視で設定行・短説明の追加だけと確認した。
  差分 24,274 pixels は `(8,180)–(466,233)` に限り、上 180 行は完全一致。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
$env:CARGO_BUILD_JOBS='4'
$env:UPDATE_SNAPSHOTS='1'
cargo test --test ui_snapshot fullscreen_fit_cycle_single_mode_dark
Remove-Item Env:UPDATE_SNAPSHOTS
cargo test --test ui_snapshot fullscreen_fit_cycle_single_mode_dark
```

- 最終 core check は以下のコマンドで成功 (17.48 秒)。ログは `target/1726-r4-core-verified.log`。既存 warning は残る。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo check -p mimageviewer --bin mimageviewer-core --jobs 4
```

- 絞り込み nextest は以下のコマンドで **233 件成功・16,658 件除外** (実行 23.205 秒、再コンパイル 1 分 39 秒)。
  ログは `target/1726-r4-focused-verified.log`、run ID は `3b6e8c5c-fd7d-4012-b0dd-ca06b4d39334`。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~initial_placement) | test(~page_start_position) | test(~settings_transfer) | test(~search_index) | (test(~viewer_context_registry) & (test(~bundle) | test(~swap) | test(~context_async_owner_registry))) | test(~holdover) | test(~page_turn_submission_tests) | test(~page_material_kind) | test(~pdf_metadata_readoption) | test(~source_absent_changed_view_or_prepare_key_uses_existing_fallback) | test(~source_absent_stable_view_keeps_native_single_and_pair)'
```

- 検証途中で、前回の未実行候補の enum 値・借用競合、寸法の優先順位を反映していない fixture、headless texture 上限、
  double-click の入力 frame 列、raw texture と完成合成 texture の ID の同一視を修正した。
  修正前に原因を報告し、期待する位置・入力取消・要求保持の assert は維持した。既存 detached テストは変更していない。
  初回コンパイル失敗は `target/1726-r4-focused.log`、途中の実行結果は同 `focused-retry.log` (223/226)、
  `focused-final.log` (230/231)、`focused-pass.log` (232/233)。最後の部分描画の失敗は fixture 変更で回避せず、上記の実装境界で修正した。
- 候補配置→実素材準備→確定と、Native の有限な選択・部分描画の維持は、別担当 Sol / xhigh の限定レビューでも構造的な整理と合意した。
  これは全体の独立実装レビューを代替しない。
- 最後に引数なしの `cargo fmt` を実行し、`cargo fmt --check` / `git diff --check` も成功。
  `python scripts/check_ui_glyphs.py` は危険グリフ 0、`python scripts/check_site_cards.py` は 53 件・超過 0。
  整形後の Rust 変更は整形のみで、追加の機能変更は行っていない。

### 9.3 追加・拡充した回帰名

r4 時点の `initial_placement_` 系は 48 件（前回の候補と §9.4 の追加 2 件を含む）。全件を同じ filter で実行した。以後の D2 回帰は §9.5。

```text
initial_placement_r4_original_thumbnail_then_full_aligns_first_complete_frame
initial_placement_r4_incomplete_final_retains_until_actual_completed_ai_material
initial_placement_r4_width_thumbnail_then_full_keeps_initial_pan
initial_placement_r4_manual_pan_during_thumbnail_survives_full_arrival
initial_placement_single_provisional_resize_restores_origin_before_complete_arrival
initial_placement_spread_provisional_resize_restores_origin_before_complete_arrival
initial_placement_r4_failed_full_load_keeps_stable_provisional_pan
initial_placement_r4_spread_waits_until_both_actual_materials_are_complete
initial_placement_r4_actual_passthrough_producer_keeps_request_until_full_arrives
initial_placement_rejected_native_uses_fitting_thumbnail_without_committing_candidate_pan
initial_placement_actual_paint_without_material_preserves_request_and_pan
initial_placement_actual_ctrl_wheel_route_cancels_request_before_changing_zoom
initial_placement_actual_double_click_reset_cancels_request_and_centers
initial_placement_actual_zoom_key_hold_start_cancels_request
initial_placement_items_generation_boundary_retires_before_next_paint
initial_placement_accepted_open_creates_request_after_resetting_transform
initial_placement_geometry_aligns_only_overflowing_axes_and_respects_direction
initial_placement_center_creates_no_request_and_does_not_change_split_pan
initial_placement_holdover_and_missing_geometry_retain_until_first_current_frame
initial_placement_stale_identity_and_changed_transform_discard_without_moving
initial_placement_replacement_request_uses_latest_anchor_and_transform
initial_placement_single_uses_book_direction_when_mode_has_no_direction
initial_placement_single_paint_starts_at_top_in_the_first_painted_frame
initial_placement_single_paint_respects_trim_rotation_and_reserved_viewport
initial_placement_single_paint_keeps_nonoverflowing_singleton_side
initial_placement_white_companion_union_controls_overflow_and_reading_start_edge
initial_placement_selected_holdover_does_not_consume_single_or_spread_request
initial_placement_spread_paint_aligns_unit_union_in_the_first_frame
initial_placement_navigation_holdover_preserves_old_single_and_spread_position
initial_placement_source_reload_holdover_keeps_live_manual_pan
initial_placement_z_navigation_holdover_uses_the_existing_live_transform_path
initial_placement_offered_animation_target_submits_aligned_flat_in_first_frame
initial_placement_input_pan_pinch_and_key_zoom_cancel_even_when_values_return_to_base
initial_placement_fit_continuous_and_edit_resets_clear_the_request
initial_placement_split_reconciliation_never_recreates_a_consumed_request
initial_placement_accepted_same_image_split_step_requests_only_reading_start
initial_placement_excluded_modes_retire_requests_without_changing_position
initial_placement_slideshow_same_image_slice_creates_request_without_reopening
initial_placement_close_clears_unresolved_request
initial_placement_panorama_retires_request
initial_placement_context_swap_keeps_sibling_requests_and_pan_separate
initial_placement_parked_geometry_resets_clear_only_the_affected_context_request
initial_placement_page_turn_target_paints_aligned_before_disabling_nonzero_pan_animation
initial_placement_detached_resume_restored_zero_pan_explicitly_clears_request
initial_placement_rejected_native_pair_commits_only_actual_thumbnail_geometry
initial_placement_one_rejected_native_side_recomputes_the_actual_mixed_unit
initial_placement_missing_native_fallback_keeps_committed_pan_and_request_for_partial_paint
initial_placement_center_and_reading_start_without_request_keep_existing_native_fallback_geometry
```

設定・UI・素材種類の追加／拡充回帰と snapshot:

```text
fullscreen_page_start_position_defaults_and_serde_are_compatible
page_start_position_transfer_roundtrip_missing_and_invalid_preserve_value
page_start_position_ui_selects_both_positions
page_start_position_search_reaches_its_anchor
page_material_kind_survives_prepare_capture_and_native_reconstruction
pdf_metadata_readoption_retires_only_dependent_live_passthrough_materials
fullscreen_fit_cycle_single_mode_dark
```

### 9.4 独立実装レビュー P2: 配置基準と CAS の分離 (2026-11-09)

独立実装レビュー (GPT-6.1 Sol / xhigh) の P2 1 件を設計担当がコードで確認し、修正を指示した。
仮表示の適用値を次の配置基準にも使っていたため、表示領域の拡大で収まった軸にも仮表示の pan が残っていた。
`src/ui_fullscreen/initial_placement.rs` の要求を不変の `origin_zoom` / `origin_pan` と CAS 用の
`applied_zoom` / `applied_pan` に分け、仮表示では後者だけを更新する。
単ページ・見開きの pan 0 の幾何は `src/ui_fullscreen.rs` で毎回 `origin_zoom` から解き、収まる軸は `origin_pan` へ戻す。

`initial_placement_single_provisional_resize_restores_origin_before_complete_arrival` と
`initial_placement_spread_provisional_resize_restores_origin_before_complete_arrival` を追加した。
実際のサムネイル描画で縦にはみ出した後、完成素材の到着前に viewport を拡大し、収まった軸が origin に戻ること、
その frame では要求が残り、完成素材を描く frame で同じ位置に確定して消えることを検査する。
open 相当の origin pan 0 と、非ゼロの origin pan の両方を通す。
既存の `initial_placement_r4_manual_pan_during_thumbnail_survives_full_arrival` と CAS 不成立の回帰は維持し、
既存テストの期待値を弱める変更はしていない。要求生成時の origin の検査も追加した。

- 追加 2 件の先行実行は成功 (2.652 秒、コンパイル 3 分 26 秒)。ログ: `target/1726-r4-p2-resize.log`。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~provisional_resize_restores_origin)'
```

- 前回と同じ絞り込み範囲は **235 件成功・16,658 件除外** (前回の 233 件 + 追加 2 件、実行 71.663 秒、再コンパイル 3 分 10 秒)。
  ログ: `target/1726-r4-p2-focused.log`、run ID: `27924f05-301b-4c37-8680-fc02d57f1a2e`。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~initial_placement) | test(~page_start_position) | test(~settings_transfer) | test(~search_index) | (test(~viewer_context_registry) & (test(~bundle) | test(~swap) | test(~context_async_owner_registry))) | test(~holdover) | test(~page_turn_submission_tests) | test(~page_material_kind) | test(~pdf_metadata_readoption) | test(~source_absent_changed_view_or_prepare_key_uses_existing_fallback) | test(~source_absent_stable_view_keeps_native_single_and_pair)'
```

- core check は成功 (40.84 秒)。ログ: `target/1726-r4-p2-core.log`。既存 warning は残る。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo check -p mimageviewer --bin mimageviewer-core --jobs 4
```

- 最後に `$env:MSBUILDDISABLENODEREUSE='1'; cargo fmt` (引数なし) を実行して成功した。
  整形後の Rust 変更は整形のみで、機能の追加変更はない。`git diff --check` も成功した。
- UI・公開文言は変更していない。製品起動・full gate・build-dev・コミットは行わない。

### 9.5 実機確認・依頼 25 D2: 原寸見開きの待ち表示 (2026-11-09)

サブ PC 担当の報告: 2400×3400 の縦長画像、原寸・右→左の見開き・読み始めで [2,1] から [4,3] へ
送ると、位置は期待どおりだが中央の「読み込み中」が残った。設計担当のログ確認では cache hit の後に再描画が止まった。
この作業では製品を起動していない。`34eb5c469` の clean な作業ツリーから App の headless 描画回帰を追加した。
負荷を抑えて同じ縦横比の 720×1020 の完成素材を 4 ページ、600×400 の viewport とし、通常の source 選択、
受理済みナビゲーション、実描画、output hook、次 frame の採用反映まで通した。

修正前の 3 条件の結果 (`target/1726-d2-repro.log`、run ID `be274f62-cc81-49cb-ba60-cb82cdfb4532`):

| 条件 | 結果 | 最後の提示証拠 |
| --- | --- | --- |
| 読み始め・右→左 | 失敗、6 frame 後も sequence が残る | 両ページ `Live`、receipt 採用済み、面積 `[0, 240000]`、照合失敗 |
| 中央・手動パンなし | 成功 | pan 0、両ページ正の面積、sequence 退役 |
| 中央・送る前に手動パン | 成功 | open で pan 0 に戻る、sequence 退役 |

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 --success-output immediate -E 'test(~cached_original_rtl_spread)'
```

原因と経路: `draw_fs_spread_with_placement` → `submit_fs_spread_prepared` は、実際に選んだ current の両ページを
`Live` として記録していた。output hook も flat receipt を採用していた。しかし
`emit_fs_page_turn_ready_for_display_unit` → `SubmittedUnitReceipt::matches` / `matches_sources` が全ページに
正の可視面積を要求し、原寸で viewport 外になった相方のため照合に失敗した。採用待ちの記録にも進めず、
`observe_fs_navigation_sequence_presented` が呼ばれないまま待ち表示が残っていた。
NavigationPrevious placement や provenance の記録漏れではない。

面積条件は取り込んだ master 側 (`git show 'HEAD^2:src/ui_fullscreen/page_turn.rs'`) にも存在する。
今回の読み始めが最初の current frame から pan を入れ、相方を画面外へ置くことで顕在化した。
中央＋送る前の手動パンは open の reset があるので、この条件の比較では発生しない。
master を別途ビルドした結果ではなく、上記の条件比較と source inspection による切り分けである。

修正は `src/ui_fullscreen/page_turn.rs` の `SubmittedUnitReceipt::matches_sources` に限定し、通常 flat の
提出済み occurrence / source / paint ID の完全一致から可視面積条件を外した。
出力採用、current の完全な page set、全ページの `Live`、`!curl` は維持する。
紙めくりの適格条件・可視性 ledger・canonical completion の条件は変更していない。
既存の照合の責務を正す小さな修正で、新しい状態・所有経路・guard・delay・repaint・reset を追加する構造変更は不要だった。
対象外の B5 / C4 / E3 と detached 述語・viewport 経路は触っていない。

追加した回帰 (`src/ui_fullscreen/page_turn/wait_indicator_tests.rs`):

```text
initial_placement_cached_original_rtl_spread_navigation_retires_wait
page_wait_cached_original_rtl_spread_center_navigation_retires
page_wait_cached_original_rtl_spread_center_manual_pan_before_navigation_retires
initial_placement_cached_original_ltr_spread_navigation_retires_wait
initial_placement_cached_original_single_navigation_retires_wait
initial_placement_cached_original_split_navigation_retires_wait
initial_placement_provisional_then_complete_spread_navigation_retires_wait
```

仮素材の回帰は、受理済み target の full cache が描画前に失われた状況で実際の thumbnail を選び、
要求を保持した後、次 frame の完成素材で要求を確定する。古い thumbnail の receipt は現在の source と一致しないため
採用し直さず、新しい完成素材の出力で sequence が退役することを検査する。

- 修正後の 7 件は成功 (0.885 秒、`target/1726-d2-fix-focused.log`、run ID `dcb55d07-cc7b-4d6f-9e65-5dde623d1227`)。
  その後、受理済み receipt・全 `Live`・画面外の相方を実際に含むこと・完成素材の最初の frame では旧 receipt で退役しないことの assert を追加した。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 --success-output immediate -E 'test(~cached_original) | test(~provisional_then_complete_spread_navigation)'
```

- 前回の集合に prepare_boundary 全体・待ち表示関連・部分 curl の除外回帰を加えた **253 件が成功・16,760 件除外**
  (27.302 秒、コンパイル 2 分 31 秒)。ログ: `target/1726-d2-regression.log`、run ID: `c631c23d-312c-4bc7-a308-87a24b16c7bc`。
  既存テストの期待値変更・削除・弱体化はない。上記の追加 assert を含む最終ソースで検証した。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~initial_placement) | test(~page_start_position) | test(~settings_transfer) | test(~search_index) | (test(~viewer_context_registry) & (test(~bundle) | test(~swap) | test(~context_async_owner_registry))) | test(~holdover) | test(~page_turn_submission_tests) | test(~page_material_kind) | test(~pdf_metadata_readoption) | test(~source_absent_changed_view_or_prepare_key_uses_existing_fallback) | test(~source_absent_stable_view_keeps_native_single_and_pair) | test(~page_turn::prepare_boundary_tests) | test(~page_turn::wait_indicator_tests) | test(~partial_leaf_receipt_cannot_complete_navigation)'
```

- core check は成功 (19.84 秒、`target/1726-d2-core.log`)。既存 warning は残る。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo check -p mimageviewer --bin mimageviewer-core --jobs 4
```

- 最後に `$env:MSBUILDDISABLENODEREUSE='1'; cargo fmt` (引数なし) を実行して成功した。
  整形後の Rust 変更は整形のみで、追加の機能変更はない。`git diff --check` も成功した。
コミット・製品起動・full gate・build-dev は行わない。

### 9.6 D2 独立レビュー P2-1 / P2-2 の対応 (2026-11-09)

設計担当が `8bbeab035` をコミット後、独立レビュー (GPT-6.1 Sol / xhigh) の P2 2 件をコードで確認し、
修正を指示した。D2 の修正方針と追加 7 件の実経路については、同レビューで妥当と判断された。

- **P2-1**: 取消後の canonical flat に画面外のページがあると、`barrier.rs` の記録・採用と
  `page_turn.rs` の照合に残っていた正の面積条件で `Completing / NeedFlat` が終わらない。
  `SubmittedUnitReceipt::matches_flat_pages` を唯一の flat identity 述語とし、`matches_sources`、
  出力再提出の `same_submitted_pages`、held receipt の照合、canonical completion event、
  barrier の flat 記録・採用をそこへ集約した。`matches_flat_unit` はその prepared unit 用の adapter。
  完全な occurrence / source / paint ID、destination、`!curl` を検査し、面積は条件にしない。
  caller が確認する出力採用・frame・generation・content binding は保ち、出力再提出の画面順も維持した。
  `capture_binding` は curl と flat の両方の内容 binding、geometry 再 binding は source 世代の照合なので、
  この flat 完了の述語へ混ぜない。curl の可視性計測・visibility ledger は変更していない。
- **P2-2**: `page_turn_resident_source` は未完成 final を既定の `Complete` resource で受理し、
  完成状態を quality 文字列 `provisional` にしか持たせていなかった。Offered の素材が通常 resolver より優先され、
  要求が未完成素材で消える。resident でも `FinalCompositeEntry::complete` から型を付け、quality は段階名 `final` に統一した。
  通常の下段代替も `Provisional` とし、安定 RAW preview の例外は通常 resolver と共通の
  `App::fs_lower_layer_material_kind` へまとめた。cached faithful rendition も `Provisional` とし、
  blocked RAW preview と thumbnail は生成元の型を保持する。注釈待ちの下地も `Provisional` とし、
  `App::cached_comic_doc_has_objects` を通常 resolver と共有して読み込み済みの情報だけで判定する。
  新しい owner・状態・producer・I/O は追加しない。

修正前に追加した App 描画回帰 3 件は **0 成功・3 失敗** (0.516 秒、コンパイル 2 分 58 秒)。
取消の 2 件は flat が `NeedFlat` に残り、resident の 1 件は Offered の型が `Complete` となる期待違反で失敗した。
ログ: `target/1726-review-repro.log`、run ID: `a5d9ad1a-4168-4917-8a51-463c04939151`。
その後、resident の型分類を source ID・選択枝とともに確認する 3 件を追加した。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~completion_placement_tests) | test(~resident_placement_tests)'
```

追加回帰 (`src/ui_fullscreen/page_turn/completion_placement_tests.rs` / `resident_placement_tests.rs`):

```text
initial_placement_cancelled_spread_offscreen_flat_releases_navigation_barrier
initial_placement_discarded_offscreen_completion_replay_releases_navigation_barrier
initial_placement_offered_incomplete_final_retains_until_completed_material_is_painted
resident_lower_stages_remain_provisional_until_final_is_available
resident_stable_raw_preview_preserves_complete_contract
resident_comic_on_completed_final_preserves_complete_contract
```

取消回帰は LTR / RTL の両方で実際の Turning を始め、拡大・パン後の通常描画で片側の面積 0 を検査し、
Completing の採用まで次の受理済みナビゲーションを保留し、その後 owning frame で再開する。
discard の回帰は同じ flat の再提出を経て採用する。resident の App 回帰は、受理後・描画前に新しい完成素材を公開し、
ordinary は新しい Complete、実際の描画は Offered の旧 Provisional であることを素材 ID で示す。
その frame は要求を保持し、最初の 1600×2400 の完成素材 frame で上端へ揃えて確定する。

修正途中の一次実行は 6 件中 4 成功・取消の 2 件が失敗した (1.453 秒、コンパイル 8 分 02 秒)。
ログ: `target/1726-review-fixed.log`、run ID: `293e621d-1951-4bf3-bdfc-55243ce259e1`。
新しい取消テストが「取消直後の最初の flat で Submitted」と仮定していたが、通常 prepare の後に取消が起きた場合、
既存の `refresh_page_turn_completion_pages` は次の Completing 描画で新しい幾何・paint ID へ再結合する。
そこで最初の取消・保留を検査し、再結合後の canonical flat で Submitted・採用・ナビゲーション再開を検査する形に修正した。
画面外ページの検査と、採用前は保留・採用後は再開という期待値は維持している。既存テストは変更していない。
次の fixture 実行も 4/6 成功 (`target/1726-review-fixed-final.log`、0.787 秒、run ID `89d82abf-5b25-4fbf-aabd-30a0819680dd`)。
incoming の completed final を prepare せず RAW の下段素材から受理しており、source 更新も混在していた。
通常描画で incoming / outgoing を先に prepare し、Offer 全素材の `Complete` を明示検査する形に直した。
その実行は 5/6 成功 (`target/1726-review-final-targeted.log`、0.819 秒、run ID `a5402e18-f0c7-41c2-9b20-c790c6d5db9d`)。
残る新テストの失敗は frame 入口の既存取消後にも Turning を要求していたもの。前 frame の Turning 検査は保ち、
取消 frame はどちらの取消入口でも同じ flat の提出・採用を満たすことを検査するようにした。
関連集合の最初の実行は 404 件中 403 成功・新しい discard テスト 1 件失敗
(`target/1726-review-regression.log`、70.736 秒、run ID `d768d9f3-2562-49eb-9901-29c064d52c9c`)。
`run_pass` が discard 時に同じ frame の closure を再実行するのを、テストが次 frame の再提出として扱っていた。
pass ごとの receipt を記録し、最初の pass は未採用・再実行の flat は採用済み、次の owning frame で再開することへ検査を修正した。
同じ操作を pass 間で再利用し、continuation の identity も保つ。ここまで既存テスト・製品の判定に追加変更はない。

- 最終の絞り込みは **404 件成功・16,615 件除外** (73.421 秒、コンパイル 2 分 01 秒)。
  前回 §9.5 の 253 件と、新規 6 件、flat 照合の共有範囲に当たる page_turn 全体を含む。
  ログ: `target/1726-review-regression-final.log`、run ID: `460b193c-58d2-4e5b-a420-0a4d0bced9bf`。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~initial_placement) | test(~page_start_position) | test(~settings_transfer) | test(~search_index) | (test(~viewer_context_registry) & (test(~bundle) | test(~swap) | test(~context_async_owner_registry))) | test(~holdover) | test(~page_turn_submission_tests) | test(~page_material_kind) | test(~pdf_metadata_readoption) | test(~source_absent_changed_view_or_prepare_key_uses_existing_fallback) | test(~source_absent_stable_view_keeps_native_single_and_pair) | test(~ui_fullscreen::page_turn::)'
```

- core check は成功 (25.08 秒、`target/1726-review-core.log`)。既存 warning は残る。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo check -p mimageviewer --bin mimageviewer-core --jobs 4
```

- 最後に `$env:MSBUILDDISABLENODEREUSE='1'; cargo fmt` (引数なし) を実行して成功した。
  整形後の Rust 差分は整形のみで、追加の機能変更はない。`git diff --check` も成功した。
製品起動・実機確認・full gate・build-dev・コミットは行わない。

### 9.7 再レビュー結果と残件 (2026-11-09、設計担当)

e9099ac8f の独立再レビュー (GPT-6.1 Sol / xhigh): P2-1 / P2-2 は解消、追加の P1 / P2 なし、master へ入れてよい。
残件 P3 (診断のみ、未対応): canonical completion が面積 0 のページを受理するようになったため、演出開始直後に拡大・パンで
片側を画面外へ出して取り消すと、まだ見えていないページにも `first_visible` (`visible_area=0`) と `fully_visible` が
記録される (page_turn.rs の completion event の `visible_pages`)。完了・再開の条件は正しく、通常操作は止まらない。
直すなら、可視性イベントの mask を receipt の実面積から求め、全ページが見えていない取消着地では `fully_visible` を出さない。

### 9.8 依頼 27 追補: ページ構成切替時の読み始め (2026-11-09)

設計担当から、`e9099ac8f` の独立再レビュー承認とサブ PC 再確認 A〜C の成功が報告された。
別件のサブ PC 担当の観測では、6000×3000 の単ページを原寸・左→右・読み始めで開いて `9` を押すと、
単ページ左端合わせの大きな pan が分割片側へ持ち越され、左側が黒くなった。開き直すと正しい位置だった。
依頼 25 の単ページ→見開きでも、旧単位の pan を持ち越す見え方が報告されていた。

設計担当の決定に従い、paged の実際のページ構成変更を要求生成の第 3 の入口にした (§1.1・§2・§3)。
`request_fs_initial_placement_after_composition_change(previous_zoom)` に方針をまとめ、ReadingStart のときだけ
pan を 0 に戻し、元の倍率と現在の anchor で既存の `request_fs_initial_placement` を呼ぶ。
受理・構成変化の判定は各入口が持つ既存の分岐を使い、別の状態や request は追加していない。
`normalize_spread_position` は anchor を揃えるため open を通り、倍率を 1 に戻す場合がある。
その場合も ReadingStart は正規化前の倍率を復元してから要求を作る。Center は従来の経路をそのまま通す。
通常の構成変更での Center の pan/zoom 持越しに加え、この既存の anchor 正規化 open のリセットも保つ。

入口の棚卸し:

| 経路 | 対応 |
| --- | --- |
| キー 1/2/3/4/5/8/9、ゲームパッド Select、一般の構成操作 | `apply_fullscreen_spread_mode` の受理後・anchor 正規化後に共通 helper |
| 本の構成が view settings を持つ場合 | `apply_fullscreen_view_mode` の受理後・正規化後に共通 helper |
| 抽出結果の view mode | 同じ入口で読書方向による mode 正規化後の実変更だけを採用。同じ正規化結果は no-op |
| ホバーバーの構成メニュー | 既存の一括保存成功後、mode/flow/direction の整合・reset 後に共通 helper。方向だけの変更は除外 |
| 同じキーで既存の見開きずらしを解除 | 実際に組が変わる受理側で helper。ずらしも無い通常の同構成 no-op は要求・位置を変えない |
| 本の構成の live projection 採用 | mode が変わった場合だけ、rows・世代・landing を採用した後、現在の anchor で helper |
| snapshot の保存表示の復元 | 保存状態には camera が含まれない。mode が実際に変わる場合だけ復元後に helper。Center と同 mode は不変 |
| `7` / popup の読書方向だけの変更 | 既存の `reset_continuous_reading_transform` による中央へのリセットを維持。新要求は作らない |
| `enter/leave_page_edit_single_view`、`leave_local_page_edit_single_view` | 編集の一時的な単ページ化／復帰であり、従来どおり要求を消す。新入口へ含めない |
| 新規一覧の `begin_extracted_result_view`、永続値の水和 | 新しい閲覧 source の初期化。後続の既存 open の入口に任せ、途中で別の要求を作らない |
| viewer bundle の swap、detached の保存 zoom/pan 復元 | 構成の適用操作ではなく所有移動／復元。既存の context 所有・明示消去を維持 |

初期配置の consumer、仮素材→完成素材の確定、入力での消去、Z/360/depth/連結等での退役は変更していない。
slice の setter / reconcile に要求は追加せず、既存の半分選択の規則も変えない。
単ページ化／復帰の編集 pivot を除外する理由は、文書を読む切替ではなく編集対象を一時表示する操作だからである。

追加回帰:

```text
initial_placement_composition_switch_aligns_first_complete_frame_and_keeps_zoom
initial_placement_composition_switch_center_keeps_manual_pan_and_zoom
initial_placement_composition_switch_reanchors_pair_without_resetting_zoom
initial_placement_composition_same_mode_preserves_position_and_request
initial_placement_composition_switch_extracted_mode_uses_normalized_direction
initial_placement_composition_switch_owned_view_mode_aligns_without_changing_book
initial_placement_composition_edit_pivot_clears_without_requesting_entry_alignment
initial_placement_composition_switch_same_mode_shift_reset_aligns_new_pair
initial_placement_snapshot_mode_restore_aligns_only_changed_paged_composition
initial_placement_live_projection_mode_change_uses_adopted_generation_and_anchor
```

App 描画は 6000×3000 と同じ比率の 1600×800 を使い、単ページ→分割 LTR/RTL、単ページ→見開き LTR/RTL・表紙、
見開き→単ページを、最初の完成 frame の unit rect と split UV で検査する。手動パン・倍率 1.5 の後も検査し、
Center の持越し・同構成の no-op・非先頭 anchor の正規化・ずらし解除・本/抽出結果の経路・編集 pivot の除外も検査する。
live projection は実 worker/poll で並べ替えと方向変更を採用し、物理ページを保ちながら新世代・新 anchor の要求に置き換わることを検査する。

- 最初のコンパイルで新 fixture の `source_dims` を u32 にしていた型誤りを修正した (`target/1726-composition-repro.log`)。
- 修正前の App 回帰は **7 件中 3 成功・4 失敗** (4.269 秒)。既存の pan 持越し、要求なし、anchor 正規化による倍率リセットで失敗。
  `target/1726-composition-repro-final.log`、run ID `4b16dde6-cfcd-4d30-8ec6-7a2983f6ac5b`。
- 実装途中は 7 件中 6 成功。残る新 fixture は pan setter の proposed_zoom が fs_zoom も更新すると思い込んでいたため、
  明示的に倍率を設定して修正した (`target/1726-composition-fixed.log`)。既存テストは変更していない。
- 構成切替の最終 8 件は **8 件成功** (4.624 秒、コンパイル 2 分 34 秒)。`target/1726-composition-fixed-final.log`、
  run ID `7f6c8f2c-1671-4120-8933-0bba40d74365`。その後、snapshot と live projection の 2 回帰を追加した。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~initial_placement_composition_)'
```

- 最終の絞り込みは **527 件成功・16,502 件除外** (93.770 秒、コンパイル 2 分 19 秒)。
  前回の 404 件、新規 10 件、本の構成 runtime / snapshot / spread mode の関連回帰を含む。
  `target/1726-composition-regression.log`、run ID `413f05e1-72bb-4238-a1db-10fde2b0a162`。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo nextest run -p mimageviewer --lib --build-jobs 4 --test-threads 4 -E 'test(~initial_placement) | test(~page_start_position) | test(~settings_transfer) | test(~search_index) | (test(~viewer_context_registry) & (test(~bundle) | test(~swap) | test(~context_async_owner_registry))) | test(~holdover) | test(~page_turn_submission_tests) | test(~page_material_kind) | test(~pdf_metadata_readoption) | test(~source_absent_changed_view_or_prepare_key_uses_existing_fallback) | test(~source_absent_stable_view_keeps_native_single_and_pair) | test(~ui_fullscreen::page_turn::) | test(~book_composition_runtime_tests) | test(~snapshot_ops::tests) | test(~spread_mode)'
```

- core check は成功 (22.24 秒、`target/1726-composition-core.log`)。既存 warning は残る。

```powershell
$env:MSBUILDDISABLENODEREUSE='1'
cargo check -p mimageviewer --bin mimageviewer-core --jobs 4
```

- 最後に `$env:MSBUILDDISABLENODEREUSE='1'; cargo fmt` (引数なし) を実行して成功した。
  整形後の Rust 変更は整形のみで、追加の機能変更はない。`git diff --check` も成功した。
製品起動・修正後の実機確認・full gate・build-dev・コミットは行わない。

## 10. r4: 仮の素材の frame と確定 (設計担当の決定、2026-11-08)

§9 の差異への対応。§2 の「最初の current frame で一度だけ確定」を次の 2 段に置き換える。§3 の条件 1〜4・消去・退役はそのまま。

- **仮の段階**: current frame で、表示単位のどれかのページが**完成素材ではない素材** (通常サムネイル、通過 rendition、
  カラー化待ちの代役など、`prepare_fullscreen_state_with_permission` が完成素材の代わりに選んだもの) で描かれるとき、
  その frame の幾何で §2 の pan を求めて `fs_pan` に入れ、同じ frame の placement をその pan で解く。**要求は消さない**。
  要求の `applied_zoom` / `applied_pan` だけをいま入れた値へ更新する (以後の利用者入力は CAS と明示消去の両方で検出される)。
  配置基準の `origin_zoom` / `origin_pan` は要求作成時の値を保ち、毎回の幾何と収まる軸の維持値に使う。
  CAS の比較には最後の自動適用値 `applied_zoom` / `applied_pan` を使い、配置基準と分ける。
- **確定**: 表示単位の全ページが完成素材で描かれる最初の current frame で、同じく pan を求めて入れ、要求を消す。以後は再計算しない (§2 のとおり)。
- 「完成素材かどうか」は、描画 resolver が実際に選んだ素材の種類 (型付きの選択結果) で判定する。寸法の比較・ページ番号・ログ文字列で推測しない。
  未完成 final 合成や通常 final の完成待ちに選んだ下段素材も仮素材とし、完成素材の最初の frame で確定する。
  指定スケールは AI 前の原寸に揃うが (指定スケール計画 §4.3)、既存の原寸・倍率制限は選んだ texture の寸法を使うため、
  未完成 AI 素材から完成 AI 素材への幾何変化も仮の段階に含める。注釈合成は既存どおり完成した下地からのみ選び、
  元画像表示・RAW の安定 preview はそれぞれ既存の完成表示の契約を保つ。
- 見え方: 幅・高さ・ページ fit ではサムネイルと完成画像の矩形が同じなので、位置は最初から読み始めで動かない。
  原寸・px 指定ではサムネイルが小さく描かれる既存挙動のまま (はみ出さなければ従来の配置)、完成画像に替わる frame で読み始めへ揃う。
  替わる frame で大きさ自体が変わる既存の見え方に重なるので、新しい跳びは作らない。
- 仮の段階は**完成素材の到着で必ず終わる**有限の区間で、利用者が触れば即座に終わる。確定後に位置を戻し続けることはない。
- 完成素材が届かない (読み込み失敗) 場合は仮の段階のまま残る。毎 frame 同じ値を入れるだけで見た目は動かない。次の切替・入力で消える (まれな失敗の割り切り)。
- 簡素化の検討: 「完成素材が届くまで要求を消費せず中央で描く」案は、幅 fit の通常のページ送りで毎回「中央→上端」の跳びを見せるので採らない。
  「サムネイルでも元寸法の基準で描く」案は既存の表示基準 (`display_basis_tests` の契約) を変えるので採らない。
- 追加の回帰: 原寸で thumbnail 150×200 → full 1500×2000 (viewport 600×400) の到着順で、完成画像の最初の frame が上端にあること。
  幅 fit で thumbnail → full の間に pan が変わらないこと。仮の段階で利用者がパンしたら、完成素材の到着後もその位置のままであること。
  単ページ・見開きとも、仮表示中の viewport 拡大で収まった軸が origin の値へ戻り、その後に完成素材が来てもその位置で確定すること。
  読み込み失敗で要求が残っても pan が動かないこと。

