# v1.1.0 AI 提案マスク機能 設計案 (Codex)

v1.1.0 では、既存の消しゴム / 隠蔽加工マスクツールに
「検出モデルで候補オブジェクトを自動生成し、ユーザーが手修正して確定する」流れを追加する。

標準機能は顔検出に絞り、MIT ライセンスで配布しやすい顔検出 ONNX モデルを同梱する。
追加の検出モデルは mIV が再配布せず、ユーザーが ONNX ファイルを指定して使う。
これにより、一般ユーザーには「写真の顔ぼかし」をすぐ使える形で提供しつつ、
上級者には任意モデルを使ったマスク候補生成を開く。

関連:

- [conceal-feature-plan.md](conceal-feature-plan.md) — 隠蔽加工 / Ctrl+E エクスポートの基盤
- [preset-and-adjustment.md](preset-and-adjustment.md) — 補正 / AI / 消しゴム / 隠蔽のキャッシュ設計
- [async-architecture.md](async-architecture.md) — AI 推論 worker / キャンセル / 進捗通知
- [ui-responsiveness.md](ui-responsiveness.md) — UI スレッドを止めないためのチェックリスト

---

## 1. 目的

### 1.1 ユーザー価値

- 大量の写真に対して、顔のぼかし / モザイク候補を短時間で作れる
- 既存の矩形 / 楕円 / ベクタ編集で、検出漏れやズレを手で直せる
- AI 生成画像やその他の用途では、ユーザー指定モデルで同じマスク UI を流用できる
- 自動処理の結果をそのまま確定扱いにせず、「AI 提案 → 確認 → 書き出し」にできる

### 1.2 製品上の位置づけ

この機能は「自動モザイク」ではなく、**AI 提案マスク**として扱う。
検出結果は通常のマスクオブジェクトに変換され、ユーザーが位置・サイズ・形状を確認してから
隠蔽加工または消しゴム処理に使う。

標準の訴求は顔検出:

- 写真 / イベント画像 / スクリーンショットのプライバシー加工
- 大量画像の一括候補生成
- mIV だけで確認、修正、エクスポートまで完結

追加モデルは拡張機能:

- mIV はモデルを同梱しない
- 対応プロファイルに合う ONNX をユーザーが指定する
- モデルのライセンス・利用条件・用途適合性はユーザーが確認する

---

## 2. スコープ

### 2.1 v1.1.0 に入れる

- 隠蔽加工モード内の「自動マスク作成...」ボタン
- 消しゴムモードからも同じダイアログを呼べる共通導線
- 顔検出モデルを標準同梱し、顔を楕円または矩形のマスク候補に変換
- 現在画像への候補生成
- チェック済み画像 / 選択画像へのバッチ候補生成
- ユーザー指定 ONNX モデルの登録 UI
- 検出プロファイル方式
  - `YuNetFace` プロファイル: 標準顔検出
  - `YoloV8Boxes` プロファイル: 上級者向けユーザーモデル
  - `NudeNetYoloV8` プロファイル: 動作確認例として扱うが、モデルは同梱しない
- しきい値、対象ラベル、形状、拡張率の設定
- 生成結果を通常の `Shape` として保存
- Undo / Redo で生成操作を取り消せる

### 2.2 v1.1.0 では入れない

- 任意の ONNX を自動解析して使える汎用ランタイム
- Python / Docker / 外部コマンドを前提にした導線
- mIV からの非同梱モデル自動ダウンロード
- モデルのライセンス適合性の保証
- 検出結果だけで「処理済み」とみなす完全自動ワークフロー
- 投稿先や公開先の基準への適合判定
- ZIP / PDF 内画像へのバッチ生成
  - 現在ページが既にデコード済みの場合の単発生成は検討可
  - フォルダ横断バッチは通常画像ファイルから開始する

---

## 3. モデル方針

### 3.1 標準同梱モデル: 顔検出

第一候補は OpenCV Zoo の YuNet。
OpenCV Zoo の `face_detection_yunet` ディレクトリは MIT License と明記されている。

- 配布元: <https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet>
- 同梱候補:
  - `face_detection_yunet_2023mar.onnx`
  - `face_detection_yunet_2026may.onnx`
- 初期設定:
  - 表示名: `顔検出`
  - 形状既定: 楕円
  - 拡張率既定: `1.20`
  - しきい値既定: 実測後に決定

配布時は、モデルの LICENSE を mIV のソフトウェア情報 / ライセンス一覧に追加する。
法的判断ではなく実装方針として、MIT 表記の確認、同梱ファイル、ライセンス文の保持を必須にする。

### 3.2 ユーザー指定モデル

ユーザーモデルは「任意 ONNX」ではなく、**対応済み出力プロファイルに合う ONNX**として扱う。
ONNX はファイル形式でしかなく、モデルごとに入力前処理・出力テンソル・後処理が異なるため。

```rust
enum DetectorProfileKind {
    YuNetFace,
    YoloV8Boxes,
    NudeNetYoloV8,
}
```

ユーザーがモデルを追加するときは、以下を登録する:

- モデルファイル (`.onnx`)
- プロファイル種別
- ラベルファイル (`labels.json` / `labels.txt`) または内蔵ラベルセット
- 使用するラベル
- 既定しきい値
- 既定形状
- 既定拡張率
- ライセンスメモ

保存先は `%APPDATA%/mimageviewer/user_models/detectors/<profile-id>/` とし、
元ファイル参照ではなく mIV 管理フォルダへコピーする。外部パス参照にすると、
移動や削除で設定が壊れやすいため。

### 3.3 モデル説明の表記

マニュアルや UI では以下の線引きを守る。

- mIV に同梱しているモデルと、ユーザーが追加したモデルを明確に分ける
- 非同梱モデルは「動作確認例」として載せる
- `推奨`、`安全`、`商用利用可`、`この用途に適合` といった保証表現は避ける
- モデルのライセンス、利用条件、用途適合性はユーザー自身で確認する旨を書く
- 特定サービスや投稿先の基準名は書かない

---

## 4. UI 設計

### 4.1 入口

消しゴム / 隠蔽加工の左パネルに共通ボタンを追加する。

```text
ツール:
 [選] [筆] [囲] [直] [縦] [横] [矩] [楕]

[自動マスク作成...]
```

基本はユーザー案の通り:

1. 消しゴムまたは隠蔽加工のマスクツールを開く
2. `自動マスク作成...` を押す
3. 適用したいモデルを複数チェックする
4. 形状を選ぶ
5. `適用` でマスクオブジェクトが自動作成される
6. 通常の選択ツールでサイズや位置を手修正する

ただし形状は、単純な `矩形 / 楕円` だけでなく `モデル既定` を先頭に置く。
顔検出は楕円、汎用物体検出は矩形の方が自然なため。

### 4.2 ダイアログ案

```text
┌─ 自動マスク作成 ──────────────────────────────┐
│ 対象:                                            │
│  ● 現在の画像                                    │
│  ○ チェック済み画像                              │
│  ○ 選択中の画像                                  │
│                                                  │
│ 検出モデル:                                      │
│  ☑ 顔検出 (標準)                                 │
│      形状: [モデル既定: 楕円 ▼]  しきい値: 0.60  │
│      拡張: 1.20x                                 │
│  ☐ User: NudeNet 320n                            │
│      形状: [モデル既定 ▼]        しきい値: 0.25  │
│      対象ラベル: [選択...]                       │
│                                                  │
│ 出力:                                            │
│  ● 現在のマスクに追加                            │
│  ○ 現在のマスクを置き換え                        │
│                                                  │
│ ☐ 重複候補を統合                                 │
│ ☐ 小さすぎる候補を除外                           │
│                                                  │
│                    [キャンセル] [適用]            │
└──────────────────────────────────────────────────┘
```

初期実装では `プレビュー` ボタンは必須にしない。
`適用` 後に通常のマスク編集 UI で候補が見え、Undo で一括取消できれば十分に扱える。

将来、検出が遅いモデルや大量バッチで誤検出が多い場合は、
`プレビュー → 適用` の 2 段階に拡張する。

### 4.3 複数モデル選択時の扱い

複数モデルをチェックした場合は、各モデルの候補を同じマスクへ追加する。
同一領域に近い候補は NMS / IoU で統合できるようにする。

統合ルール:

- 同じモデル内の重複はモデルプロファイル側の NMS で削減
- モデル間の重複はオプション `重複候補を統合` が ON のときだけ統合
- 統合時は score が高い候補を基準にし、bbox は union ではなく高 score 側を優先

理由: union にすると候補が大きくなりすぎ、手修正量が増えるため。

### 4.4 出力形状

```rust
enum SuggestedMaskShape {
    ModelDefault,
    Rect,
    Ellipse,
}
```

候補 bbox から `Shape` を作る。

- Rect: `Shape::Rect { center, half_w, half_h, rotation_rad: 0.0 }`
- Ellipse: `Shape::Ellipse { center, rx, ry, rotation_rad: 0.0 }`

拡張率は bbox 中心基準で掛ける。

```text
expanded_w = bbox_w * expand
expanded_h = bbox_h * expand
```

顔検出の初期値は楕円 + 1.20x。
ユーザー指定モデルはプロファイルごとに既定を持つ。

---

## 5. データモデル

### 5.1 検出プロファイル

```rust
struct DetectorProfile {
    id: String,
    display_name: String,
    kind: DetectorProfileKind,
    source: DetectorSource,
    labels: Vec<DetectorLabel>,
    enabled_labels: Vec<String>,
    default_threshold: f32,
    default_shape: SuggestedMaskShape,
    default_expand: f32,
    license_note: Option<String>,
}

enum DetectorSource {
    Builtin,
    UserModel { model_path: PathBuf },
}

struct DetectorLabel {
    id: String,
    display_name: String,
}
```

ユーザー定義プロファイルは `settings.db` に保存する。
大きな ONNX BLOB は DB に入れず、APPDATA 配下のファイルパスだけ保持する。

### 5.2 検出結果

```rust
struct Detection {
    label: String,
    score: f32,
    bbox: RectPx, // x, y, w, h in original image pixels
    source_profile_id: String,
}
```

検出結果は永続化せず、すぐ `Shape` に変換する。
ただし生成した `Shape` には、将来の置換 / 表示 / デバッグのために任意メタ情報を持てるとよい。

```rust
struct ShapeMeta {
    generated_by: Option<String>, // detector profile id
    generated_label: Option<String>,
    generated_score: Option<f32>,
}
```

既存 JSON との互換性を保つため、`ShapeMeta` は `#[serde(default)]` の optional field にする。
旧データには meta が無いものとして読む。

### 5.3 保存先

最終的なマスクは既存の保存先に乗せる。

- 隠蔽加工: `conceal_db`
- 消しゴム: `mask_db`
- サイドカー: 既存のマスク sidecar 形式に追従

AI 提案専用の DB は作らない。
提案後は通常のマスクオブジェクトとして編集できることを優先する。

---

## 6. アーキテクチャ

### 6.1 新規モジュール案

| モジュール | 役割 |
| --- | --- |
| `src/ai/mask_detector.rs` | 検出リクエスト / 結果型 / worker 起動 API |
| `src/ai/detector_profiles.rs` | 標準 / ユーザー検出プロファイルの管理 |
| `src/ai/detectors/yunet.rs` | YuNet 前処理・後処理 |
| `src/ai/detectors/yolo_v8.rs` | YOLOv8 boxes 系の後処理 / NMS |
| `src/ui_auto_mask_dialog.rs` | 自動マスク作成ダイアログ |
| `src/ui_dialogs/detector_models.rs` | ユーザーモデル追加 / 削除 UI |

`ai/mod.rs` に detector 系を公開する。
ONNX Runtime のロードは既存 `ai/runtime.rs` の方針に合わせ、UI スレッドではセッションを作らない。

### 6.2 Worker

検出処理は必ず worker thread で行う。
現在画像 1 枚でも、ONNX session 初期化や画像リサイズで UI が止まる可能性があるため。

```rust
struct AutoMaskPending {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<AutoMaskEvent>,
}

enum AutoMaskEvent {
    Progress { done: usize, total: usize, current: String },
    ImageDone { target: MaskTargetKey, shapes: Vec<Shape> },
    Failed { target: MaskTargetKey, message: String },
    AllDone,
}
```

UI は毎フレーム `try_recv` で取り込み、受け取った `Shape` を現在のマスクまたは対象ページの
保存済みマスクに merge する。

キャンセル条件:

- ダイアログのキャンセル
- フォルダ切替
- フルスクリーン終了
- 新しい自動マスク作成の開始

バッチ時は画像 1 件ごとの開始前に cancel を確認する。

### 6.3 入力画像

現在画像:

- `fs_cache` / `adjustment_cache` 由来の表示済み画像を使うか、
  元画像から worker でデコードする
- 座標は必ず元画像ピクセル系へ戻して保存する

バッチ:

- v1.1.0 は通常ファイルの画像を対象にする
- ZIP / PDF / 動画フレームは対象外
- デコードは worker 側
- I/O は `GlobalIoSemaphore` の Normal または Low を使う

### 6.4 画像補正との関係

検出は原則として元画像ピクセルに対して行う。
補正 / ポストフィルタ後の表示画像で検出すると、保存されるマスク座標と元画像座標の対応が
分かりにくくなるため。

ただし、現在画像に対して既に回転・表示変換がある場合は、
既存のマスクツールと同じ座標変換を使う。
この部分は `Shape` 生成前に必ず確認する。

---

## 7. ユーザーモデル追加 UI

環境設定に `AI / 検出モデル` ページを追加する。

```text
検出モデル

標準:
  顔検出 (同梱)        [有効]

ユーザーモデル:
  NudeNet 320n         profile: NudeNet YOLOv8   [編集] [削除]
  COCO detector        profile: YOLOv8 Boxes     [編集] [削除]

[モデルを追加...]
```

追加ダイアログ:

```text
モデルを追加

名前: [                         ]
ONNX: [参照...]
プロファイル: [YOLOv8 Boxes ▼]
ラベル: [labels.txt を選択...] または [内蔵ラベルセット ▼]

既定:
  しきい値: 0.25
  形状: [モデル既定 ▼]
  拡張: 1.20x

ライセンスメモ:
  [                                ]

☑ このモデルのライセンスと利用条件を自分で確認しました

[キャンセル] [追加]
```

このチェックは法的な免責ではなく、mIV がモデルの権利関係を保証しないことを UI 上で明確にするため。

---

## 8. バッチ生成

大量写真の顔ぼかし用途では、単発生成だけでは弱い。
v1.1.0 ではチェック済み / 選択画像に対してバッチ生成できるようにする。

### 8.1 バッチの動き

```text
グリッドで対象画像をチェック
→ 隠蔽加工モードを開く
→ 自動マスク作成...
→ 対象: チェック済み画像
→ 顔検出を選択して適用
→ 各画像にマスク候補が保存される
→ ユーザーが順番に開いて確認・修正
→ Ctrl+E で書き出し
```

### 8.2 バッチ時の注意

- 既存マスクに追加するか置換するかを必ず選ばせる
- 置換は「同じ検出プロファイル由来の候補だけ置換」が理想
- v1.1.0 初期は単純に `現在のマスクを置き換え` / `追加` の 2 択でもよい
- エラー画像は一覧に残し、全体処理は止めない
- 進捗モーダルを出し、キャンセル可能にする

---

## 9. 実装フェーズ

| Phase | 内容 | 目安 |
| --- | --- | --- |
| 0 | 詳細調査: YuNet ONNX の入力 / 出力確認、同梱するモデルファイル決定、LICENSE 収集 | 1-2 日 |
| 1 | `DetectorProfile` / 設定保存 / 標準顔検出プロファイル | 2-3 日 |
| 2 | YuNet 推論 worker、前処理 / 後処理、bbox → `Shape` 変換 | 4-6 日 |
| 3 | 隠蔽加工 / 消しゴムパネルの `自動マスク作成...` UI、現在画像への適用、Undo | 3-5 日 |
| 4 | バッチ生成、進捗モーダル、キャンセル、保存済みマスクへの merge | 4-6 日 |
| 5 | ユーザーモデル登録 UI、APPDATA へのコピー、ラベル管理 | 3-5 日 |
| 6 | YOLOv8 boxes / NudeNet 互換プロファイル、NMS、対象ラベル選択 | 7-12 日 |
| 7 | テスト、ドキュメント、ライセンス一覧、手動 E2E | 4-6 日 |

合計目安: 28-45 日。
YuNet の出力処理が素直に実装でき、YOLOv8 互換プロファイルを最小限に絞れば下限に近づく。
ユーザーモデル UI とバッチ確認を丁寧に作るなら上限寄り。

---

## 10. テスト計画

### 10.1 Unit

- YuNet 出力テンソル → bbox 変換
- YOLOv8 boxes 出力 → bbox 変換
- NMS / IoU の境界ケース
- bbox 拡張率
- bbox → Rect / Ellipse `Shape`
- `DetectorProfile` の settings roundtrip
- ユーザーモデル import のパス検証
- 旧 `Shape` JSON が `ShapeMeta` なしで読めること

### 10.2 Integration

- 現在画像に顔検出を適用 → `conceal_db` に保存 → 再起動相当で復元
- 消しゴムモードで同じ導線から候補生成
- バッチ生成で複数画像にマスク保存
- バッチ中キャンセル
- ユーザーモデル削除時に設定と APPDATA ファイルが整合すること

### 10.3 Manual E2E

- 顔が 1 つの写真
- 複数人の写真
- 横顔 / 小さい顔 / 眼鏡 / マスクなど検出しづらい写真
- 顔なし写真
- 4K 以上の写真
- 100 枚程度のバッチ
- 検出後に楕円を手修正し、Ctrl+E でぼかし / モザイクを書き出す

---

## 11. ドキュメント更新

実装時に以下を更新する。

- [spec.md](spec.md)
  - AI 提案マスク
  - 標準顔検出
  - ユーザー検出モデル
- [architecture-overview.md](architecture-overview.md)
  - `ai/mask_detector.rs`
  - `ui_auto_mask_dialog.rs`
  - 検出モデル管理
- [async-architecture.md](async-architecture.md)
  - AutoMask worker
  - バッチ進捗 / キャンセル
- [preset-and-adjustment.md](preset-and-adjustment.md)
  - 消しゴム / 隠蔽マスク生成との関係
- [keymap-spec.md](keymap-spec.md)
  - 必要ならショートカット追記
- `htdocs/mimageviewer/manual/`
  - 顔検出による自動マスク作成
  - ユーザー指定モデルの追加
  - モデルのライセンス確認について

マニュアルでは「顔ぼかし」「プライバシー加工」「任意モデルによる候補生成」を中心に説明する。
特定サービスの基準や、モデル利用時の適合保証は書かない。

---

## 12. 未確定事項

- YuNet は `2023mar` と `2026may` のどちらを同梱するか
- 顔検出の既定しきい値
- 顔 bbox の拡張率初期値
- バッチ対象に検索結果 / 現在フォルダ全体を含めるか
- `ShapeMeta` を v1.1.0 で入れるか、将来拡張に回すか
- ユーザーモデル追加 UI を v1.1.0 でどこまで親切にするか
- YOLOv8 互換プロファイルを v1.1.0 に含めるか、v1.1.x に分けるか

---

## 13. Codex 推奨

v1.1.0 の軸は **標準の顔検出 + 手修正しやすい AI 提案マスク** に置く。
ユーザーモデル追加は同じ仕組みの拡張として入れるが、訴求の中心にはしない。

UI はユーザー案の「自動マスク作成ボタン → モデルを複数チェック → 形状選択 → 適用」でよい。
ただし、形状は `モデル既定 / 矩形 / 楕円` にし、顔検出は楕円既定にする。
これで一般ユーザーは迷わず顔ぼかしに使え、上級者は必要に応じてモデルを足せる。
