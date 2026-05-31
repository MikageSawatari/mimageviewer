# 自動マスク作成 (領域検出 → 隠蔽/消しゴムマスク) 設計案 — v1.1.0

> ステータス: **Claude 案 / 未着手**。実装前にユーザーレビュー → 必要なら Codex レビューを経て着手する。
> このドキュメントは [preset-and-adjustment.md](preset-and-adjustment.md)（マスク系・キャッシュ無効化）と
> [async-architecture.md](async-architecture.md)（worker / cancel）を前提にしている。先にそちらを参照のこと。

## 0. 一行で

隠蔽加工 (conceal) / 消しゴム (erase) のマスク編集画面に「**自動マスク作成**」ボタンを足し、
ONNX 物体検出モデルで検出した領域を **編集可能な Shape オブジェクト (矩形/楕円)** として
自動生成する。**標準は MIT ライセンスの顔検出 (YuNet) を同梱**、**追加検出器はユーザーが
自分で ONNX を入れて足せる (BYO)** 二層構造にする。

---

## 1. 目的・スコープ

### 1.1 やること (v1.1.0)

- **標準同梱の顔検出 (YuNet)** による「顔ぼかし/顔モザイク」をボックスから出してすぐ使える状態にする
  (プライバシー/匿名化用途。写真の大量処理を想定)
- **ユーザー追加検出器 (BYO)**: ユーザーが ONNX 検出モデルを所定フォルダに置くと、追加の自動マスク
  対象として選べる (例: アニメ局部検出 → R18 モザイク補助)
- 検出結果を **既存マスクシステムの Shape オブジェクトとして注入** し、その後ユーザーが
  選択ツールでドラッグ/リサイズ/回転/削除して微修正できる ("AI が提案 → 人が確定")

### 1.2 やらないこと (このバージョンの範囲外)

- **完全自動 (人手レビュー無し) の確定処理**: あくまで叩き台生成。最終判断はユーザー
- **グリッドからの一括バッチ処理 + 焼き込みエクスポート**: 価値はあるが UX が別物なので将来版
  (§11 フェーズ 3 にメモ)。v1.1.0 は **編集画面で開いている 1 ページ** が対象
- **モデルの同梱配布 (YuNet 以外)**: AGPL 等のライセンス問題があるため BYO に限定 (§9)
- **「性的表現か否か」のような主観的・サイト依存の判定**: 検出器は客観的な領域 (顔/局部) だけを
  出す。基準適合の判断はしない (CLAUDE.md モザイク表記ポリシー参照)

### 1.3 リリース判断 (永続データ)

本機能の永続データ (Shape オブジェクト) は **既存の conceal.db / mask.db の `Shape` 形式を
そのまま使う**。自動生成した Shape も手描き Shape と同一表現なので、**スキーマ変更も
マイグレーションも不要**。検出器プロファイルや設定は `settings_kv` に JSON で載るだけ
(後述)。新規 ModelKind / 設定フィールドはすべて未リリース機能なので破壊的変更の心配もない。

---

## 2. 全体像 — 二層構造

```
                          ┌─────────────────────────────────────────────┐
                          │  自動マスク作成 (auto mask)                   │
                          │  conceal / erase のマスク編集画面のボタン      │
                          └───────────────┬─────────────────────────────┘
                                          │ 選択モデル群 + 形状(矩/楕) + 余白
                                          ▼
   ┌──────────────────────────┐   ┌──────────────────────────────────────┐
   │ 標準 (同梱・MIT)          │   │ 追加 (BYO・非同梱)                     │
   │ YuNet 顔検出              │   │ ユーザーが ONNX を置く                 │
   │ OutputFormat::YuNet       │   │ 例: deepghs 局部検出 (YOLOv8)          │
   │                           │   │ OutputFormat::Yolov8                   │
   └────────────┬─────────────┘   └────────────────┬─────────────────────┘
                │  検出ボックス(+landmark)            │  検出ボックス
                └──────────────┬────────────────────┘
                               ▼
              DetectorProfile で前処理/デコードを抽象化
                               ▼
            ボックス → 余白 dilate → Shape::Rect / Ellipse (op=Add)
                               ▼
        既存の conceal_shapes / erase shapes に push (編集可能)
                               ▼
        既存の合成パイプライン (composite_mask_region → conceal_compose)
```

ポイント:

- **検出 → マスク変換** の core は検出器の種類に依らず共通。違うのは「前処理 (入力サイズ/正規化)」と
  「出力デコード (YuNet 系 vs YOLOv8 系)」だけ。これを `DetectorProfile` + `OutputFormat` enum で吸収する
- **MIT 同梱可の顔検出はすべて非 YOLO** (YuNet/CenterFace/Ultra-Light)、**BYO の局部検出はすべて YOLOv8**
  なので、**デコーダは 2 系統**になる。これは避けられないが、いずれも枯れた後処理で `DetectorProfile` に収まる
- 生成物は **手描きと区別のない Shape オブジェクト**。したがって編集・Undo・スロット・永続化・サイドカー
  バックアップはすべて既存実装をそのまま流用できる (新規ゼロ)

---

## 3. UI 設計

### 3.1 操作フロー (ユーザー提案を採用 + 微調整)

> ユーザー提案: 「マスクツールを起動した画面で、自動マスク作成ボタン → 適用したいモデルを複数チェックで
> 選ぶ & 形状を矩形か楕円で選ぶ → 適用でオブジェクトが自動作成される」。**この流れを採用する**。
> 強みは「生成物が編集可能オブジェクト」である点 — 提案された領域をそのまま確定せず、ユーザーが
> 削除/移動/リサイズして仕上げる "提案 → 確定" ループが UI 上で自然に閉じる。

1. conceal (C キー) または erase 編集モードに入る (既存)
2. 左パネルの 8 ツールパレット (`ui_conceal.rs:1906-1971`) の下に **「自動マスク作成」ボタン**を追加
3. クリックで小さなポップアップ (Area + Frame::popup) を開く:

   | 要素 | 内容 | 既定 |
   |---|---|---|
   | モデル (複数チェック) | ☑ 顔 (YuNet, 同梱) / ☐ 各 BYO 検出器 (あれば: クラス名で表示) | 顔のみ ON |
   | 形状 | ○ 矩形 / ○ 楕円 | 顔は楕円、それ以外は矩形 (モデル既定。手動上書き可) |
   | 余白 | 検出ボックスを何 % 膨らませるか (スライダー) | +15% |
   | しきい値 (詳細) | confidence 下限 | モデルの `threshold.json` 由来、なければ 0.5 |
   | [適用] | 検出を worker で実行 → 完了時に Shape 群を生成 | — |

4. 適用すると worker で検出 (UI は止めない、§7)。完了時に **検出領域ごとに Shape::Rect or Ellipse
   (op=Add) を `conceal_shapes` / erase shapes に push** し、`clear_conceal_caches(idx)` で再合成
5. 生成された Shape は **選択ツール (S) でそのまま編集可能**。誤検出は Del、位置/サイズはハンドルで調整
6. 通常どおりモード終了で DB + サイドカーへ保存 (既存 `save_conceal_with_sidecar` / `save_mask_with_sidecar`)

### 3.2 ボタンの置き場と共有

- conceal: `ui_conceal.rs` の左パネル、ツールパレット直下に追加
- erase: `ui_erase.rs` の対応箇所に同じボタン。**Shape と合成ロジックは共有 (`mask_db.rs`) なので、
  注入先 (`conceal_shapes` vs erase の shapes) を切り替えるだけ**
- 主用途は **conceal (モザイク/ぼかし)**。erase (MI-GAN inpaint) 側は機構上ほぼ無償で乗るので
  ボタンは出すが副次的扱い (顔を inpaint で消す等のニッチ用途)

### 3.3 形状生成の規則

検出ボックス `(x, y, w, h)` (検出を行ったソースピクセル座標系) から:

- **矩形**: `Shape::Rect { center: (x+w/2, y+h/2), half_w: w/2*(1+margin), half_h: h/2*(1+margin), rotation_rad: 0 }`
- **楕円**: `Shape::Ellipse { center: 同上, rx: w/2*(1+margin), ry: h/2*(1+margin), rotation_rad: 0 }`
- `op` は **常に `ShapeOp::Add`** (マスク領域を足す)
- **将来**: YuNet は 5 landmark を返すので、両目の座標から顔の傾きを推定して楕円の `rotation_rad` に
  反映できる (傾いた顔にフィット)。v1.1.0 MVP では `rotation_rad = 0` 固定、傾き対応は §11 フェーズ 4

### 3.4 座標系の注意

検出は **「いまマスクが乗っているソースピクセル」** に対して行う。conceal の現行ソースは
`current_conceal_source_pixels` が `erase_result > adjustment_cache > ai_upscale_cache > fs_cache` の順に
解決する (preset-and-adjustment.md §3)。**この解決済みピクセルを検出入力にし、ボックスをその解像度の
座標へスケールして Shape を作る**。こうすれば筆/囲みストロークと同じ座標系に乗り、AI アップスケールで
ソース解像度が変わっても既存の Shape リスケール機構 (同 §3) に乗る。

---

## 4. 検出器の抽象化 — DetectorProfile / OutputFormat

新規モジュール `src/ai/detect.rs` に集約する。

```rust
/// 出力テンソルのデコード方式 (アーキ依存)
pub enum OutputFormat {
    /// YuNet (SCRFD 系): priors を生成して loc/conf/landmark を decode、5 landmark あり
    YuNet,
    /// Ultralytics YOLOv8 検出: 出力 [1, 4+C, A] を transpose → xywh + class scores → NMS
    Yolov8,
    // 将来: CenterFace (heatmap), Ultra-Light (SSD) 等を必要に応じて追加
}

pub enum Normalize {
    /// pixel / 255.0  (YOLOv8 既定)
    ZeroToOne,
    /// YuNet は生 0..255 をそのまま入力する実装が多い (要実機確認)
    Raw,
    /// (pixel/255 - mean) / std
    MeanStd { mean: [f32; 3], std: [f32; 3] },
}

pub struct DetectorProfile {
    pub id: String,                 // "yunet_face" / BYO はファイル stem
    pub display_name: String,       // UI 表示 ("顔" など)
    pub model_path: PathBuf,        // 同梱は APPDATA/models/、BYO はユーザーフォルダ
    pub model_kind: ModelKind,      // ort セッションキャッシュのキー (ai/mod.rs)
    pub input_size: (u32, u32),     // YOLOv8=640x640。YuNet は dynamic → 既定 320x240 等
    pub normalize: Normalize,
    pub letterbox: bool,            // アスペクト維持 padding (YOLOv8=true)
    pub output_format: OutputFormat,
    pub class_names: Vec<String>,   // labels.json 由来
    pub mask_classes: Vec<usize>,   // この検出器のどのクラスをマスク化するか
    pub conf: f32,                  // confidence 下限
    pub iou: f32,                   // NMS IoU
    pub default_shape: AutoMaskShape, // Rect / Ellipse の既定
}

pub struct Detection {
    pub class_idx: usize,
    pub score: f32,
    pub bbox: (f32, f32, f32, f32),    // ソースピクセル座標 (x, y, w, h)
    pub landmarks: Option<Vec<(f32, f32)>>, // YuNet の 5 点 (将来の傾き推定用)
}

/// 1 枚の画像に対して検出を行う (worker thread から呼ぶ)
pub fn detect(
    runtime: &AiRuntime,
    profile: &DetectorProfile,
    image: &image::DynamicImage,   // current_conceal_source_pixels から変換
    cancel: &Arc<AtomicBool>,
) -> Result<Vec<Detection>, AiError>;
```

- 前処理 (resize/normalize/letterbox/NCHW) は `classify.rs:87` の `preprocess` を雛形に `profile` 駆動で書く
- 推論は `runtime.with_session(profile.model_kind, |s| s.run(ort::inputs![t]))` (`runtime.rs:627`)
- デコードは `output_format` で分岐 (`decode_yunet` / `decode_yolov8`)、最後に NMS

### 4.1 プロファイルの決定 (汎用性をどこまで広げるか)

deepghs の YOLOv8 検出器は **共通サイドカー schema** を同梱している (実物確認済み、2026-05-31):

- `model_artifacts.json` → `args.imgsz` (入力サイズ) / `model_type: "yolov8"` / `nc` (クラス数)
- `labels.json` → クラス名配列 (例 `["nipple_f","penis","pussy"]`)
- `threshold.json` → クラス別しきい値

BYO 検出器のプロファイルは次の優先順で埋める:

1. **サイドカー JSON があれば** それを読む (deepghs 形式は無加工で対応 → 「deepghs YOLOv8 検出器なら何でも」差し替え可)
2. **無ければ ONNX introspection**: 入力テンソル形状 → 入力サイズ、出力 `[1, 4+C, A]` → クラス数 `C = ch-4`
3. **それでも足りなければ** ユーザー添付の `<model>.json` (mIV 独自プロファイル)

クラス名 (UI ラベル + どれをマスク化するか) だけは人間由来の情報なので labels.json かユーザー指定が要る。

---

## 5. モデル管理

### 5.1 標準同梱: YuNet (MIT)

既存の埋め込みパターン (PDFium / 既存 AI モデルと同じ) に乗せる:

1. `vendor/models/yunet_face_detection_2023mar.onnx` を配置 (~0.23 MB)
   - 取得元: `https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx`
   - ⚠ `raw.githubusercontent.com` は LFS ポインタ (131B) が返るので `media.githubusercontent.com/media/...` を使う
   - ライセンス: **MIT** (opencv_zoo の per-model LICENSE, ©2020 Shiqi Yu)。`vendor/` は .gitignore なので
     `scripts/setup-*.sh` 系に取得スクリプトを足すか bootstrap に組み込む
2. `ModelKind::DetectorYuNetFace` を `ai/mod.rs:111` の enum + `as_str`/`from_str` に追加
3. `EMBEDDED_MODELS` (`model_manager.rs:18`) に `include_bytes!` で登録 → 起動時 `ensure_models_extracted()` が
   `%APPDATA%/mimageviewer/models/` へ展開 (サイズ一致でスキップ)
4. exe サイズ増は **+0.23 MB のみ** (現行 AI モデル群に比べ無視できる)

> YuNet 出力は YOLOv8 形式ではない (SCRFD 系: priors + 5 landmark + NMS)。`OutputFormat::YuNet` デコーダを
> 1 つ実装する。OpenCV の `FaceDetectorYN` の decode 実装が数式の参照になる。

### 5.2 追加: BYO 検出器

- 置き場: `%APPDATA%/mimageviewer/models/detectors/<name>/` (設定でフォルダ変更可)
  - 各検出器 = `model.onnx` + 任意のサイドカー (`model_artifacts.json` / `labels.json` / `threshold.json`、
    または mIV 独自 `<name>.json`)
- 発見: 機能初回利用時 (またはアプリ起動時) にフォルダを走査して `DetectorProfile` を構築。UI のモデル
  チェックリストに「クラス名」で並べる
- ロード: 同梱と同じ `runtime.load_model(kind, &path)` (`runtime.rs:448`、idempotent)。セッション層は
  同梱/BYO を区別しない
- **「対応モデル」ダイアログ**: 環境設定にボタンを置き、動作確認済みモデルを一覧:
  - モデル名 / HF 直リンク / 配置フォルダ / **ライセンス注記** / 期待出力形式 を淡々と列挙
  - 「mIV はこれらのモデルを配布していません。ライセンスはご自身でご確認ください」と明記 (§9)
  - mIV からの **アプリ内ダウンロードも可** (ファイルは HF → ユーザー機へ直送、mIV は再配布しない)。
    ただし MVP はリンク + 手動配置で十分

---

## 6. マスク生成への接続 (既存システムへの注入)

検出 → Shape 変換の確定処理 (poll 側、UI スレッド):

```rust
// 検出結果 Vec<Detection> を受け取って Shape を注入する
fn inject_auto_mask_shapes(&mut self, idx: usize, dets: Vec<Detection>, shape: AutoMaskShape, margin: f32) {
    let added: Vec<Shape> = dets.iter().map(|d| {
        let (x, y, w, h) = d.bbox;
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let (hw, hh) = (w * 0.5 * (1.0 + margin), h * 0.5 * (1.0 + margin));
        match shape {
            AutoMaskShape::Rect    => Shape::Rect    { op: ShapeOp::Add, center: (cx, cy), half_w: hw, half_h: hh, rotation_rad: 0.0 },
            AutoMaskShape::Ellipse => Shape::Ellipse { op: ShapeOp::Add, center: (cx, cy), rx: hw, ry: hh, rotation_rad: 0.0 },
        }
    }).collect();

    // 既存 API でそのまま注入 (conceal の場合)
    for s in &added { self.conceal_shapes.push(*s); }
    self.clear_conceal_caches(idx);            // 再合成を誘発 (preset-and-adjustment.md §4)
    self.push_conceal_undo_batch(&added);      // §6.1: まとめて 1 Undo
    // erase の場合は erase 側 shapes に push + erase_mask_generation を進める
}
```

参照する既存実装 (Explore で確認):

- `Shape` enum: `mask_db.rs:162`（Line/Rect/Ellipse、`op`/`center`/`half_w,half_h` or `rx,ry`/`rotation_rad`）
- 単発注入の前例: `commit_conceal_shape` (`ui_conceal.rs:1445`) が `conceal_shapes.push(shape.with_op(op))` +
  キャッシュクリア + 自動選択を行う。自動マスクは複数 push なので近い処理を batch 化する
- 合成: `composite_mask_region` (`mask_db.rs:598`) がビットマップ下地 + Shape を順に適用 (Add/Subtract)。
  自動生成 Shape はそのまま既存合成に乗る
- conceal の最終描画: `conceal_compose.rs` (`compose_mosaic` / fill / blur) で `conceal_cache` を生成

### 6.1 Undo

自動マスク作成は **1 操作 = 1 Undo エントリ** にする (生成した N 個の Shape をまとめて取り消し)。
既存の `conceal_undo_stack` / `erase_undo_stack` (preset-and-adjustment.md §8.X / §5) にバッチ追加用の
エントリを 1 件積む。Undo で N 個一括除去、Redo で再注入。

### 6.2 形状の編集・永続化・スロット (すべて流用)

生成物が通常の Shape なので、以下は **追加実装ゼロ**:

- 選択ツールでの drag/resize/rotate/delete (`ui_conceal.rs` の vector_edit、preset-and-adjustment.md §5.2)
- conceal.db / mask.db への保存、`mimageviewer.dat` サイドカーミラー (§9 of preset-and-adjustment.md)
- F9/F10 スロット保存・再適用 (自動生成済みマスクをスロットに保存して別ページへ配布も可)

---

## 7. 非同期 / worker (UI を止めない)

検出推論は数十〜数百 ms かかりうるので **必ず別スレッド**。既存 `ai_upscale` の worker パターン
(`app.rs` の `ai_upscale_pending` / `maybe_start_ai_upscale` / `poll_ai_upscale`) を雛形にする。

ただし自動マスクは **「編集中の 1 ページに対するユーザー明示操作」** なので、prefetch のような複雑な
キャンセル戦略は不要。単一 pending で足りる:

```rust
struct AutoMaskPending {
    idx: usize,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Vec<Detection>, AiError>>,
    shape: AutoMaskShape,
    margin: f32,
}
// App: auto_mask_pending: Option<AutoMaskPending>
```

- **適用時** (`start_auto_mask`):
  1. `ensure_ai_runtime()` (`app.rs`、既存)
  2. 選択された各 `DetectorProfile` の `runtime.load_model(kind, path)`
  3. `current_conceal_source_pixels(idx)` を `Arc<ColorImage>` で snapshot (UI スレッドはここまで)
  4. `std::thread::spawn` で各モデルの `detect()` を順に実行し結果を結合、`tx.send`
  5. `cancel` は編集モード退出 / 再適用 / 別ページ移動で立てる
- **poll** (`poll_auto_mask`、毎フレーム `App::update` から `try_recv`):
  - Ok(dets) → §6 の `inject_auto_mask_shapes`、トースト「N 個の領域を検出しました」(0 件なら「検出なし」)
  - Err → エラートースト (モデル読込失敗・推論失敗)
  - 世代ガード: 受信時に `idx` が現在の編集対象と一致するか確認 (フォルダ移動等で陳腐化した結果を捨てる)

cancel チェックは `detect()` 内のモデル単位 (複数モデル選択時、各モデル開始前に `cancel.load(Relaxed)`)。
YuNet/YOLOv8 単発推論はタイル分割しないので、`upscale` のようなタイル境界 cancel は不要。

---

## 8. 設定追加

`settings.rs` の `Settings` に単純フィールドを足す (`#[serde(default)]` → `settings_kv` に JSON 保存、
schema 変更不要。feature-expansion-ideas.md §共通方針)。

```rust
#[serde(default = "default_auto_mask_margin")]
pub auto_mask_margin: f32,                  // 既定 0.15
#[serde(default)]
pub auto_mask_default_shape: AutoMaskShape,  // 既定 Ellipse (顔), モデル既定で上書き
#[serde(default)]
pub detector_models_dir: Option<PathBuf>,    // BYO 検出器フォルダ override
#[serde(default)]
pub auto_mask_last_models: Vec<String>,      // 前回チェックしたモデル id (利便)
```

`AutoMaskShape` enum は `Serialize/Deserialize` + `Default`(=Ellipse) を付ける。

---

## 9. ライセンス・配布

### 9.1 同梱 YuNet (MIT)

- **環境設定 → ソフトウェア情報** と `installer/readme.txt` に 1 行:
  `YuNet face detector — MIT License, Copyright (c) 2020 Shiqi Yu`
- FFmpeg / PDFium / ONNX Runtime の通知と並べるだけ。MIT は表記以外の義務なし
- exe サイズ +0.23 MB

### 9.2 BYO 検出器 (非同梱)

- mIV は **weights を一切同梱・再配布しない** → mIV 本体は MIT クリーンを維持
- ユーザーが公開ホスト (HF 等) から個人利用目的で取得・使用するのは AGPL の配布/ネットワーク提供
  トリガ外
- 「対応モデル」ダイアログ / マニュアルでの記載方針 (CLAUDE.md モザイク表記ポリシー + 下記):
  1. **ユーザー起点** (mIV が黙って自動 DL して「mIV の AI 機能」と売り込まない)
  2. **ライセンスを正確に開示**: deepghs の HF タグ `mit` を**そのまま「MIT」と書かない**。
     「Ultralytics YOLOv8 由来で AGPL-3.0 の主張があります。再配布/商用ではご自身で確認を」と注記
  3. **中立な技術記述**: 投稿サイト名・基準・適合判定は書かない。モデル名/URL/処理内容は可
  4. **「mIV は本モデルを配布していません」** を明記
- Vector / 窓の杜 正式配布なので、完全な安心が欲しければ最終法務確認は妥当 (ただしリンク + 案内
  レベルは低リスクでブロッカーではない)

### 9.3 同梱不可モデルの一覧 (実装者向けメモ)

| モデル | 実態 | 同梱 |
|---|---|---|
| YuNet / CenterFace / Ultra-Light-1MB | MIT、非 YOLO | ✅ 顔検出のみ |
| deepghs/anime_censor_detection | HF タグ mit だが YOLOv8 (Ultralytics AGPL 主張) | ❌ BYO のみ |
| deepghs/nudenet_onnx | apache タグだが上流 NudeNet AGPL + YOLOv8 | ❌ |
| InsightFace SCRFD/RetinaFace | コード MIT だが **weights 非商用研究のみ** | ❌ |
| yolov5-face / YOLOv8-face フォーク | GPL-3.0 / AGPL | ❌ |

---

## 10. 既存パイプラインとの整合 (壊さないための確認)

- **キャッシュ無効化** (preset-and-adjustment.md §4): Shape 追加は「マスク変更」なので
  conceal は `clear_conceal_caches(idx)` (該当 idx の conceal_cache のみ)、erase は
  `erase_mask_generation[idx]` を進めて `erase_result_cache` を stale 化。`fs_cache` /
  `ai_upscale_cache` / `adjustment_cache` は **下位入力として保持**
- **表示優先順位** (display-pipeline.md): `conceal_cache > erase_result_cache > adjustment_cache >
  ai_upscale_cache > fs_cache` は不変
- **post_filter バイパス**: 編集モード中は `post_filter_bypassed = true`。検出入力は
  `current_conceal_source_pixels` (= バイパス済みの見た目) を使うので、ユーザーが見ている画像と
  検出対象が一致する
- **ZIP/PDF ページ**: conceal/erase は既に ZIP 内画像・PDF ページ対応。検出はレンダリング済み
  ページピクセルに対して走る (分岐追加不要)
- **見開き (spread)**: 編集対象ページ (target_idx) のソースに対して検出。左右別々に実行
- **AI アップスケールで解像度が変わるケース**: 既存の「編集中ソース解像度変化 → Shape リスケール」
  機構 (preset-and-adjustment.md §3) にそのまま乗る (Shape は通常オブジェクトなので)

---

## 11. 段階的実装計画

### フェーズ 1 — 顔検出 MVP (同梱) ★v1.1.0 の主目玉

1. `src/ai/detect.rs`: `DetectorProfile` / `OutputFormat` / `detect()` + `decode_yunet`
2. YuNet 同梱 (`ModelKind::DetectorYuNetFace`, EMBEDDED_MODELS, vendor 取得スクリプト)
3. worker (`AutoMaskPending` + `start_auto_mask` / `poll_auto_mask`)
4. conceal 編集画面に「自動マスク作成」ボタン + ポップアップ (モデル=顔のみ / 矩形・楕円 / 余白)
5. ボックス → Shape 注入 + batch Undo + トースト
6. 設定フィールド + ソフトウェア情報に MIT 表記

→ この時点で「**写真の顔を検出して楕円ぼかしを叩き台生成 → 手修正**」が箱から出して動く

### フェーズ 2 — BYO 検出器 (ユーザー追加) ★v1.1.0

7. `decode_yolov8` + NMS、`DetectorProfile` をサイドカー JSON / ONNX introspection から構築
8. BYO フォルダ走査 + モデルチェックリストへの反映
9. 「対応モデル」ダイアログ (一覧 + ライセンス注記 + 配置案内、任意でアプリ内 DL)
10. erase 編集画面にも同ボタン (注入先切替のみ)

→ deepghs 局部検出 (YOLOv8) を各自入れれば R18 モザイク補助として使える

### フェーズ 3 — 一括処理 (将来、v1.2 候補)

- グリッドで複数選択 → 全ページに検出 + マスク適用 → **焼き込みエクスポート** (公開用)
- 検出 core は流用。新規は「選択範囲のバッチ実行」+ 既存キャプチャ保存経路 (display pipeline 合成結果の
  ファイル化) を使った一括出力
- 大量処理 (写真の顔ぼかし) の本命だが UX が別物なので分離

### フェーズ 4 — 精度・利便の上積み (将来)

- YuNet landmark から顔の傾きを推定して楕円 `rotation_rad` に反映
- CenterFace (MIT, hard-set 精度上) を代替同梱オプションに
- クラス別の形状/余白プリセット

---

## 12. エッジケース

| ケース | 動作 |
|---|---|
| 検出 0 件 | トースト「検出されませんでした」、Shape 追加なし |
| BYO モデルが壊れ/未対応形状 | ロード時に出力形状を検証、不一致ならエラートースト + そのモデルを無効化 |
| 巨大画像 | 検出器入力は固定サイズなので縮小コピーで推論、ボックスを原寸へスケール。メモリ問題なし |
| 多数検出 (群衆など) | 各ボックスが Shape に。NMS 後でも多ければユーザーが選択ツールで間引く |
| 適用中に再度「適用」 | 進行中 worker を cancel して新規実行 (二重注入防止) |
| 適用中にモード退出 / ページ移動 | cancel を立て、受信結果は世代ガードで破棄 |
| 透過 PNG | 検出は RGB 化して実行。マスクは座標のみなので透過は保持される |
| 見開き | target_idx のソースに対して実行 |

---

## 13. テスト計画

- **単体** (`src/ai/detect.rs`):
  - ボックス → Shape 変換 (矩形/楕円、margin、座標スケール) の数値検証
  - `decode_yolov8` / `decode_yunet` を合成テンソルで検証 (既知入力 → 既知ボックス)
  - NMS (重複ボックスの抑制)
  - `DetectorProfile` 構築: deepghs `model_artifacts.json` / `labels.json` パース、ONNX introspection の
    クラス数推定 (`C = ch - 4`)
- **App レベル**:
  - 自動マスク注入で N 個の Shape が conceal_shapes に入り、Undo 1 回で全消え、`conceal_cache` がクリアされる
  - cancel / 0 件トースト / 世代ガード
  - 注入後に選択ツールで編集 → モード終了で DB + サイドカーに保存される
  - conceal と erase の両方で動く
- **手動 E2E** ([e2e-smoke-test.md](e2e-smoke-test.md) に追記):
  - 写真で YuNet 顔検出 → 楕円ぼかし → 手修正 → 保存 → 再表示
  - BYO 検出器 (deepghs 局部) を配置 → 自動マスク → モザイク
  - 巨大画像 / ZIP・PDF ページ / 見開き

---

## 14. 残課題・レビュー観点

- [ ] YuNet の正規化 (Raw 0..255 か /255 か) と入力サイズの最適値を実機確認 (`model_artifacts` 相当が無いので OpenCV 実装に合わせる)
- [ ] YuNet 出力テンソルの正確な形状・priors 生成パラメータ (stride/min_sizes) の確定
- [ ] 余白 (margin) の既定値・上限。顔ぼかしは広め、局部は法的要件次第でユーザー調整
- [ ] 形状の既定をモデル単位で持つか (顔=楕円 / 局部=矩形)、グローバル設定 1 つにするか
- [ ] 「対応モデル」ダイアログでアプリ内 DL を v1.1.0 に入れるか (MVP は手動配置で十分か)
- [ ] BYO サイドカー JSON の独自フォーマット仕様 (deepghs 形式が無いモデル向け) を v1.1.0 で出すか後回しか
- [ ] erase 側の自動マスクは v1.1.0 に含めるか (機構は共有だが用途が薄い → フェーズ 2 後半)
- [ ] 複数モデル同時適用時、クラスごとに形状を変えたい要望が出るか (MVP は 1 形状)

---

## 15. ドキュメント / マニュアル更新義務 (実装時)

CLAUDE.md「コード修正時のドキュメント同時更新」に従い、実装と同時に:

- `docs/preset-and-adjustment.md` — マスク系に「自動マスク作成」を追記 (§5 周辺)
- `docs/architecture-overview.md` — `src/ai/detect.rs` 追加、検出器サブシステム
- `docs/async-architecture.md` — `AutoMaskPending` worker を追加
- `docs/spec.md` — 機能一覧・設定項目
- `htdocs/mimageviewer/manual/` — 顔ぼかし操作、BYO モデルの配置とライセンス注記 (中立表記)
- `htdocs/mimageviewer/index.html` — 新機能紹介 (プライバシー/匿名化の顔ぼかし)
- `docs/README.md` — 本ドキュメントを索引に追加 (このコミットで実施)

---

## 16. 補足: なぜこの設計か (判断の記録)

- **顔検出を標準・局部検出を BYO** にしたのは、(1) MIT で同梱できる顔検出が実在し (YuNet)、局部検出は
  実用モデルが全部 AGPL/GPL で同梱できない、(2) 顔ぼかし (プライバシー) は本流で機微性が低く、R18 局部は
  オプトインに留めるのが配布上も健全、という 2 点による
- **検出結果を編集可能 Shape にする** のは、検出精度が完璧でない前提 ("AI 提案 → 人が確定") を UI に
  内蔵するため。確定ビットマップに焼くと誤検出の修正が効かない
- **DetectorProfile + OutputFormat enum** は「標準=顔 / 追加=任意」を enum 追加だけで拡張できる継ぎ目。
  検出器ローダ基盤は将来 mosaic 以外 (顔クロップ等) にも再利用可能
- 一括処理を分離したのは、編集画面の対話的 UX とバッチ UX が別物で、混ぜると両方中途半端になるため
