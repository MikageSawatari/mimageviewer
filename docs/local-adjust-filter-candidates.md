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
- `効果選択` ボタンで、画面中央のダイアログを開く。
- ダイアログ右上の閉じるボタンでキャンセルできる。
- ダイアログ内では、効果を「色調・カラー」「ぼかし・ディテール」「光・雰囲気」などの
  グループに分け、ボタンとして並べる。
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

この方針により、効果数を増やしても「まずプリセットで見た目を選び、必要なら個別パラメータを触る」
導線を維持する。

---

## 0. 現状実装済み (重複追加しないための棚卸し)

### `LocalEffect` (部分補正レイヤー、36種)
Tone(明度/コントラスト/γ/彩度/vibrance/色温度/tint), ToneCurve(5点・RGB合成),
RgbToneCurve(全体+RGB別5点), ColorBalance(シャドウ/中間/ハイライト別),
ThreeWayColorGrading(3-way), SelectiveColor(対象色相+HSL), ChannelMixer(白黒/チャンネル混合),
Hsl(単一・全体), ColorMixer(8色帯), CubeLut(.cube 3D LUT), Posterize(階調数指定),
Threshold(2値化), Invert(階調反転/ネガ), Duotone(2色/3色インク), Equalize(ヒストグラム平坦化),
HighlightsShadows, Clarity, Texture, HighPass, Dehaze, Blur(box), MotionBlur, TiltShift, LensBlur, RadialBlur, SoftFocus, Mosaic, Sharpen, Look(15プリセット),
GradientMap, Bloom, Vignette, FilmGrain, ChromaticAberration, Halftone, StarGlow, EdgeSmooth, Median

### マスク種別 (併用可能・差別化の武器)
Full / Raster / RasterVector / LinearGradient / RadialGradient / LumaRange / ColorRange /
Subject(被写体分離) / Segmentation

### グローバル `PostFilter` 側にあって local には無いもの (= local へ移植候補)
OilPaint(Kuwahara) / Sketch(Sobel) / LightLeak / 減色8機種 / CRT 3種＋複合 / カラーグレード11種

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
- [ ] **アンシャープマスク フル制御 (radius/threshold)** **易** — 現 `Sharpen` を高機能化 (全ソフト)
- [ ] **スマートシャープ (deconvolution 寄り)** **中** — エッジ保持シャープ (PS)

---

## 4. 変形・歪み系

- [ ] **波形 / さざ波 / ジグザグ** ★ **易〜中** — 水面・反射・揺らぎ。背景イラストで多用 (PS / CSP)
- [ ] **つまむ / 球面化 (魚眼)** **中** — レンズ歪み・誇張 (PS Pinch/Spherize)
- [ ] **渦巻き (Twirl)** **易** — 渦・魔法陣演出 (PS)
- [ ] **極座標 (rect↔polar)** **中** — tiny planet・円形構図 (PS)
- [ ] **変位マップ / ガラス** **中** — テクスチャ沿い歪み (PS Displace/Glass)
- [ ] **レンズ補正 (樽型/糸巻き/周辺減光除去)** **中** — 写真の幾何補正 (PS / Lightroom)
- [ ] **ゆがみ / ワープ (Liquify)** **難** — 部分プッシュ/膨張で比率調整。効果大だが対話 UI が必要 (= フィルタ枠を超える大物) (PS / CSP)

---

## 5. スタイライズ・絵画調系 ★イラスト重要

- [ ] **線画抽出 (Extract lines / Find Edges 強化)** ★★★ **中** — 写真/3D 描画を線画化してトレース・合成。CSP がイラスト向けに専用機能化している領域。グローバルの Sketch を制御付き＆ local 化 (CSP 線画抽出 / PS)
- [ ] **拡散光彩 (Diffuse Glow)** ★ **易〜中** — 粒状感のある夢幻的グロー。Bloom とは別の柔らかさ (PS)
- [ ] **風 / スピード (Wind)** ★ **易** — 横方向の流線。動き・エフェクト (PS)
- [ ] **水彩 / 色鉛筆 / 鉛筆画** **中** — フォトを絵画調に。背景素材化 (PS / CSP Artistic)
- [ ] **ドライブラシ / 塗料 / パレットナイフ** **中** — 筆致テクスチャ (PS)
- [ ] **切り絵 (Cutout)** **中** — フラット・ベクター調 (PS)
- [ ] **エンボス / 浮き彫り** **易** — 質感・モノクロ装飾 (PS / Krita)
- [ ] **結晶化 / 点描 / Facet / メゾチント** **中** — 粒状スタイライズ (PS Pixelate)
- [ ] **ソラリゼーション** **易** — 反転トーン芸術効果 (PS / Krita)
- [ ] **エッジの光彩 (Glowing Edges)** **中** — ネオン輪郭 (PS)
- [ ] **OilPaint / Sketch を local へ移植** **易** — 既にグローバルにある資産の部分適用化 (自前)

---

## 6. 描画・光源系 ★イラスト重要 (仕上げの主役)

- [ ] **グラデーション オーバーレイ / 塗り (ブレンド指定)** ★★★ **易** — 夕焼けのオレンジを乗算/スクリーンで被せる等、空気感・統一感の最頻出仕上げ (PS / CSP)
- [ ] **ネオングロー (`NeonGlow`)** ★★★ **中** — 明るい/鮮やかな部分の光を周囲へにじませる。色付きネオン対応。**§B で詳細設計**
- [ ] **光芒 / God rays (放射状ボリューム光)** ★★★ **中** — 木漏れ日・差し込む光。アニメ調背景の花形
- [ ] **レンズフレア** ★★ **中** — 光源演出の定番 (PS Render)
- [ ] **集中線 / スピード線 (procedural)** ★ **中** — 放射/平行の線を自動生成オーバーレイ (manga) (CSP)
- [ ] **雲 / 霧 (procedural fog)** **中** — 大気・遠近感 (PS Clouds)
- [ ] **ライティング (スポットライト)** **難** — 局所光源 (PS Lighting Effects)

---

## 7. ノイズ・テクスチャ系

- [ ] **網点 / スクリーントーン (線・濃度・グラデ)** ★★ **中** — 漫画仕上げの中核。現 `Halftone` (輝度ドット) を超えた本格トーン (CSP トーン)
- [ ] **カラーハーフトーン (CMYK 4版ドット)** ★ **中** — ポップアート/アメコミ調。現 `Halftone` はグレーのみ (PS Pixelate)
- [ ] **テクスチャライザ (紙/キャンバス重ね)** ★ **易〜中** — 紙質・手描き感 (PS Texture)
- [ ] **ノイズ付加 (Gaussian/Uniform, mono 指定)** **易** — 汎用ノイズ。FilmGrain と別系統 (PS / Krita)
- [ ] **ゴミ・キズ取り / ディスペックル** ★ **中** — スキャン点ノイズの非 AI 高速除去 (スポット用) (PS / CSP / Krita)

---

## 8. イラスト特化の合成 ★ (既存マスク資産との合わせ技)

mIV は既に Subject(被写体分離) / Segmentation / LumaRange / ColorRange マスクを持つので、これらと組み合わせると差別化になる。

- [ ] **縁取り / アウトラインストローク** ★★ **中** — Subject マスクの外周に色枠を生成 (ステッカー風/キャラ分離)。既存 seg マスクをそのまま活用
- [ ] **色トレス (線画の色を下地に馴染ませる)** ★ **中** — アニメ塗りの定番処理
- [ ] **空気感 / 逆光合成** **易〜中** — gradient overlay + glow + マスクの複合プリセット

---

## 9. 特化型エフェクト候補 (特定用途の単機能)

TiltShift / NeonGlow のように「特定の見た目を狙い撃ちする」単機能エフェクト。
§1〜8 の汎用カテゴリから漏れる、用途特化系をまとめる。
(幾何/対称・アナグリフ3D は今回スコープ外として除外)

### 9-A. デジタル / グリッチ系
- [ ] **グリッチ / RGBずれ・データモッシュ** ★★ **中** — ブロックずれ・走査線断裂・チャンネル分離。現 `ChromaticAberration` (均一色ずれ) とは別物。サイバーパンク/vaporwave
- [ ] **ピクセルソート** ★ **中** — 行/列の画素を輝度でソートする独特のグリッチアート。閾値帯で部分適用
- [ ] **走査線グリッチ / ホログラム** ★ **易〜中** — 横線ノイズ＋色ずれ＋明滅。UI/SF 演出

### 9-B. アナログ実機 / レトロ系
- [ ] **VHS / アナログビデオ風** ★★ **中** — 色にじみ (chroma bleed)・トラッキングノイズ・ゴースト・ヘッドスイッチング帯。CRT (既存グローバル) とは別系統
- [ ] **アナモルフィックフレア (横方向の青い光条)** ★ **中** — シネマ調の水平ストリーク。`レンズフレア` の特化版
- [ ] **回折スターバースト (絞り点光源の光条)** **中** — 点光源の絞り羽根状の光芒。`StarGlow` (十字) とは別の絞り再現
- [ ] **オールドフィルム / 古写真** ★ **易** — 傷＋退色＋ビネット＋粒状の複合プリセット (既存要素の束ね)

### 9-C. アニメ / イラスト特化の光・陰影 ★最重要
- [ ] **ハレーション (アニメの暖色白浮き)** ★★★ **中** — 明部と肌/輪郭の境界が暖色白でにじむ。`Bloom` とは別の局所暖色ブリード。**§C で詳細設計**
- [ ] **トゥーン / セルシェード量子化** ★★ **中** — 輝度を数段のフラット帯に量子化 (境界線も可)。`ポスタリゼーション` (RGB 各 ch 量子化) とは別。**§D で詳細設計**
- [ ] **リムライト / 縁の光 追加** ★★ **中** — 被写体エッジの光源側だけ発光。既存 Subject マスク＋方向指定で実現。`縁取り` (均一枠) とは別物。**§E で詳細設計**
- [ ] **接触影 / 簡易 AO (輪郭際の陰)** ★ **中** — エッジ内側を暗く締めて立体感を底上げ
- [ ] **覆い焼きカラー発光 (color dodge glow)** ★ **易〜中** — 光源・エフェクトを発光合成。魔法/エモーション演出

### 9-D. 漫画の陰影表現
- [ ] **カケアミ / ハッチング (線の陰影)** ★ **中** — `スクリーントーン` (ドット) とは別の線パターン陰影。モノクロ漫画
- [ ] **集中線フラッシュ (白黒反転フラッシュ)** ★ **中** — 中心から放射する白/黒フラッシュ。`集中線` の演出強化版

### 9-E. 自然現象 / 大気エフェクト ★背景イラスト
- [ ] **陽炎 / 熱揺らぎ (heat haze)** ★★ **中** — 局所の上昇する波打ち歪み。夏・炎・砂漠の空気
- [ ] **水中コースティクス (光の網)** ★ **中** — 水面越しの揺らぐ光網。水中シーン
- [ ] **雨 / 雪 / 花びら 粒子オーバーレイ** ★ **中** — 方向・密度付きの粒子。`雲/霧` の粒子版
- [ ] **オーロラ / 光のカーテン** **中** — 縦の発光カーテン

### 9-F. 印刷 / 版画 / 質感系 ★トレンド
- [ ] **リソグラフ / シルクスクリーン風** ★★ **中** — 限定スポットカラー＋版ズレ＋粒状。近年イラストで人気の質感
- [ ] **CMYK 版ズレ / 印刷ズレ** ★ **中** — 4 版を微妙にずらす印刷物風。`カラーハーフトーン` と相性
- [ ] **銅版画 / エングレービング (線彫り調)** **中** — 等高線状の線で陰影。古典挿絵
- [ ] **新聞印刷 / 古印刷物** **易〜中** — 粗ハーフトーン＋退色＋紙地

### 9-G. 補正系の特殊ツール
- [ ] **パートカラー (1色だけ残してグレー化)** ★ **易** — `セレクティブカラー` の一発版。被写体強調の定番
- [ ] **周波数分離 / ウェーブレット分解** **中** — 質感と色を分離してレタッチ (Krita wavelet decompose 相当)。スキャン補修
- [ ] **色収差除去 / Defringe** **易〜中** — `ChromaticAberration` の逆。輪郭の色フチ取り
- [ ] **オートン効果 (Orton)** ★ **易** — ボケコピーを乗せる夢幻グロー。`拡散光彩` に近いが彩度・コントラスト挙動が独特

### 9-H. 立体視 / 特殊光学
- [ ] **レンズ汚れ / 水滴 / レンズダスト オーバーレイ** **中** — レンズ越し演出
- [ ] **玉ボケスプライト (ハート/星形ボケ)** ★ **中** — 形状付きボケ粒子を明部に散らす。`レンズぼかし` の装飾版

### 特化系の中でのイラスト軸の推し
1. **ハレーション** — アニメ塗り仕上げの花形なのに現リストの穴だった (§C)
2. **トゥーンシェード量子化** — フォト/3D をアニメ塗り風に (§D)
3. **リムライト追加** — 既存 Subject マスク資産が活きる (§E)
4. **リソグラフ風** — 近年人気の質感、差別化になる (9-F)
5. **陽炎 / 熱揺らぎ** — 背景イラストの空気感 (9-E)
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
正（＝光源を向いている縁）の部分だけに、指定色のハイライトを width/falloff 付きで加算する。
Subject マスクが無いレイヤーでは輝度エッジで代替。

### パラメータ案
```rust
pub struct RimLightParams {
    pub light_angle_degrees: f32, // 光源方向 (縁のどちら側を光らせるか)
    pub width_px: f32,            // 縁光の太さ
    pub strength: f32,
    pub color: [u8; 3],           // 縁光の色
    pub falloff: f32,             // 内側への減衰
    pub wrap: f32,                // 光の回り込み量 (0=正面側のみ, 大=側面まで)
}
```
- このエフェクトは**マスク（被写体）を入力として使う**点が他と違う。`apply_layer` で
  `evaluate_layer_mask` の結果（被写体アルファ）を effect 側にも渡す配線が要る。
- `wrap` を上げると半逆光気味に縁光が側面まで回り込む。キャラの立ち上げに有効。

---

## 推奨着手順 (イラスト軸・効果対コスト)

1. **グラデーションマップ** — 色設計・仕上げの即戦力、LUT で軽い (§1)
2. **色相別 HSL / カラーミキサー** — 局所色補正、現 `Hsl` の正統進化 (§1)
3. **グラデーション オーバーレイ (ブレンド指定)** — 空気感付与の最頻出、実装容易 (§6)
4. **ネオングロー** — 色付き発光、`Bloom` の弱点 (threshold≥0.90) を埋める (§B)
5. **チルトシフト + ジオラマ プリセット** — 奥行き表現・ミニチュア、AI 不要 (§A)
6. **モーションブラー ＋ 玉ボケ (レンズぼかし)** — 動き・被写界深度 (§2)
7. **線画抽出 (Find Edges 強化)** — フォト/3D からの線画起こし、CSP の領域 (§5)
8. **光芒 (God rays) / レンズフレア** — 光源演出、見栄えのインパクト大 (§6)
9. **RGB 独立トーンカーブ** — グレーディングの底力、既存 `ToneCurve` 拡張 (§1)
10. **網点 / スクリーントーン** — 漫画ユーザー向けの差別化 (§7)

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
