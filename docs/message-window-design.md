# MessageWindow / メッセージウィンドウ — リサーチ + 設計

Status: 設計確定前のリサーチ + 提案 (実装未着手)
作成: 2026-06-04 セッション (Part2 調査エージェントの成果)

対象: `crates/comic-core` + `tools/comic_lab`。新しい注釈タイプ「メッセージウィンドウ
(対話ボックス/ダイアログ枠)」を、既存の吹き出し(`BubbleObject`)と並ぶ独立した
`AnnotationKind` バリアントとして追加するための調査と設計案。

> lab 試作の正本は [comic-lab-progress.md](comic-lab-progress.md)。本書はその Part2
> (メッセージウィンドウ) の調査・設計ドキュメント。

---

## 1. 実システム & 既存ツールの調査

吹き出し(balloon)が「形が意味を持つ・しっぽで話者を指す」のに対し、メッセージウィンドウは
**画面に固定された矩形パネルにテキストを流し込む** UI 系列。慣習が全く別なので、別タイプとして
持つ判断は正しい。

### 1.1 JRPG メッセージウィンドウ (ドラクエ / FF クラシック〜モダン)

- **Dragon Quest (1986〜)**: 黒地(または濃紺)+ **白の角丸枠**、白文字。テキスト完了後、続きが
  あると **中央下に白い▼三角(continue indicator)** が点滅する。位置は画面の「1 レイヤー上」に
  重ねる固定枠。
- **Final Fantasy (〜FF7, 1997)**: **青のグラデーション地 + ベベル(面取り)エッジ + 白い枠線**。
  この「青箱・白枠」が JRPG ダイアログの視覚標準として定着。
- **枠の作り**: モダンな実装はほぼ **9-slice (nine-patch)** — 四隅は原寸固定、上下左右の辺だけ
  伸縮/タイルしてどんなサイズにも歪まず対応する。**v1 では画像 9-slice をインポートせず、
  角丸+枠線を手続き生成で再現する**(後述)。
- **話者名**: 初期は無名 → やがて「枠内インラインに名前」または「枠の角/上に名前プレート」。
  後年は **顔/立ち絵を枠の上か横に**配置する形へ。
- **continue/▼ 指標**: 標準は白い三角。
- **位置 / ページネーション**: 上 or 下に常駐枠、テキストはワードラップ + ページ送り。テキスト
  速度は静的エディタには無関係だが、**「枠に収まる行数(レイアウト)」はアーティファクトとして
  残る**(RPG Maker は標準 4 行)。
- **モダンの傾向**: 低不透明度の半透明地で背景を透かす、枠線を消してシームレスに溶け込ませる。

### 1.2 ビジュアルノベルエンジン

#### Ren'Py (最も語彙が豊富 — 主要参照元)

ADV モード(1 行ずつ下部の枠) と NVL モード(画面全体に複数行積む) を `Character(kind=nvl)` で
切替。GUI 変数が「レイアウト語彙」をそのまま露出している:

ADV say window (textbox):
| 変数 | 既定値 | 意味 |
|---|---|---|
| `gui/textbox.png` | (画像) | 枠の背景(=9-slice 画像) |
| `gui.textbox_height` | `278` | 枠の縦サイズ(px) |
| `gui.textbox_yalign` | `1.0` | 縦位置プリセット(**0.0 上 / 0.5 中央 / 1.0 下**) |
| `gui.dialogue_xpos` | `402` | テキスト域の左インセット |
| `gui.dialogue_ypos` | `75` | テキスト域の上インセット |
| `gui.dialogue_width` | `1116` | **ワードラップ最大幅** |
| `gui.dialogue_text_xalign` | `0.0` | テキスト揃え(0/0.5/1) |

namebox(名前プレート):
| 変数 | 既定値 | 意味 |
|---|---|---|
| `gui/namebox.png` | (画像) | 名前枠の背景 |
| `gui.namebox_borders` | `Borders(5,5,5,5)` | **9-slice の枠幅(左,上,右,下)** |
| `gui.namebox_tile` | `False` | 枠を伸縮 or タイル |
| `gui.namebox_width/height` | `None` | 既定はテキストに自動サイズ |
| `gui.name_xpos / name_ypos` | `360 / 0` | 名前の位置(枠左上からの px。**負値も可**=枠の上に飛び出す) |
| `gui.name_xalign` | `0.0` | 名前揃え |

NVL モード: `gui.nvl_borders=Borders(0,15,0,30)` / `nvl_height=173` / `nvl_spacing=15` /
`nvl_text_xpos/ypos/width=0.5/60/740` / `nvl_name_xpos/ypos=0.5/0`。

**抽出できる語彙**: 背景画像(9-slice)、枠の borders(9-slice 幅)、padding/inset
(text_xpos/ypos)、ラップ幅(width)、縦位置プリセット(yalign)、名前プレート(別 Frame +
位置/揃え/負オフセット)。

#### RPG Maker (MV/MZ)

- **位置プリセット**: **Bottom / Middle / Top** から選択。
- **背景タイプ**: **Window(枠あり) / Dim(暗いグラデスクリム) / Transparent(枠なし)** の 3 種。
  Dim が「枠なし + 下地グラデ」= frameless scrim。
- **顔グラフィック域**: 顔画像は **メッセージ枠の左側**。標準で **4 行**収まる。
- **名前ボックス**: メッセージの**上に**名前を出す独立枠。
- **continue 指標**: `\!` ボタン入力待ち、`\.`/`\|` ウェイト(静的エディタには ▼ の有無のみ重要)。

#### TyranoScript / Kirikiri (補強)

- メッセージレイヤを `[position]` で配置 — `left top width height marginl marginr margint
  marginb` = **枠位置 + テキストの 4 辺マージン(inset)を別管理**。
- 名前は `[ptext]` で独立した位置(layer/x/y/width/color/size) = **名前プレートは枠とは別に
  自由配置**する思想。

### 1.3 ソーシャルゲーム / モバイル ADV 下部メッセージ

- **frameless(枠なし)**: テキストを直接、下端の **グラデーションスクリム(下が濃く上が透明へ
  抜ける帯)**の上に乗せる。RPG Maker "Dim" と同思想。
- **framed(枠あり)**: 角丸の半透明パネル。
- 共通要素: **話者ラベル・チップ**(枠の左上に小さい名前バッジ)、**立ち絵/ポートレート**
  (左または右)、**全幅 vs インセット**(左右に余白を残す)。

### 1.4 コミックのキャプションボックス

吹き出しと並ぶ別概念として、コミックには **caption box(ナレーション枠)** がある: **矩形・
パネル端寄り・色付き枠**で、白い吹き出しと区別する。ナレーション/場所時間/モノローグに使う。
既存 comic_lab の `RoundRect{corner=0}` 吹き出しで概ね表現できるが、「枠付き矩形 + テキスト」
という点で MessageWindow と連続性がある。

> 既存の `speech-bubble-text-tool-plan.md` は「ナレーション枠 = `RoundRect{corner=0}` +
> しっぽ無し」で済ませている。MessageWindow はその延長を **位置プリセット・名前プレート・
> 立ち絵・continue 指標・スクリム背景**まで拡張したもの、と位置づけられる。

---

## 2. タクソノミー(変動の直交軸)

| 次元 | 取りうる値 | 出典 |
|---|---|---|
| **frame style 枠** | None / SolidRounded(角丸+単線) / DoubleLine(二重線,FF/JRPG) / Beveled(将来) / 9-slice画像(**v1非対応**) | DQ白枠 / FFベベル / Ren'Py borders |
| **background fill 背景** | None / Solid / Translucent(半透明α) / GradientScrim(下濃→上透明) | RPG Maker Window/Dim/Transparent, scrim |
| **corner radius 角丸** | px (0 = 矩形キャプション) | Ren'Py / caption box |
| **position preset 位置** | Top / Bottom / Center / Free | RPG Maker top/mid/bottom, Ren'Py yalign |
| **size mode サイズ** | FullWidth(横マージン) / Inset(中央寄せ固定) / AutoFitText(文字に合わせ) | Ren'Py dialogue_width, RPG Maker 行数 |
| **padding 内側余白** | px(4辺。簡易は一律) | Ren'Py dialogue_xpos/ypos, Tyrano margin* |
| **name plate 名前** | None / InlineCorner(枠内左上チップ) / SeparateBoxAbove(枠の上に別枠) | RPG Maker name box, Ren'Py namebox(負ypos), ソシャゲchip |
| **portrait 立ち絵** | None / Left / Right | RPG Maker face left, ソシャゲ portrait |
| **text align / vertical anchor** | 水平: Start/Center/End (既存`TextAlign`)、垂直: Top/Center/Bottom | |
| **continue indicator** | None / Triangle(▼) | DQ ▼, RPG Maker `\!` |
| **outline / shadow** | テキスト袋文字(既存`TextBlock.outline`) + パネルのドロップシャドウ(将来) | 可読性 |

**グルーピング方針**: frame style と background fill は **直交**(枠なし+スクリム、枠あり+
半透明 が全部成立)なので別フィールド。9-slice は `frame` の将来バリアントとして名前だけ予約し、
v1 は手続き(SolidRounded / DoubleLine)に限定。

---

## 3. comic_lab への提案(データモデル + bake + UI + 実装計画)

既存の `model.rs` / `raster.rs` / `tessellate.rs` の規約に厳密に合わせる。座標は source
image-pixel space、`#[serde(default)]` 多用で前方互換、色は `Rgba`(straight alpha)、枠線は
`StrokeStyle`、テキストは **`TextBlock` を再利用**。

### (a) `MessageWindowObject` 構造体と `AnnotationKind` への組み込み

```rust
/// 枠の描き方。9-slice 画像は将来バリアントとして名前だけ予約 (v1 は手続きのみ)。
pub enum FrameStyle { None, SolidRounded, DoubleLine /* , 将来 NineSlice */ }
// default = SolidRounded

/// 背景の塗り方。
pub enum FillMode { None, Solid, Translucent, GradientScrim }  // default = Solid

/// 画面内の位置プリセット (pivot はベイク時にこれと size から解決)。
pub enum WindowPosition { Top, Bottom, Center, Free }  // default = Bottom

/// 横サイズの決め方。
pub enum SizeMode {
    FullWidth { margin_px: f32 },  // 画像全幅 - 左右マージン
    Inset,                         // 中央寄せ固定 (half_w/half_h)
    AutoFitText,                   // 文字に合わせ自動 (BubbleObject.auto_size 同思想)
}                                  // default = FullWidth{48.0}

pub enum NamePlateMode { None, InlineCorner, SeparateBoxAbove }  // default None
pub enum PortraitSide { None, Left, Right }                      // default None
pub enum ContinueIndicator { None, Triangle }                   // default None
pub enum VAnchor { Top, Center, Bottom }                        // 縦専用 (TextAlign は水平)

pub struct NamePlate {
    pub mode: NamePlateMode,
    pub name: TextBlock,                  // ← TextBlock 再利用
    pub fill: Option<Rgba>, pub fill_opacity: f32,
    pub outline: StrokeStyle, pub corner_px: f32, pub padding_px: f32,
    pub offset: (f32, f32),               // 枠左上からの px。負値で枠の上に飛び出す
}

pub struct MessageWindowObject {
    // パネル形状
    pub size_mode: SizeMode, pub position: WindowPosition,
    pub half_w: f32, pub half_h: f32, pub corner_px: f32,
    // 背景
    pub fill_mode: FillMode, pub fill: Option<Rgba>, pub fill_opacity: f32,
    pub scrim_dense_side: VAnchor,
    // 枠
    pub frame: FrameStyle, pub outline: StrokeStyle, pub frame_gap_px: f32,
    // テキスト
    pub text: TextBlock,                  // ← TextBlock 再利用
    pub padding_px: f32, pub v_anchor: VAnchor,
    // 付帯要素
    pub name_plate: NamePlate,
    pub portrait: PortraitSide, pub portrait_w: f32, pub portrait_fill: Option<Rgba>,
    pub continue_indicator: ContinueIndicator,
    // プリセットリンク (BubbleObject.shape_preset_link と同契約)
    pub style_preset_link: Option<String>,
}

pub enum AnnotationKind {
    Bubble(BubbleObject),
    Text(TextBlock),
    MessageWindow(MessageWindowObject),   // ← 追加
}
```

**要対応箇所(既存 match の拡張)**:
- `text_block()` / `text_block_mut()` に `MessageWindow(w) => &w.text` を追加。
- `AnnotationObject::new_message_window(...)` コンストラクタ追加(`new_bubble` に倣う)。
- `raster.rs` の `bake_object_unrotated` / `object_local_aabb` の match に `MessageWindow` 枝。
- `bake_overlay` のマージ判定(`matches!(.., Bubble(_))`)は MessageWindow を対象外のまま
  → 自動的に単体 bake 経路に乗る(MessageWindow は結合しない)。

> **`pivot` 解釈**: comic-core は画像 W/H を知らないので Top/Bottom を px に解決できない。
> **comic-core は `pivot` を矩形中心の絶対座標として受け取り、プリセット→pivot 変換は
> lab/本体側(画像サイズを知る層)で行う**(しっぽ tip が絶対座標なのと同じ流儀)。
> `WindowPosition` はデータとして保持し、リサイズ/画像差し替え時に lab が pivot を再解決。

### (b) 同梱する「ウィンドウスタイルプリセット」(system presets)

1. **DQ風 紺枠** — 濃紺/Solid, DoubleLine, 白2px, corner=8, Bottom, FullWidth, 白文字。
2. **FF風 青ベベル** — Translucent青, SolidRounded(将来Beveled), 白枠, 白袋文字。
3. **ノベル 半透明スクリム** — frame=None, GradientScrim(下濃黒α), Bottom, FullWidth, 白袋文字。
4. **ADV 枠なし下部** — frame=None, Translucent黒α≈0.55, corner=0, Bottom, 白文字。
5. **名前プレート付き** — 上記 + name_plate=SeparateBoxAbove(offset 負で枠上にチップ)。
6. **ノベル白枠(VN標準)** — 白α≈0.85, SolidRounded, 濃グレー枠, corner=16, 黒文字。
7. **コミックキャプション** — corner=0, Solid(淡黄/白), 細枠, Inset, Free, 黒文字(ナレーション)。

> 立ち絵付きプリセットは v1 では出さない(実画像が無いとプレースホルダ矩形は用途が薄い)。

### (c) bake プラン(描画順)

`draw_bubble_parts` に倣い `draw_message_window_parts(overlay, pivot, win, fonts)` を新設:

1. **背景 fill** — Solid/Translucent は角丸矩形ポリゴンを `fill_polygon`。GradientScrim は
   `draw_soap_bubble` 同様 per-pixel で `scrim_dense_side` から反対辺へ線形に α を落とす新関数
   `fill_scrim_rect`。
2. **立ち絵プレースホルダ**(portrait != None) — 枠の左/右に `portrait_w` 幅の塗り矩形。
   テキスト域はこの分内側に狭める。
3. **枠 frame** — SolidRounded は `stroke_polygon`、DoubleLine は外側 + `frame_gap_px` 内側に
   もう一本、None は何もしない。
4. **名前プレート**(mode != None) — fill → stroke → 名前テキスト(`bake_text`)を枠左上 +
   `offset` 起点に。SeparateBoxAbove は枠上辺の上(負 y)、InlineCorner は枠内左上。
5. **本文テキスト** — テキスト域 = パネル矩形 - padding - (立ち絵幅) - (InlineCorner名前高)。
   `v_anchor` と `text.align` で配置。**`bake_text` に「描画矩形 + 水平/垂直揃え」を渡す薄い
   ラッパ `bake_text_in_rect`** を追加(レイアウトエンジン本体は無改造、`layout_text().bounds`
   で origin 算出)。
6. **continue ▼** — Triangle のときパネル下辺中央に小さい三角ポリゴンを `fill_polygon`(色は
   `text.color`)。▼ は**ベイク内ポリゴンで描く**ので `check_ui_glyphs.py` 対象外。

回転は既存 `bake_into` がそのまま効く(`object_local_aabb` に MessageWindow 枝を足すだけ)。
AABB 枝 = 「パネル矩形 ∪ 名前プレート(負offset含む) ∪ 立ち絵 ∪ ▼ ∪ テキスト袋文字幅」+
枠線半幅。`tessellate` 側は **既存 `round_rect` をそのまま再利用**(corner=0 で矩形)。

### (d) 右パネルのタブ構成

MessageWindow には**しっぽが無い**。既存 `PropTab{Serifu,Body,Tail,Deco}` を種別で読み替え:

| タブ | MessageWindow での内容 | 色 |
|---|---|---|
| **セリフ** | 本文 + 垂直アンカー + padding (`draw_text_body`/`draw_text_font`/`draw_serifu_tab` 流用) | 青 |
| **枠(本体)** | position / size mode / corner / fill mode (+scrim方向) / frame style (+double間隔) / 枠線 | 緑 |
| **名前/立ち絵** | name plate mode + 名前テキスト/色/プレート + offset / portrait side+幅+色 / continue ▼ | 橙(「しっぽ」枠を読み替え) |
| **飾り** | (将来) ドロップシャドウ等。v1 は非表示 | 金 |

`prop_tab` グローバルを共有したまま、選択が MessageWindow のとき各タブを MessageWindow 用
関数(`tab_window_body` / `tab_window_name` 等)へ dispatch。常時表示部(本文/フォント/プリ
セット)は吹き出しと共通の `draw_text_body`/`draw_text_font` をそのまま使える。

### (e) 段階的実装計画

**v1(最小・現実的): 静的エディタ、CPU bake の WYSIWYG、アニメ無し**
- `FrameStyle{None,SolidRounded,DoubleLine}`(**手続き生成のみ、9-slice 画像なし**)。
- `FillMode{None,Solid,Translucent,GradientScrim}`。
- `WindowPosition{Top,Bottom,Center,Free}` / `SizeMode{FullWidth,Inset,AutoFitText}` /
  `corner_px` / `padding_px` / `v_anchor` + 既存 `TextAlign`。
- `NamePlateMode{None,InlineCorner,SeparateBoxAbove}`(名前は `TextBlock` 再利用)。
- `ContinueIndicator{None,Triangle}`。
- `MessageWindow` を `AnnotationKind` に追加、bake/AABB/text_block の match 更新、回転は既存
  ラッパで自動対応。system プリセット 7 種。右パネルタブ §(d)。`bake_text_in_rect` ラッパ +
  `fill_scrim_rect` 新関数。
- comic-core テスト: 「Solid 矩形がピクセルを書く」「DoubleLine は SolidRounded より枠
  ピクセルが多い」「scrim は濃い側 α > 反対側 α」「FullWidth で margin を引いた幅」「名前
  プレートの負 offset が枠上に出る」。

**deferred(将来)**
- **9-slice 画像インポート**(`FrameStyle::NineSlice` 予約済)。
- **立ち絵に実画像**(v1 は色プレースホルダ矩形のみ、または portrait UI 自体を後回し)。
- Beveled フレーム(FF風面取り)、ドロップシャドウ/グロー、2 色グラデ fill。
- 詳細 padding(4辺個別)、ページネーション/行数上限プレビュー。
- 本体 mImageViewer 統合(吹き出しと同じ最前面オーバーレイ段に合流)。

---

## 4. 統合機能チェックリスト (Claude 案 + Codex 案の和集合)

同じ調査を Codex (gpt-5.5, read-only, 自前知識 + コードベース読込) にも独立依頼し、§1〜3
(Claude 調査エージェント) と突き合わせて**漏れの少ない**機能リストにしたもの。

### 4.1 各案が独自に拾った点 (diff)

**Codex が追加で挙げた (Claude §1-3 が欠落/過小だった) — 重要な gap**:
- **テキストの折返し/オーバーフロー**: 幅制約ワードラップ / content rect クリップ / **最大行
  ガイド・警告** / fit-to-size。Claude 案は本文ラップに一切触れていなかった(吹き出し流の手動
  改行前提)。全幅ウィンドウでは要検討。← **最大の gap**
- **セーフエリア/端インセット** (モバイル下端、crop 端回避)。
- **outer パネル矩形と inner テキスト content rect を明示的に区別**(Claude は padding のみ)。
- **背景 fill の細分化**: ローカル Dim / 線形グラデ(パネル全体) を edge-scrim と別立て。
- **per-corner 角丸** (CornerRadii{tl,tr,br,bl})、inner-line / corner-bracket フレーム。
- **名前プレートのモード拡充**: プレーンラベル(枠なし) / タブ・チップ を別モードに。
- **続き指標の種類**: 三角だけでなく chevron / diamond / dots、固定コーナー vs text-end。
- **単純ドロップシャドウを v1 候補**に(Claude は影を全て後回し)。
- **lab 統合の実務項目**: 追加ダイアログのプリセットサムネ / オブジェクト一覧ラベル /
  `WindowStylePreset` 新設(ShapeStylePreset は流用しない)。
- **前方互換 enum**: choice サブボックス / NVL 複数エントリ / リッチテキスト(ruby/icon/色) /
  タイムスタンプ widget / スピーカーを指す pointer / アニメ hooks / 外部スクリプト export。

**Claude 案が独自に持っていた (Codex が出さなかった) 強み**:
- Ren'Py GUI 変数の**具体的既定値**(textbox_height=278, namebox_borders=Borders(5,5,5,5),
  dialogue_width=1116 等) — 実装時のデフォルト値の根拠。
- **system プリセット 7 種の具体パラメータ**(色/corner/枠太さまで)。
- `fill_scrim_rect` を **`draw_soap_bubble` 流用の per-pixel** で実装する具体策。
- **▼ はベイク内ポリゴンで描く**(フォント依存記号を UI に増やさない方針=`check_ui_glyphs`
  と整合) — コードベース固有の良い指摘。
- VAnchor + 既存 `TextAlign` 流用の明示。

両案が一致: 新 `AnnotationKind::MessageWindow`、pivot=絶対座標(プリセット解決は lab 側)、
`TextBlock` 再利用、9-slice は v1 非対応で enum 予約、`bake_text_in_rect` ヘルパ新設、
`bake_into` 回転そのまま、`object_local_aabb` 拡張、raster テスト。

### 4.2 統合チェックリスト ([v1] / [v1?]=要判断 / [後回し])

**オブジェクト/座標/配置**
- [v1] `AnnotationKind::MessageWindow(MessageWindowObject)` 追加
- [v1] pivot=矩形中心の絶対座標、位置プリセット→pivot 解決は lab/本体側
- [v1] 位置プリセット Top / Middle / Bottom / Center / Free + カスタムドラッグ + リサイズハンドル
- [v1] 全幅バンド(左右マージン) / Inset(中央寄せ固定)
- [v1] 既存 `rotation_rad` を bake で尊重
- [v1] **セーフエリア/端インセット(per-edge)** ← Codex 追加
- [v1?] AutoFitText / content-fit(文字に合わせ自動): Claude は v1、Codex は後回し。要判断
- [後回し] auto-height(幅固定で高さ自動)

**背景 fill**
- [v1] None / Solid / 半透明(opacity を text と独立)
- [v1] ローカル Dim パネル / グラデーションスクリム(端→透明) / 全幅バンド・全画面スクリム
- [v1?] 線形グラデ(パネル全体 2 色) ← Codex 追加。v1 か後回しか要判断

**枠 frame**
- [v1] None / 単線 / 二重線(gap) / 枠色・太さ独立
- [後回し] inner-line / bevel・inset / corner-bracket / image・9-slice 予約(tiling vs stretch)

**角丸 / padding / 内側矩形**
- [v1] 一様 corner radius(0=矩形キャプション)
- [後回し] per-corner 角丸 ← Codex 追加
- [v1] padding(まず一律)、[v1?] per-side padding(Codex は v1)
- [v1] **outer パネル矩形と inner テキスト content rect を区別**、portrait 分を content rect から除外 ← Codex

**本文テキスト**
- [v1] body = `TextBlock` 再利用(フォント/サイズ/色/向き/揃え/gap/袋文字/記法 流用)
- [v1] 縦アンカー Top/Middle/Bottom + 横/縦書き + 横 start/center/end
- [v1] **content rect クリップ** + **最大行ガイド/警告** ← Codex 追加
- [v1?] **幅制約の簡易ワードラップ** ← Codex は v1。Claude 案は手動改行のみ。
  layout.rs に折返し追加が要る非自明作業。**最重要の要判断**(v1=手動改行+幅ガイドのみ /
  v1=簡易ラップも入れる)
- [後回し] kinsoku/高度ワードラップ、縦書き自動列調整、shrink-to-fit、真のページネーション

**名前プレート**
- [v1] None / プレーンラベル(枠なし) / ボックス namebox / タブ・チップ / フローティング(枠上/重なり)
  ← Codex でモード拡充
- [v1] 負オフセットで枠上に飛び出す / auto幅・固定W/H / padding / 独立 fill・frame・radius
- [v1] 名前テキストは独立 `TextBlock`

**立ち絵/顔スロット**
- [v1] None/Left/Right + 幅 + content rect から除外 + プレースホルダ塗り
- [後回し] 実画像差込 / crop形状(角・丸・円) / パネル外へはみ出し

**続き指標**
- [v1] None / 三角▼(ベイク内ポリゴン) + chevron/diamond/dots + 固定コーナーアンカー
- [後回し] text-end アンカー / 点滅・バウンスアニメ

**影/グロー**
- [v1?] 単純ドロップシャドウ(オフセット矩形) ← Codex は v1、Claude は後回し。要判断
- [後回し] ぼかし影 / outer glow / inner highlight・gloss

**装飾/付帯 (ほぼ後回し)**
- [後回し] コーナー飾り / 区切り線 / スピーカーアイコン / 場所・時刻・タイムスタンプ ラベル
- [後回し] スピーカーを指す pointer/notch/callout(windows は基本 tailless)
- [後回し] choice サブボックス・メニュー / NVL 複数エントリ積み・2カラム・履歴
- [後回し] リッチテキスト(inline 色/アイコン/絵文字/ruby/サイズ/per-run)
- [後回し] アニメ hooks(opacity/reveal/typing メタ) / Ren'Py・RPGMaker・Tyrano への export

**プリセット & lab 統合**
- [v1] **`WindowStylePreset` 新設**(ShapeStylePreset は流用しない)、style_preset_link 個別編集で解除
- [v1] 追加ダイアログにウィンドウ用プリセットサムネ + オブジェクト一覧ラベル
- [v1] system プリセット(統合 ~10 種): JRPG青グラデ / JRPG黒クラシック / RPGMaker Window /
  RPGMaker Dim / Social枠なし下部スクリム / Social枠あり下部 / VN ADV下部 textbox /
  VN NVL全画面 / コミックキャプション白 / 場所・時刻キャプション
- [v1] 右パネルタブ: セリフ(本文+presets) / 本体(位置・サイズ・fill・frame・padding) /
  **橙タブを「名前・立ち絵・影・指標(部品)」に読み替え** / 飾りは非表示。
  しっぽは非表示、bubble の merge/tail/deco コントロールは出さない

**bake / core**
- [v1] bake order: scrim → shadow → fill → portrait → frame(outer/inner) → body(clip/align) →
  name plate → continue indicator
- [v1] `bake_text_in_rect(block, rect, x_align, y_anchor, wrap, clip)` ヘルパ(`TextBlock` 無改造)
- [v1] `fill_scrim_rect`(per-pixel、`draw_soap_bubble` 流用)
- [v1] `bake_into` 回転そのまま / `object_local_aabb` に window・shadow・name plate・portrait・
  indicator・text-outline を含める
- [v1] raster テスト: solid fill / frame / name plate / indicator / placement-neutral baking
- [後回し] snapshot 追加(UI 安定後)

### 4.3 着手前に決める点 (要ユーザー判断)
1. **本文の自動ワードラップ** — 一番大きい判断。v1 で幅制約ラップを入れる(layout.rs に折返し
   追加、非自明)か、v1 は**手動改行 + 最大幅/行数ガイドのみ**で自動ラップは後回しか。
2. **ドロップシャドウ** — v1 に入れる(Codex)か後回し(Claude)か。
3. **AutoFitText / 線形グラデ fill / per-side padding** — それぞれ v1 か後回しか。
4. **タブ構成** — 橙タブを「部品(名前/立ち絵/影/指標)」に読み替えで OK か。
5. **プリセット種類** — 統合 ~10 種でよいか(増減)。

---

## 重要な実装メモ(レビュー観点)

- **9-slice はやらない**(指示)。FF/DQ 枠は SolidRounded/DoubleLine の手続きで十分、
  `tessellate::round_rect` がそのまま使える。`NineSlice` は enum に名前だけ予約。
- **TextBlock を必ず再利用**(本文と名前)。縦書き/縦中横/袋文字/マーカー記法が無償で乗る。
- **`pivot` 解決は lab/本体側**(comic-core は絶対座標のみ受ける)。
- **MessageWindow はマージ対象外** — 既存 `matches!(.., Bubble(_))` 判定でそのまま単体 bake。
- **`bake_text` の一般化が唯一の非自明作業** — 現状 `centered` bool のみ。矩形内の
  水平/垂直揃え + inset を効かせるラッパが要る(レイアウトエンジン本体は無改造)。
- ▼ はベイク内ポリゴンで描く(フォント依存記号を UI に増やさない方針と整合)。

---

## 参照ソース

- Ren'Py: GUI Customization Guide / NVL-Mode Tutorial / ADV vs NVL
  (renpy.org/doc/html/gui.html, nvl_mode.html)
- RPG Maker MZ: Messages 公式ヘルプ (rpgmakerofficial.com/product/MZ_help-en/01_10_01.html)
- TyranoScript: Control Characters (tyranoscript.com/usage/tech/chara)
- JRPG ダイアログ史: champicky.com/2020/09/15/dialog-box-in-jrpgs/
- 9-slice: Unity 9-Slicing / Kenney Fantasy UI Borders
- スクリム: Sky UI Scrim component
- コミックキャプション: BW Spotlight / Blambot comic grammar
