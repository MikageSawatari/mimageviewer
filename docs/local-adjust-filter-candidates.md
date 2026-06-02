# 部分補正レイヤー フィルタ拡充候補リスト

部分補正レイヤー (`LocalEffect` / `tools/local_adjust_lab/`) に追加していくフィルタ候補を
網羅的にまとめたもの。**イラスト用途を主眼**に、既存ソフト (Photoshop / Clip Studio Paint /
Krita / Lightroom) のフィルタ機能を調査して、現状未実装のものを抽出した。

- 実装本体: [crates/local-adjust-core/src/lib.rs](../crates/local-adjust-core/src/lib.rs) (`LocalEffect` enum / `apply_layer`)
- 検証ラボ: [tools/local_adjust_lab/](../tools/local_adjust_lab/)
- 全体設計: [docs/local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md)
- 補正全般の設計: [docs/preset-and-adjustment.md](preset-and-adjustment.md)

凡例:
- 優先度 ★★★ = イラストで効果大・着手推奨 / ★★ = 有用 / ★ = あると良い / (無印) = 余力で
- 難易度 **易** = per-pixel LUT か小カーネルで既存高速経路に相乗り / **中** = 多パス・大カーネル (既存 OilPaint/Bloom/StarGlow と同パターン) / **難** = 対話 UI や新基盤が必要
- このセッションでの方針: **AI 単眼深度推定は不要** (イラスト主体のため)。被写界深度はグラデーション深度方式で行く。

---

## UI 方針

フィルタ数が増えるほど、単純なプルダウンでは選択肢の意味が分かりづらくなる。
補正レイヤーでは、加工内容を次の形で選ぶ。

- 左パネルには現在の加工内容と `効果選択` ボタンを表示する。プルダウンは使わない。
- `効果選択` ボタンで、ドラッグ移動・リサイズできるダイアログを開く。`egui::Window::anchor` で中央固定せず、初期位置だけを指定する。
- ダイアログ右上の閉じるボタンでキャンセルできる。
- ダイアログ内では、効果を「色調補正」「色変換・ルック」「ぼかし・フォーカス」
  「シャープ・ディテール」「変形・歪み」「表現・絵画調」「描画・塗り」「光・雰囲気」などの
  グループに分け、ボタンとして並べる。長い効果名は、説明ツールチップを残したまま選択用の短い表示名にする。
- 効果ボタンをホバーすると、「明るい部分から光の線を十字などで描写する効果です」のように、
  何が起きるかを説明する。
- ボタンをクリックすると、その効果へ切り替えてダイアログを閉じる。

各効果のパラメータ UI は、初心者でも試しやすい順に置く。

- 効果の default は、選択直後になるべく見た目を壊さない値にする。
- 実用値は「プリセット」ボタンで投入する。
- プリセットは最大 10 個程度に抑える。単純な効果は弱 / 中 / 強、見た目の方向が複数ある効果は
  違いが分かる名前付きプリセットを用意する。
- パラメータのスライダーやチェックボックスはホバー説明を持つ。
  例: 「効果をつける明るさのしきい値です。値を大きくすると、より明るい部分のみに効果がつきます。」
- パラメータ名だけで用途が分かりにくいものは、UI 上の表示名を専門用語に寄せすぎない。
- 右パネルの `加工パラメータ` 見出し横には `コピー` / `ペースト` / `リセット` を置く。
  コピー/ペーストは効果種類とパラメータをまとめて扱い、同じ効果種類へ貼り付ける場合は
  前後マスク設定を維持する。違う効果種類へ貼り付ける場合は、貼り付けた効果の標準の前後マスク設定を適用する。
- 左右パネルは左アクセント線で区切る。加工内容は黄、追加 / 削除マスクやマスク設定は緑、
  描画ツールは青、それ以外の操作は灰色を使い、新しい操作 UI は該当セクション内へ置く。
- 画像上の中心、光源位置、効果中心などの normalized image-space 位置パラメータは、
  スライダーだけにせずキャンバス上のドラッグハンドルも用意する。スライダーは精密調整と
  数値確認のために残し、ハンドル表示は `画像ハンドルを表示` で切り替えられるようにする。
- `画像ハンドルを表示` は、効果パネル冒頭ではなく、対象になる中心 X/Y・光源 X/Y などの
  画像空間パラメータの直前に置く。ハンドルで動かす対象と数値スライダーを同じまとまりに
  入れ、新規効果でも同じ配置にそろえる。
- 線形 / 円形グラデーション、チルトシフト範囲のように既に専用ハンドル系があるものは、
  効果中心ハンドルに混ぜず、その専用ハンドルでドラッグ編集を維持する。新規効果追加時は
  `effect_has_position_handles` / `draw_effect_position_handles` の対象漏れを確認する。
- レイヤーマスクは効果計算前 (`前`) と効果計算後 (`後`) の適用を個別に切り替えられる。
  通常の部分補正は `前OFF/後ON`、風・発光・光芒・クロス光のように範囲外へ広がる効果は
  `前ON/後OFF` をデフォルトにする。波形ゆがみなど、対象素材だけを変形して結果も範囲内に
  収めたい場合は `前ON/後ON` を使う。

この方針により、効果数を増やしても「まずプリセットで見た目を選び、必要なら個別パラメータを触る」
導線を維持する。

---

## 0. 現状実装済み (重複追加しないための棚卸し)

### `LocalEffect` (部分補正レイヤー、98種)
Tone(明度/コントラスト/γ/彩度/vibrance/色温度/tint), ToneCurve(5点・RGB合成),
RgbToneCurve(全体+RGB別5点), ColorBalance(シャドウ/中間/ハイライト別),
ThreeWayColorGrading(3-way), SelectiveColor(対象色相+HSL), PartColor(指定色だけ残す白黒化), ChannelMixer(白黒/チャンネル混合),
Hsl(単一・全体), ColorMixer(8色帯), CubeLut(.cube 3D LUT), Posterize(階調数指定),
Threshold(2値化), Invert(階調反転/ネガ), Duotone(2色/3色インク), Equalize(ヒストグラム平坦化),
HighlightsShadows, Clarity, Texture, HighPass, Dehaze, Blur(box), MotionBlur, Wind, SpeedLines, RadialFlash, TiltShift, LensBlur, BokehSprite, RadialBlur, WaveDistortion, HeatHaze, PinchSpherize, Twirl, PolarCoordinates, GlassDisplacement, LensCorrection, LineExtract, ArtisticMedia, BrushStroke, Cutout, ToonShade, Emboss, PixelStylize, Solarize, GlowingEdges, OilPaint, SoftFocus, Orton, Mosaic, Sharpen(radius/threshold), SmartSharpen(edge-aware), Look(15プリセット),
GradientMap, ColorFill, OutlineStroke, RimLight, ContactShadow, ColorTrace, ColorOverlay, NeonGlow, DiffuseGlow, Bloom, Halation, ColorDodgeGlow, GodRays, LensFlare, AnamorphicFlare, LightLeak, BacklightHaze, CloudFog, WaterCaustics, ParticleOverlay, Aurora, Spotlight, Vignette, FilmGrain, Noise, ChromaticAberration, Defringe, ScanlineGlitch, Vhs, DataMosh, PixelSort, OldFilm, Halftone, ScreenTone, ColorHalftone, CmykPlateShift, Lithograph, Engraving, NewspaperPrint, Textureizer, StarGlow, DiffractionStarburst, EdgeSmooth, Despeckle, Median

### マスク種別 (併用可能・差別化の武器)
Full / Raster / RasterVector / LinearGradient / RadialGradient / LumaRange / ColorRange /
Subject(被写体分離) / Segmentation

### グローバル `PostFilter` 側にあって local には無いもの (= local へ移植候補)
減色8機種 / CRT 3種＋複合 / カラーグレード11種

---

## 1. 色調・カラーグレーディング系 ★イラスト最重要

すべて per-pixel LUT 系で既存の高速色調パイプラインに素直に乗る。まずここで「本格カラーグレーディング suite」を揃えるのがコスパ最良。

- [x] **グラデーションマップ** ★★★ **易** — 輝度→任意グラデの色に置換。色設計・色トレス的仕上げの定番 (PS / CSP / Krita)
- [x] **色相別 HSL / カラーミキサー (6〜8色帯)** ★★★ **易〜中** — 「肌だけ」「空だけ」など色域ごとに H/S/L 調整。現 `Hsl` は全体一律なので別物 (Lightroom / PS / Krita)
- [x] **チャンネル別トーンカーブ (R/G/B 独立・多点)** ★★ **易** — 現 `ToneCurve` は5点合成のみ。RGB 独立化＋多点でクロスプロセス等が自在に (PS / CSP / Krita)
- [x] **カラーバランス (シャドウ/中間/ハイライト別の色偏移)** ★★ **易** — Look より自由なグレーディング (PS / CSP)
- [x] **3-way カラーグレーディング (カラーホイール)** ★ **中** — 上位版。Lightroom Color Grading / DaVinci 風
- [x] **セレクティブカラー / ポイントカラー** ★ **易〜中** — 特定色を選んでその色だけ調整 (Lightroom / PS)
- [x] **チャンネルミキサー / 本格白黒変換 (色別の明度寄与)** ★ **易** — モノクロ化を色ごとに制御 (PS / Krita)
- [x] **3D LUT (.cube) 読み込み** ★ **中** (infra は軽い) — 外部シネマ LUT を取り込み (PS Color Lookup)
- [x] **ポスタリゼーション (階調数指定)** ★ **易** — フラット/グラフィック調。減色フィルタとは別物 (PS / CSP / Krita)
- [x] **2値化 (しきい値)** ★ **易** — 線画・モノクロ化 (manga) (PS / CSP / Krita)
- [x] **階調反転 / ネガ** **易** — 反転。local に無い (全ソフト)
- [x] **ダブルトーン / トライトーン** **易** — グラデマップの特殊化。ポスター調 (PS)
- [x] **ヒストグラム平坦化 (Equalize)** **易** — 自動補正 (PS / Krita)
- [x] **`Tone` に tint (緑-マゼンタ) 追加** **易** — 現状 temperature のみで WB として不完全 (Lightroom)

---

## 2. ぼかし・フォーカス系 ★イラスト重要

現状: Blur(box), MotionBlur, TiltShift, LensBlur, RadialBlur, SoftFocus, EdgeSmooth, Median を実装済み。box blur は半径非依存コスト (積算和 O(n)) なので可変半径ぼかしの土台に最適。

- [x] **移動ぼかし (モーションブラー・方向指定)** ★★★ **易〜中** — 動きの表現。イラスト/漫画で必須級 (CSP / PS / Krita)
- [x] **被写界深度 / チルトシフト (`TiltShift`)** ★★★ **中** — 奥行きの向きを指定して奥をぼかす + ジオラマ風。**§A で詳細設計**
- [x] **レンズぼかし / 玉ボケ (Bokeh)** ★★★ **中** — 絞り形状ボケ＋ハイライト滲み。Gaussian/box とは見た目が別物 (CSP Lens / PS Lens Blur)
- [x] **回転ぼかし / ズーム (放射) ぼかし (`RadialBlur`)** ★★ **中** — 躍動感・集中線的フォーカス (CSP Radial / PS Spin)
- [x] **表面ぼかし / バイラテラル (エッジ保持平滑 / `EdgeSmooth`)** ★★ **中** — 平面を滑らかに・線は残す。塗りの均し/スキャン整え (PS Surface / Smart Blur)
- [x] **メディアン (中央値 / `Median`)** **易** — 微ノイズ・ディテール除去 (PS)

---

## 3. シャープ・ディテール系

- [x] **テクスチャ (中周波ディテール)** ★ **易** — Clarity と別系統。Lightroom は両方持つ
- [x] **ハイパス** ★ **易** — シャープ/ディテール抽出の定番テクニック。中間グレー抽出表示と、Overlay 合成による細部強調を持つ (PS Other)
- [x] **アンシャープマスク フル制御 (radius/threshold)** **易** — 現 `Sharpen` を高機能化。しきい値で低コントラストのノイズを避けられる (全ソフト)
- [x] **スマートシャープ (deconvolution 寄り)** **中** — エッジ重みとフチ抑制を持つ、通常シャープより halo が出にくいエッジ保持シャープ (PS)

---

## 4. 変形・歪み系

- [x] **波形 / さざ波 / ジグザグ** ★ **易〜中** — 横波 / 縦波 / さざ波 / ジグザグのサンプリングゆがみ。水面・反射・揺らぎ。背景イラストで多用 (PS / CSP)
- [x] **つまむ / 球面化 (魚眼)** **中** — 中心と半径を指定する Pinch / Spherize。魚眼、ふくらみ、つまみ変形に対応 (PS Pinch/Spherize)
- [x] **渦巻き (Twirl)** **易** — 中心・半径・回転量を指定する渦巻き変形。渦・魔法陣演出 (PS)
- [x] **極座標 (rect↔polar)** **中** — 矩形→円形 / 円形→矩形、中心・半径・角度オフセット・内外反転を指定。tiny planet・円形構図 (PS)
- [x] **変位マップ / ガラス** **中** — 手続き型の変位マップでサンプル位置をずらす GlassDisplacement。すりガラス / 波ガラス / 面ガラスに対応 (PS Displace/Glass)
- [x] **レンズ補正 (樽型/糸巻き/周辺減光除去)** **中** — `LensCorrection`。樽型/糸巻き補正、ズーム/切り抜き、中心調整、周辺減光補正に対応 (PS / Lightroom)
- [ ] **ゆがみ / ワープ (Liquify)** **難** — 部分プッシュ/膨張で比率調整。効果大だが対話 UI が必要 (= フィルタ枠を超える大物) (PS / CSP)

---

## 5. スタイライズ・絵画調系 ★イラスト重要

- [x] **線画抽出 (Extract lines / Find Edges 強化)** ★★★ **中** — `LineExtract`。Sobel エッジから線画を生成し、白地黒線 / 黒地白線 / 元画像への黒線・白線重ね、しきい値・柔らかさ・太さに対応 (CSP 線画抽出 / PS)
- [x] **拡散光彩 (Diffuse Glow)** ★ **易〜中** — `DiffuseGlow`。明部抽出を白く拡散し、粒状ノイズでムラを加える夢幻的グロー。Bloom とは別の柔らかさ (PS)
- [x] **風 / スピード (Wind)** ★ **易** — `Wind`。明部 / 暗部 / 輪郭を指定方向へ引きずる流線。距離、しきい値、柔らかさ、乱れ、seed に対応 (PS)
- [x] **水彩 / 色鉛筆 / 鉛筆画** **中** — `ArtisticMedia`。水彩 / 色鉛筆 / 鉛筆画モードで、色のなじませ、輪郭、紙目/筆致、色量を調整できる絵画調フィルタ (PS / CSP Artistic)
- [x] **ドライブラシ / 塗料 / パレットナイフ** **中** — `BrushStroke`。方向付きストロークで色を引き、ドライブラシ / 塗料 / パレットナイフの筆致、幅、角度、輪郭、色量を調整できる (PS)
- [x] **切り絵 (Cutout)** **中** — `Cutout`。色面をなじませて階調を減らし、切り絵 / フラット・ベクター調の面構成、輪郭、色量を調整できる (PS)
- [x] **エンボス / 浮き彫り** **易** — `Emboss`。明るさの傾きから方向付きの浮き彫り陰影を作り、角度、深さ、コントラスト、色量を調整できる (PS / Krita)
- [x] **結晶化 / 点描 / Facet / メゾチント** **中** — `PixelStylize`。結晶化 / 点描 / Facet / メゾチントの4モードで、セルサイズ、輪郭、色量、ばらつき、seed を調整できる粒状スタイライズ (PS Pixelate)
- [x] **ソラリゼーション** **易** — `Solarize`。しきい値より明るいトーンを反転し、柔らかさ、反転量、色量、コントラストを調整できる反転トーン芸術効果 (PS / Krita)
- [x] **エッジの光彩 (Glowing Edges)** **中** — `GlowingEdges`。Sobel 輪郭をネオン色で描き、線幅、光彩半径、明るさ、色相、背景残しを調整できるネオン輪郭 (PS)
- [x] **OilPaint を local へ移植** **易** — `OilPaint`。既存グローバルの Kuwahara オイルペイントを補正レイヤーへ移植し、半径、彩度、コントラスト、強さを調整できるようにした (自前)

---

## 6. 描画・光源系 ★イラスト重要 (仕上げの主役)

- [x] **グラデーション オーバーレイ / 塗り (ブレンド指定)** ★★★ **易** — `ColorOverlay`。単色 / 線形 / 円形グラデーションを、通常・乗算・スクリーン・オーバーレイ・ソフトライト・カラーで合成できるようにした (PS / CSP)
- [x] **ネオングロー (`NeonGlow`)** ★★★ **中** — 明るい/鮮やかな部分の光を周囲へにじませる。色付きネオン、二段ハロー、発光源の色指定に対応。**§B で詳細設計**
- [x] **光芒 / God rays (放射状ボリューム光 / `GodRays`)** ★★★ **中** — 木漏れ日・差し込む光。明部を拾い、指定中心から外側へ伸びる暖色寄りの光芒を生成できる
- [x] **レンズフレア (`LensFlare`)** ★★ **中** — 光源演出の定番。指定光源からコア、ハロー、ゴースト、光条を重ねられるようにした (PS Render)
- [x] **アナモルフィックフレア (`AnamorphicFlare`)** ★ **中** — 明部から横方向の色付きストリークを生成する。しきい値、長さ、太さ、強さ、光色、着色量を調整でき、`前ON/後OFF` をデフォルトにしてマスク外へ光が伸びるようにした
- [x] **集中線 / スピード線 (`SpeedLines`)** ★ **中** — 放射/平行の線を自動生成オーバーレイ。線色、線数、線幅、中心抜き、線長、seed を調整できる (manga) (CSP)
- [x] **雲 / 霧 (`CloudFog`)** **中** — 大気・遠近感。霧/雲モード、色、密度、コントラスト、上下フェード、seed を調整できる procedural fog/clouds として追加 (PS Clouds)
- [x] **水中コースティクス (`WaterCaustics`)** ★ **中** — 水面越しの揺らぐ光網。スケール、光量、コントラスト、水色、陰影、位相、seed、強度を調整できる
- [x] **雨 / 雪 / 花びら 粒子オーバーレイ (`ParticleOverlay`)** ★ **中** — 方向・密度付きの粒子。雨/雪/花びらモード、密度、サイズ、長さ、角度、色、seed、強度を調整できる
- [x] **オーロラ / 光のカーテン (`Aurora`)** **中** — 縦に揺れる発光カーテン。主色/副色、カーテン数、幅、高さ、揺らぎ、柔らかさ、明るさ、位相、seed、強度を調整できる
- [x] **ライティング / スポットライト (`Spotlight`)** **中** — 局所光源。指定中心を明るくし、周辺影と光色を調整できるスポットライトとして追加 (PS Lighting Effects)

---

## 7. ノイズ・テクスチャ系

- [x] **網点 / スクリーントーン (線・濃度・グラデ)** ★★ **中** — `ScreenTone`。網点 / 線 / カケアミ、セル、角度、濃度、元画像の明暗への階調追従、柔らかさ、強度を調整できる漫画用トーン (CSP トーン)
- [x] **カラーハーフトーン (CMYK 4版ドット)** ★ **中** — `ColorHalftone`。CMYK 4版の角度違いドット、セル、角度オフセット、ドット増減、黒版量、柔らかさ、強度を調整できるポップアート/アメコミ調フィルタ (PS Pixelate)
- [x] **CMYK 版ズレ / 印刷ズレ (`CmykPlateShift`)** ★ **中** — CMYK各色版を別位置からサンプルして再合成する。色版ずれ、ずれ方向、黒版ずれ、黒版量、インク増減、強度を調整でき、透明RGBを拾わない alpha-aware サンプリングにした
- [x] **テクスチャライザ (紙/キャンバス重ね)** ★ **易〜中** — `Textureizer`。紙目 / キャンバス / リネンの手続き型テクスチャを、スケール、凹凸、コントラスト、紙色、強度、seed で調整してソフトライト合成できる紙質・手描き感フィルタ (PS Texture)
- [x] **ノイズ付加 (Gaussian/Uniform, mono 指定)** **易** — `Noise`。均一 / ガウス分布、単色 / カラーノイズ、量、seed を指定できる汎用ノイズ。FilmGrain と別系統 (PS / Krita)
- [x] **ゴミ・キズ取り / ディスペックル** ★ **中** — `Despeckle`。周囲の中央値から大きく外れた孤立点だけを、半径、検出しきい値、強さで選択的に補修する非 AI 高速点ゴミ除去 (スポット用) (PS / CSP / Krita)

---

## 8. イラスト特化の合成 ★ (既存マスク資産との合わせ技)

mIV は既に Subject(被写体分離) / Segmentation / LumaRange / ColorRange マスクを持つので、これらと組み合わせると差別化になる。

- [x] **縁取り / アウトラインストローク (`OutlineStroke`)** ★★ **中** — Subject / Segmentation などのマスク境界から外側・内側・中央の色枠を生成する。ステッカー風/キャラ分離向け。外側へ出せるよう `前ON/後OFF` をデフォルトにする
- [x] **マスク塗りつぶし / 背景塗り (`ColorFill`)** ★★ **易** — マスク範囲を単色、線形2〜3色、円形2〜3色グラデーションで置き換える。被写体切り抜き背景や確認用の単純な色面作成に使う
- [x] **色トレス (`ColorTrace`)** ★ **中** — 暗い線画を検出し、線を除外してぼかした周辺色を暗めにして線色へ混ぜる。黒線を下地に馴染ませるアニメ塗りの定番処理
- [x] **空気感 / 逆光合成 (`BacklightHaze`)** **易〜中** — 光源位置、光色、範囲、ヘイズ、グロー、影持ち上げ、コントラスト/彩度フェードをまとめた逆光・大気感フィルタとして追加

---

## 9. 特化型エフェクト候補 (特定用途の単機能)

TiltShift / NeonGlow のように「特定の見た目を狙い撃ちする」単機能エフェクト。
§1〜8 の汎用カテゴリから漏れる、用途特化系をまとめる。
(幾何/対称・アナグリフ3D は今回スコープ外として除外)

### 9-A. デジタル / グリッチ系
- [x] **グリッチ / RGBずれ・データモッシュ (`DataMosh`)** ★★ **中** — ブロックずれ、フリーズ、スメア、RGB分離、ノイズを組み合わせるデジタル破損演出。現 `ChromaticAberration` (均一色ずれ) とは別物。サイバーパンク/vaporwave
- [x] **ピクセルソート (`PixelSort`)** ★ **中** — 行/列の画素を指定した明るさ帯の連続区間ごとに輝度順でソートする独特のグリッチアート。方向、並び順、明るさ下限/上限、最大区間長、強度を調整できる
- [x] **走査線グリッチ / ホログラム (`ScanlineGlitch`)** ★ **易〜中** — 横走査線、行ずれ、RGBずれ、破損行、ノイズ、seed、強度を調整できる UI/SF 演出

### 9-B. アナログ実機 / レトロ系
- [x] **VHS / アナログビデオ風 (`Vhs`)** ★★ **中** — 輝度を残して色成分を横ににじませ、色ずれ、横ゴースト、トラッキング帯、走査線、ノイズ、退色を調整できる。CRT (既存グローバル) とは別系統
- [x] **アナモルフィックフレア (横方向の青い光条 / `AnamorphicFlare`)** ★ **中** — シネマ調の水平ストリーク。明部抽出から横方向に色付きフレアを伸ばし、しきい値、長さ、太さ、強さ、色、着色量を調整できる
- [x] **回折スターバースト (絞り点光源の光条 / `DiffractionStarburst`)** **中** — 点光源から絞り羽根数に応じた細い光条を伸ばす。奇数羽根では光条数が倍になり、点光源ハローと軽い色ズレも調整できる
- [x] **オールドフィルム / 古写真 (`OldFilm`)** ★ **易** — セピア、退色、ビネット、粒子、ホコリ、縦傷、seed、強度を調整できる古写真/古いフィルム風の複合仕上げ

### 9-C. アニメ / イラスト特化の光・陰影 ★最重要
- [x] **ハレーション (`Halation`)** ★★★ **中** — 明部と肌/輪郭の境界を暖色白でにじませる。`Bloom` とは別の局所暖色ブリード。暖色、エッジ寄せ、スクリーン合成を調整できる。**§C で詳細設計**
- [x] **トゥーン / セルシェード量子化 (`ToonShade`)** ★★ **中** — 明度を数段のフラット帯に量子化し、色相維持、影色/光色ティント、段差線を調整できる。`ポスタリゼーション` (RGB 各 ch 量子化) とは別。**§D で詳細設計**
- [x] **リムライト / 縁の光 追加 (`RimLight`)** ★★ **中** — 被写体エッジの光源側だけ発光。既存 Subject マスク＋方向指定で実現。`縁取り` (均一枠) とは別物。`前ON/後OFF` をデフォルトにし、幅、減衰、回り込み、光色を調整できる。**§E で詳細設計**
- [x] **接触影 / 簡易 AO (`ContactShadow`)** ★ **中** — マスク境界の内側を暗く締めて立体感を底上げ。`前ON/後ON` をデフォルトにし、全周AOから下側だけの接触影まで、幅、ぼかし、方向性、影色を調整できる
- [x] **覆い焼きカラー発光 (`ColorDodgeGlow`)** ★ **易〜中** — 明部から色付きの光を作り、スクリーンと覆い焼きを混ぜて発光合成する。魔法/エモーション演出向け。`前ON/後OFF` をデフォルトにし、しきい値、半径、覆い焼き量、光色、着色量を調整できる

### 9-D. 漫画の陰影表現
- [x] **カケアミ / ハッチング (線の陰影)** ★ **中** — `ScreenTone` の線 / カケアミモードとして吸収。モノクロ漫画向けの線パターン陰影
- [x] **集中線フラッシュ (白黒反転フラッシュ / `RadialFlash`)** ★ **中** — 中心から白黒のくさび形フラッシュを放射する。中心位置ハンドル、中心抜き、外側範囲、白黒反転に対応

### 9-E. 自然現象 / 大気エフェクト ★背景イラスト
- [x] **陽炎 / 熱揺らぎ (`HeatHaze`)** ★★ **中** — 局所の上昇する波打ち歪み。夏・炎・砂漠の空気。横揺れ、上昇、乱れ、にじみ、位相、強さを調整でき、透明ピクセルの隠しRGBを拾わない alpha-aware サンプリングにした
- [x] **水中コースティクス (光の網 / `WaterCaustics`)** ★ **中** — 水面越しの揺らぐ光網。水中シーンやプールの反射光向けに追加
- [x] **雨 / 雪 / 花びら 粒子オーバーレイ (`ParticleOverlay`)** ★ **中** — 方向・密度付きの粒子。`雲/霧` の粒子版として追加
- [x] **オーロラ / 光のカーテン (`Aurora`)** **中** — 縦の発光カーテンとして追加

### 9-F. 印刷 / 版画 / 質感系 ★トレンド
- [x] **リソグラフ / シルクスクリーン風 (`Lithograph`)** ★★ **中** — 2色スポットインク、紙色、版ズレ、粒状感、紙目を調整できる印刷/版画風フィルタとして追加
- [x] **CMYK 版ズレ / 印刷ズレ (`CmykPlateShift`)** ★ **中** — 4 版を微妙にずらす印刷物風。`カラーハーフトーン` と相性。版ズレ0・インク増減0では元色へ戻る減法再合成にした
- [x] **銅版画 / エングレービング (`Engraving`)** **中** — 平行線、クロスハッチ、等高線状の線、紙色、インク色で陰影を作る古典挿絵風フィルタとして追加
- [x] **新聞印刷 / 古印刷物 (`NewspaperPrint`)** **易〜中** — 粗い網点、黄ばんだ紙色、紙目、インクにじみ、退色を調整できる新聞紙・古印刷物風フィルタとして追加

### 9-G. 補正系の特殊ツール
- [x] **パートカラー (1色だけ残してグレー化)** ★ **易** — `PartColor`。指定RGBの色相だけを残し、他の色をグレー化する。対象色はRGB共有コントロールと画像クリックのスポイトで指定でき、残す範囲、境界ぼかし、グレー化強度、対象色の彩度/明度を調整できる
- [ ] **周波数分離 / ウェーブレット分解** **中** — 質感と色を分離してレタッチ (Krita wavelet decompose 相当)。スキャン補修
- [x] **色収差除去 / Defringe (`Defringe`)** **易〜中** — `ChromaticAberration` の逆。強いエッジ上で周辺より彩度が高い色フチを検出し、半径、エッジしきい値、色フチしきい値、中和、強度を調整して除去できる
- [x] **オートン効果 (Orton)** ★ **易** — `Orton`。alpha を考慮したボケコピーを明るくし、彩度/コントラストを整えてスクリーン合成する夢幻グロー。SoftFocus よりルック寄りで、半径、強さ、明るさ、コントラスト、彩度を調整できる

### 9-H. 立体視 / 特殊光学
- [ ] **レンズ汚れ / 水滴 / レンズダスト オーバーレイ** **中** — レンズ越し演出
- [x] **玉ボケスプライト (ハート/星形ボケ / `BokehSprite`)** ★ **中** — 形状付きボケ粒子を明部に散らす。`レンズぼかし` の装飾版

### 特化系の中でのイラスト軸の推し
1. **ハレーション** — `Halation` として実装済み。アニメ塗り仕上げの花形 (§C)
2. **トゥーンシェード量子化** — `ToonShade` として実装済み。フォト/3D をアニメ塗り風に (§D)
3. **リムライト追加** — `RimLight` として実装済み。既存 Subject マスク資産が活きる (§E)
4. **リソグラフ風** — `Lithograph` として実装済み。近年人気の質感、差別化になる (9-F)
5. **陽炎 / 熱揺らぎ** — `HeatHaze` として実装済み。背景イラストの空気感 (9-E)
6. **グリッチ / VHS** — サイバー/レトロ演出の鉄板 (9-A/9-B)
7. **パートカラー** — 一発で映える、実装は易 (9-G)

---

## §A. 被写界深度 / チルトシフト `TiltShift` (詳細設計)

「奥行きの向きを指定して奥をぼかす」「ジオラマ風」を1エフェクトでカバーする。
**AI 深度は使わない**。グラデーション深度のみ。

### 中核
場所ごとに半径が変わる可変半径ぼかし。**CoC (錯乱円 = ぼかし半径) フィールド**を作り、
既存 `box_blur_rgba` を数段の半径 (r, 2r, 4r…) で前計算 → 画素ごとの CoC で線形補間する
**ピラミッド方式**が定番・高速 (box blur は半径非依存コストなので相性抜群)。

### パラメータ案
```rust
pub struct TiltShiftParams {
    pub source: DepthSource,   // 深度の出どころ (グラデのみ)
    pub focus: f32,            // 合焦距離 0..1 (手前=0, 奥=1)
    pub focus_width: f32,      // シャープに残す帯の幅
    pub falloff: f32,          // 帯外の立ち上がり
    pub max_radius_px: f32,    // 最遠/最近での最大ボケ
    pub far_only: bool,        // true=奥だけぼかす(手前シャープ) / false=対称チルトシフト
    pub highlight_bokeh: f32,  // 任意: 明部を膨らませて玉ボケ感
}

pub enum DepthSource {
    Linear { angle_degrees: f32, center: [f32; 2] }, // 平面的奥行き(風景・街路)。angle が「奥行きの向き」
    Radial { center: [f32; 2], radius: [f32; 2] },   // 点から放射(アイリス/ジオラマ)
}
```

- `angle_degrees` が「奥行きの向き」の指定そのもの (上に行くほど奥 / 消失点へ向かって奥)。
- 各画素の深度 = 焦点ライン/点からの符号付き距離 → CoC に変換。
- `far_only=true` で「奥だけぼかす」、`false` で手前・奥両方ぼかすチルトシフト (= PS Tilt-Shift / Iris Blur 相当)。

### ジオラマ (ミニチュア) プリセット
ぼかしだけだと「浅い被写界深度の写真」に見えるだけ。ミニチュア錯覚には
**彩度 UP ＋ コントラスト UP (＋わずかに暖色/vibrance)** の "おもちゃっぽい鮮やかさ" が要る。
→ **`TiltShift(Radial, 対称) + Tone(彩度+, コントラスト+)` を束ねたワンクリックプリセット**にする
(Look 系の延長で実装可)。

### 品質の注意点
- **前景の滲み (ハロー)**: 単純な gather ブラーは深度境界でシャープな手前色がボケ背景へ滲む。
  グラデ深度は単調なので軽微 (ビューア用途なら無視可)。
- **玉ボケ形状**: box blur のボケは角張る。丸い玉ボケが欲しければ円盤カーネル＋明部ブースト
  (既存 SoftFocus/Bloom の明部抽出が流用可) を `highlight_bokeh` として後段に。

### 今すぐの近似
`Blur` レイヤー + `LinearGradient` マスクで簡易チルトシフトは現状でも作れる。ただしこれは
「固定半径のぼかしを不透明度でクロスフェード」なので、奥ほどボケ量が増える写真的 DoF とは
見え方が違う (フェードっぽくなる)。本格版は上記専用エフェクトで。

---

## §B. ネオングロー `NeonGlow` (詳細設計)

「明るい部分の光をぼかして周囲へ広げる」。現 `Bloom` の上位版。

### 現 `Bloom` の挙動と限界 (lib.rs:1798)
仕組みは既にネオングローの定石:
```
明部抽出(色は維持・輝度で重み付け) → box_blur で拡散 → 元画像に加算
```
→ 白〜ほぼ白の光源なら `Bloom` 単体で実現済み (Full マスク + radius↑/strength↑)。

ただし2点が足りない:
1. **`threshold` が `clamp(0.90, 0.9999)`・luma 基準** → 彩度の高い色付きネオン
   (例: シアン管 RGB(0,200,255) は luma≈0.7) は閾値に届かず**一切光らない**。
2. **単一半径・加算のみ** → 芯の強い内側グロー＋広い外側ハローの二段 falloff が出ない。
   重なると白飛び。glow の色味・着色も制御不可。

### ⚠️ 「輝度マスク + Bloom」は逆効果 (重要)
レイヤーマスクは `blend_rgb_with_mask` で「エフェクト結果をどこに塗るか」だけを決める
(`base = lerp(base, effected, mask)`)。グローは**暗い周囲へにじみ出す**のが本質なので、
`LumaRange` で明部だけ選ぶと**光輪が落ちる暗い隣接画素が mask≈0 で消され、にじみがクリップ
される**。→ グローでは**マスクは Full (または光源＋周囲を含む広い領域マスク)** にし、
「明るい所だけ光らせる」判定はエフェクト内部の threshold に任せる。

### パラメータ案
```rust
pub struct NeonGlowParams {
    pub threshold: f32,           // 0.2..1.0 まで下げられる
    pub by_saturation: bool,      // 輝度ORビビッドさで抽出(色付き光を拾う)
    pub inner_radius_px: f32,     // 芯のにじみ
    pub outer_radius_px: f32,     // 広いハロー
    pub strength: f32,
    pub glow_saturation: f32,     // 光輪の彩度ブースト(発光感)
    pub tint: Option<[u8; 3]>,    // 任意: グロー色を固定(全部シアンに光らせる等)
    pub screen_blend: bool,       // 加算の白飛びを避ける
    pub source_color: Option<ColorRangeMask>, // 任意: 特定色のラインだけ光らせる
}
```

### Bloom との差分 (= ネオンらしさの肝)
- **彩度/最大チャンネル基準の抽出**で白くない色付き光も拾える (threshold を低く許可)。
- **色相維持＋彩度ブースト**で "発光した光" の色を周囲へ。`tint` で色固定も。
- **内側＋外側の二段ブラー**で芯の強い→広く柔らかい neon falloff。
- **screen 合成**オプションで重なりの白飛び抑制。
- **線画対応**: 白地に暗い線のネオン文字は luma 抽出だと拾えないので、`source_color`
  (既存 `ColorRange` ロジック流用) で「この色のラインだけ光らせる」モード。

### 既存 `StarGlow` との関係
`StarGlow` は異方性の十字光条。`NeonGlow` (等方ハロー) と組み合わせると、ハロー＋きらめきで
発光表現が一通り揃う。

実装は既存の `box_blur_rgba` ＋ HSL ヘルパー流用で `apply_layer` に1分岐足すだけ。

---

## §C. ハレーション `Halation` (詳細設計)

アニメ塗りの定番仕上げ。明部と中間調の境界が**暖色の白**でにじむ「白浮き」。
`Bloom`（等方の明部加算）とは別物で、(1) 暖色寄せ、(2) より広く柔らかい、(3) 明/中間の
境界を強調、の3点が違う。

### 中核
明部抽出 → 広めの `box_blur_rgba` → **暖色 tint をかけて screen 合成**。Bloom のコードを
ほぼ流用でき、tint と edge_bias を足すだけ。

### パラメータ案
```rust
pub struct HalationParams {
    pub threshold: f32,      // 明部抽出 0.5..1.0 (Bloom より低め可)
    pub radius_px: f32,      // にじみ幅 (広め)
    pub strength: f32,
    pub warmth: f32,         // 暖色寄せ量 0..1
    pub tint: [u8; 3],       // 既定 ~(255, 235, 200) 暖白
    pub edge_bias: f32,      // 明/中間境界の強調 (0=均一, 1=境界優先)
    pub screen_blend: bool,  // 既定 true (白飛び抑制)
}
```
- `edge_bias` は「明部の勾配（エッジ）」で抽出重みを増やすと、ベタ明部全体ではなく
  輪郭際だけが白浮きするアニメ的な見え方になる。
- 肌・光源境界に効かせたいときは `LumaRange` ではなく `ColorRange`（肌色）マスクと併用可。
  ※グロー同様、にじみが落ちる暗部を消さないようマスクは広めに取る (§B の注意点参照)。

---

## §D. トゥーンシェード量子化 `ToonShade` (詳細設計)

フォト/3D 描画を「アニメ塗り風のフラットな階調」に量子化する。`ポスタリゼーション`
（RGB 各 ch を独立量子化 → 色割れが出る）と違い、**明度だけを数段の帯に量子化して色相・彩度は
維持**するので、塗りが破綻しない。

### 中核
画素の輝度を N 段にスナップ → 元の色相/彩度を保ったまま明度を帯値に置換。境界を
なめらかにするか（アンチエイリアス）ハードにするか、帯ごとに色を寄せるか、境界線を引くかを
パラメータ化。

### パラメータ案
```rust
pub struct ToonShadeParams {
    pub bands: u32,                   // 階調数 2..6
    pub softness: f32,                // 帯境界のなめらかさ (0=ハード)
    pub preserve_hue: bool,          // true: 明度のみ量子化 (彩度/色相維持)
    pub shadow_tint: Option<[u8; 3]>, // 影帯に寄せる色 (任意)
    pub light_tint: Option<[u8; 3]>,  // 光帯に寄せる色 (任意)
    pub outline_strength: f32,        // 帯境界に線を引く (0=なし)
}
```
- `shadow_tint` を寒色、`light_tint` を暖色にすると、アニメ塗りの「影は青み・光は暖色」の
  定番ライティングをワンクリックで付けられる。
- `outline_strength` は帯境界（量子化の段差）に沿って 1px 程度の線を描くと、セル画の
  塗り分け線風になる。

---

## §E. リムライト `RimLight` (詳細設計)

被写体の輪郭の**光源側だけ**を発光させる縁取り光。`縁取り/アウトラインストローク`（全周
均一の枠）とは別で、方向を持つ。

### 中核
**既存 Subject / Raster マスクのアルファ勾配**から輪郭法線を求め、`dot(法線, 光源方向)` が
正（＝光源を向いている縁）の部分だけに、指定色のハイライトを width/falloff 付きで重ねる。
Full など境界のないマスクでは効果は出ない。Subject / Raster / Segmentation など、境界を持つ
マスクと組み合わせる前提にする。

### パラメータ案
```rust
pub struct RimLightParams {
    pub light_angle_degrees: f32, // 光源方向 (縁のどちら側を光らせるか)
    pub width_px: f32,            // 縁光の太さ
    pub falloff: f32,             // 内側への減衰
    pub strength: f32,
    pub color_rgb: [u8; 3],       // 縁光の色
    pub wrap: f32,                // 光の回り込み量 (0=正面側のみ, 大=側面まで)
}
```
- このエフェクトは `前ON/後OFF` をデフォルトにし、`mask_rgba_input` 後のアルファから
  マスク境界を検出する。`縁取り` と同じく効果側の alpha を RGB オーバーレイ量として使う。
- `wrap` を上げると半逆光気味に縁光が側面まで回り込む。キャラの立ち上げに有効。

---

## 推奨着手順 (イラスト軸・効果対コスト)

1. **グラデーションマップ** — 色設計・仕上げの即戦力、LUT で軽い (§1)
2. **色相別 HSL / カラーミキサー** — 局所色補正、現 `Hsl` の正統進化 (§1)
3. **グラデーション オーバーレイ (ブレンド指定)** — `ColorOverlay` として実装済み。空気感付与の最頻出 (§6)
4. **ネオングロー** — `NeonGlow` として実装済み。色付き発光、`Bloom` の弱点 (threshold≥0.90) を埋める (§B)
5. **チルトシフト + ジオラマ プリセット** — 奥行き表現・ミニチュア、AI 不要 (§A)
6. **モーションブラー ＋ 玉ボケ (レンズぼかし)** — 動き・被写界深度 (§2)
7. **線画抽出 (Find Edges 強化)** — フォト/3D からの線画起こし、CSP の領域 (§5)
8. **光芒 (God rays) / レンズフレア** — 光源演出、見栄えのインパクト大 (§6)
9. **RGB 独立トーンカーブ** — グレーディングの底力、既存 `ToneCurve` 拡張 (§1)
10. **網点 / スクリーントーン** — `ScreenTone` として実装済み。漫画ユーザー向けの差別化 (§7)

実装観点では §1 と 9 は per-pixel LUT で既存高速色調パイプラインに相乗り、
§2/5/6/§A/§B はカーネル/多パス系で既存 OilPaint/Bloom/StarGlow と同じ実装パターンが流用可能。
まず軽い色系で「本格カラーグレーディング suite」を揃え、次に光源系・線画化で
「イラスト仕上げ」を厚くする順が効率的。

---

## 出典 (調査ソース)

- Clip Studio Paint: [Filters](https://help.clip-studio.com/en-us/manual_en/390_filters/Filters.htm) / [Tonal Correction Effects](https://help.clip-studio.com/en-us/manual_en/390_filters/Tonal_Correction_Effects.htm) / [Filters & Effects for Illustration](https://www.clipstudio.net/en/characterart/tool/filters.html)
- Photoshop: [Filter effects reference](https://helpx.adobe.com/photoshop/using/filter-effects-reference.html) / [Filter Gallery](https://helpx.adobe.com/photoshop/desktop/effects-filters/get-started-with-filters/filter-gallery.html) / [Liquify](https://helpx.adobe.com/photoshop/desktop/effects-filters/artistic-stylize-filters/overview-of-liquify-filter.html)
- Krita: [Filters](https://docs.krita.org/en/reference_manual/filters.html) / [Adjust](https://docs.krita.org/en/reference_manual/filters/adjust.html)
- Lightroom: [Local adjustments](https://helpx.adobe.com/lightroom-classic/help/apply-local-adjustments.html) / [Texture vs Clarity vs Dehaze](https://thelenslounge.com/lightroom-texture-clarity-dehaze/)
- イラスト仕上げ: [Gradient Maps for color grading](http://www.tipsquirrel.com/color-grading-with-gradient-maps/) / [Anime Glow/Bloom workflow](https://www.deviantart.com/gubnub/art/YVZSTUDIOS-Anime-Glow-Bloom-Tutorial-1-603365955)
