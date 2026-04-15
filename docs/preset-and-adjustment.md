# 補正プリセット・AI キャッシュ設計

画像補正 (adjustment) と AI アップスケール/デノイズ/Inpaint は、複数レイヤーのキャッシュと優先順位の決定ロジックで
成り立っている。「補正したら元に戻った」「AI 結果が一瞬消える」といった不具合は、ここの無効化ルールの間違いから起きる。

---

## 1. プリセットの階層

プリセットは **5 枠 (0〜4) + 保存スロット 10 枠** で構成される:

```
preset_idx  種類                保存先
─────────────────────────────────────────────────────────
  0         グローバルプリセット  settings.json の global_preset
  1〜4      フォルダ別プリセット  adjustment.db の presets テーブル
                                  (フォルダ/ZIP/PDF ごと)

保存スロット 0〜9  (独立)         settings.json の preset_slots
```

### 1.1 選択の流れ

1. **アクティブプリセット**: UI の現在の選択 (`active_preset_idx`)。0〜4。
2. **ページ別オーバーライド**: 画像ごとに `adjustment_page_preset` で特定のプリセットに紐付け可能。
   未指定ならアクティブプリセットにフォールバック。
3. **表示**: フルスクリーン時、「そのページの有効なプリセット」の `AdjustParams` で画像を補正する。

フォルダを切り替えると:
- `adjustment.db` から新しいフォルダの 1〜4 を読み直す
- ページ別割当 (`adjustment_page_preset`) もロード
- `adjustment_cache` はクリア

### 1.2 保存スロット

10 個の名前付きスロット。フルスクリーンで数字キー `1-9`, `0` を押すと、
そのスロットのパラメータを**アクティブプリセットにコピー**する。

スロット自体はフォルダに依存しない (グローバルな「よく使う補正」)。
AI モデル設定もスロットに含まれるため、適用時にモデル変更があれば AI キャッシュも影響を受ける (後述)。

---

## 2. AdjustParams の中身

`adjustment.rs::AdjustParams`:

- `brightness`, `contrast`, `gamma`, `saturation` (色情報の定番)
- `temperature` (±100。色温度。±0 以外だと f32 パイプライン必須)
- `black_point`, `white_point`, `midtone` (トーンカーブのレベル補正)
- `auto_mode`: `None` / `Auto` / `MangaCleanup` (自動補正モード)
- AI 関連: `ai_upscale_model`, `ai_denoise_model` (`Option<ModelKind>`)

### 2.1 適用順序

`adjustment.rs::apply_adjustments_fast`:

```
Levels (黒点/白点/中間調) → Gamma → Brightness/Contrast → Saturation → Temperature
```

- `temperature == 0` なら u8→u8 LUT で高速処理
- `temperature != 0` なら f32 パイプライン (やや遅い)

### 2.2 Auto モード

- **Auto**: ヒストグラムの 0.5/99.5 パーセンタイルでレベル補正
- **MangaCleanup**: 紙/インク検出 → グレースケール → S 字カーブ → γ=0.85 → コントラスト ≥15

---

## 3. キャッシュ構造 (フルスクリーン時)

| キャッシュ | 型 | 内容 |
| --- | --- | --- |
| `fs_cache` | `HashMap<idx, FsCacheEntry>` | 生デコード結果。Static / Animated / Failed |
| `adjustment_cache` | `HashMap<idx, FsCacheEntry>` | 補正適用済みテクスチャ |
| `ai_upscale_cache` | `HashMap<idx, FsCacheEntry>` | AI アップスケール/デノイズ適用済み |

描画時 ([display-pipeline.md](display-pipeline.md) を参照) は:

```
adjustment_cache > ai_upscale_cache > fs_cache
```

の優先順位で最も処理済みのテクスチャを選ぶ。

### 3.1 補正の適用タイミング

`App::maybe_apply_adjustment(idx)` が毎フレーム呼ばれ:

1. `adjustment_cache[idx]` が存在する → 何もしない
2. 有効パラメータが恒等 (何の補正もない) → 何もしない (`fs_cache` をそのまま使わせる)
3. それ以外 → `apply_sync_adjustment(idx)` で同期的に補正してキャッシュに格納

CPU 処理は LUT ベースなので 1 枚あたり数ミリ秒で済む。UI スレッドで OK。

### 3.2 AI の適用タイミング

AI は**重い** (数秒〜数十秒) ので必ず別スレッド:

1. フルスクリーンで今のプリセットに AI モデル指定があれば、`ai_upscale_pending[idx]` を作成 + 推論開始
2. 完了すると `ai_upscale_cache[idx]` に結果が入る
3. 以降のフレームはそのテクスチャが使われる (さらに補正が必要なら `adjustment_cache` が上書き)

補正と AI の合成順序:
- **先に AI アップスケール** → それを入力として**後から補正**
- つまり `adjustment_cache` の中身は「AI 後に補正を掛けた」テクスチャ

---

## 4. キャッシュ無効化ルール (早見表)

**これを間違えると高確率でバグる**。変更する前に必ず以下を確認:

| 変更された内容 | `adjustment_cache` | `ai_upscale_cache` | 実行中 AI ジョブ |
| --- | --- | --- | --- |
| 補正パラメータ (色系)* | 該当 idx のみクリア | 残す | 残す |
| アクティブプリセット切替 | 全クリア | 残す (AI モデルが同じなら) | 残す |
| ページ別プリセット割当変更 | 該当 idx のみクリア | 残す | 残す |
| AI モデル変更 | 全クリア | **全クリア** | **キャンセル** |
| 保存スロット読込 | 全クリア | AI モデルが異なれば全クリア | 同上 |
| フォルダ切替 | 全クリア | 全クリア | キャンセル |
| 回転変更 | **クリアしない** (描画時の GPU 行列で回転) | クリアしない | — |
| 消しゴムマスク変更 | 該当 idx のみクリア (再合成) | 該当 idx のみクリア | — |

*「色系」= brightness/contrast/gamma/saturation/temperature/levels/auto_mode

### 4.1 ヘルパー関数

`App` には 2 系統の無効化ヘルパーがある:

```rust
fn clear_adjustment_caches(&mut self, idx_opt: Option<usize>)
    // adjustment_cache のみクリア。idx 指定 or 全削除

fn clear_all_adjustment_and_ai_caches(&mut self)
    // adjustment_cache + ai_upscale_cache + ai_upscale_pending (キャンセル)
```

AI モデル変更を伴う可能性がある操作は必ず後者を使う。

### 4.2 AI 切替時に注意

AI モデルを変更したら、古い AI 結果は**概念的に無効**。残しておくと:

- 次に表示した時、古いモデルの結果が一瞬見えてから新しい結果に差し替わる
- ユーザーは「変えたのに反映されてない」と思う

なので AI モデル変更時は `ai_upscale_cache` を全クリアが正解。

---

## 5. 消しゴム (Erase) との関係

`ui_erase.rs` と `mask_db.rs` で実装された消しゴム機能は、補正パイプラインと連携している:

```
生画像 (fs_cache) ─▶ AI upscale (ai_upscale_cache) ─▶ 補正 (adjustment_cache) ─▶ 画面
                                                   ▲
                                                   │
                    mask_db (消しゴムマスク) ─▶ MI-GAN で inpaint
                    ※ マスク編集が確定したら、inpaint 結果で fs_cache (または ai_upscale_cache) を上書き
```

マスクが存在する画像は、ロード時に inpaint 済みの結果を最終キャッシュに載せる。
マスクが変更されたら **該当 idx の全キャッシュ** を再計算。

---

## 6. 新しい補正項目を追加する時

チェックリスト:

- [ ] `AdjustParams` に新フィールド追加 (デフォルト値は「効果なし」)
- [ ] `apply_adjustments_fast` の適用順序を決めてその中に挟む
- [ ] LUT で済むか、f32 パイプラインが必要かを判断 (必要なら `temperature != 0` 判定に合流)
- [ ] UI パネル (`ui_adjustment_panel.rs`) にスライダー追加
- [ ] 変更ハンドラで `clear_adjustment_caches(Some(idx))` を呼ぶ
- [ ] `AdjustParams` の JSON/SQLite シリアライズ互換性を確認
- [ ] グローバル (settings.json) と フォルダ別 (adjustment.db) の両方で読み書きテスト
- [ ] 保存スロットでもコピーされるか確認
- [ ] サムネイルには**適用しない**方針を維持 (現状は意図的に差をつけている)

---

## 7. AI モジュールの構成

`src/ai/` 以下:

| ファイル | 役割 |
| --- | --- |
| `runtime.rs` | ONNX Runtime (ort) + DirectML EP の初期化、セッションキャッシュ |
| `model_manager.rs` | exe 埋め込みモデルを `%APPDATA%/mimageviewer/models/` に展開 |
| `upscale.rs` | タイル分割 + オーバーラップブレンド (2x/4x モデル) |
| `denoise.rs` | 1x ノイズ除去 (タイル推論は upscale を流用) |
| `inpaint.rs` | MI-GAN による穴埋め。見開き中央ギャップ補完、消しゴムマスク適用 |
| `classify.rs` | 画像種別分類 (MobileNetV3, Illustration/Comic/3D/RealLife) |

`ModelKind` でモデルを識別。セッションは最初の推論時に遅延生成される。
メモリ負荷が大きいので、不要になった ModelKind はセッションを drop する (runtime.rs)。
