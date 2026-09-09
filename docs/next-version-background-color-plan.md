# 次版: フルスクリーン余白色 実装計画

この文書は `docs/backlog-on-hold.md` §1.203 の実装境界と受け入れ条件を固定する。
次版全体の進行は `docs/next-version-development.md` が所有し、本書はフルスクリーン余白色だけを扱う。

## 1. 表示仕様

- 静止画・ZIP / PDF のページを表示するメインキャンバスの、画像の外側を共通の RGB 色で塗る。
- 既定値は黒 `[0, 0, 0]`。環境設定には黒、灰 `[128, 128, 128]`、白の早選びと、既存の RGB カラーピッカーを置く。保存中の RGB 自体がカスタム色であり、別のモードや履歴は持たない。
- 単ページ、見開き、縦 / 横連結、比較、表示トリム、90 度回転、任意角度回転、通常フルスクリーン、in-window、F12 の active / holdover / passive frozen で同じ意味にする。
- 360 度パノラマと画像分析も、静止画のメインキャンバスを使う範囲では同じ余白色にする。
- 静止画 seek strip は `strip_content` のセル外空欄を同じ余白色にする。列固定ボタンと、通常バーを隠したときの表示切替ボタンはセル上へ重ね、専用の黒い空欄幅を残さない。
- native 動画は DComp canvas の letterbox と、拡大・パン / scaling resolve の source 外画素を同じ余白色にする。native seek strip は場面サムネイル / 波形の両表示とも余白色から独立し、全域で従来の暗色下地と配色を維持する。実動画画素、素材に含まれる黒帯、字幕は変更しない。
- 音声、動画の音声モードは黒を維持する。ナビゲータ、ルーペ、HUD、左右パネル、静止画 strip の実セル背景など、部品内部の黒 / 暗色も変更しない。

## 2. 所有権

`Settings::fullscreen_image_margin_color: [u8; 3]` を永続値の唯一の所有者にする。
`App.settings` から各フレームで読むため、`ViewerContextBundle` や frozen snapshot へ複製しない。
環境設定は既存どおり編集用 `Settings` のコピーを変更し、既存の Apply / 保存経路へ渡す。
設定 DB は `Settings` の通常の `settings_kv` 保存を使い、新しいテーブルや同期 I/O を追加しない。

画像内の透明部分に敷く色は余白色とは別責務である。現在の
`fs_transparent_bg_mode` から、必ず不透明な黒 / 白 / 市松へ解決する。既定モードと
市松 texture 未準備時の fallback も明示的な黒にし、viewport の塗り色へ依存する
`Default = no fill` は廃止する。

passive frozen 表示では、透明下地を表示単位ごとに一度だけ snapshot へ capture する。
単ページと複数ページで同じ値を使い、各 frozen page へ重複して持たせない。一方、余白色は
静止画用 `DeferredDetachedImageWindowView` を毎フレーム構築するときに現在の global 設定を
投影する。動画は mounted main / active detached / ParkedLive の各 `poll_video` mount から current
player の native output へ現在値を投影し、純音声と動画の音声モードだけ黒を渡す。

native transport は `NativeVideoOutputConfig` の birth snapshot と、bar lock から独立した coalesced
`SetVideoCanvasColor` command を使う。output-local atomic が同色を除外するため、Settings I/O と GPU
clear / Present を毎 frame 発生させない。render thread は DComp root 最下層の canvas と source 外
shader 定数を同じ current RGB から作り、clear / Present 成功後だけ current 値を更新する。pause 中の
visible held frame は既存 visual-change 経路で再 present し、hidden held frame は show 時の既存経路で
再 present する。placement recreate / resize も同じ current 値を使い、別の pending state は持たない。

Lanczos / nearest、NIS、Anime4K の native resolve は source 外に同じ RGB を返す。NIS と Anime4K は
静止画とも shader を共有するため、still 側 uniform packer / layout も拡張するが、そこには従来どおり
固定の不透明黒を渡す。Anime4K は generator を正本として全 variant を再生成する。動画画素の clamp /
convolution と panorama 投影の無効画素は変更しない。

## 3. 描画 geometry

透明下地は `DisplayedImageTransform` が画像に使う最終 paint quad と同じ 4 頂点を使う。
`paint_rect` の四隅を `full_image_rect.center()` の周りに `free_rotation_rad` だけ回した頂点を
共通 helper が返し、画像 texture と透明下地の双方がその helper を使う。回転 quad の AABB は
塗らない。表示トリム時は trim 後の `paint_rect` だけを塗り、通常 texture と可視領域だけの
`paint_texture_source_region` / shader-region のどちらでも、同じ clip の下で先に下地を描く。

市松は quad の頂点だけでなく page-local UV も一緒に回す。UV の範囲は
`paint_rect.width() / CHECKER_TILE_PX`、`paint_rect.height() / CHECKER_TILE_PX`
とし、Repeat texture を 1 枚へ引き伸ばさない。比較表示など軸平行の経路も同じ quad painterへ
矩形の 4 頂点を渡す。

## 4. detached / holdover の境界

既存の session predicate、viewport ID、recreate、runtime、host、placement、focus、close lifecycle は
変更しない。active / embedded / enter / defer の holdover は、既存の typed display-unit payload と
共通の単ページ / 見開き painter を通して余白と透明下地を描く。

keepalive backstop の live still branch は、現在の単純な contain-fit `painter.image` を使わず、
単ページと両側が揃った見開きは通常表示と同じ capture 済み `FsDisplayUnitHoldover` payload を描く。
見開きの片側だけが読み込み済みで navigation holdover も無い場合は、一時的な typed
`CurrentSpread { left, right }` から通常の見開き painter を呼び、読み込み済み側と未準備側の
両 slot を維持する。古い centered-single fallback は戻さない。
連結読みは 1 / 2 ページの holdover で代用せず、通常レンダラの canonical continuous painter を
呼び、同じ visible-pages layout と共通 quad painter を使う。この選択は item 種別・音楽ビュー・
既存 continuous predicate から作る一時的な typed draw payload だけが所有する。
動画 / 音声は still payload を作らない。native 動画の canvas は current RGB、音声系の egui / native
backdrop は黒という別 owner の分類を保つ。

general keepalive と embedded deferred は、typed static display unit が実在するときだけ現在の
余白色を使い、unit が無い media transition は黒にする。静止画 viewport 入場 holdover は
静止画専用 setter / predicate が owner を保証するため、texture が未準備でも現在の余白色を使う。
passive frozen single の direct texture は常に source-region mesh を使い、保存済み回転と表示 trim の
両方がある場合も、透明下地と画像が同じ trim 後 quad を使い、direct texture の UV も trim 範囲にする。

この変更は detached の所有権や lifecycle を変えず、重複していた paint geometry を共通の
表示 payload / quad painter へ接続する構造修正である。

## 5. 受け入れ条件

- 設定キー欠落時は黒になり、任意 RGB が通常の Settings DB save / reload を往復する。
- 黒、灰、白ボタンと RGB picker は同じ 3 byte 値だけを変更し、検索から該当箇所へ移動できる。
- 90 度 4 方向、任意角度回転、表示トリムで、画像 texture と下地の quad 頂点が一致する。
- 非黒の余白色でも、透明画像の黒 / 白 / 市松は画像 quad 内だけに残る。市松は page-local に反復し、任意角度回転時も画像と一緒に回る。solid / checker / image は同じ clip を使う。
- 単ページ、見開き、縦 / 横連結、比較、source-region、holdover で同じ painter を通る。
- paused single は透明下地を失わず、paused continuous / spread は表示単位の下地を共有したまま trim / UV を保持する。
- passive static view は global RGB の変更を次の view projection で反映し、capture 済み透明下地は変えない。
- backstop の still 表示は rotation / trim / spread / continuous と透明下地を失わない。
- backstop の見開きは片側だけ読み込み済みでも通常の両 slot layout を保って読み込み済み側を描き、
  両側が揃えば capture 済み display unit を使う。canonical continuous の描画は mounted owner、
  session state、現在 index を変えない。
- general / embedded の holdover が無い media transition は黒を維持し、静止画専用 viewport 入場は
  現在の余白色を維持する。
- passive frozen single の direct texture は保存済み回転 + trim でも下地と画像の頂点・clip が一致し、
  texture UV は trim 範囲になる。
- native 動画は通常 / F12 / ParkedLive で letterbox と source 外画素へ同じ RGB を使い、birth / live
  変更 / resize / placement recreate で一致する。pause 中の visible held frame は新色で再 present し、
  hidden held frame は show 時に新色になる。同色 sync は新しい command / GPU work を発生させない。
- 純音声と動画の音声モードは黒を維持し、通常動画へ戻れば現在の RGB を受け取る。
- Resample / NIS / Anime4K は非対称 RGB を source 外へ正しい byte 順で渡し、共有する still NIS /
  Anime4K packer は固定の不透明黒を維持する。Anime4K の全 generated output は正本 generator と一致する。
- 静止画 seek strip の全幅 `strip_content` は同じ余白色を下地にし、loaded / failed cell 固有背景は従来の暗色を維持する。lock と、通常バーを隠したときの toggle はセル上の overlay となり、control 起点の body input を除外しつつ body 起点 drag は横切れる。5 段階の高さ、極狭領域、LTR / RTL で layout / request / paint を同じ座標から解決する。
- native 動画 seek strip は場面サムネイル / 波形の両表示とも canvas RGB から独立した暗色 body を使う。サムネイルは pending / Ready upload 前 / failed を含む動画範囲内の各セルを暗色 backing / border で描き、動画範囲外には偽のセルを置かない。波形は従来の不透明な黒 / analyzed 暗色 / colored foreground raster、白 texture tint、曲外 shade を維持する。両表示の range 文字は shadow / foreground の実描画色を分け、marker は従来の赤線だけにする。`NativeEguiOverlay` は RGB snapshot を持たず、RGB は worker / cache key に入れないため、余白色変更で overlay redraw や再解析を起こさない。音楽画面の timeline raster は従来どおり不透明にする。
- ナビゲータ、ルーペ、HUD、左右パネルの黒背景は維持する。
- 画像データ、AI 処理、copy / capture / export、cache invalidation、keymap、worker、同期 I/O に変更を加えない。

## 6. レビューと検証

実装前の構造レビューでは、global current margin と captured image underlay の分離、exact quad、
Audio 除外、detached の描画 payload 限定変更に合意した。実装後は同じ観点で独立レビューを行う。
自動検証は settings / geometry / detached state の単体テスト、UI glyph check、関連 UI snapshot、
`cargo fmt --check`、対象 crate の check / test、必要な full gate、`scripts/build-dev.ps1` の順で行う。
アプリは agent が起動しない。
