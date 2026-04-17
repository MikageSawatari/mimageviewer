# 補正プリセット・AI キャッシュ設計

画像補正 (adjustment) と AI アップスケール/デノイズ/Inpaint は、複数レイヤーのキャッシュと優先順位の決定ロジックで
成り立っている。「補正したら元に戻った」「AI 結果が一瞬消える」といった不具合は、ここの無効化ルールの間違いから起きる。

---

## 1. スコープ (v0.6.0 で簡素化)

補正パラメータは **2 スコープ + 10 スロット** で構成される:

```
スコープ              保存先
────────────────────────────────────────────────
グローバル            settings.json の global_preset
ページ個別            adjustment.db の page_params テーブル

保存スロット 0〜9     settings.json の preset_slots  (独立)
```

旧 (v0.5.0〜v0.6.0 開発版) の「フォルダ単位 4 プリセット + ページ→プリセット idx」方式は廃止した。
未リリース機能だったため DB マイグレーションは行わず、`AdjustmentDb::open()` が
旧テーブル `presets` / `page_presets` を `DROP TABLE IF EXISTS` で破棄し、
新しい `page_params(page_path TEXT PK, params_json TEXT)` を作成する。

### 1.1 有効パラメータの決定

表示時のページ `idx` の有効パラメータは:

```
effective = adjustment_page_params.get(idx) ?? settings.global_preset
```

`adjustment_page_params: HashMap<usize, AdjustParams>` はフォルダ/ZIP/PDF ロード時に
`AdjustmentDb::load_page_params(prefix)` で一括読込される。

### 1.2 自動個別化

補正パネルのスライダーや AI モデル選択を操作した瞬間に、その変更を含む
`AdjustParams` が「現在のページ個別パラメータ」として書き込まれる
(`App::set_page_params(idx, params)`)。スコープ切替という明示操作は存在しない。

`set_page_params` は **「個別パラメータがグローバルプリセットと完全一致」** したときだけ
個別レコードを削除する (フォールバックでグローバルが使われるため保存不要)。
旧バージョンは `is_removable()` (= identity かつ AI 未使用) で削除判定していたが、
**「グローバルが AI ON、特定ページだけ AI OFF」のような上書き** を保存した直後に
個別が消えてしまい、ユーザの意図 (デノイズ OFF など) が反映されない不具合があった。
グローバルとの等価比較に変更した時点で、DB 側 (`AdjustmentDb::set_page_params`) の
`is_removable` 判定も廃止し、呼び出し側 (`App::set_page_params`) で
削除/保存の振り分けを行う構造になった。

### 1.3 アクションボタン

補正パネルに 3 つのボタンがある:

| ボタン | 動作 |
| --- | --- |
| 全画像に適用 | 現在のパラメータを、現フォルダ/ZIP/PDF の全画像ページに一括書込 (`apply_params_to_all_pages`)。色調のみの一斉調整に使う |
| 全画像から削除 | 現フォルダ/ZIP/PDF の全画像ページから個別設定を削除 (`clear_all_page_params`)。「全画像に適用」の取り消し |
| 標準にする | 現在のパラメータを `settings.global_preset` にコピー (`copy_params_to_global`)。全フォルダ共通の既定値を更新する |
| 個別設定を解除 | 現ページの個別レコードを削除 (`clear_page_params`)。標準設定に戻す。`Ctrl+Backspace` でも実行可能 |

### 1.4 保存スロット

10 個の名前付きスロット。フルスクリーンで `Ctrl+0〜9` を押すと
`App::apply_slot_to_current_page(slot_idx)` が呼ばれ、該当スロットのパラメータを
**現在のページ個別設定として書き込む** (= そのページを個別化する)。

> 旧来は `Shift+0〜9` だったが、egui の logical-key 方式ではキーボード配列によって
> Shift+数字が記号 (`!"#$%&'()` など) に置き換わり `Key::Num1` 等にマッチしないため
> Ctrl 修飾に変更した (JIS 配列の Shift+0 は文字を生成しないため特に致命的だった)。

補正パネルの保存スロット欄 (`💾` ボタン) で現在のパラメータをスロットに保存できる。

---

## 2. AdjustParams の中身

`adjustment.rs::AdjustParams`:

- `brightness`, `contrast`, `gamma`, `saturation` (色情報の定番)
- `temperature` (±100。色温度。±0 以外だと f32 パイプライン必須)
- `black_point`, `white_point`, `midtone` (トーンカーブのレベル補正)
- `auto_mode`: `None` / `Auto` / `MangaCleanup` (自動補正モード)
- AI 関連: `upscale_model`, `denoise_model` (`Option<String>`)

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
2. 有効パラメータが identity (無補正) → 何もしない (`fs_cache` をそのまま使わせる)
3. それ以外 → `apply_sync_adjustment(idx)` で同期的に補正してキャッシュに格納

CPU 処理は LUT ベースなので 1 枚あたり数ミリ秒で済む。UI スレッドで OK。

### 3.2 AI の適用タイミング

AI は**重い** (数秒〜数十秒) ので必ず別スレッド:

1. 有効パラメータに AI モデル指定があれば、`ai_upscale_pending[idx]` を作成 + 推論開始
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
| 色系パラメータ変更* (ページ個別) | 該当 idx のみクリア | 残す | 残す |
| AI モデル変更 (ページ個別) | 全クリア | **全クリア** | **キャンセル** |
| 保存スロット読込 → 現ページに適用 | 全クリア | AI モデルが異なれば全クリア | 同上 |
| 「全画像に適用」 / 「全画像から削除」 | 全クリア | AI 設定が変わる idx のみクリア + pending キャンセル | あり |
| 「標準にする」 (global_preset 更新) | 全クリア | global の AI 設定が変わった場合、継承ページ (override なし) の idx をまとめてクリア + pending キャンセル | あり |
| 「個別設定を解除」 (Ctrl+Backspace) | 該当 idx のみクリア | AI 設定が変わるなら該当 idx のみクリア + pending キャンセル | あり |
| フォルダ切替 | 全クリア | 全クリア | キャンセル |
| 回転変更 | **クリアしない** (描画時の GPU 行列で回転) | クリアしない | — |
| 消しゴムマスク変更 | 該当 idx のみクリア (再合成) | 該当 idx のみクリア | — |

*「色系」= brightness/contrast/gamma/saturation/temperature/levels/auto_mode

### 4.1 ヘルパー関数

`App` には 3 系統の無効化ヘルパーがある:

```rust
fn clear_adjustment_caches(&mut self, idx: usize)
    // adjustment_cache[idx] のみクリア

fn clear_all_adjustment_and_ai_caches(&mut self, idx: usize)
    // adjustment_cache[idx] + ai_upscale_cache 全クリア + ai_upscale_pending キャンセル
    // (単一 idx 操作で AI モデル変更が起きたとき用)

fn clear_ai_caches_for_indices(&mut self, indices: &[usize])
    // 指定 idx 群の ai_upscale_cache / failed / pending をまとめてクリア
    // (bulk / global 系の操作で「AI 設定が変わった idx だけ」落とすとき用)
```

単一 idx で AI モデル変更を伴う可能性がある操作は `clear_all_adjustment_and_ai_caches`、
複数ページにまたがる操作は `clear_ai_caches_for_indices` を使う。

`set_page_params` / `clear_page_params` / `apply_params_to_all_pages` /
`clear_all_page_params` / `copy_params_to_global` の実装内でも、必要に応じて
`adjustment_cache` をクリアしている (全クリア vs 部分クリア)。詳細はソース参照。

特に `clear_page_params(idx)` は、削除後の effective params を見て
**old.ai_settings_eq(new) が false なら** その `idx` の `ai_upscale_cache` /
`ai_upscale_failed` / `ai_upscale_pending` をクリアする。これがないと
「個別で AI OFF にしていたページから個別を解除しても、グローバルの AI が
再実行されない」という不具合になる (実際、`ui_fullscreen.rs` から
`Ctrl+Backspace` で解除した直後に上記不具合が発生していた)。

同じ考え方を bulk / global 系にも横展開している:
- `apply_params_to_all_pages(params)`: 書換前の各 idx の effective params と
  `params` を `ai_settings_eq` で比較し、一致しない idx だけ AI キャッシュを落とす。
- `clear_all_page_params()`: 個別削除後の effective params は global_preset になるため、
  書換前の effective params と global を比較して差がある idx だけ落とす。
- `copy_params_to_global(params)`: 旧 global と新 `params` を比較し、AI 設定が
  変わった場合のみ「override を持たない (= global 継承) 画像ページ」を対象に落とす。
  override を持つページは effective params が変わらないので触らない。

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
- [ ] UI パネル (`ui_adjustment_panel.rs::draw_sliders`) にスライダー追加
- [ ] 変更検出は `draw_sliders` が `(changed, dragging)` を返すので追加対応不要
- [ ] `AdjustParams` の JSON シリアライズ互換性を確認 (`#[serde(default)]` 等)
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
| `ui_erase.rs` | MI-GAN によるマスク領域 inpaint (`InpaintMiGan` モデルを直接 `with_session` で呼び出し)。見開き中央ギャップ補完は精度不足で削除済み (タグ `v0.6.0-with-spread-inpaint` 参照) |
| `classify.rs` | 画像種別分類 (MobileNetV3, Illustration/Comic/3D/RealLife) |

`ModelKind` でモデルを識別。セッションは最初の推論時に遅延生成される。
メモリ負荷が大きいので、不要になった ModelKind はセッションを drop する (runtime.rs)。

---

## 8. UI / UX メモ

### 8.1 ページ切替時のトースト

`open_fullscreen(idx)` 時、`adjustment_page_params` に該当 idx が含まれていれば
右上に `ページ補正適用` トーストを 1.2 秒表示する
(`FEEDBACK_TOAST_DURATION`, `ui_fullscreen.rs:46`)。

### 8.2 サムネイルの補正済みバッジ

グリッド表示で個別補正があるページの左上に青い「補」バッジを表示する
(`draw_cell` の `has_page_override` フラグ)。

### 8.3 フルスクリーン上部バーのボタン

画像補正パネルトグルボタン (🎨) を 1 つだけ置く。
パネルが開いているときは青、個別設定があるときは薄い警告色、それ以外は通常色。

---

## 9. フォルダ側サイドカーバックアップ

ページ個別補正 (`adjustment.db`) と消しゴムマスク (`mask.db`) は、中央 DB だけだと
「フォルダを別ドライブへ移動するとパスキーが無効化されて設定が失われる」という弱点がある。
これを補うため、各ユーザーフォルダ直下に `mimageviewer.dat` (Hidden+System 属性の JSON)
をバックアップとして配置する。

### 9.1 ミラーの原則

- **中央 DB が authoritative**。サイドカーはあくまでバックアップ。
- **すべての書き込みはミラー**: DB 更新と同じタイミングでメモリ上のサイドカー表現
  (`App::sidecars`) を更新し dirty フラグを立てる。
- **実ディスク書き込みのタイミング**:
  1. フォルダ切替時 (`start_loading_items` 冒頭で `flush_all_sidecars`)
  2. アプリ正常終了時 (`on_exit` 内)
  3. 5 秒アイドル時 (毎フレーム `flush_idle_sidecars` で `is_dirty && now - last_change >= 5s` を判定)
- **読み込み**: `start_loading_items` 内で `import_sidecar_to_dbs` が走り、中央 DB に無い
  エントリだけサイドカーからインポートする (冪等)。

### 9.2 キー規則

サイドカー内のエントリはフォルダ**相対**キーで保存する (絶対パスにすると移動で意味が消えるため):

| GridItem         | サイドカー置き場       | 相対キー                                      |
| ---------------- | ---------------------- | --------------------------------------------- |
| `Image(p)`       | `p.parent()`           | `"{filename_lower}"`                          |
| `ZipImage`       | `zip_path.parent()`    | `"{zip_filename_lower}::{entry_name_lower}"`  |
| `PdfPage`        | `pdf_path.parent()`    | `"{pdf_filename_lower}::page_{n}"`            |

これらは `App::page_path_key` が返す絶対 DB キーと 1:1 で対応する。ヘルパー:

- `App::sidecar_folder(idx)` → 置き場
- `App::sidecar_relative_key(idx)` → 相対キー
- `sidecar::reconstruct_image_key(folder, rel)` / `reconstruct_virtual_key(folder, rel)` → 絶対 DB キー再構成

**キーの整合性はユニットテストで担保**。`adjustment_db::normalize_path` の挙動と揃っていないと
インポートで復元されない。

### 9.3 マスクの書き込みタイミング

消しゴムマスクは書き込みコストが大きい (1bit/pixel pack + deflate + JSON 埋め込み) ため、
**消しゴムモードの確定点でのみ** 書く:

1. `ESC` 終了 (ui_erase.rs の ESC ハンドラ内 `save_mask_with_sidecar`)
2. `E` 補完実行 (ui_erase.rs `execute_erase_inpaint` 内 `save_mask_with_sidecar`)
3. 「マスク全削除」ボタン (ui_erase.rs 内 `delete_mask_with_sidecar`)

ストローク毎の書き込みは行わない。中央 DB もサイドカーも同じタイミングでしか書かない。

### 9.4 空になったファイル

サイドカーエントリは `{adjust?, mask?}` 構造。`adjust` と `mask` が両方 `None` になると
`items` マップから削除する。`items` が空のまま flush されると **ファイル自体を `remove_file`**
する (消しゴムマスクも全削除しないと空にはならないので、ユーザの明示的操作が前提)。
ファイル削除失敗は黙って無視。

### 9.5 設定トグル (`sidecar_backup_enabled`)

`Preferences > フォルダ > 設定のバックアップ` に配置。デフォルト ON。
OFF にすると **読み書き両方スキップ** (既存 `.dat` は削除しない)。単一の分岐点 (`App::sidecar_mut`
が `None` を返す) で済むので、デバッグ面でのコストは最小。

### 9.6 エラーハンドリング

読み取り専用メディアや権限不足で IO が失敗した場合:
- ログに 1 行書いて無視
- `SidecarFile::disabled = true` を立てて以降同フォルダは再試行しない (ログ汚染防止)
- アプリ再起動で `disabled` はリセット

ユーザへのダイアログ表示はしない (視聴体験の邪魔になるため)。

### 9.7 テスト

サイドカーの動作は 3 層で自動テスト済み:

- **単体テスト**: `src/sidecar.rs` の `#[cfg(test)] mod tests` に 9 件
  (set/remove、空→削除、JSON ラウンドトリップ、キー再構成など)
- **統合テスト**: [tests/sidecar_import.rs](../tests/sidecar_import.rs) に 12 件
  - **フォルダ移動シナリオ**: 空 DB + サイドカー → DB に復元
  - 中央 DB が authoritative (既存エントリが上書きされない)
  - 部分的重複時の正しいスキップ/インポート振り分け
  - ZIP / PDF エントリのキー整合 (`adjustment_db::normalize_path` と一致)
  - サイドカー無しの no-op
  - 将来バージョンの `.dat` をインポートしない
  - 書き込み不能パスで panic しない
  - Hidden+System 属性付きファイルの再読込・上書き
- **手動 E2E**: フルスクリーン UI 経由での編集→フォルダ移動→復元までは
  [docs/e2e-smoke-test.md](e2e-smoke-test.md) を参照。

統合テストは `cargo test --test sidecar_import` で 1 秒程度で走る。
GUI 起動を含まないため CI でも実行可能。テストの多くは
`AdjustmentDb::open_at` / `MaskDb::open_at` で一時 DB を作って隔離している
(デフォルトの `open()` は `%APPDATA%` を使うのでテスト用途には不向き)。
