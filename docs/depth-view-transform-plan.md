# 3D 写真の動きを続けたまま拡大・縮小・回転する — 設計書 (§1.874)

状態: **設計 R4 (2026-11-23)。独立設計レビュー R4 で「実装開始可」(§11)。利用者判断 (§8) 済み、どちらも推奨どおり。実装中。**
設計: ClaudeCode Opus 5.5。worktree `G:\home\mimageviewer-3d-874`、ブランチ `depth-zoom-874` (基点 master `7577e4ba5`)。
要望: [next-release-backlog.md](next-release-backlog.md) §1.874 (5ch >>882 「3D中の拡大縮小回転」)。
3D 写真の正本は [depth-parallax-plan.md](depth-parallax-plan.md)。本書はその §5.2 (置き場所と変換)・§6.5 (他の機能との関係)・
§7 (書き出し)・§17.2 (Ctrl+クリック)・§17.3 (マウス追従) を変える差分の設計で、実装時にそれらの節を本書に合わせて直す。

以下、行番号は基点 `7577e4ba5` のもの。関数名と併記する。

## 1. 範囲

- 3D 写真の表示中に、視差の動き (自動の動き・マウス追従) を止めずに、**閲覧用の拡大・縮小・パン・画像面の自由回転**を行えるようにする。
- 含めない: 被写体の裏側の生成、立体を任意の視点から見る機能、平面画像を奥行き方向へ傾ける §1.819、動画への適用、
  Z キーの全画面ズームモード (ZipPla 風、§4.4)、ナビゲータ (§4.5)。
- 決まり (統括から): **表示倍率・角度・位置を変えるたびに AI の深度推定をやり直さない** (§3.4 で不変条件とテストにする)。

## 2. 今の作り (コード確認)

- 描画の入口 `render_fs_body` の通常の単ページ分岐 (`ui_fullscreen.rs:29564` 付近) は、Depth 中だけ
  `zoom_pan = None`・`free_rot = 0`・`fit_mode = Page`・`fit_scale_limits = 既定` で `DisplayedImageTransform` を作る。
  2D のズーム・パン・自由回転・フィット設定は描画に渡らない (状態は消さずに保持)。
- 入力: `fs_zoom_owner()` (`ui_fullscreen.rs:15539`) が Depth 中は `FsZoomOwner::Depth` を返し、キー・ピンチ・
  中ボタン・スクラブ・パンの各 dispatch で無操作になる。ゲームパッドのズーム・パン (`16021`・`16043`)、フィット切替
  (`cycle_fullscreen_fit_mode`、`47381`)、リングのズーム (`gamepad_input.rs::apply_ring_action`) も Depth で止める。
  キャンバスのドラッグ (パン・Ctrl+ドラッグ回転) は `42551` の分岐で丸ごと飛ばす。クリックの分類
  (`plan_fs_canvas_click`、`42892`) は Depth 中だけ変換を「fit・パン 0・回転 0」とみなす。
- シェーダ (`depth_parallax_wgpu.rs::DEPTH_SHADER`) は callback の矩形 = `transform.paint_rect` 全体に描き、
  頂点の uv (callback の viewport 内の 0〜1) から画像の位置を出す。90 度回転は `rotate_uv` で 4 通り。自由回転は無い。
  egui-wgpu は callback の viewport を画面内へ切り詰める (`epaint ViewportInPixels::from_points`) ので、
  画面より大きい矩形をそのまま渡すと、切り詰めた viewport に画像全体が押し込まれて歪む。
- 止まる深さのクリック (`focus_depth_at`、`depth_parallax_wgpu.rs:144`) は paint_rect の軸平行写像だけを持つ。
- マウス追従 (`MouseFollow::update`) の中心は画像矩形の中心 (fit 固定なのでキャンバスの中心とほぼ同じ)。
- 書き出しは `record_display` (`app/depth_export.rs:535`) で、表示の `uv_rect`・90 度回転・`paint_rect` の大きさを写す。
  大きさは透過部分の市松の升目を書き出しの寸法へ拡大縮小する比に使う (`depth_export.rs:616`)。
- 深度の素材は表示中の元テクスチャ (§4.1 の `DepthSourceKey { texture_id }`)。素材の選択
  (`resolve_fs_processed_payload_and_depth`) は倍率に依存しない。GPU 縮小 (Lanczos) は描画時の派生で鍵を変えない。
  ただし **PDF は表示の変換から解像度を決めて再ラスタし、新しいテクスチャ = 新しい鍵になる**。表示の変換を入力にする
  PDF の需要は次の 3 系統 (§3.4):
  1. 倍率の需要 (legacy フィット): `maybe_rerender_pdf(zoom)` (`ui_fullscreen.rs:16004`) → `request_pdf_rerender`
     (`app.rs:83343`)。入力の確定時 (キー・ドラッグの終了・ゲームパッド・リセット等) に**出来事として**呼ばれ、毎フレームの
     照合は無い。Z モードだけは描画ごとに `fs_pdf_rerender_demand.note_legacy(idx, full_image_zoom)` で照合する (`15483`)。
  2. 絶対寸法の需要 (指定倍率フィット): `reconcile_specified_pdf_demand` (`33076`) が描画したページの
     `layout_full_image_rect` (2D の倍率を含む) から毎フレーム `request_pdf_absolute_demand` を出す。
  3. AI 用の native 再ラスタ: `pdf_native_rerender_pending(respect_zoom, ..)` (`app.rs:94615` 付近) が
     `fs_zoom ≠ 1` のとき起動しない。素材 (AI の入力) を完成させる処理。
  - ページの読み込み時の解像度 (`fs_pdf_display_target`、`ui_fullscreen.rs:29026`) と、その不足を補う
    `ensure_pdf_display_resolution` → `request_pdf_rerender_at_target` (`app.rs:83308`) は、窓の大きさ・90 度回転と
    **利用者のフィット設定** (`pdf_display_fit_mode`、`ui_fullscreen.rs:47263`。設定値を読み、今の 3D の描画が使う
    Page fit の強制とは無関係) で決まる。2D の倍率・パン・自由回転には依存しない。
  - AI 用 native 再ラスタの起動 (通常・合成時の保証・先読み、`app.rs:94576`・`93295`・`86328`) も `request_pdf_rerender` を通る。
- 描画の確定: 通常の単ページ描画 (`ui_fullscreen.rs:46380`〜`46590`) は、初期配置 (「読み始め」) の**提案**を変換に
  入れてから描き、2D の payload 準備 (`prepare_fs_paged_paint_payload`) の後で描画時のパン補正
  (`draw_pan_correction`) を計算し、最後に `resolve_fs_initial_placement` と `commit_fs_draw_pan` で `fs_pan` へ**確定**する。
  Depth の描画 (`try_paint_depth`、`46470`) はこの確定の前で return するので、今は Depth 中に初期配置とパン補正が
  確定しない (変換が fit 固定なので問題にならなかった)。

## 3. 決めたこと

### 3.1 3D の表示変換は 2D の表示変換そのもの (状態を足さない)

**3D 専用の一時的な表示変換は持たない。** 3D 中も、その viewer context の既存の 2D の表示状態
(`fs_zoom`・`fs_pan`・`fs_free_rotation`、フィット設定、フィットの拡大・縮小の制限、画像ごとの 90 度回転) で
`DisplayedImageTransform` を作り、深度のシェーダはその変換の四角形 (`paint_quad`) にそのまま描く。
3D 中のズーム・パン・回転の操作は 2D と同じ値を変える。

- 入るとき: 2D で拡大・回転していれば、その見え方のまま 3D になる (今は fit に戻して描く)。
- 抜けるとき: 3D 中に変えた拡大・回転は 2D にそのまま残る (今は 3D 中に変えられない)。
- リセット: 2D と同じ。変換があるときのダブルクリック (`FsCanvasClickAction::ResetTransform`)、既存のリセット操作、
  ページ移動の既存の初期化規則がそのまま効く。3D 専用のリセットは足さない (フィット切替は 3D 中は受け付けない、§3.4)。
- 90 度回転 (R / L、`rotation_db` に保存): 今と同じ。テクスチャに焼き込まないので深度は作り直さない。
- 自由回転・ズーム・パンは今までどおり保存しない (2D と同じ寿命。自由回転は一時的、倍率はセッション保持)。

**設計の簡素化の検討** (CLAUDE.md):

| 案 | 内容 | 判断 |
| --- | --- | --- |
| **A (採用候補)** | 3D は 2D の表示状態をそのまま使う | 状態は増えない。入力側は Depth の「無操作」分岐を消すだけで、2D と同じ持ち主 (`FsZoomOwner::Image`) を通る。ページ移動・F11・F12・独立窓・別 context の寿命は既存の 2D 状態の規則に乗る |
| B | 3D 専用の一時変換を `DepthViewState` に持ち、抜けると捨てて 2D を元に戻す | 持ち主が 2 つになる。ホイール・ピンチ・中ボタン・キー・スクラブ・ゲームパッド・リング・ドラッグのパン・Ctrl+ドラッグ回転・ダブルクリックのリセット・境界つきパン・初期配置・ページ移動時の初期化のすべてに Depth の書き先を足し、2D と同じ式を 2 か所に持つことになる。§6.5 の「書き先を間違えても画面が静止するだけで気付かない」型の取りこぼしを再び作りやすい |
| C | 3D に入るとき 2D の変換を fit に戻す | 2D で拡大していた状態が失われる。利用者の操作を勝手に捨てるので採らない |

A は **リリース済みの 3D の入り方が変わる** (今は入ると必ず画面全体に合わせて表示し、Width / 原寸などのフィット設定も
3D 中は無視する)。不便になる変更ではない見込みだが、見え方の変化なので利用者に確認する (§8 の質問 1)。

### 3.2 描画 — 2D と同じ四角形・同じ clip に描く

- 呼出元の分岐 (`ui_fullscreen.rs:29566`・`29573`・`29588`・`29635`) から Depth の特別扱いを消し、2D と同じ
  `zp`・`free_rot`・`fit_mode`・`fit_scale_limits` で変換を作る。3D の絵は 2D の絵と同じ painter・同じ clip・同じ
  `paint_quad` に出る (画像外の余白・上下のバー・パネルとの重なりは 2D と同じになり、3D 側で別の規則を持たない)。
- **シェーダの写像を「画面の画素 → 画像の局所座標」にする**: 頂点の uv をやめ、フラグメントの
  `@builtin(position)` (枠の物理画素) から次で画像の局所 uv を出す。
  1. `q = rotate(position.xy − center_px, −θ)` (center = `full_image_rect.center()`、θ = 自由回転。2D の
     `ImagePaintQuad::from_rect` と同じ中心・同じ向き)
  2. `in_uv = (center_px + q − paint_min_px) / paint_size_px`
  3. 以降は今の式 (`base = (in_uv − .5)/zoom + .5` → 動き → `rotate_uv` (90 度) → crop) のまま
  4. **最後に** `in_uv` が [0,1]² の外なら透明 (premultiplied で 0) にする (`select` で結果を差し替える)
  - **順序の制約**: `dpdx` / `dpdy` (今の `fs_main` の先頭、`depth_parallax_wgpu.rs:45`) は一様な制御フローでしか
    呼べない (WGSL)。画像外の判定で途中 return を入れず、微分は全フラグメントで先頭で計算し、画像外は最後の差し替えで
    透明にする。レイマーチのループ内のサンプルは今と同じ `textureSampleLevel`、色は `textureSampleGrad` のまま。
    書き出し (`depth_parallax_wgpu/export.rs:53`) も同じ WGSL を使うので、同じ順序で両方が検証を通ることをテストで確かめる。
  - 物理画素 = points × `pixels_per_point`。uniform に `paint_min_px`・`paint_size_px` (vec4) と `center_px`・`cos θ`・`sin θ`
    (vec4) を足す (48 → 80 バイト。書き出しの pipeline も同じ uniform を使う)。
  - callback の矩形は **四角形の外接矩形と painter の clip の交わり** にする (空なら描かない)。viewport の切り詰めで
    歪まず、clip の外は scissor で切れる。
  - 書き出しは `paint = (0,0,W,H)`・center = (W/2,H/2)・θ = 0 を渡す (今と同じ絵、§3.6)。
  - これで §17.2 の「callback の viewport が物理画素へ丸められるための近似」も 2D の四角形との一致に置き換わる。
- **動きの向きは画面基準**: 自動の動き・マウス追従の offset は今も 90 度回転の後の表示空間 (= 画面の軸) で決まっている。
  自由回転でも画面の軸のまま (左右の動きは画面の左右) にするため、シェーダで画像の幅を単位とした offset を −θ 回して
  から縦に `aspect` を掛ける (今の `offset × vec2(1, aspect)` の前に回転を挟む)。90 度回転と同じ考え方の延長で、新しい設定は足さない。
- **動きの量は画像に対する割合のまま**: 拡大すると画面上の揺れも同じ倍率で大きくなる (動いている写真をそのまま拡大する)。
  縁を隠す拡大 (`zoom = 1 + 2.2 × 強さ`) も画像の局所座標なので、拡大・回転しても画像の外が覗かない。
- 下地 (`paint_image_underlay(transform.paint_quad())`) は今も四角形で描いているので変えない。

### 3.3 マウス追従と止まる深さのクリック

- **マウス追従の中心は「画像の外接矩形の可視範囲の中心」にする**: 画像の四角形の外接矩形 (`transform.hit_rect` と同じ作り。
  自由回転を含む) とキャンバス (呼出元の media rect) の交わりの中心。交わりが空ならキャンバスの中心。外接矩形による近似で、
  自由回転した画像が窓の隅だけに見える場合などは、中心が実際の画像の外 (余白) に落ちることがある。実画像内の中心は
  保証しない (正確な可視多角形 `visible_source_uv_rect` の clipping は使わない。操作の中心としては近似で足りる)。
  - 今は画像矩形の中心。拡大・パンで画像の中心が画面外へ出ると、ポインタがどこにあっても端に張り付くので変える。
  - 画像全体が見えている間 (今の 3D の表示はすべてこれ。片側だけの見開き配置 `SingletonSpread` で画像がキャンバスの
    中心からずれている場合を含む、`displayed_image_transform.rs:446`) は、交わり = 画像の外接矩形なので、中心は今と同じ
    画像の中心になり、今の操作感は変わらない (R1 の「キャンバスの中心」は片側配置で操作感を変えるので採らない。独立レビュー P2)。
  - 半径 (`0.25 × キャンバスの短辺`)・縦 0.6 倍・符号・パネル上で固定は §17.3 のまま。中心は呼出元が transform と
    キャンバスから純粋関数で出して `MouseFollow::update` に渡す。新しい状態は足さない。
- **止まる深さ (Ctrl+クリック)**: `focus_depth_at` を §3.2 と同じ写像 (自由回転の逆変換 → paint_rect → 局所 uv) を
  通す形にする。写像の式は 1 か所の純粋関数 (例: `DepthScreenMapping`) に置き、uniform の値の作成と CPU の逆引きの両方が
  それを使う (同じ式を 2 か所に書かない)。offset 0 で引く理由と「同じ pass の描画時に解決する」(§17.2) は変えない。
- **クリックの分類**: Depth 中も 2D と同じ規則 (`plan_fs_canvas_click` の `has_transform` / `belongs_to_view` /
  ダブルクリックのリセット) を使う。拡大・パン・回転している間の修飾なしクリックは、2D と同じくページを送らない。
  Ctrl+クリックだけは、変換の有無にかかわらず `DepthFocusAt` にする (`belongs_to_view` で返る前に判定。ダブルクリックの
  リセットはその前のまま)。ドラッグ (egui の判定) は 2D と同じく Ctrl+ドラッグ = 自由回転、修飾なし = パン。
  - 実装担当は、Ctrl+クリックの押下で `fs_rotation_drag_start` が作られても、ドラッグと判定されない限り
    自由回転が変わらないことをテストで確かめる。
- **3D パネルの除外**: クリック・タップは既に除外している (`ui_fullscreen.rs:41152`、§17.2)。ドラッグのパン・回転・
  中ボタン・スクラブ・Ctrl+ホイール・ピンチも、表示中の 3D パネルの上では始めない (2D の左右パネルの除外と同じ扱い)。
  実装担当は開始判定の箇所を列挙し、2D のパネル除外を使っている所へ 3D パネルの矩形を加える。

### 3.4 深度推定をやり直さない (不変条件)

**3D 中に倍率・パン・自由回転だけを変えても、深度の要求 (`DepthRequest`) を新しく出さない。** フィット設定は 3D 中に変えられない (下)。

- 3D は §4.2 の色テクスチャ (元テクスチャの大きさ + mip) を拡大・縮小して描くので、倍率に応じた素材は要らない。
- **組み合わせを減らす — フィットの設定は 3D 中に変えない**: 3D は利用者のフィットの設定で描く (§3.1) が、3D 中は
  **全画面のフィットの設定** (フィット方式・指定倍率の値と上限・拡大しない / 縮小しない) を変える閲覧操作を受け付けない。
  受付の境界 (R3 の独立レビュー P1 で、`cycle_fullscreen_fit_mode` の抑止だけでは足りないと分かった):
  - キー・リング・ジェスチャ・マウスボタンのフィット循環 `cycle_fullscreen_fit_mode` (`ui_fullscreen.rs:47381`): 今と同じく止める。
  - 上部バーのズーム / フィットのメニュー (フィット方式・指定倍率の値と上限・拡大しない / 縮小しない): 今も 3D 中は出ない
    (`FsTopMenuEligibility::image` が占有中は false、`ui_fullscreen.rs:547`・`55181`)。変えない。
  - ゲームパッドの設定ピッカーのフィットの行 (プレビュー `gamepad_input.rs:6277`・確定 `7655`): Depth 中はこの行を変更不可にする
    (既存の `layout_supported = false` の行と同じ見せ方を使う。新しい見せ方は作らない)。
  - 防御として、全画面からフィット方式を変える共通の setter `set_fullscreen_fit_mode_for_current` (`47330`) も Depth 中は
    何もしない (呼び出し元を 1 つ増やしても漏れないように)。
  - **例外: 環境設定の画面** (モーダル) での変更は今までどおり受け付ける。倍率の操作ではない設定変更として、それで素材が
    変われば深度を作り直す (今の 3D と同じ)。
  - 実装担当は、上の 4 種の設定値を書き換える全箇所 (`settings.fullscreen_fit_mode`・`fullscreen_specified_scale`・その上限・
    `fullscreen_fit_no_upscale` / `_no_downscale`) を列挙し、環境設定の画面以外で 3D 中に届く入口が残っていないことを確かめる。これで、フィット設定を
  入力にする読み込み時の解像度の経路 (`pdf_display_fit_mode` → `fs_pdf_display_target` → `ensure_pdf_display_resolution`、§2)
  は 3D の操作では動かない (R2 の独立レビュー P1 は、3D 中のフィット切替がこの経路で新しいラスタを作る点だった。経路を
  個別に止めるのではなく、3D 中にフィット設定が変わる組み合わせそのものを無くす)。窓の大きさ・90 度回転・設定画面での
  変更による再ラスタと深度の作り直しは今の 3D と同じ (§6.3。倍率の操作ではない)。
- **PDF — 止めるのは「2D の倍率から決まる需要」の 2 か所だけ** (発行の共通入口 `request_pdf_rerender` は AI 用 native
  再ラスタも通るので、そこでは止めない。R2 の独立レビュー P1):
  1. 倍率の需要: `maybe_rerender_pdf` (`ui_fullscreen.rs:16004`) を Depth 中は何もしない形にする。呼び出し元
     (キー・ドラッグ・ゲームパッド・リセット等) には分岐を足さない。Z の描画の `note_legacy` → `maybe_rerender_pdf` は
     Depth 中は Z が休止しているので通らない。
  2. 絶対寸法の需要: `reconcile_specified_pdf_demand` (`33076`) を Depth 中は何もしない形にする (記録 `note_absolute` より前、
     関数の先頭で返す)。連結読みの `reconcile_continuous_pdf_demand` は 3D に入れないので対象外。
  - 3D 中の PDF は、3D に入ったときのラスタのまま拡大・縮小して描く (拡大すると 2D より柔らかく見え得る。§7 で確認)。
- **2D に戻ったときの再照合 — 記録を空にして既存の照合に任せる**:
  1. 上の 2 か所が需要を止めたとき、context 所有の需要の持ち主 `PdfRerenderDemand` (`fs_pdf_rerender_demand`、既に
     `ViewerContextBundle` の swap・fork の移動対象、`viewer_context_registry.rs:4247`・`4931`) に印を 1 つ立てる
     (「Depth 中に止めた表示需要がある」。ページ番号は持たない)。新しい bundle の field は足さない。
  2. **印は、Depth でない最初の単ページ描画 (通常・Z モード) が消費する**: 需要の記録 (legacy・absolute) を**すべて空にして**
     から (`PdfRerenderDemand::reset` と同じ)、通常の描画は今の `fs_zoom` で `maybe_rerender_pdf` を 1 回呼ぶ (入力の確定時と
     同じ値・同じ経路)。Z の描画は既存の `note_legacy(full_image_zoom)` の照合が、指定倍率は既存の毎フレームの
     `reconcile_specified_pdf_demand` が、空になった記録に対してそのまま需要を出す。記録を空にするので、入場前と同じ需要へ
     戻る場合も古い記録で捨てられない (R2 の独立レビュー P2)。今のラスタで足りていれば、発行先の既存の判定
     (`app.rs:83445` 付近) が省く。印は Depth 中の描画では消さない。
  3. `set_fs_exclusive_view` (presentation の確定前・fork の前にも呼ばれる、`depth_view.rs:1057`・
     `viewer_context_registry.rs:4451`) には副作用を足さない。F11 / F12 / fork で Depth が終わっても、印は context と一緒に
     移り、その context が次に 2D を描いた時点の実効の変換で需要が決まる (R1 の「抜けるときに `fs_zoom` で 1 回」は、
     Z 復帰の倍率と確定前の呼び出しを表せないので採らない。R1 の独立レビュー P2)。
- **AI 用 native 再ラスタは止めない** (素材を完成させる処理で、2D でも AI の結果が届けば素材が変わる)。Depth 中は、共通の
  判定 `pdf_native_rerender_pending` (`app.rs:94623`) の倍率条件 (`respect_zoom` で `fs_zoom ≠ 1` なら起動しない) を
  「倍率によらず可」として評価する (起動 KICK と保留の両方がこの関数を使うので両方に効く。in-flight・到達済みの既存の判定は
  そのまま)。Depth 中は倍率の再ラスタをしないので、`respect_zoom` の理由 (表示中のページは今の倍率でラスタ済み) が成り立たない。
  起動は AI の素材の完成につき 1 回で、倍率の操作では増えない。
- 実装担当は、`request_pdf_rerender*` / `request_pdf_absolute_demand` / `request_continuous_pdf_*` / `note_*` の全呼び出し元を
  列挙し、上の 2 か所と AI 用・読み込み時の解像度・連結読み以外に、2D の倍率・パン・自由回転を入力に PDF の再ラスタを
  要求する経路が無いことを確かめる。あれば止めて設計担当へ返す。
- 非 PDF: フィット設定・倍率は素材を変えない (GPU 縮小は描画時の派生)。
- 素材の選択 (`resolve_fs_processed_payload_and_depth` / `fullscreen_paint_payload_for_page`) は倍率に依存しない
  (§2)。実装担当は、拡大・回転で素材が `NotYet(NativeOnly)` 等へ変わる経路が無いことを確かめ、あれば設計担当へ返す。

### 3.5 入力の扱い (2D に揃えるもの・3D 固有に残すもの)

**2D と同じにする** (Depth の無操作・特別扱いを消す):

| 箇所 | 変更 |
| --- | --- |
| `fs_zoom_owner` (`15539`) | Depth 中は `FsZoomOwner::Image` を返す (`FsZoomOwner::Depth` を消す)。Z の全画面ズームモードは今と同じく Depth 中は休止 (Depth の判定が ZoomMode より先なので、`fs_zoom_active` を残したまま描画にも入力にも出ない) |
| キー・ピンチ・2 本指パン・中ボタン・スクラブのズーム、`apply_touch_zoom_factor_about_pivot` (`15625`) | Depth の no-op 分岐を消す |
| ホイールの分類 `fs_wheel_action_requests_navigation` (`ui_fullscreen.rs:42108` の Depth 専用分岐) と、同じ pass の事前予測 `predict_fs_wheel_frame` (`42764`、**共通の占有述語 `fs_exclusive_view_active` で止めている**) | 両方とも Depth を 2D と同じに扱う (予測と実適用を同じ規則にし、同じ pass のホイールとクリックで変換が食い違わないようにする)。Ctrl+ホイールのズーム、修飾なしホイールの 2D の規則 (はみ出すときのページ内スクロールを含む) とも 2D と同じ。360° の扱いは変えない |
| キャンバスのドラッグ (`42551`) | Depth の分岐を消し、2D のパン・Ctrl+ドラッグ回転・境界つきパンを通す |
| クリックの分類 (`42892`) | §3.3 |
| ゲームパッドのスティックのズーム・パン (`apply_gamepad_fullscreen_zoom` / `_pan`、`16021`・`16043`) | 占有の判定を「360°だけ止める」にする (通常の `fs_zoom` / `fs_pan` を動かす操作) |
| リング (`gamepad_input.rs::apply_ring_action`、`8364`〜) | **今の分類を維持する**。リングで Depth 中に止めている `ImageZoomMode` は Z の全画面ズームモードで通常の倍率ではない (`8378`・`8816`)。pixel grid・比較 pin の抑止、metadata = 3D パネル切替も変えない (R1 の「360°だけ止める」はリングには当てはまらない。独立レビュー P2) |
| タッチのタップ領域 (§6.5 の表の `37999` 相当) | 2D と同じ。3D パネル上のタップの除外は維持 |
| ページ内スクロール・N 字 / Z 字スクロール (`in_page_scroll.rs:361` `fs_in_page_scroll_surface`) | 判定が `fs_zoom_owner() == Image` なので、owner の変更で 3D 中も 2D と同じく使える (はみ出す表示のとき)。マニュアルの「3D には使いません」を直す |
| ページ切替後の表示位置「読み始め」(`ui_fullscreen/initial_placement.rs`) | 3D 中も 2D と同じく適用する。今は 3D が `zoom_pan = None` で描くため結果が描画に出ないだけと読める。実装担当は Depth を除外している箇所があるか確かめ、あれば外す。マニュアルの「3D 写真には適用しません」を直す |

**3D 固有に残す** (今のまま): 3D パネル (I / Tab)、修飾なしクリック・タップのページ送り (変換が無いとき。2D と同じ)、
Ctrl+クリックの止まる深さ、**フィットの設定の変更を受け付けない** (循環キー・上部バーのメニュー・ゲームパッドのピッカー。§3.4 の受付の境界。3D は利用者のフィットの設定で描くが、3D 中に変えない)、紙めくりなし、Z の全画面ズームモードを使わない、編集系・比較・ルーペ・分析・補正パネルの
占有、ナビゲータを出さない (§4.5)、360° との排他。

実装担当は、`is_depth()` / `ExclusiveViewKind::Depth` / `FsZoomOwner::Depth` / `FsExclusiveView::Depth` の全出現と、
**共通の占有述語 `fs_exclusive_view_active(..).is_some()` の全出現** (Depth と 360° を区別せずに止めている箇所。識別子の
検索では Depth と分からない) を上の 2 分類 + 「360° 固有のまま」で表にして本書の実装記録に残す (段 B の §6.5 の表と同じ形式)。
360° の挙動は変えない。

### 3.5.1 配置の確定 — Depth も 2D と同じ確定を通す

案 A では 3D の変換に初期配置 (「読み始め」) と描画時のパン補正が効くので、**Depth が描く場合も、2D と同じく
採用した変換で `fs_pan` を確定する**。今の `try_paint_depth` の位置 (`ui_fullscreen.rs:46470`、確定より前で return) の
ままだと、画面は読み始めの位置で描かれるのに `fs_pan` は未更新で、次の入力 (ドラッグ・ホイール・クリックの分類) が古い
`fs_pan` を基準にする (独立レビュー P2)。

- **境界: 採用する素材が決まってから、共通の確定へ進む**。単ページ描画の流れを次の順にする。
  1. 変換の解決 (初期配置の提案を含む)。今と同じ。
  2. **採用する素材を決める**:
     - **Depth で描ける場合**: `try_paint_depth` の今の前提 (`depth_view_may_continue`、wgpu の描画状態、`state.ready()`、
       資源の鍵 `resource.key` が payload の元テクスチャと一致、`depth_view.rs:1211`〜`1222`) を、描く前の**判定**として
       切り出す。満たす場合だけ「Depth を採用、素材は `Complete`」とする (Depth の Ready は完成した元テクスチャ、§4.1)。
       この場合は 2D の payload 準備 (`prepare_fs_paged_paint_payload`、GPU 縮小を作る) を行わない。
     - **それ以外 (2D)**: 今の 2D の準備・パン補正後の再準備・失敗時の fallback (`continue`)・provisional のときの補正候補の
       破棄と初期配置の要求の保持 (`ui_fullscreen.rs:46495`・`46550`) を、**すべて確定より前に今のまま**残す。
  3. 描画時のパン補正の候補を出す。適格条件 (`fs_draw_pan_eligible`・素材が `Complete`・page turn の held page でない・
     通常の placement、`46524`) は 2D と同じ条件を Depth にも使う (Depth は紙めくりの対象外なので held page に当たらない)。
     Depth の場合、補正後の変換に対して 2. の判定を同じ pass でやり直す必要は無い (判定は変換に依存しない) が、
     描画は補正後の変換で行う。
  4. **確定** (`resolve_fs_initial_placement` / `commit_fs_draw_pan`) を、Depth と 2D で共通の 1 つの関数で行う。
  5. 描く (Depth なら `try_paint_depth` の描画部分、2D なら今の描画)。Depth の描画が 2. の判定の後で描けなかった場合は
     無い (同じ pass・同じ前提) ことを assert で守る。
- パン補正は変換だけから決まる (`draw_pan_correction`) ので、Depth と 2D で同じ値になる。
- 先読み済みのページへ移って、最初の描画から Depth が Ready で描く場合も同じ確定を通る (回帰テスト対象、§6)。
- 2D の挙動は変えない (2. の 2D 側は今の処理の順と条件のまま。確定の関数へ切り出すだけ)。切り出しで 2D の確定条件を
  保てない形と分かったら、実装担当は止めて設計担当へ返す。

### 3.6 書き出し — 既存の出力仕様を変えない

**書き出しには 2D の拡大・パン・自由回転・フィット設定を入れない。** 出力は今までどおり、画像全体
(表示の切り取りの範囲)・90 度回転・パネルの動きの設定で作る。

- 今は表示の変換がたまたま「Page fit・拡大なし・自由回転なし」なので、`record_display` に表示の変換を渡せば
  書き出しの基準になった。§3.1 で表示が 2D の変換に変わるので、**書き出し用の基準の変換を別に作って渡す**:
  描画の呼出元で、今まで 3D が使っていた入力 (fit = Page、`zoom_pan = None`、自由回転 0、フィットの制限なし、
  同じ切り取り・90 度回転) で `DisplayedImageTransform` を解決し、`record_display` はそれから `uv`・90 度回転・
  大きさ (市松の比) を写す。画面に描く変換は書き出しに使わない。
  - 自由回転中は表示の切り取り (`content_bbox`) が描画に効かない (`effective_bbox`) が、書き出しの基準は自由回転 0
    なので今までどおり切り取りが効く。
- 書き出しダイアログの文言・選択肢は変えない。「表示中の範囲・角度で書き出す」は別の要望として扱う (§8 の質問 2)。
- §17.4 のとおり、止まる深さのハイライトは書き出しに入らない (変えない)。

## 4. 所有と寿命

### 4.1 メイン・F12・独立窓

- 新しい状態を足さないので、所有は既存のまま: `fs_zoom` / `fs_pan` / `fs_free_rotation` は `ViewerContextBundle` の
  swap 対象 (`viewer_context_registry.rs:4258` 付近)、Depth は context 所有 (§6.4)。どの presentation でも同じ
  `render_fs_body` の通常分岐で描く。
- F11 / F12 / 独立性の実切替で Depth を終える規則 (§6.3、`commit_viewer_session`) は変えない。2D の変換の受け渡しも
  既存のまま (3D 側で足さない)。
- **detached の述語・viewport の選択は変えない**。変更は zoom の持ち主・入力の分類・単ページ描画の確定順 (§3.5.1)・
  Depth の描画・書き出しの基準・PDF の表示需要 (§3.4) で、どれも presentation によらず同じ `render_fs_body` の経路。
  Depth の唯一の持ち主 `set_fs_exclusive_view` には副作用を足さない (§3.4 の 3.)。F11 / F12 / fork で Depth が終わる経路の
  影響は、context と一緒に移る PDF の印 (§3.4) と、2D の変換が今までどおり context に残ることだけで、これらは
  `ViewerContextBundle` の既存の swap 対象の中で完結する。R1 の独立レビューは「detached の述語・viewport 選択自体を変える
  必要は無い」と確認した。実装の独立レビューでも、述語・viewport 経路に触れていないことを確認項目にする。触れていると
  判定された場合は CLAUDE.md の「Detached viewer リワーク中のルール」(双方のレビューで構造的な修正と合意、
  detached-rework-plan.md へ記録) に従う。

### 4.2 ページ移動・スライドショー

- ページを移ったときの 2D の変換の扱い (初期配置・倍率の保持・自由回転の解除など) は 2D の既存の規則をそのまま使う。
  3D 側で変換を初期化・保存しない。初期配置とパン補正は §3.5.1 の共通の確定を通る (先読み済みのページで最初から Depth が
  Ready の場合を含む)。
- 3D のスライドショー ([depth-slideshow-plan.md](depth-slideshow-plan.md)) の先読み・待ちは変えない。

### 4.3 Z の全画面ズームモード

- 今と同じく 3D 中は使えない (入口条件の `fs_zoom_mode_context_ok` で弾かれる)。Z モードを確定したまま 3D に入った場合も
  今と同じく休止し、3D を抜けると戻る。§3.5 の owner の順 (Depth を先に見る) でこれを保つ。
- 採らなかった案: Z モードを 3D 中も有効にする。Z の照準はポインタ位置で表示を動かし、マウス追従もポインタで視差を
  動かすので、同じポインタに 2 つの意味が重なる。今回の要望 (拡大・縮小・回転) は通常のズームで満たせる。

### 4.4 マウス追従とドラッグの重なり

- パン・回転のドラッグ中もマウス追従は動く (ポインタの位置で視差が変わる)。ドラッグ中だけ止める状態は足さない。
  操作感は利用者の実機確認で見る (§7)。違和感があれば次の段で「ドラッグ中は目標を固定」(パネル上と同じ扱い) を検討する。

### 4.5 ナビゲータ

- 3D 中は今と同じく出さない (§6.5 の表の `37258`・`37328`)。今までの理由 (保存した 2D の変換を守る) は無くなるが、
  ナビゲータは自分の入力の除外 (`navigator_exclusion`) を持ち、3D パネルの除外とは別の規則になっている
  (`FullscreenSidePanelClickChrome::contains_pointer`)。v5.7.0 ではこの組み合わせを増やさない。
  拡大中の位置が分かりにくいという声が出たら別のバックログで扱う。

## 5. まれな失敗

既定の割り切り (§13)。この変更で新しい非同期処理・保存・失敗の経路は増えない。

## 6. テスト

- **深度を作り直さない**: 3D の Ready 中に、Ctrl+ホイール・キー・ピンチのズーム、パン、Ctrl+ドラッグ回転、
  ダブルクリックのリセットを行っても `DepthRequest` が増えず、`DepthSourceKey` が変わらない。PDF のページ (legacy フィットと
  指定倍率フィットの両方) でズームしても `maybe_rerender_pdf` / `reconcile_specified_pdf_demand` が需要を出さず、需要の記録も
  変わらない。AI 用 native 再ラスタは Depth 中も起動し (先読み・合成時の保証を含む)、その起動可否が Depth 中の倍率で変わらない。
  Depth 中は、フィットの循環 (キー・リング)・上部バーのズーム / フィットのメニュー (方式・指定倍率・拡大しない / 縮小しない)・
  ゲームパッドのピッカーのフィットの行 (プレビュー・確定) のどれでもフィットの設定が変わらない。`set_fullscreen_fit_mode_for_current`
  も Depth 中は何もしない。環境設定の画面からの変更は受け付ける。
- **PDF の印の消費**: Depth 中にズームしてから、(a) D で 2D へ戻る、(b) F12 で Depth が終わる、(c) Z モードを確定したまま
  入っていた場合に 2D へ戻る、(d) 指定倍率フィットで入場前と同じ需要へ戻る、のそれぞれで、Depth でない最初の 2D 描画が
  記録を空にしてから実効の変換の倍率で需要を出し (今のラスタで足りれば発行先が省く)、印が消える。Depth 中の描画では印が
  消えない。`set_fs_exclusive_view` は PDF の需要を出さない。
- **配置の確定**: 「読み始め」の設定で、先読み済みの縦長ページへ 3D のまま移り、最初の描画から Depth が Ready で描く場合に、
  画面の位置と `fs_pan` が一致する (次のドラッグ・ホイールが描いた位置から始まる)。描画時のパン補正も Depth と 2D で同じ値で
  `fs_pan` に確定する。Depth で描く pass では 2D の payload 準備 (GPU 縮小) を行わない。
- **ホイールの予測**: 3D 中の同じ pass のホイール (Ctrl あり・なし) とクリックで、`predict_fs_wheel_frame` の予測した変換と
  実適用が一致する。
- **シェーダの検証**: 画面用・書き出し用の pipeline が wgpu の検証を通る (微分が一様な制御フローにある)。
- **描画の位置 (GPU)**: 拡大 (画面より大きい)・パン・自由回転 (正負の角度、90 度回転・切り取りとの組み合わせ) で、
  offset 0 の 3D の出力が同じ変換の 2D の `paint_texture` の出力と許容誤差内で一致する (縁を隠す拡大のぶんは比較時に
  合わせる)。四角形の外は透明で、clip の外に描かない。callback の矩形が画面外へはみ出しても歪まない。
- **動きの向き**: 自由回転 θ のとき、左右の動きの画面上の変位が画面の左右になる。
- **止まる深さ**: 拡大・パン・自由回転・90 度回転・切り取りの各組み合わせで、求めた `focus` で描くとクリックした画素が
  offset ± で動かない (§17.6 と同じ許容誤差)。CPU の写像とシェーダが同じ純粋関数の値を使う。
- **クリックの分類 (handler-level)**: Depth + 変換あり + 修飾なしクリック → ページを送らない。Depth + 変換あり +
  Ctrl+クリック → `DepthFocusAt`。Depth + 変換あり + ダブルクリック → `ResetTransform`。Ctrl+クリックで自由回転が
  変わらない。変換なしの既存の結果 (§17.6) は変わらない。
- **入力の持ち主**: Depth 中の各ズーム・パンの入力が `fs_zoom` / `fs_pan` を変える。3D パネル上で始めたドラッグ・
  Ctrl+ホイール・ピンチは変えない。Z モードを確定したまま 3D に入って抜けると Z モードが戻る。360° の既存テストは変わらない。
- **マウス追従**: 画像全体が見えているとき (片側だけの見開き配置を含む) の中心が今と同じ画像の中心。拡大・パンで画像の
  中心が画面外にあるとき、中心は外接矩形の可視範囲の中心で、そこにポインタを置くと offset 0。自由回転では外接矩形を使う。
- **リング**: Depth 中の `ImageZoomMode`・pixel grid・比較 pin の抑止と metadata = 3D パネル切替が今のまま。
- **書き出し**: 2D の拡大・パン・自由回転・フィット設定を変えて作った書き出し要求の `uv`・90 度回転・市松の比が、
  変換なしで作った要求と一致する。既存の画面と書き出しの画素一致の検査 (§7.2) は通る。
- **入退場**: 2D で拡大・回転してから D で入ると同じ見え方で 3D になり、3D 中に変えた倍率・回転が抜けた後も残る。
- 既存の depth 関連テスト (`test(/depth/)`)・クリック・タッチ・360° のテストを流す (§17.9 と同じ filter)。
- UI スナップショット: パネルは変えないので差分が出ない見込み。出たら目視して理由を確かめる。

## 7. 実機確認 (利用者)

両 presentation (MainWindow・Fullscreen) と F12 の別ウィンドウで: Ctrl+ホイール・ピンチでの拡大縮小、ドラッグのパン、
Ctrl+ドラッグの回転を、自動の動き・マウス追従の両方で行い、動きが続くこと・画像の外が覗かないこと・パネルと重なる部分の
見え方。拡大中の Ctrl+クリックで止まる深さが決まること。PDF で拡大したときの細かさ (3D 中は再ラスタしないので 2D より
柔らかく見える場合がある) と、3D を抜けた後に細かさが戻ること。ドラッグ中のマウス追従の違和感 (§4.4)。書き出しが
今までどおり画像全体になること。

## 8. 利用者に決めてもらう点 (推奨を先頭に)

1. **3D 中の拡大・回転を 2D の表示状態と共有するか**
   - **推奨 (案 A)**: 共有する。2D で拡大していれば 3D もその見え方で始まり、3D 中に変えた拡大・回転は 3D を抜けても残る。
     フィット設定 (幅に合わせる等) と「読み始め」の初期位置・ページ内スクロールも 3D 中に効くようになる (フィットの切替だけは
     3D 中は受け付けない。キーの循環と上部バーのメニューは今も 3D 中は使えないが、ゲームパッドの設定ピッカーのフィットの行は
     今は 3D 中も設定を変えられ (3D の表示には出ず、抜けた後に効く)、これを 3D 中は変えられないようにする)。操作の規則が 2D と
     同じになるので、**拡大・パン・回転している間は、2D と同じく修飾なしのクリックではページを送らない** (ダブルクリックで
     元に戻すと送れるようになる)。ホイールも 2D と同じく、はみ出している間はページ内をスクロールしてから送る。今の 3D は
     常に画面全体に合わせて表示するので、クリック・ホイールはいつもページ送りになっている。3D 専用の状態は足さない。
   - 案 B: 3D は今までどおり画面全体に合わせて始め、3D 中の拡大・回転は 3D を抜けると捨てて 2D の状態に戻す。
     ホイール・ピンチ・中ボタン・キー・スクラブ・ゲームパッド・ドラッグのパン・回転・リセット・初期配置・PDF の需要の
     すべてに 3D 用の書き先と規則が要り、案 A より明らかに大きい。
2. **書き出しに拡大・回転を反映するか**
   - **推奨**: 反映しない (今の出力仕様のまま、画像全体・90 度回転だけ)。表示中の範囲・角度で書き出す機能が欲しければ、
     別のバックログにする。
   - 反映する: 書き出しの寸法・縦横比・切り取りの決め方を新しく設計する必要があり、v5.7.0 には入れない。

### 8.1 利用者の判断 (2026-11-23、統括の AskUserQuestion への回答)

- 質問 1 (共有): 「2D と共有する (推奨)」→ 案 A。
- 質問 2 (書き出し): 「反映しない (推奨)」→ 書き出しは今の出力仕様のまま。
- 統括からの追加の指示: 拡大・回転している間はクリックでページを送らないなど、3D の操作の規則が変わる点をマニュアルと
  CHANGELOG の下書きに書く。途中で共通化できない形と分かったら、その時点で見積もりを返す (v5.7.0 から外すかを利用者と決める)。

## 9. 規模と見積もり

- 変更の中心: `ui_fullscreen.rs` の Depth の特別扱いと共通の占有述語の分類 (入力・ホイール予測・クリック)、単ページ描画の
  確定順の組み替え (§3.5.1)、`depth_parallax_wgpu.rs` のシェーダと写像、`app/depth_view.rs::try_paint_depth`、マウス追従の中心、
  `record_display` の呼出元 (書き出しの基準の変換)、PDF の需要の 2 入口と `PdfRerenderDemand` の印・消費 (§3.4)。
  新しい型・非同期処理・設定項目・bundle の field は無い。
- 見積もり (案 A・書き出し不変): 中程度。重いのは §3.5.1 の確定順の組み替え (2D の確定条件を壊さない共通化) と §3.4 の
  PDF の需要。実装 1 回 + 独立コードレビュー 1〜2 回、関連テスト、文書。v5.7.0 の残り日数次第で、収まる見込みは
  あるが確約はできない (コードからは期限まで裏付けられない。独立レビューの指摘)。実装の途中で §3.5.1 を共通化できない
  形と分かった場合は、その時点で設計担当が見積もりを返す。
  案 B は入力・配置・PDF の全経路に 3D 用の書き先が要り、案 A より明らかに大きい。

## 10. 文書

- [depth-parallax-plan.md](depth-parallax-plan.md) の §5.2 (3D は 2D の変換で描く)、§5.3・§17.3 (マウス追従の中心)、
  §6.5 (ズーム・パン・ホイール・クリックの扱い、§10 の分類表への注記)、§7.1 (書き出しの基準の変換)、§17.2 (写像) を本書に
  合わせて直す。
- マニュアル `htdocs/mimageviewer/manual/fullscreen.html`: 3D 写真の節 (`#depth-photo`) に、通常表示と同じ操作で
  拡大・縮小・パン・回転でき、動きは続くこと、書き出しは画像全体のままであることを書く。ページ内スクロール (341 行) と
  「読み始め」(504 行) の対象外の列挙から 3D を外す。製品ページの 3D のカードは要約なので、文言が今も正しいかだけ確かめる。
- `docs/keymap-spec.md` の 3D 写真の節: Ctrl+ホイールのズームを止めていた記述を外す。
- `docs/display-pipeline.md` の 3D の記述があれば合わせる。CHANGELOG の下書き。

## 11. 独立設計レビュー

### R1 (2026-11-23、GPT-6.1 Sol / xhigh、read-only、対象 `84e50e96c`)

判定「指摘修正後に実装開始可」。P1 2 件・P2 4 件。設計担当がそれぞれ根拠のコードを確かめ、すべて採って R2 に直した。

| 指摘 | 対応 |
| --- | --- |
| P1 PDF の再推定の抑止は `maybe_rerender_pdf` だけでは成立しない (指定倍率の毎フレームの絶対需要 `ui_fullscreen.rs:33076`、AI 用 native の倍率条件 `app.rs:94634`)。「フィット設定でも要求しない」と「フィット由来は変えない」が矛盾 | §2 に 3 系統を列挙し、§3.4 で境界を確定: 倍率・絶対寸法の需要は発行の 2 入口で止めて記録を更新しない、AI 用 native は止めずに倍率条件を外す、読み込み時の解像度は変えない。矛盾した文を削除 |
| P1 画像外の途中 return の後に `dpdx` / `dpdy` を置けない (WGSL の一様性) | §3.2 で微分を先頭、画像外は最後の `select` で透明。書き出し pipeline も同じ WGSL で検証 (§6) |
| P2 Depth の描画は初期配置・描画時パン補正の確定より前で return する (`46470`・`46523`・`46585`) | §3.5.1 を追加。確定を Depth と 2D で共通の 1 か所にし、その後で描く。先読み済みページの最初から Ready の着地を回帰対象に |
| P2 ホイールの事前予測 `predict_fs_wheel_frame` が共通の占有述語で止めている。リングの `ImageZoomMode` は Z モード | §3.5 に両ホイール関数を追加、監査対象に共通の占有述語と `FsExclusiveView::Depth` を追加。リングは今の分類を維持 |
| P2 抜けるときの `maybe_rerender_pdf(fs_zoom)` 1 回では Z 復帰の倍率・需要の重複除外・確定前の setter 呼び出しを表せない | §3.4 の 2.・3.: 抑止の印を context 所有の `PdfRerenderDemand` に残し、Depth でない最初の 2D 描画が実効の変換で消費する。setter には副作用を足さない。§4.1 の説明を具体化 |
| P2 マウス追従の中心をキャンバスの中心にすると片側だけの見開き配置で操作感が変わる。質問 1 にクリック・ホイールの規則の変化も明示を | §3.3 で中心を「画像の外接矩形 ∩ キャンバスの中心」に変更 (画像全体が見えていれば今と同じ)。§8 の質問 1 にクリック・ホイールの変化を明記。§9 の見積もりを確約しない形に直した |

確認済みとされた点: 案 A の所有方針、PDF 以外の素材選択が倍率に依存しないこと、描画の幾何 (絶対フラグメント座標・
外接矩形 ∩ clip・offset の回転と aspect の順)、Ctrl+クリックの順序、書き出しの基準変換、§8 の 2 つの基本判断。

### R2 (2026-11-23、同じセッションの継続、対象 `ea7f9da39`)

判定「指摘修正後に実装開始可」。R1 のシェーダ・入力分類・追従中心は解消。PDF と配置の確定に P1 2 件・P2 2 件・P3 1 件。
PDF は指摘のたびに別の経路が出る形になりかけたので、経路を足すのではなく組み合わせを減らして直した (CLAUDE.md「設計の簡素化」)。

| 指摘 | 対応 |
| --- | --- |
| P1 読み込み時の解像度 (`fs_pdf_display_target` → `ensure_pdf_display_resolution` → `request_pdf_rerender_at_target`) にフィット設定が入り、止める 2 入口を迂回する | **3D 中はフィット切替を今と同じく受け付けない**ことにし、3D 中にフィット設定が変わる組み合わせを無くした (§3.4・§3.5)。R2 の「フィット切替を 2D と同じく許す」行を削除 |
| P1 `request_pdf_rerender` で一律に止めると AI 用 native 再ラスタ (通常・保証・先読み) も止まり、AI の保留が完成を待ち続ける | 止める場所を発行入口ではなく、2D の倍率から需要を作る呼び出し元 2 か所 (`maybe_rerender_pdf`・`reconcile_specified_pdf_demand`) にした。AI 用は止めず、共通の `pdf_native_rerender_pending` で倍率条件だけ外す (§3.4) |
| P2 指定倍率で「記録を更新しない」だけでは、入場前と同じ需要へ戻ると古い記録で捨てられる | 印の消費時に需要の記録をすべて空にしてから既存の照合に任せる (§3.4)。「入場前と同じ需要へ戻る」を §6 に追加 |
| P2 §3.5.1 が、確定より前に要る素材の採用条件 (準備・再準備・fallback・provisional) を決めていない | §3.5.1 を「採用素材が決まってから共通の確定へ進む」5 段に書き直した。2D の準備・再準備・fallback・provisional の扱いは確定前に今のまま。Depth は描く資源の鍵の一致まで確かめた場合だけ `Complete` で合流 |
| P3 「見えている部分の中心」は外接矩形による近似 | §3.3・§6 を「外接矩形の可視範囲の中心」とし、実画像内の中心は保証しないと明記 |

### R3 (2026-11-23、同じセッションの継続、対象 `5cb395b59`)

判定「指摘修正後に実装開始可」。R2 の AI 発行 P1・需要記録 P2・配置確定 P2・追従中心 P3 は解消。P1 1 件・P2 1 件。

| 指摘 | 対応 |
| --- | --- |
| P1 ゲームパッドの設定ピッカーのフィットの行 (プレビュー・確定) が `set_fullscreen_fit_mode_for_current` を直接呼び、3D 中にフィット設定を変えられる | §3.4 に「全画面のフィットの設定 4 種を 3D 中に変える閲覧操作を受け付けない」受付の境界を定義し、入口 (循環・上部バーのメニュー・ピッカー) と共通 setter の防御、環境設定の画面の例外、全書き込み箇所の列挙を書いた。上部バーのメニューは今も 3D 中は出ない (コードで確認)。ピッカーだけが今は 3D 中も変えられるので、その差分を §8 の質問 1 に明記。§6 に回帰検査を追加 |
| P2 R3 の差分で §3.6 (書き出しの基準変換) が消えていた (設計担当の一括置換の範囲の誤り) | R2 の §3.6 をそのまま復元 |

### R4 (2026-11-23、同じセッションの継続、対象 `65aaf97b6`)

判定「**実装開始可**」。R3 の P1 (フィットの設定の受付境界) と P2 (§3.6 の復元) は解消。新しい P1 / P2 / P3 なし。

## 12. CHANGELOG の下書き

未作成。§13 のホイールの前提の食い違いを設計担当へ返し、文言と仕様が決まってから作成する。
`CHANGELOG.md` は変更していない。

## 13. 実装記録

2026-11-23、`depth-zoom-874`、基点 `a392c9b83`。**未完了。前提の食い違いにより停止した未コミット差分。**
コンパイル・テストは実行していない。製品の起動・vendor の変更・コミット・push・ブランチ切替は行っていない。
設計担当が実行する予定の検査へ渡す前に、以下の設計判断と残作業が必要。

### 停止理由: 修飾なしホイールの既定動作

§3.5・§8 と依頼文の「はみ出している間はページ内をスクロールしてから送る」を、すべての設定に対する
挙動として実装すると、現行 2D と同じにはならない。

- `src/settings.rs::FullscreenWheelAction` の既定は `PageTurn`。
- `src/ui_fullscreen/in_page_scroll.rs::fs_wheel_segment_in_page_scroll` は、設定が `ScrollThenPageTurn` の場合だけ
  同名の分類を返す。`PageTurn` は `Legacy` へ進む。
- `src/ui_fullscreen.rs::apply_fs_wheel_frame_proposal` は、その `Legacy` の修飾なしホイールを
  `spread_page_nav` に渡す。`spread_page_nav` の単ページ分岐は直接 `FsPageNav::Delta` を返す。
  拡大によるはみ出しを見てスクロールへ変える判定は、この経路にはない。
- 既存マニュアルの「端まで読んだらページを送る」も、「スクロール＋ページ送り」を選んだ場合の動作として説明している。

設計担当への判断事項: (A) 2D と同じくホイール設定に従い、マニュアル・下書きの説明を条件付きにするか、
(B) 3D のホイールを設定によらずスクロール優先にして 2D と異なる規則を持つか。
A はコードの分類を共有でき、説明とテストの設定を明示する修正が中心。B は共有方針の変更であり、
設定との優先順位・入力の予測と実適用・既存操作の変更を再設計する必要がある。実装担当では選択していない。

### 途中差分

| ファイル | 入れた変更 |
| --- | --- |
| `src/ui_fullscreen.rs` | 2D と同じ変換と zoom owner、Depth の入力抑止の撤去、クリック分類の順序、パネル上での開始除外、PDF 需要の抑止と復帰、素材選択後の配置確定、書き出し基準の別解決 |
| `src/app/depth_view.rs` | Depth の採用判定を描画から分離、可視外接矩形の追従中心、共有写像による深さの逆引き、callback の外接矩形と clip の交差 |
| `src/depth_parallax_wgpu.rs`、`src/depth_parallax_wgpu/export.rs` | 絶対フラグメント位置からの逆写像、80 バイト uniform、微分後に画像外を select で透明化、offset の逆回転、書き出しには全体矩形・回転 0 を指定 |
| `src/depth_parallax.rs` | MouseFollow に呼出元が求めた中心を渡す |
| `src/app.rs` | PdfRerenderDemand の Depth 抑止の印、復帰時の比較記録の破棄、AI native の倍率条件の例外 |
| `src/app/gamepad_input.rs` | ピッカーのフィット行の値変更・一覧・プレビュー・確定の受付を Depth 中は止める |
| `src/ui_fullscreen/depth_view_transform_tests.rs` | 共有状態・クリック・配置・PDF 抑止・ホイール予測・パネルの入力除外のテストを追加 |
| `src/depth_focus_sampling_tests.rs` | GPU の止まる深さの比較に自由回転を追加 |
| `src/ui_fullscreen/depth_focus_tests.rs` | 書き出し基準変換を渡す引数へ追従 |
| `src/ui_fullscreen/depth_tests.rs`、`src/app/depth_view_tests.rs` | Depth が Image の zoom owner を使う期待値へ変更 |
| `src/ui_fullscreen/initial_placement/tests.rs` | Depth を読み始めの対象外から外す期待値へ変更 |
| `src/app/depth_main_window_tests.rs` | fit 固定から共有倍率と clip された callback の期待値へ変更 |
| `src/app/depth_export.rs` | 表示倍率・パン・自由回転・フィット設定が書き出し基準を変えないテストを追加 |
| `src/app/tests.rs` | AI native の既存テストに Depth 中の倍率非依存の条件を追加 |
| `docs/depth-parallax-plan.md`、`docs/display-pipeline.md`、`docs/keymap-spec.md` | 変換・入力・PDF・書き出しの説明の途中更新 |
| `htdocs/mimageviewer/manual/fullscreen.html` | 操作・クリックの規則・共有状態・フィット・書き出しの説明、ページ内スクロールと読み始めの対象外から 3D を削除。ホイールの説明は上の判断待ちで未確定 |

既存テストの期待値を変えた理由は、fit 固定・変換入力停止・読み始め対象外という旧仕様を、本書の共有状態へ
変更するため。Z の休止、紙めくりとナビゲータの抑止、360° の規則の期待値は維持する。

### §3.5 の分類表 (途中)

**全出現の分類は未完了。** 以下は変更対象の分類のみ。共通の占有述語を含む全出現の一覧を追加する作業が残る。

| 箇所 | 分類 | 途中差分の扱い |
| --- | --- | --- |
| `fs_zoom_owner` と `FsZoomOwner::Depth` の dispatch | 2D と同じにする | Image に統合。Depth の判定は Z より先に維持 |
| 単ページ描画の `zoom_pan`・自由回転・フィット設定・倍率制限 | 2D と同じにする | Depth の強制値を撤去 |
| `apply_gamepad_fullscreen_zoom` / `_pan`、`predict_fs_wheel_frame` | 2D と同じにする | 共通の占有抑止から 360° だけの抑止へ変更 |
| `fs_wheel_action_requests_navigation`、ホイール実適用、キャンバスドラッグ | 2D と同じにする | Depth の専用無操作分岐を撤去。ホイール仕様の記述は上記の判断待ち |
| `plan_fs_canvas_click` | 2D と同じにする / 3D 固有に残す | 変換とリセットは共通、Ctrl の深さ指定は belongs_to_view の抑止前 |
| フィット循環・上部メニュー・ピッカー | 3D 固有に残す | 閲覧操作のフィット変更を停止。環境設定の画面は例外 |
| PDF の表示需要・AI native の条件 | 3D 固有に残す | 2 入口で需要を抑止し、最初の非 Depth 単ページ描画で比較記録を破棄して再照合 |
| リング `ImageZoomMode`・pixel grid・比較 pin・metadata | 3D 固有に残す | 既存の分類を変更しない |
| パノラマの視線・FOV | 360° 固有のまま | 変更しない |

### 追加したテストと残る検査

追加したテスト (未実行):

- `depth_transform_inputs_keep_the_ready_source_and_share_the_exit_view`
- `depth_transform_click_classification_focus_and_double_reset_use_the_shared_view`
- `depth_transform_ready_first_draw_commits_reading_start_and_bounds_like_2d`
- `depth_transform_pdf_suppression_keeps_comparisons_until_the_first_2d_draw`
- `depth_transform_wheel_prediction_matches_application_for_zoom_and_local_scroll`
- `depth_transform_panel_rejects_wheel_and_drag_starts`
- `depth_transform_mapping_roundtrips_fractional_geometry_and_physical_uniforms`
- `depth_transform_gpu_matches_2d_zoom_pan_rotation_crop_and_clip`
- `depth_fit_picker_row_rejects_preview_value_list_and_commit`
- `depth_pdf_demand_resume_retires_all_absolute_records_and_survives_zoom_reset`
- `depth_export_baseline_ignores_shared_zoom_pan_free_rotation_and_fit_policy`

特にホイール予測の追加テストは、既定設定のままでスクロールを期待しており、停止理由を解決してから修正が必要。
§6 の GPU の動きの方向、パネル上のピンチ・中ボタン・スクラブ、PDF の実描画での印の消費と各終了経路等の
検査の追加・精査も残る。§3.4 の要求元とフィット設定書き込みの全一覧、§3.5 の全出現分類、§12 の下書きも未完了。
detached の述語・viewport 配送・`set_fs_exclusive_view` の実装は変更していない。

設計判断後、設計担当が実行する予定の nextest filter:

```text
test(/depth/) | test(/pdf_native_rerender_decision_tracks_app_state/) | test(/specified_pdf/) | test(/initial_placement/) | test(/fullscreen_click|canvas_click|touch|panorama/)
```

パネルのスナップショットは更新していない。ビルド・テスト・実機の結果はこの差分にはない。
