# 通常動画のズームとパン 実装計画

作成: 2026-09-04  
対象: backlog §1.167  
状態: 実装済み、Windows 実機表示の確認待ち。本書を本機能の設計・実装状況の正本とする。

## 1. 目的と不変条件

通常（非 360）動画をフルスクリーンで拡大・パンできる表示モードを追加する。
入口は既存の `KeyAction::FsPanorama`（既定 V）に統一し、360 判定が付く動画では従来の
360 モード、付かない通常動画では本モードを切り替える。`ini_name()` の `FsPanorama` は
出荷済み keymap との互換のため変更しない。

- モードは動画キャンバスのホイールと左ドラッグを所有する。シークストリップ、端パネル、
  モーダルなど `pointer_region_owns_wheel` が所有する領域からはホイールを奪わない。
- 再生、上下バー、シークストリップ、情報パネル、動画補正は継続する。静止画 360 の
  機能制限モードとは扱いを分ける。
- 360 と通常動画ズームは素材判定で排他にし、同時に active にしない。
- モードへ入った時だけ表示領域全体の surface へ切り替える。倍率・中心の変更は定数更新だけで
  表し、ホイールごとに swap chain / buffer を resize しない。
- fit（100%）では従来と同じ縦横比で中央表示し、レターボックス部分は黒にする。
- 項目変更、フルスクリーン終了、動画の音声モードへの遷移、音楽 VST シェルへの遷移で
  state を破棄する。360 の `panorama_intent` に相当するセッション越しの意図は持たない。
- タッチ pinch とリセット専用 `KeyAction` は初回対象外。`accepts_pinch` は変更しない。

## 2. 状態と純ロジック

GPU、`App`、Win32 に依存しない `src/video/zoom_view.rs` を置く。

- `VideoZoomState`: fit を 1.0 とする `scale`（1.0〜16.0）と、向き補正後 source 上の
  正規化中心を所有する。初期値と `reset()` は 100% / 中央。
- `VideoZoomSourceRect`: 向き補正後 source pixel 座標の `origin` / `extent`。
- source geometry は raw 寸法、orientation、SAR から向き補正後の pixel 寸法と pixel aspect
  を作る。90/270 度では SAR の効く軸も入れ替える。
- fit 矩形は、表示領域を source の display aspect で割った範囲を source pixel 座標へ戻す。
  一方の extent は source と一致し、他方はレターボックス分だけ source より大きくなるため、
  origin が負になり得る。拡大・パンも同じ矩形で表す。
- wheel は `new_scale = old_scale * 1.2^(delta / 120)` を上下限へ丸め、pointer の表示領域内
  比率に対応する source 座標を更新前後で一致させて中心を更新する。無効値と zero delta は
  state を変えない。
- drag は表示領域上の point 差を現在の source extent 比へ変換し、画像を掴んで動かす向きに
  中心を移す。
- extent が source 以上の軸は常に中央へ戻す。拡大軸は source 矩形が画像から完全に外れない
  （少なくとも source の 1 pixel 幅が残る）範囲へ中心を clamp する。

単体テストは、wheel の固定点不変、pan clamp の両端、横・縦レターボックスの fit 矩形、
SAR / 軸入替、scale 上下限を固定する。

## 3. App と入力の所有境界

`App` に `video_zoom_state: Option<VideoZoomState>` を `panorama_state` の隣へ置く。
`src/app/native_video.rs` では現在の `native_video_panorama_input_active` から
「対象 `fs_idx` がフルスクリーン / 動画音声モードではない / 同じ項目の音楽 VST シェルではない」
という共通 base predicate を 1 個だけ抽出する。360 と zoom の述語は、この答えに各 active
state を重ねる。

現行 base predicate に detached 専用の分岐は無い。本変更でも detached 判定、placement、
viewport、保存状態を追加・変更せず、現在の意味をそのまま引き継ぐ。したがって detached
リワークの症状 guard ではなく、2 モードの入力可否を同じ所有境界へ集約する構造変更である。

V のキー経路と `NativeVideoOutputEvent::TogglePanorama` 経路は、共通の純粋な dispatch 判定を
使い、`360 / Zoom / NoOp` の 3 結果を返す。360 検出を常に先にし、通常動画として使える場合だけ
zoom を選ぶ。両入口を handler-level test で同じ表に対して確認する。

wheel は presenter が `NativeVideoMouseWheelEvent.x/y` と実際の video target rect から
pointer の領域内位置・領域寸法を作り、zoom 専用 command/event に載せる。drag は既存の
`NativeVideoPointerDown::PanoramaDrag` と兄弟の `ZoomPan` を使い、再生クリックへ落とさない。
App は state 更新ごとに値 snapshot を `set_native_video_zoom_state` setter へ同期し、render thread が
現在 frame の geometry と表示領域から source rect を導出する。

## 4. native presenter と描画

render thread は `panorama_pose` の隣に `VideoZoomState` の値 snapshot を持ち、frame の geometry と
表示領域から draw 時の source rect を導出する。`src/video/mod.rs` の typed
command と `VideoPlayer` setter を通して App から同期し、source switch では新 source の state を
持ち越さない。

`surface_policy::VideoSurfaceSizeInput` に `video_zoom_active` を追加し、zoom active は
`panorama_active` と同じく video target rect 全体の `DisplayResolution` surface を選ぶ。
`OsDefault` と物理 1:1 の早期 `LegacySource` より先に zoom active を判定する。zoom inactive の
入力では従来と同じ decision を返す。

resample は全方式で同じ向き補正後 source rect を使う。

- Lanczos / nearest: `ResampleConstants` と `video_resample.hlsl` に origin / extent を追加する。
  Lanczos の horizontal pass は rect の X、vertical pass は rect の Y を使う。縮小 smoothing の
  `stretch()` は source 全体でなく rect extent を使う。
- NIS: 既存 `source_origin` / `source_extent` に rect を設定する。
- Anime4K: convolution の source 全体処理は維持し、resolve の既存 `source_region` に rect を
  設定する。
- `select_video_resample_mode` へ渡す source 軸寸法は rect の実効 extent にし、zoom 中に
  拡大用 filter が縮小用 Lanczos へ誤分類されないようにする。

### 範囲外の黒

調査の結果、resample / NIS / Anime4K resolve は fullscreen triangle で最終 target の全 pixel を
上書きする。`create_swap_chain_backbuffer` やテスト helper の黒 clear、および背後の
`NativeBlackBackground` は、resolver が clamp した edge pixel を書いた後には見えない。
したがって clear 任せにはせず、各 resolver が source 中心座標を範囲判定して範囲外へ不透明黒を
返す。Lanczos / nearest HLSL、NIS WGSL、全 Anime4K variant の共通 resolve 生成元を同じ規則で
更新し、edge clamp は範囲内 pixel の filter tap にだけ残す。

### surface 上限

現行上限は長辺 8192、総画素 16,777,216。3840×2160（8,294,400）と横 2 面の
7680×2160（16,588,800）は収まり、一般的な 4K 全画面と 2 面配置には余裕がある。これを超える
巨大 span は既存の `DisplaySizeLimitExceeded` typed fallback を維持し、安全上限を広げない。
wheel / drag では target 寸法を変えないため、上限判定と surface 交換は mode enter または
表示 geometry 変更時だけである。

## 5. UI

native 上バーは通常動画 zoom active 中に `PanoramaReset` と同じ形のリセットボタンと、
丸めた `100%` / `150%` 形式の倍率を表示する。内部用語は表示しない。shortcut help には
「ホイール: 拡大縮小」「左ドラッグ: 移動」「V: 表示モードを終了」「リセットボタン」を加える。
`FsPanorama::description()` と keymap 文書は 360 と通常動画拡大の両方を指す文言にするが、
action 名と既定 V は変えない。

## 6. 実装段階と検証

1. 本計画、`docs/README.md`、`docs/video-architecture.md` の参照（計画コミット）。
2. 純ロジックと単体テスト。
3. App state、共通可否述語、V dispatch、wheel / drag、上バー / help、終了 lifecycle。
4. presenter typed state、surface policy、resample 各方式と shader、描画テスト。
5. 正本の実装結果、backlog、keymap、利用者 manual を更新。

各段階で `cargo fmt` をかける。最終確認は `cargo check -p mimageviewer --bin
mimageviewer-core`、`cargo test -p mimageviewer --lib`、`cargo fmt --check`、UI 文言変更に対する
`python scripts/check_ui_glyphs.py` を実行し、利用者確認用に `scripts/build-dev.ps1` を実行する。
D3D11 の見え方は利用者が通常 profile の検証 binary で確認する。

実装では `video_zoom_active` だけを surface / preparation signature に含め、scale と center は
定数更新に留めた。通常表示の `OsDefault`、物理 1:1、既存 filter の選択規則は変えず、拡大モード中の
`OsDefault` だけ標準 resample へ送る。Lanczos の中間 texture は従来どおり source 行数基準である。
Anime4K は生成スクリプトの共通 resolve template を正本として全 variant を再生成した。

2026-09-04 の最終自動確認では、`cargo check -p mimageviewer --bin mimageviewer-core`、
`cargo test -p mimageviewer --lib`（7,349 tests）、`cargo fmt --all -- --check`、
`git diff --check`、`python scripts/check_ui_glyphs.py`、Anime4K 生成物の `--check` がすべて
成功した。通常 feature set の `scripts/build-dev.ps1` も完了し、`target/dev-runtime/` に
実機確認用 core / remote service と FFmpeg DLL を配置した。

## 7. 実機で確認する項目

- 通常動画で V → 100% 表示、ホイール固定点 zoom、左ドラッグ pan、上バー reset、V で終了。
- 縦長 / 横長、回転 metadata、非正方 SAR で fit の黒帯と pointer 固定点が一致する。
- `OS に任せる` を含む各 scale filter で同じ rect を表示し、edge が黒へ滲まず、zoom 時に
  拡大用 filter が選ばれる。
- シークストリップ / 端パネル / モーダル上の wheel が zoom に奪われない。
- 再生・パネルを保ったまま操作でき、項目切替・fullscreen 終了・音声モード・音楽 VST
  シェル遷移後に次動画へ倍率が残らない。
- 360 動画では V が従来の 360 だけを切り替え、通常 zoom が同時に active にならない。
