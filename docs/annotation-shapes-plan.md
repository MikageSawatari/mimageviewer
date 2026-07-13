# 注釈図形 (マーキング) 機能 仕様書

ステータス: **Stage 1〜4 実装済み (v1 完了)** (2026-07-13)。
体制: 実装 = Codex Sol (`codex exec`) / ブリーフ・レビュー・検収 = ClaudeCode /
実機確認 = ユーザー。本書が正本。

関連正本: [comic-integration-plan.md](comic-integration-plan.md)
(層構造・保存・合成の契約)、[stamp-feature-design.md](stamp-feature-design.md)
(スタンプ機構)。

---

## 0. 決定事項サマリ (2026-07-13 ユーザー確定)

| # | 論点 | 決定 |
|---|---|---|
| 1 | 蛍光マーカーの合成 | **乗算 (Multiply) を採用**。`blend` フィールドを最初からモデルに持たせる (§4.3/§5) |
| 2 | v1 スコープ | **Tier 2 (番号バッジ・カーソルスタンプ) まで含める** (§3) |
| 3 | 設置方式 | **クリック設置で統一** (既存ツールと同じ作法)。ドラッグ描画は不採用 (§6.2) |
| 4 | メニュー文言 | **「注釈追加」** |
| 5 | 乗算の z 順 | **作成順 (z 順) で処理**。マーカーはその時点までの合成結果へ乗算する (吹き出し内テキストへのマーキングも可)。「常に通常注釈の下敷き」案は不採用 (§5) |
| 6 | 乗算オブジェクトの描画モード | **1 オブジェクト = 1 モード**。fill だけ乗算・枠線だけ通常のような混在はしない。マーカーは枠線なしの塗り専用図形とし、枠線・テキストの編集 UI を出さない (§4.3) |

---

## 1. 目的・ユースケース

テキスト注釈ツール (comic、Ctrl+T) に、スクリーンショット・写真向けの
「注釈図形」を追加する。

1. **操作説明スクショの強調**: ボタン・UI 要素を赤い長方形で囲む
2. **注目位置の指示**: 矢印 / マウスカーソル風スタンプで場所を指す
3. **写真の注目箇所**: 赤い楕円で囲む
4. **文書写真・文字スクショのマーキング**: 蛍光カラーで下線・文字マーク (半透明)

既存の「追加」ボタン群 (吹き出し/ウィンドウ/テキスト/オノマトペ/スタンプ) に
**「注釈追加」** ボタンを足し、プリセットダイアログから設置する。

---

## 2. 調査サマリ (2026-07-13 実施)

### 2.1 市場調査 (注釈ツール 14 本)

Snagit / Windows Snipping Tool / ペイント / Greenshot / ShareX / Flameshot /
PicPick / Lightshot / Monosnap / macOS プレビュー / CleanShot X /
iPhone マークアップ / LINE / Adobe Acrobat を公式ドキュメント中心に調査した。

**図形の保有率 (14 ツール中)**:

| 図形 | 保有数 | 備考 |
|---|---|---|
| 長方形 (枠) / フリーハンド / テキスト | 14 | 全ツール保有 |
| 矢印 / 直線 | 13 | LINE のみ非搭載 |
| 楕円 | 12 | |
| 蛍光マーカー | 12 | 黄・半透明が慣習 |
| ぼかし/モザイク | 9 | mIV は隠蔽ツールで提供済み |
| 吹き出し | 8 | mIV は提供済み |
| 番号ステップバッジ (1,2,3…) | 8 | **操作説明特化ツールは例外なく保有** |
| 拡大鏡 | 6 | |
| スポットライト (領域外暗転) | 4 | |
| カーソルスタンプ | **3** | Snagit / ShareX / PicPick のみ。**差別化要素** |

**既定スタイルの慣習**:

- 既定色は **図形 = 赤、マーカー = 黄** がデファクト。実績値: ShareX は
  ソースコード上 `RGB(242, 60, 60)`・枠 4px・実線が既定。Flameshot も既定赤。
  Snagit の代表プリセットも赤矢印・黄円。
- 線太さは 4px 前後。1-2px はスクショ縮小時に消えるため避けられる。
- 高機能系 (Snagit/ShareX/Greenshot/プレビュー) は注釈をベクタオブジェクトとして
  保持し後から選択・移動・スタイル変更できる。mIV の非破壊注釈と同じ思想。
- 設置方式は Windows 系 = ドラッグ描画、macOS プレビュー系 = クリック設置→
  ハンドル調整の二流派。番号バッジ・スタンプは全ツールでクリック設置。
  → mIV は既存ツールの作法 (クリック設置) に統一する (§0 決定 3)。

**蛍光マーカーの実装 3 方式**: 矩形ドラッグ型 (Snagit/ShareX/Greenshot) /
フリーハンド型 (Snipping Tool/iPhone/Flameshot) / テキスト検出スナップ型
(CleanShot X/Acrobat)。ユースケース④ (文書の行マーク) には**矩形型が最適**。
フリーハンドは既存モデルに線オブジェクトが無く、追加コストの割に用途が薄い。

### 2.2 既存実装 (comic 注釈) の土台

必要な機構の大半が既存で、モデル変更は最小で済む:

| 要素 | 既存状況 |
|---|---|
| 長方形 | `BubbleShape::RoundRect { corner_px }` (corner_px=0 で直角) が既存 |
| 楕円 | `BubbleShape::Ellipse` が既存 |
| 矢印 | `BubbleShape::Arrow` は**テキスト内包用のブロック矢印** (先端三角 = 全長の前 45%・軸半太 = half_h の 45% の固定比率)。注釈用の細い線矢印はパラメータ拡張が必要 (§4.2) |
| カーソルスタンプ | `StampObject` + SVG→resvg ラスタライズ機構 (`comic_stamp.rs`) がそのまま流用可 |
| 選択/移動/リサイズ/回転/undo | Inc 6 のハンドル・専用 undo スタックがそのまま効く |
| 色・不透明度 UI | `color_edit_button_srgba` + `fill_opacity` の前例多数 |
| 蛍光マーカー (乗算) | **前例なし**。合成は全経路 straight-alpha source-over のみ (§5 で拡張) |

**永続化と互換性 (本機能の基本方針の根拠)**: `comic.db` に
1 画像 = `Vec<AnnotationObject>` の serde JSON (externally tagged enum)。

- **enum バリアント追加は危険**: 新バリアント入り JSON を旧バイナリで読むと
  その画像の `Vec` 全体のパースが失敗 → 注釈が丸ごと非表示。さらに旧バイナリで
  その画像を編集保存すると実データ喪失。comic 注釈は v1.1.0 でリリース済みなので
  この非対称は現実のリスク。
- **`#[serde(default)]` フィールド追加は安全**: 旧バイナリは未知フィールドを
  無視して読める (劣化表示のみ・データ喪失なし)。

→ **バリアント追加を避け、フィールド加法拡張で実現する** (§4.6)。

---

## 3. v1 スコープ (確定)

| # | 図形 | 実現方法 | 既定スタイル |
|---|---|---|---|
| 1 | 枠長方形 | Bubble プリセット (RoundRect, fill なし) | 赤 (242,60,60)・角丸 0 |
| 2 | 枠角丸長方形 | 同上 (corner_px > 0) | 赤・角丸 = 線太さ×2 |
| 3 | 枠楕円 | Bubble プリセット (Ellipse, fill なし) | 赤 |
| 4 | 注釈矢印 | `Arrow` のフィールド加法拡張 (§4.2) | 赤・塗り三角先端 |
| 5 | 蛍光マーカー | RoundRect + fill + 乗算合成 (§4.3/§5) | 黄 (255,235,59) |
| 6 | 蛍光下線 | 5 と同一オブジェクトの高さ細いプリセット | 黄 |
| 7 | 番号バッジ (①②③…) | Bubble プリセット (Ellipse + fill 赤 + 白数字) + 自動採番 (§4.5) | 赤地・白太字 |
| 8 | カーソルスタンプ | 同梱 SVG アセット追加 (§4.4) | 白矢印/黒矢印/指差し/I ビーム/クリックリング |

### 見送り (既存機能で代替 / 将来検討)

- **ぼかし/モザイク** — 既存の隠蔽ツールが提供済み (注釈レイヤーへ統合しない)
- **吹き出し・テキスト** — 既存機能そのまま
- **チェック✓・×マーク** — 既存スタンプ (twemoji ✅❌ 等) で代替
- **フリーハンドペン** — 線オブジェクトが無く、ユースケース上の必然性が薄い
- **ドラッグ描画設置** — 不採用 (§0 決定 3。クリック設置で統一)
- **拡大鏡・スポットライト・ドロップシャドウ・カーブ/テーパー矢印・
  スタイルプリセット保存 (Snagit Quick Styles 風)** — 将来候補

---

## 4. データモデル

**新しい `AnnotationKind` は追加しない。** 既存の `BubbleObject` / `StampObject` を
「注釈プリセット」として使い、足りない表現力はフィールドの加法拡張で足す。
comic-core の変更は comic-integration-plan §2 の「純加法・単体テスト付き」の
範囲に収める。

### 4.1 枠長方形・枠楕円 — モデル変更なし

```rust
BubbleObject {
    shape: RoundRect { half_w, half_h, corner_px: 0.0 },  // または Ellipse
    fill: None,
    outline: StrokeStyle { color: 赤, width_px: 既定太さ },
    text: TextBlock { text: "" },   // テキストなし
    auto_size: false,
    ..
}
```

テキスト空の吹き出しとして成立させる。空テキスト時のレイアウト・当たり判定・
ベイクが問題ないことをテストで確認する (問題があればその修正も加法で行う)。

### 4.2 注釈矢印 — `BubbleShape::Arrow` のフィールド加法拡張

**新フィールド (いずれも `#[serde(default)]`、None = 現行と同一形状)**:

```rust
Arrow {
    half_w: f32,
    half_h: f32,
    #[serde(default = "arrow_up")]
    dir_rad: f32,
    /// 先端三角の長さ (px)。None = 従来比率 (全長の前 45%)。
    #[serde(default)]
    head_len_px: Option<f32>,
    /// 軸の半太さ (px)。None = 従来比率 (half_h の 45%)。
    #[serde(default)]
    shaft_half_px: Option<f32>,
}
```

- 注釈矢印プリセット = `head_len_px: Some(線太さ×6)` +
  `shaft_half_px: Some(線太さ/2)` + `half_h = 線太さ×3` (先端三角の半幅) +
  `fill: Some(赤)` + `outline: width 0`。塗り三角先端 + 細い軸の注釈矢印になる。
- 7 点ポリゴン生成 (`tessellate.rs::arrow`) の `head_x` / `shaft_hh` 算出を
  Option 値で上書きするだけの加法変更 (clamp: `head_len_px` は全長以下、
  `shaft_half_px` は `half_h` 以下)。既存テストは無修正で緑を維持し、
  新パラメータの幾何テストを追加。
- **向きの扱い (確定)**: プリセット設置時は `dir_rad = 0` (右向き)・object
  rotation = 0 で置き、向き変更は**既存の回転ノブ (object rotation)** で行う。
  `dir_rad` は専用 UI に出さない (二重の回転手段を新設しない)。
- **注釈矢印専用エディタ (実機確認後の確定)**: `shape_preset_link ==
  "miv:annot-arrow"` かつ shape が Arrow のときは、全長・線太さ・先端の長さ・
  先端の幅・塗り色・不透明度だけを本体欄に表示する。形状コンボ、塗り on/off、
  輪郭、結合、セリフ/しっぽ/飾りタブ、通常の本体プリセットは表示しない。
  object rotation の回転スライダーは従来どおり表示する。タグのない通常 Arrow は
  従来の吹き出し UI のままとする。
- **四隅リサイズの比例規則 (実機確認後の確定)**: `half_w` の変更比で
  `head_len_px`、`half_h` の変更比で `shaft_half_px` を追随スケールする。
  Option が None の値は None のまま、変更前の対応 half extent が 0 以下なら
  比率 1 としてゼロ除算を避ける。これにより注釈矢印の先端と軸の比率を保つ。
- **旧バイナリで読むと**: 未知フィールドが無視され従来比率のブロック矢印として
  表示される (劣化表示だが注釈全体は失われない)。
- struct variant へのフィールド追加なので、`Arrow { half_w, half_h, dir_rad }`
  形式の非 `..` パターンマッチはコンパイルエラーで洗い出される
  (comic-core / ui_text.rs / tools/comic_lab の全 match を追随させる)。

### 4.3 蛍光マーカー — `BubbleObject` に合成モードフィールドを追加

```rust
/// オブジェクトの合成モード。既定 Normal (従来の source-over)。
#[derive(Default, ...)]
enum FillBlend { #[default] Normal, Multiply }

BubbleObject {
    shape: RoundRect { corner_px: 小 },
    fill: Some(黄), fill_opacity: 0.55,
    outline: StrokeStyle { width_px: 0.0 },
    #[serde(default)]
    blend: FillBlend,
    text: TextBlock { text: "" },
    ..
}
```

- **`blend` はオブジェクト単位のモード** (決定 6)。`Multiply` のオブジェクトは
  描画のすべてが乗算レイヤーに入り、通常レイヤーには何も出さない
  (fill だけ乗算・枠線だけ通常のような混在はしない)。
- **マーカーには枠線機能を付けない**: プリセットは outline width 0・テキスト空で
  生成し、詳細パネルは `blend == Multiply` のとき枠線・テキスト系のコントロールを
  非表示にする (編集できるのは色・不透明度・形状サイズ・角丸のみ)。防御として、
  データ上 outline やテキストを持つ Multiply オブジェクトが来ても乗算レイヤー側で
  描く (通常レイヤーへ漏らさない = 1 オブジェクト 1 モードの不変条件)。
- Multiply オブジェクトは merge chain (`merge_with_below`) の対象外とする
  (UI で設定不可 + ベイク側ガード)。
- Multiply 時の `fill_opacity` は**乗算の強さ**として解釈する:
  乗算係数 = `lerp(白, fill色, fill_opacity)` (§5)。UI ラベルは既存の
  「不透明度」のままでよい (0 = 無効果、1 = fill 色をフルに乗算)。
- 「蛍光下線」は同じオブジェクトの高さ細いプリセット (別モデル不要)。
- **旧バイナリで読むと**: `blend` が無視され通常アルファの黄矩形として表示
  (文字がやや白っぽくなる劣化表示のみ。データ喪失なし)。

### 4.4 カーソルスタンプ — 同梱 SVG アセットの追加 (モデル変更なし)

- twemoji とは**別枠**のアプリ固有アセット (例: `assets/annotation-stamps/*.svg`)
  を新設し、build.rs で twemoji と同様に `include_bytes!` 同梱 →
  `comic_stamp.rs` のカタログに**「注釈」カテゴリ**として登録。
- 参照は既存 `StampSource::Emoji(キー)` と同じ軽量キー参照。twemoji の
  コードポイントキーと衝突しない命名にする (例: `miv:cursor-arrow-white`)。
- **アセットは描き起こしのオリジナル SVG** (Windows 標準カーソルのビットマップ
  流用はライセンス上不可)。v1 収録 (5 種):
  - 矢印カーソル (白縁黒 / 黒縁白)
  - 指差しハンドカーソル
  - I ビーム (テキストカーソル)
  - クリックリング (同心円の波紋、クリック位置の可視化)
- 旧バイナリで読むと: 未知キーのスタンプは画像が解決できず非表示になるが、
  パースは通るので他の注釈は無事。

### 4.5 番号バッジ — プリセット + 自動採番 (`TextBlock` bool 加法拡張)

- 実体 = `Ellipse + fill 赤 (fill_opacity 1.0) + outline 白 +
  TextBlock(白・太字・中央揃え)` の Bubble。
- バッジ識別は既存の `shape_preset_link` 文字列フィールドに `"miv:step-badge"`
  を入れる (モデル変更なし・旧バイナリ無害)。
- **採番規則 (確定)**: 設置時、その画像内の既存バッジ (`shape_preset_link ==
  "miv:step-badge"`) の本文を整数パースし、**最大値 + 1** をセットする
  (パース不能な本文は無視、既存ゼロ件なら 1)。削除しても再採番はしない
  (Snagit の Step と同じ体験)。
- 数字は普通の TextBlock なので後から手で書き換え可能 (A、B… も可)。
- 数字はグリフの実描画範囲 (インク bbox) の中心を円の中心に合わせる。
  `TextBlock.v_center_ink = true` を番号バッジ preset だけに設定し、横書きのときだけ
  有効とする。既定値は `false` で、既存の吹き出し等は従来の行送り箱中心合わせを
  1px も変更しない。空白のみ・縦書きは従来方式へフォールバックする。

### 4.6 互換性ルール (本機能で守ること)

- **リリース済みの comic 注釈への enum バリアント追加は禁止**。表現力の拡張は
  `#[serde(default)]` フィールド追加で行う。
- `DOC_VERSION` (現 1) は**上げない** (旧バイナリは参照しておらず防御にならない。
  フィールド加法なら上げる必要もない)。
- comic-core への変更はすべて単体テスト付きの加法変更とし、既存テストは
  無修正で緑を維持する。
- `tools/comic_lab` は comic-core を共有しているため、フィールド追加に伴う
  コンパイル追随 (match / struct literal) は行う。lab 側 UI に新フィールドの
  編集コントロールを足すかは任意 (本機能は mIV 側ダイアログが正)。

---

## 5. 描画: 乗算 (Multiply) 合成パスの設計

現状の合成は全経路 straight-alpha source-over のみ
(`RgbaOverlay::blend_px` → `composite_overlay_over`)。オーバーレイは下地を
知らずに焼かれる一枚絵なので、**乗算は既存経路上では表現不可能**
(乗算は合成時に下地画素が必要)。

**設計: z 順セグメント合成 (決定 5)**

1. **ベイク**: z 昇順にソートしたオブジェクト列を、`blend` の切り替わり位置で
   **セグメント分割**する (連続する Normal → 1 つの通常オーバーレイ、連続する
   Multiply → 1 つの乗算バッファ)。comic-core に純加法ヘルパー
   `bake_annotation_layers(objects, w, h, fonts, stamps) -> Vec<AnnotationLayer>`
   を追加する (`enum AnnotationLayer { Normal(RgbaOverlay), Multiply(RgbaOverlay) }`)。
   - 通常セグメントのベイクは既存 `bake_overlay_with_stamps` の機構 (merge chain
     グループ化 + rayon 並列 + AABB バッファ) をセグメント単位でそのまま流用する。
   - 乗算セグメントは白 (255,255,255 = 無効果) 初期化のバッファへ、オブジェクトの
     全描画を画素値 = `lerp(白, fill色, fill_opacity)` の**乗算累積**で焼く
     (重ね塗りで濃くなる = 実物の蛍光ペンと同じ)。回転は既存の回転合成機構を
     流用。バッファはセグメントの AABB に限定してよい。
   - **Multiply オブジェクトが 1 つも無ければ従来どおり単一の通常オーバーレイ**
     になり、既存経路の出力・コストは不変 (バイト一致をテストで担保)。
2. **合成**: 共通ヘルパー `composite_annotation_layers(base, &layers)`
   (mIV 側 `comic_overlay.rs`、既存 `composite_overlay_over` の一般化) に一本化し、
   列を先頭から `Normal → source-over` / `Multiply → dst × 係数` の順で下地へ
   適用する。合成点は 3 箇所、すべてこのヘルパーを呼ぶ:
   - 表示: `ensure_comic_composite_texture` (app.rs)
   - 本フルベイク: books.rs の comic 合成経路
   - エクスポート: `comic_composited_pixels_for_export` (app.rs)
3. **z 順の意味**: 一覧の順序 = 塗り重ね順。マーカーはその時点までの合成結果
   (下地 + それより下の注釈) に乗算されるため、吹き出しやテキスト注釈の**上**に
   置いたマーカーはそれらのテキストごとマーキングされ、**下**に置いたマーカーは
   不透明な吹き出しに隠れる (見たまま = WYSIWYG)。新規オブジェクトは最上位 z に
   追加される既存規則のため、「後から引いたマーカーは重なった注釈ごと色が乗る。
   嫌なら一覧で下へ移動」という操作体系になる。同一セグメント内のマーカー同士は
   乗算の可換性により順序不問。
4. **スケーリング**: AI アップスケール・エクスポート時は `scale_scene` 済みの
   オブジェクト列から同じヘルパーで焼く (全レイヤーで寸法・座標が一致すること)。
5. 選択ハンドル・ドラッグ中のプレビューは従来どおり egui 側描画 (乗算はベイク
   結果にのみ現れる。編集中の再ベイクは既存の `mark_comic_dirty` 経路で発火)。

---

## 6. UI 仕様

### 6.1 「注釈追加」ボタンとダイアログ

- 左パネルの追加ボタン群 (ui_text.rs「── 追加 ──」ブロック) の
  **「スタンプ追加」の上**に「注釈追加」ボタンを追加 (全幅・26px、既存と同型)。
- クリックで**注釈プリセットのダイアログ**を開く
  (`draw_text_add_bubble_dialog` と同じ作法の新ダイアログ)。構成:

```
[枠(長方形)] [枠(角丸)] [枠(楕円)] [矢印]
[蛍光マーカー] [蛍光下線] [番号バッジ]
[カーソル] → 白矢印 / 黒矢印 / 指差し / Iビーム / クリックリング
色: [■赤] [■橙] [■黄] [■緑] [■青] [■ピンク] + カスタム   線太さ: 細 / 標準 / 太
```

- 色・太さはダイアログ上で設置前に選べる (既定: 図形 = 赤 / マーカー・下線 = 黄。
  マーカーを選んだときは色既定を黄に切り替える)。カスタム色は
  `color_edit_button_srgba`。
- 設置後は既存の詳細パネル (`bubble_body_ui` / `stamp_ui`) でフル編集可能。
  ただしマーカー (`blend == Multiply`) は §4.3 のとおり枠線・テキスト系の
  コントロールを出さない (色・不透明度・形状サイズ・角丸のみ)。
- **前回使ったプリセット・色・太さを記憶**し、次回ダイアログを開いたとき復元する
  (App 内 state で保持。設定 DB への永続化は任意 — 実装が軽ければ
  `settings.rs` 経由で永続化してよい)。

### 6.2 設置方式 (確定: クリック設置で統一)

- ダイアログでプリセットを選んで「追加」→ 既存の吹き出し追加と同じ規則で
  キャンバス上に既定サイズで設置 → 既存ハンドルで位置・大きさ・回転を調整。
- ドラッグ描画は実装しない (既存ツールの作法との一貫性を優先)。
- 設置位置・重なり時のオフセット規則は既存の吹き出し/テキスト追加の慣習に合わせる。

### 6.3 既定スタイル値

| 項目 | 値 | 根拠 |
|---|---|---|
| 図形色 | RGB(242, 60, 60) | ShareX 既定の実績値 |
| マーカー色 | RGB(255, 235, 59) | 黄がデファクト |
| 線太さ (標準) | 画像長辺 × 0.25% を 3〜12px にクランプ | 座標系がソース画素空間のため画像サイズ追従が必要 (テキスト既定サイズ `sh*0.04` clamp(24,96) と同じ考え方)。1920px 画像で約 5px ≒ ShareX の 4px。細 = ×0.6 / 太 = ×1.8 |
| 図形初期サイズ | 長方形/楕円: half_w = 画像幅×12%・half_h = 画像高×7% 目安。矢印: 全長 = 画像長辺×18% 目安 | クリック設置後にハンドル調整する前提の「掴みやすい」サイズ |
| マーカー初期サイズ | half_h = テキスト 1 行相当 (画像高×2%)、下線: half_h = 線太さ×0.75 | 文書の行マーク・下線用途 |
| 角丸 | 線太さ × 2 | Snagit 風 |
| 矢印先端 | head_len = 線太さ×6、shaft_half = 線太さ/2、half_h = 線太さ×3 | 実機確認で先端を大きくし、細い軸との視認性を改善 |
| マーカー濃度 | fill_opacity 0.55 (乗算強度) | 下の文字を完全に残しつつ色がはっきり乗る |
| 番号バッジ | 半径 = 画像長辺×2% clamp、フォント = 半径×1.2 | Snagit Step 相当の存在感 |

数値は実装時の初期値であり、実機の見た目で微調整してよい (ただし既定色 2 つと
「線太さの画像サイズ追従」は確定仕様)。

注釈矢印の四隅リサイズは、長さ軸比を `head_len`、交差軸比を `shaft_half` にも
適用して既定の x6 / x3 比率を含む描画時のプロポーションを維持する。専用エディタで
各値を直接変えた場合は UI の入力範囲だけを適用し、描画上の上限処理は comic-core の
既存 clamp を正とする。

### 6.4 四隅ハンドルのリサイズ

- 吹き出し（枠・矢印・マーカー等を含む）/ メッセージウィンドウ / スタンプは、通常の
  四隅ドラッグで掴んだ角の反対角を固定し、pivot を移動しながらリサイズする。
- ドラッグ中に <kbd>Ctrl</kbd> を押している間だけ、ドラッグ開始時の pivot を固定した
  中心対称リサイズへ切り替える。<kbd>Ctrl</kbd> は毎フレーム評価し、途中で押す / 離す
  操作にリアルタイムで追従する。
- 計算はドラッグ開始時の pivot・half extents・回転・角番号と現在カーソルから毎回
  再計算する。モード切替を繰り返しても増分誤差を蓄積せず、最小寸法クランプ中も
  反対角を固定する。スタンプは両モードで開始時のアスペクト比を維持する。
- 単独テキストはフォントサイズ変更とレイアウト中心再配置を組み合わせる既存方式を維持し、
  この反対角アンカー方式の対象外とする。

### 6.5 一覧・ラベル

- 既存の一覧 (`object_list_label`) では種別ラベルが「吹き出し」になるため、
  `shape_preset_link` のタグ (`miv:step-badge` 等、注釈プリセットにも
  `miv:annot-rect` / `miv:annot-arrow` / `miv:annot-marker` などを設置時に付与)
  を見て「枠」「矢印」「マーカー」「バッジ」等に出し分ける (表示のみの変更)。

---

## 7. keymap / 入力

- モード入退場・undo/redo・確定は既存 (`FsTextMode` / `TextUndo` / `TextRedo` /
  `TextConfirm`) のままで**新規 KeyAction なし**。
- ダイアログ内の Enter/Escape は必ず `dialog_enter_pressed` /
  `dialog_escape_pressed` ヘルパー経由 (IME 対応、CLAUDE.md の規約)。

---

## 8. 実装ステージングと受け入れ基準

Codex Sol へは Stage 単位で依頼する。各 Stage 完了時に `cargo fmt` +
`cargo test` 緑 + Codex/Claude レビューを通す。

| Stage | 内容 | comic-core 変更 |
|---|---|---|
| 1 | **実装済み** — 「注釈追加」ダイアログ + 枠長方形/角丸/楕円プリセット + 番号バッジ (自動採番) + 一覧ラベル出し分け | なし |
| 2 | **実装済み** — 注釈矢印 (Arrow フィールド拡張 + プリセット) | 加法 (model + tessellate、テスト付き) |
| 3 | **実装済み** — 蛍光マーカー/下線 (`FillBlend` フィールド + z 順セグメント合成: 表示/本ベイク/エクスポートの 3 合成点を共通ヘルパー化) | 加法 (`blend` フィールド + `bake_annotation_layers`、テスト付き) |
| 4 | **実装済み** — カーソルスタンプ (SVG アセット描き起こし + build.rs 同梱 + カタログ「注釈」カテゴリ) | なし |

**受け入れ基準 (テスト観点)**:

- 空テキスト Bubble の当たり判定・レイアウト・ベイクが正常 (Stage 1)
- 番号バッジ自動採番: 最大値+1 / パース不能無視 / ゼロ件で 1 (Stage 1、unit test)
- Arrow 新フィールド: 旧既定値 (None) で従来形状とポリゴン一致 / 新値で
  head/shaft 寸法が指定どおり / clamp 動作 (Stage 2、unit test)
- 乗算合成の画素検証: 白地→係数色が乗る、黒地→黒のまま / fill_opacity 0 で
  無効果 / 同一セグメント内の重ね塗りで係数が乗算累積 / Multiply 0 件時に
  既存経路の出力がバイト一致で不変 (Stage 3、unit test)
- z 順検証: 吹き出しの**上**に置いたマーカーは吹き出しの塗り・テキストごと
  乗算され、**下**に置いたマーカーは不透明な吹き出しに隠れる / 一覧の ↑↓
  (z 変更) で結果が切り替わる (Stage 3、unit test)
- 1 オブジェクト 1 モードの不変条件: outline・テキストを持つ Multiply
  オブジェクトでも全描画が乗算レイヤーに入り通常レイヤーへ漏れない /
  Multiply オブジェクトが merge chain から除外される (Stage 3、unit test)
- 新フィールド入り JSON の round-trip + フィールド無し旧 JSON の読み込み
  (Stage 2/3、unit test)
- カーソルスタンプ: 「注釈」カテゴリに 5 キーを列挙し、各 SVG を resvg で
  `RgbaOverlay` へデコードでき、不透明画素を含む (Stage 4、unit test)
- 本フルベイク (`books.rs`) とエクスポート (`Ctrl+E`) に乗算マーカーが反映される
  (Stage 3、既存テストの拡張)
- 既存 comic-core テストが無修正で緑 (全 Stage)
- UI 見た目の変更は ui-snapshot-policy.md に従いスナップショット追加/更新
- 実機確認 (ユーザー): スクショに赤枠+矢印+バッジ、文書写真にマーカー/下線を
  置いて Ctrl+E 書き出し → 意図どおりの見た目か

---

## 9. 実装リファレンス (touch points)

行番号は 2026-07-13 時点の目安 (別作業と並行のためズレうる。シンボル名で探すこと)。

**モデル (crates/comic-core/src/model.rs)**:
- `BubbleShape` enum :312 (Arrow :363、`arrow_up` default fn 近傍)
- `BubbleObject` :619 (fill :624 / fill_opacity :625 / outline / shape_preset_link)
- `StampObject` :1109 / `StampSource` :1077
- `AnnotationObject` :1156 / `AnnotationKind` :1147 (**変更しない**)

**幾何・ベイク (crates/comic-core/src/)**:
- `tessellate.rs::arrow` :142 (7 点ポリゴン。head_x :145 / shaft_hh :146 を
  Option 上書き)、`fit_bubble_shape` の Arrow 分岐 :302 (注釈矢印はテキスト
  フィットさせない — 空テキストなので実質影響なしだが確認)
- `raster.rs`: `bake_overlay_with_stamps` :121 / `bake_object_unrotated` :420 /
  `blend_px` :62 (**触らない**。乗算は新ヘルパー側) / `object_image_aabb` :291
- 新規: `bake_annotation_layers` (comic-core raster.rs に追加) +
  `composite_annotation_layers` (mIV 側 comic_overlay.rs、既存
  `composite_overlay_over` の一般化) (§5)

**合成点 (mIV 側)**:
- 表示: `App::ensure_comic_composite_texture` (app.rs :44479 付近) +
  `comic_overlay.rs::composite_overlay_over` :71 (乗算適用はこの合成の直前段)
- 本フルベイク: books.rs :896-916 (`BookComicSnapshot` 経路)
- エクスポート: `App::comic_composited_pixels_for_export` (app.rs :44784 付近)

**UI (src/ui_text.rs)**:
- 追加ボタン群 :1761-1814 (「スタンプ追加」:1809 の上に挿入)
- ダイアログ open フラグ配線 :1952-1971 / 雛形 `draw_text_add_bubble_dialog` :2059
- 一覧ラベル `object_list_label` :5030 / `kind_label` :878
- 詳細編集 `bubble_body_ui` :6271 (fill_opacity・色の既存 UI。Multiply 時も同 UI)
- `ShapeKind` enum :7325 (吹き出しダイアログの形状ピッカー。注釈ダイアログは
  これとは独立のプリセット列でよい)

**スタンプ (src/comic_stamp.rs)**:
- カタログ・カテゴリ (`EmojiCategory` :85)、レンダリング `EMOJI_RENDER_PX` :27、
  build.rs の twemoji codegen (同型の annotation-stamps テーブルを追加)

**保存 (触らない)**: `comic_db.rs` はスキーマ・API とも変更なし
(JSON の中身が増えるだけ)。

---

## 10. ドキュメント更新対象 (実装時)

- `htdocs/mimageviewer/manual/annotation.html` — 注釈追加の操作説明
  (内部用語なし。「乗算」は「下の文字を潰さない蛍光ペン風の重ね方」等の平易表現)
- `htdocs/mimageviewer/index.html` — 製品ページ機能一覧 (「スクリーンショットへの
  注釈: 赤枠・矢印・蛍光マーカー・番号バッジ・カーソル」)
- `docs/spec.md` — 機能一覧への追記
- 本書 — 実装状況の追記 (Stage ごと)
- [comic-integration-plan.md](comic-integration-plan.md) — モデル拡張
  (Arrow フィールド / FillBlend / 乗算パス) の契約追記
- README 更新履歴 (リリース時)
