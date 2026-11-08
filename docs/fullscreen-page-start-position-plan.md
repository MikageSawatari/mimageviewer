# ページ切替後の初期表示位置「中央 / 読み始め」(§1.726)

- 状態: 設計 r1 (2026-11-08、設計担当 ClaudeCode)。独立設計レビュー待ち。
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

**加えて適用する**: 分割表示 (1 枚を半分ずつ読む) の片側→片側の移動
(`set_fullscreen_page_slice_for_display_unit`)。表示単位の切替だが open を通らず、現行は pan を戻さない。
`読み始め` のときだけ、倍率は保ったまま初期位置を求める。`中央` のときは現行どおり何もしない (pan を保つ)。

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
    page_slice: PageSlice,      // 分割の片側
    base_zoom: f32,             // 要求作成時の fs_zoom (open では 1.0)
    base_pan: egui::Vec2,       // 要求作成時の fs_pan (open では ZERO)
}
```

- **作る**: 設定が `読み始め` のときだけ、§1.1 の適用入口で作る (前の要求は上書き)。`中央` では作らない。
- **解く (唯一の consumer)**: 単ページ / 見開きの paint で、placement を解く**直前**に 1 回。条件:
  1. 要求の items 世代・anchor・slice が今の frame の表示単位と一致し、holdover (前の表示単位の描き残し) ではなく live の描画である
  2. `fs_zoom == base_zoom` かつ `fs_pan == base_pan` (作成後に誰かが位置・倍率を動かしていない)
  3. `fs_zoom_owner() == Image`、paged 表示、比較・分析・オーバーレイ編集・表示トリム編集・Z 照準でない
  4. 表示単位の全ページの幾何入力が確定している (`display_basis` が Unavailable でない)

  1〜3 のどれかが成り立たなければ要求を捨てる (位置は現行どおり)。4 だけが不成立なら要求を残し、次の frame で再評価する
  (寸法が後着する PDF・RAW 等。後着した最初の frame で一度だけ確定する)。
  成立したら `base_zoom`・pan 0 で幾何を解き、§2 で pan を求めて `fs_pan` に入れ、要求を消し、同じ frame の placement をその pan で解く。
  **中央で 1 frame 描いてから跳ぶ見え方を作らない。**
- **消す**: 解いたとき、条件 1〜3 不成立のとき、フルスクリーンを閉じたとき、items 世代の交代、
  および利用者の位置・倍率入力の入口 (`set_fs_pan_from_input`、ホイール拡大、キー拡大、ダブルクリック表示リセット、
  fit 方式変更のリセット、連結リセット、編集ツールの単ページ化 / 復帰、detached 再開の zoom/pan 復元)。
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
- 紙めくり演出: pan が 0 でない状態では既存規則で演出しない (`page_turn_view_eligible`)。はみ出す表示単位は
  ROI 不足で元から演出対象外。演出の着地 frame で通常 flat と位置がずれないこと (演出が通常 layout の矩形を使う) をテストで確かめる。
- 見開きの holdover は open 時の pan (=0、現行どおり) で描き、要求は live の frame まで解かない。前のページが動かない。

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
- 状態遷移: `中央` では要求を作らない (現行 pan=0)。`読み始め` で open→最初の live frame で確定→次の frame で再計算しない。
  holdover frame では解かない。寸法未確定の間は保持し、確定 frame で一度だけ。手動パン・ホイール拡大・Z・表示リセット後は解かない。
  高画質差し替え後も位置不変。分割の片側移動で確定、`中央` では pan 保持。連結読み・動画・360・depth では作らない / 捨てる。
  detached 再開で復元した pan を上書きしない。兄弟 context の pan と要求を変えない。
- 設定: 既定値・serde 欠落・転送分類・設定数テストの更新。環境設定の UI スナップショット。

## 7. detached viewer 凍結ルールとの関係

`ViewerContextBundle` へ一般の表示状態フィールドを 1 つ足すだけで、detached 述語・viewport 経路・placement・HWND には触れない。
detached 専用の bool / Option ではなく、既存の `fs_pan` と同じ context 固有の表示状態 (憲法 §2-3 の対象外と判断)。
独立レビューで「症状パッチではなく構造的な追加である」ことへの合意を取り、detached-rework-plan.md §11 に記録する。
