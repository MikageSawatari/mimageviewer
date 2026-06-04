# スタンプ（画像ステッカー）機能 — 仕様 / 実装

Status: **v1 実装済み（lab）** — 2026-06-04
作成: 2026-06-04 セッション

## 実装サマリ（2026-06-04・lab）

§7 の選択は推奨案どおりで確定・実装した: **SVG+resvg / Twemoji(CC-BY) / ピッカー
ダイアログ＋クイック行 / ユーザー画像 v1 あり**。

- **comic-core** (`model.rs` / `raster.rs`): `AnnotationKind::Stamp(StampObject)` を追加。
  `StampObject { source: StampSource(Emoji|File), half_w, half_h, opacity, flip_h/v,
  outline, style_preset_link }`。baker は decode-free のまま:
  `bake_overlay_with_stamps(objects, w, h, fonts, stamps: &StampImages)`
  (`StampImages = HashMap<object id, RgbaOverlay>`) を追加し、`bake_overlay` は空 map の
  wrapper。`draw_stamp` が target サイズへ bilinear scale + flip + opacity + ステッカー縁取り
  (silhouette alpha を `rad` 拡張、画像端の外側にも出る) を合成。画像欠落時は
  `draw_stamp_placeholder` (半透明グレー枠＋×)。回転は既存の `bake_into`/`rotate_blit` が
  オブジェクト共通で処理 (StampObject 自体に回転フィールドは持たない)。`object_local_aabb`
  に Stamp 枝。回帰テスト 4 本 (image bake / outline widening / missing placeholder / rotation)。
- **comic_lab** (`stamp.rs` + `main.rs`): ピッカーダイアログ (カテゴリタブ＋検索＋最近使った
  行＋絵文字グリッド＋「画像ファイルから追加」)、右パネルのスタンプ専用プロパティ
  (大きさ=一様スケール / 不透明度 / 反転 / 縁取り色・太さ / 別スタンプに変更)、選択ハンドル
  (回転ノブ＋四隅=アスペクト固定の一様スケール)、最近使った履歴の永続化
  (`recent_stamps.json`)。emoji は **curated catalog** (`EMOJI_CATALOG` ~120 種、カテゴリ/名前
  付き) を単一の真実として持ち、`scripts/setup-twemoji.sh` がその key を Twemoji から DL する。
  decode キャッシュ (source key 単位) で同一 emoji/画像は 1 回だけ展開。
- **アセット形式**: emoji は **SVG → resvg** で `EMOJI_RENDER_PX=512` にラスタ (premultiplied →
  straight に demultiply)、ユーザー画像は `image` crate (png/jpg/webp/gif/bmp)。
- **graceful fallback**: アセット未配置でもピッカーは開き、「画像ファイルから追加」と
  最近使った行は機能する。emoji グリッドは key チップ表示＋設置スクリプト案内を出す。
- 依存追加: `comic_lab/Cargo.toml` に `resvg`、`image` に webp/gif/bmp feature。
- **未着手 (将来)**: 本体 mImageViewer 統合 (最前面オーバーレイ段 / Ctrl+E / キャプチャ経路)、
  tint・影・アニメスタンプ・お気に入り管理。LGPL ではないが Twemoji の CC-BY 帰属表示は
  配布時に必要 (本体統合時に「ソフトウェア情報」へ追記)。

対象: `crates/comic-core` + `tools/comic_lab`（将来 mImageViewer 本体）。
背景: SNS 投稿向け画像加工で、カラー絵文字を**フォント混植ではなく画像スタンプ**として手軽に
置けるようにする（フォントカラー絵文字を採らない判断は
[vertical-text-opentype-plan.md](vertical-text-opentype-plan.md) の決定を参照）。

## 0. コンセプト
- **スタンプ = 画像を注釈オブジェクトとして配置**するもの。吹き出し / メッセージウィンドウ /
  テキスト と並ぶ **第4の `AnnotationKind`**。
- ソースは **(1) 同梱絵文字セット**（Unicode 絵文字一式）＋ **(2) ユーザー読込画像**。
- **環境非依存で絵柄が一定**（どの OS でも同じ）、**自由に拡大・回転・縁取り**でき、
  既存の「画像にオブジェクトを置く」モデルにそのまま乗る。

## 1. データモデル（comic-core）

```rust
pub enum AnnotationKind {
    Bubble(BubbleObject),
    Text(TextBlock),
    MessageWindow(MessageWindowObject),
    Stamp(StampObject),               // ← 追加
}

pub struct StampObject {
    pub source: StampSource,          // どの画像か（キー or パス）
    pub half_w: f32,                  // 画面上の半幅(px)。half_h は source のアスペクトから
    pub half_h: f32,
    #[serde(default = "one")] pub opacity: f32,     // 0..1
    #[serde(default)] pub flip_h: bool,
    #[serde(default)] pub flip_v: bool,
    /// ステッカー風の縁取り（白フチ等）。アルファを dilation してハロを敷く。
    #[serde(default)] pub outline: Option<StrokeStyle>,
    #[serde(default)] pub style_preset_link: Option<String>, // 任意（縁取り等のプリセット）
    // 位置(pivot=中心) / 回転(rotation_rad) / z / enabled は AnnotationObject 共通
}

pub enum StampSource {
    /// 同梱絵文字のキー（例: emoji codepoint 列 "1f600" や "1f1ef-1f1f5"(🇯🇵)）。
    Emoji(String),
    /// ユーザー読込画像の絶対パス。
    File(PathBuf),
}
```

**重要な責務分離**: `comic-core` は egui-free の純ロジックで**画像デコード/SVG をしない**。
スタンプの**実ピクセルはラボ側（または本体側）が用意して bake に渡す**。

- `bake_overlay(objects, w, h, fonts, stamps)` に **`stamps: &HashMap<u64 /*object id*/, RgbaImage>`** を
  追加（各 Stamp オブジェクトの「画面上サイズで rasterize 済み RGBA」）。
- ラボが `StampSource` → `RgbaImage`（目標サイズで）を解決・キャッシュして渡す。
- `comic-core` は受け取った RGBA を **`rotate_blit`（既存・premultiplied・双線形）で回転合成**し、
  `opacity` 乗算、`outline` 指定時はアルファ dilation でハロ（袋文字と同じ要領）を敷く。
- `RgbaImage { w, h, pixels: Vec<u8> }`（straight alpha）を comic-core に定義（or 既存 `RgbaOverlay` 流用）。

これで comic-core は decode/SVG 依存ゼロのまま、回転・不透明度・縁取りだけ担当する。

## 2. アセット：ソースと形式

### 形式（PNG vs SVG）
| 方式 | 利点 | 欠点 |
|---|---|---|
| **SVG → resvg でラスタ**（推奨） | **任意サイズで crisp**（4K に大きく貼っても綺麗）、SVG は容量が小さい | 依存 resvg/usvg/tiny-skia 追加（純 Rust・成熟） |
| PNG（image crate） | 新規依存なし（image は既存） | 解像度固定。大きく拡大すると**ぼやける**。高解像度同梱は容量増 |

SNS 用途はスタンプを大きく貼ることがあり**拡大時の鮮明さが効く**ので、**同梱絵文字は SVG(resvg)**を
推奨。**ユーザー画像は image crate（PNG/JPG/WebP…既存）**。resvg を避けたい場合は v1 を高解像度 PNG
（例 OpenMoji 618px / Noto 128px）にして将来 SVG 化も可。

### 絵文字セット（ライセンス）
| セット | ライセンス | 備考 |
|---|---|---|
| **Twemoji** | CC-BY 4.0 | SNS で最も見慣れた絵柄。**帰属表示が必要**（ソフトウェア情報へ記載）。SVG あり |
| Noto Emoji | Apache-2.0（グラフィック） | 帰属義務ゆるい（ライセンス同梱のみ）。PNG/各種 |
| OpenMoji | CC-BY-SA 4.0 | **share-alike**（やや制約強）。SVG/PNG、618px |

mIV は MIT。**Twemoji(CC-BY 4.0) または Noto(Apache-2.0)** が相性良い。推奨は見栄えの **Twemoji**
（帰属を「ソフトウェア情報」「installer/readme.txt」に1行追記、既存の LGPL/VST3 表記と同じ運用）。
同梱は ~3,500 ファイル（SVG なら数 MB）。本体は `include_dir!`/`include_bytes!` で埋め込み、
初回 APPDATA 展開（PDFium/Susie と同じパターン）か、vendor ディレクトリから読む。

## 3. UI（comic_lab）

### 追加導線
左パネルの追加ボタンに **「スタンプ追加」** を1行追加（吹き出し / ウィンドウ / テキスト / **スタンプ**）。

### 一覧の出し方（ユーザー案への所感）
ご提案の「右側にスタンプ一覧→クリック挿入」について:
- **~3,500 個の絵文字を常時右パネルに並べるのは窮屈**（他パラメータも圧迫）。
- 推奨は **吹き出し/ウィンドウと同じ“ピッカーダイアログ”**（既存 add-dialog の踏襲・UX 統一）:
  - **カテゴリタブ**（顔 / 人・手 / 動物 / 食べ物 / 物 / 記号 / 旗 …）＋ **検索ボックス**（名前/かな）
    ＋ **最近使った**行。グリッド表示、可視行のみ rasterize（font dialog と同じ予算制御）。
  - クリックで**画面中央に挿入**して選択状態に。
  - ダイアログ右上に「**画像ファイルから追加**」（ユーザー画像スタンプ）。
- **右パネル**は「選択中スタンプのプロパティ」を担当（後述）。＝ **ピッカー＝挿入 / 右パネル＝編集**
  の分担（フォント選択フローと同じ思想）。
- 補助として、右パネルに **「最近/お気に入りスタンプ」のクイック行**（数個）を置くのは有用。

> 右パネル常設リスト「だけ」にしたい場合も実装可能だが、検索・カテゴリ・大量表示の使い勝手で
> ダイアログ＋クイック行を推奨。

### 選択中スタンプの右パネル（プロパティ）
- 大きさ（スケール % または半幅スライダ。アスペクト固定）
- 不透明度
- 左右反転 / 上下反転
- **縁取り（ステッカー風）**: ON/OFF＋色＋太さ（白フチでステッカー化＝SNS で需要）
- 「別のスタンプに変更」ボタン（ピッカー再表示）
- z 順は既存のオブジェクト一覧で操作

### キャンバス操作（ハンドル）
既存の選択ハンドル機構を流用:
- **本体ドラッグ=移動**、**回転ノブ=回転**（吹き出し/ウィンドウと共通）。
- **四隅ドラッグ=拡大縮小**。スタンプは**アスペクト固定の一様スケール**（吹き出しの自由 half_w/half_h
  とは違い、縦横比維持）。
- → `handle_points`/`draw_selection_handles`/Corner ドラッグに Stamp 分岐を足す（Corner は一様スケール）。

## 4. bake 統合
1. ラボが各 Stamp の `StampSource` を**画面上サイズ**（`2*half_w × 2*half_h` × zoom 非依存の image-px）で
   解決 → `RgbaImage` をキャッシュ（キー=(source, 量子化サイズ)）。SVG は resvg で目標 px に render、
   PNG/画像は image で decode＋必要時スケール。
2. `bake_overlay` に id→RgbaImage を渡す。`bake_object`（回転ラッパ `bake_into`）の Stamp 枝で:
   - `outline` 指定時、まずアルファ dilation のハロを縁取り色で敷く（袋文字と同要領）。
   - 本体 RGBA を `rotate_blit` で pivot 周りに回転合成、`opacity` 乗算、flip 適用。
3. `object_local_aabb` の Stamp 枝＝回転後の矩形（+縁取り幅）。

## 5. 永続化
- `StampObject` を sidecar に保存。**ソースは参照（emoji キー / ファイルパス）**＋ジオメトリのみ
  （ピクセルは保存しない）→ sidecar は小さく、再 bake で**目標サイズに crisp 再描画**。
- 旧 sidecar 互換は `#[serde(default)]`。ユーザー画像はパス保存（移動時は欠落→プレースホルダ表示）。
  将来「画像を sidecar 横へコピー」オプション検討。

## 6. v1 スコープ / 将来
**v1**: Emoji(同梱・SVG) ＋ User画像、挿入ピッカー（カテゴリ/検索/最近）、移動/回転/一様スケール、
不透明度、反転、ステッカー縁取り、保存復元、回帰テスト（comic-core: 「RGBA を回転合成して非空」
「縁取りでアルファが太る」等／lab: ピッカー挿入・スケール）。
**将来**: tint(単色化)、影、アニメスタンプ（GIF/APNG）、お気に入り管理、本体統合（最前面
オーバーレイ段に合流、Ctrl+E/キャプチャ経路一致）。

## 7. 要確認（実装着手前）
1. **アセット形式**: SVG+resvg（推奨・鮮明）/ 高解像度 PNG（依存最小）。
2. **絵文字セット**: Twemoji(CC-BY)/ Noto(Apache)/ OpenMoji(CC-BY-SA)。
3. **一覧 UI**: ピッカーダイアログ（推奨）/ 右パネル常設リスト / 併用。
4. v1 で**ユーザー画像スタンプ**も入れるか（推奨: 入れる。実装は軽い）。

## 参照
- Twemoji: github.com/twitter/twemoji（CC-BY 4.0）/ Noto Emoji（Apache-2.0）/ OpenMoji（CC-BY-SA 4.0）
- resvg/usvg（純 Rust SVG レンダラ）, image crate（既存依存）
- 既存パターン: 追加ダイアログ（吹き出し/ウィンドウ）、`rotate_blit`/袋文字 dilation（raster.rs）、
  アセット埋め込み（PDFium/Susie の include_bytes!＋APPDATA 展開）
