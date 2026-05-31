# 補正プリセット・AI キャッシュ設計

画像補正 (adjustment) と AI アップスケール/デノイズ/Inpaint は、複数レイヤーのキャッシュと優先順位の決定ロジックで
成り立っている。「補正したら元に戻った」「AI 結果が一瞬消える」といった不具合は、ここの無効化ルールの間違いから起きる。

---

## 1. スコープ (v0.8.1 で 3 層化)

補正パラメータは **3 スコープ + 10 スロット** で構成される:

```
スコープ              保存先
────────────────────────────────────────────────
グローバル            settings.json の global_preset
お気に入り標準        adjustment.db の favorite_params テーブル (favorite_id TEXT PK)
ページ個別            adjustment.db の page_params テーブル

保存スロット 0〜9     settings.json の preset_slots  (独立)
```

旧 (v0.5.0〜v0.6.0 開発版) の「フォルダ単位 4 プリセット + ページ→プリセット idx」方式は廃止した。
未リリース機能だったため DB マイグレーションは行わず、`AdjustmentDb::open()` が
旧テーブル `presets` / `page_presets` を `DROP TABLE IF EXISTS` で破棄し、
新しい `page_params(page_path TEXT PK, params_json TEXT)` を作成する。

v0.8.1 で「お気に入り単位の標準 (favorite_params)」を追加した。フォルダ単位より粗く・
グローバルより細かい中間層で、用途別 (スキャン画像 / AI 生成 / カメラ写真 / Twitter DL など)
に既定を切り替えたい要件に応える。

### 1.1 有効パラメータの決定

表示時のページ `idx` の有効パラメータは 3 層のカスケードで決まる:

```
effective =
    adjustment_page_params.get(idx)
      ?? adjustment_favorite_params.get(nearest_favorite_id_of(container(idx)))
      ?? settings.global_preset
```

解決は `App::effective_params(idx)` に集約される。お気に入り層のルックアップには
`App::current_favorite_id_for_idx(idx)` → `App::find_nearest_favorite(container_path)`
を使う。**最も近い祖先** (パス最長一致) を選ぶため、ネスト登録されたお気に入り
(例: `G:\pics` と `G:\pics\AI` が両方登録されている) のときは `G:\pics\AI` が優先される。

ZIP/PDF を開いている最中は `container(idx)` が ZIP/PDF 本体のパスになるため、
「AI 生成画像」お気に入り配下の ZIP 内ページにもそのお気に入り標準が自動適用される。

`adjustment_page_params: HashMap<usize, AdjustParams>` はフォルダ/ZIP/PDF ロード時に
`AdjustmentDb::load_page_params(prefix)` で一括読込される。
`adjustment_favorite_params: HashMap<Uuid, AdjustParams>` は **起動時に 1 回**
(`App::hydrate_adjustment_favorite_params`) 全件ロードされ、
`settings.favorites` に存在しない orphan 行は `prune_favorite_params` で掃除される。

### 1.2 自動個別化

補正パネルのスライダーや AI モデル選択を操作した瞬間に、その変更を含む
`AdjustParams` が「現在のページ個別パラメータ」として書き込まれる
(`App::set_page_params(idx, params)`)。スコープ切替という明示操作は存在しない。

`set_page_params` は **「個別パラメータが `effective_default_for_idx(idx)` (= お気に入り標準
があればそれ、なければグローバル) と完全一致」** したときだけ個別レコードを削除する
(フォールバックでその標準が使われるため保存不要)。v0.8.1 以前はグローバル
との等価比較だったが、お気に入り標準が登場したのでそこに切り替えた。

旧バージョンは `is_removable()` (= identity かつ AI 未使用) で削除判定していたが、
**「グローバルが AI ON、特定ページだけ AI OFF」のような上書き** を保存した直後に
個別が消えてしまい、ユーザの意図 (デノイズ OFF など) が反映されない不具合があった。
同じ原理で、「お気に入り標準が AI ON、特定ページだけ AI OFF」も `effective_default_for_idx`
との等価比較によって保存される。DB 側 (`AdjustmentDb::set_page_params`) の
`is_removable` 判定は廃止済みで、呼び出し側 (`App::set_page_params`) で
削除/保存の振り分けを行う構造になっている。

### 1.3 アクションボタン

補正パネルに 6 つのボタンがある (v0.8.1):

| ボタン | 動作 |
| --- | --- |
| このフォルダの全画像に適用 | 現在のパラメータを、現フォルダ/ZIP/PDF の全画像ページに一括書込 (`apply_params_to_all_pages`)。`matches_default` 判定はその一覧の先頭 idx の `effective_default_for_idx` で代表する (一覧内は同じコンテナに属するため同じ標準を共有する) |
| このフォルダの全画像から解除 | 現フォルダ/ZIP/PDF の全画像ページから個別設定を削除 (`clear_all_page_params`)。個別を解除するとお気に入り標準 or グローバルにフォールバック |
| このお気に入り「{name}」の標準にする | 現在のパラメータをそのお気に入りの標準として保存 (`set_favorite_default`)。お気に入り登録フォルダ配下にいないとき disabled |
| このお気に入り「{name}」の標準を解除 | そのお気に入りの標準を削除 (`clear_favorite_default`)。以後そのお気に入りはグローバルにフォールバック。未登録のとき disabled |
| 標準にする | 現在のパラメータを `settings.global_preset` にコピー (`copy_params_to_global`) |
| 個別設定を解除 [Q] | 現ページの個別レコードを削除 (`clear_page_params`)。フォールバックで「お気に入り標準 → グローバル」の順にマッチしたものが効く。フルスクリーン・グリッドとも `Q` / `Ctrl+Backspace`、グリッドはチェック済み (なければ選択 1 件) に一括適用可能 (`clear_page_params_for_selection`) |

ボタンラベル内の `{name}` は `ui_helpers::truncate_name(name, 10)` で 10 文字に切り詰める。
スコープ表示 (ヘッダー直下) も同様に 3 種類: `個別設定を適用中` / `お気に入り「{name}」の標準を適用中` / `標準設定を適用中`。

### 1.4 保存スロット

10 個の名前付きスロット。フルスクリーンで `Ctrl+0〜9` を押すと
`App::apply_slot_to_current_page(slot_idx)` が呼ばれ、該当スロットのパラメータを
**現在のページ個別設定として書き込む** (= そのページを個別化する)。

> 旧来は `Shift+0〜9` だったが、egui の logical-key 方式ではキーボード配列によって
> Shift+数字が記号 (`!"#$%&'()` など) に置き換わり `Key::Num1` 等にマッチしないため
> Ctrl 修飾に変更した (JIS 配列の Shift+0 は文字を生成しないため特に致命的だった)。

補正パネルの保存スロット欄 (`💾` ボタン) で現在のパラメータをスロットに保存できる。

### 1.5 見開き表示中の左右独立補正 + コピー

`adjustment.db` の `page_params` はもともと「画像 / ZIP エントリ / PDF ページ」単位で
独立しているため、見開き Double 表示中の左右ページは別々の `AdjustParams` を持てる。
補正パネルは見開き Double のときだけヘッダー直下に **L/R セレクタ + コピーボタン** を出し、
編集対象の切替と片側→もう片側への転写を 1 パネルで完結させる。

| 要素 | 仕様 |
| --- | --- |
| L/R セレクタ | 「左ページ」「右ページ」の 2 ボタン (selectable_label)。既定は **常に左ページ**。`open_fullscreen` (= ページ送り、ペア切替、初回フルスクリーン) と spread_mode 切替で `AdjustSpreadTarget::Left` にリセット。Single (単ページ / 表紙単独 / 横長単独) のときは表示しない |
| L/R 基準 | **画面上の左右で固定** (LTR/RTL 不変)。消しゴム `EraseSpreadCtx` の慣習と統一 |
| 編集対象解決 | パネル冒頭で `resolve_spread_pair(fs_root_idx)` を引き、Double のとき `adjust_spread_target` に応じて `target_idx = left or right`。以降のスライダー読み書き / 個別化 / Undo はすべて `target_idx` 経由 (= 単ページ経路と同一) |
| コピーボタン | 「← コピー」(右→左) と「コピー →」(左→右) の対称配置。`App::copy_spread_adjust(src, dst)` を呼ぶ |
| 同一判定 | `effective_params(left) == effective_params(right)` (実効値の `==`)。一致しているときは両コピーボタン disabled (= 「揃っている」状態が UI から自明) |

#### コピー操作の中身

`copy_spread_adjust(src, dst)` は `effective_params(src)` (3 層解決後) を `set_page_params(dst, ...)`
で書く。`set_page_params` 内部の `matches_default` 比較で、dst の標準 (お気に入り標準 or
グローバル) と一致するなら page_params エントリは作られず、カスケード解決に任せる挙動が
自動で得られる (= DB エントリが無駄に増えない)。書き換えは `capture_adjust_full` で囲んで
あるので Undo に乗る。AI 設定 (upscale_model / denoise_model) が変わるなら
`clear_all_adjustment_and_ai_caches(dst)` で AI キャッシュも落として再アップスケールを誘発する。

#### 見開き描画と補正適用の隠れた前提 (実装メモ)

これまで `adjustment_active = self.adjustment_mode && !is_spread_double` でパネル全体が
disabled だった頃は顕在化していなかった 2 点を併せて修正している:

- `draw_fs_spread_page` のテクスチャ取得を `adjustment_cache → fs_cache → thumbnail → holdover`
  の優先順に変更。これがないと見開きでスライダーを動かしても画面が変わらない (補正前
  fs_cache がそのまま描かれる)。
- `maybe_apply_adjustment` の早期 return ガードを「`fullscreen_idx` と一致 **または**
  resolve_spread_pair で見開きペアの片方」に緩和。さらに呼び出し元 (フレーム末尾の補正適用
  フェーズ) で右ページ idx についても追加で 1 回呼ぶ。これがないと見開きの右ページだけ
  補正適用が走らない。

#### 「両ページ同時編集」を入れない理由

設計初期は「両ページ」モードを検討したが、左右が異なる状態で「両ページ」を選んだ瞬間に
片方の補正値がもう片方で上書きされる破壊的操作が避けられず、デフォルト選択
(同一なら両ページ / 異なれば左) のロジックも暗黙的になりやすい。代わりに
「常に左ページから始まる + 揃えたいときはコピーボタン」という明示的 UI に絞った。
両ページ同値の運用要望は **お気に入り標準 (favorite_params) で代替できる** ため、
今回は意図的にスコープ外。

---

## 2. AdjustParams の中身

`adjustment.rs::AdjustParams`:

- `brightness`, `contrast`, `gamma`, `saturation` (色情報の定番)
- `temperature` (±100。色温度。±0 以外だと f32 パイプライン必須)
- `black_point`, `white_point`, `midtone` (トーンカーブのレベル補正)
- `auto_mode`: `None` / `Auto` / `MangaCleanup` (自動補正モード)
- AI 関連: `upscale_model`, `denoise_model` (`Option<String>`)
- **ポストフィルタ**: `post_filter: PostFilter` (レトロ系表示エフェクト、色調補正の後に適用)

### 2.1 適用順序

`adjustment.rs::apply_adjustments_fast` → `post_filter::apply`:

```
Levels (黒点/白点/中間調) → Gamma → Brightness/Contrast → Saturation → Temperature → ポストフィルタ
```

- `temperature == 0` なら u8→u8 LUT で高速処理
- `temperature != 0` なら f32 パイプライン (やや遅い)
- ポストフィルタは `PostFilter::None` 以外のときだけ追加適用

### 2.2 ポストフィルタ (PostFilter enum)

レトロ系 (CRT ブラウン管風・機種別減色・複合) と写真系 (カラーグレーディング・アナログ・絵画風・実用)
をまとめて扱う表示エフェクト。全 38 バリアント:

| グループ | バリアント | 内容 |
| --- | --- | --- |
| 基本 | `None` | フィルタなし (LINEAR サンプラー、デフォルト) |
| 基本 | `Nearest` | ピクセル補完なし (NEAREST サンプラーのみ、CPU 変換は clone) |
| CRT | `CrtSimple` | sin² スキャンライン + RGB アパーチャマスク + 微 glow |
| CRT | `CrtFull` | CrtSimple + 樽型歪み + 強 phosphor glow |
| CRT | `CrtArcade` | 太スキャンライン + 濃マスク + 高輝度 |
| 減色 (色数昇順) | `Dither1bit` | 2 階調 Bayer ディザ |
| 減色 | `GameBoy` | 緑系 4 階調 (固定) |
| 減色 | `Pc98` | PC-98 アナログモード (適応 16 色、`palette_gen::generate` で median cut) |
| 減色 | `GameGear` | ゲームギア (12bit → 32 色適応) |
| 減色 | `Famicom` | NES 固定 ~52 色ハードパレット |
| 減色 | `MegaDrive` | メガドライブ (9bit → 61 色適応、3bit/ch の階段階調) |
| 減色 | `Msx2Plus` | MSX2+ SCREEN 8 (256 色固定 GRB 3:3:2) |
| 減色 | `Sfc` | スーパーファミコン (15bit → 256 色適応) |
| 複合 | `ComboFamicomCrt` | Famicom 固定 + CRT Simple |
| 複合 | `ComboPc98Crt` | PC-98 適応 + CRT Simple |
| 複合 | `ComboMsx2PlusCrt` | MSX2+ 固定 + CRT Simple |
| 複合 | `ComboMegaDriveCrt` | メガドライブ適応 + CRT Simple |
| 複合 | `ComboSfcCrt` | スーパーファミコン適応 + CRT Simple |
| カラーグレーディング | `Sepia` | 古写真風の暖色モノクロ (Microsoft 係数マトリクス) |
| カラーグレーディング | `MonoNeutral` / `MonoCool` / `MonoWarm` | ITU-R BT.601 輝度モノクロ、冷/暖ティント版 |
| カラーグレーディング | `WarmTone` / `CoolTone` | 全体 +R/-B、-R/+B の温度補正 |
| カラーグレーディング | `TealOrange` | 影=青緑 / ハイライト=橙、シネマ調 + 彩度 +12% |
| カラーグレーディング | `KodakPortra` | 落ち着いた彩度 + 肌色 tweak + lift 5 のフィルム調 |
| カラーグレーディング | `FujiVelvia` | 青緑チャンネルゲイン + S 字 + 彩度 +35% |
| カラーグレーディング | `BleachBypass` | 彩度 -55% + S 字 0.55 の銀残しシネマ調 |
| カラーグレーディング | `CrossProcess` | 影=青緑 / ハイライト=黄、彩度 +25% |
| カラーグレーディング | `Vintage` | lift + 赤寄りシャドウ + 黄色ハイライト + 彩度 -20% |
| アナログ | `FilmGrain` | 暗部強めの粒状ノイズ (wang-hash 由来の決定論ノイズ) |
| アナログ | `Vignette` | 対角正規化距離^2 で周辺を最大 -45% 減光 |
| アナログ | `LightLeak` | 左上から対角減衰、暖色 (240,150,80) の Screen ブレンド |
| アナログ | `SoftFocus` | 明部抽出 + 分離可能ボックスブラー 11-tap + Screen blend |
| 絵画風 | `Halftone` | 6×6 セルの輝度平均 + 距離判定ドット、2 階調グレー |
| 絵画風 | `OilPaint` | 7×7 Kuwahara フィルタ (4 象限の輝度分散最小領域の平均色) |
| 絵画風 | `Sketch` | Sobel 3×3 + 強度反転グレー |
| 実用 | `Sharpen` | 5-tap 分離可能ブラーのアンシャープマスク (amount 0.6) |

**複合プリセットの方針**: **非液晶機種 (CRT TV / モニタ接続が標準)** とブラウン管フィルタを
セットにして、実機の視聴体験に近づける。GameBoy / ゲームギアは LCD なので CRT 合成は除外。

**写真系の alpha 保持**: 全フィルタが元ピクセルの alpha を `from_rgba_unmultiplied` で伝播する。
透過 PNG / WebP / GIF を通しても透過部分が不透明化しない。CRT 系は bilinear で alpha も補間
(`BilinearYCtx::sample_rgba`)、減色系・写真系は元ピクセルから単純継承。

### 減色の分類: 固定 vs 適応

- **固定パレット** (GameBoy/Famicom/MSX2+/Dither1bit): ハードウェア仕様で定義された色セットをそのまま使う。
  実機の挙動を再現するが、パレットに無い色味 (例: NES の肌色) は画像によって大きく変化する。
- **適応パレット** (PC-98/ゲームギア/メガドライブ/SFC): 実機では各ゲームが画像に合わせて色を選んで
  いた機種。median cut で画像から最適な N 色を抽出し、ハードウェアの bit 深度に
  `quantize_channel_bits` で丸める。`palette_gen::generate` がサンプル 50k 制限で median cut を行う。

### CRT 系の設計方針

**明るさ均等化**: 3 プリセットとも `brightness_boost = 1 / (scan_atten × mask_atten)` を
適用し、フィルタなしとほぼ同じ輝度になるよう補正。スキャンライン・マスクの輝度低下を
ブーストで相殺する (実機 CRT のビーム出力と同等の考え方)。CrtArcade のみ +10% で高輝度に。

**パターンの滑らかさ**: sin² falloff のスキャンライン + sin² 分布の RGB アパーチャ +
bilinear 補間ソースサンプリング + 水平ブラー (h_blur) で、「硬い矩形ピクセル」ではなく
アナログ CRT 特有の「柔らかく滲む phosphor ドット」感を出す。

**出力解像度**: 適応アップスケール (長辺 1024px 以下 → 4x / 2048px 以下 → 2x / 超 → 1x)、
出力長辺 4096px ハードキャップ。減色系はソース解像度を維持。

### サンプラー選択

- `PostFilter::Nearest` のみ `TextureOptions::NEAREST` でアップロード
- その他 (CRT/減色/複合) は `LINEAR` でアップロード。縮小時のモアレを防ぎ、CRT の phosphor
  感を出すため。NEAREST だと CRT 結果を画面スケールに合わせる際に周期的黒線が出る。

**並列化**: `rayon::par_chunks_mut` で行単位に並列処理。4K 画像でも 4080ms 程度。

### 2.3 ポストフィルタの一時バイパス

消しゴム / 隠蔽加工 / 分析モード中は `App::post_filter_bypassed: bool` が `true` になり、
`apply_sync_adjustment` が post-filter 段をスキップし color-only の `adjustment_cache` を生成する。
モード解除時に false に戻し、描画用 cache だけをクリアして post-filter 適用状態で
再生成させる。これは編集時の見やすさだけの切替なので、消しゴム確定結果の
`input_generation` は進めない。

- **消しゴム**: 減色プリセット (GameBoy 4 色など) が有効だと境界が潰れてマスクを精密に塗れないため
  ただし MI-GAN の preview / apply / auto-apply / ensure-result 入力は最終表示順序に合わせ、
  post-filter を含めた `色補正 → post-filter → 消しゴム` の画像を使う。
- **分析**: ヒストグラムは `fs_cache` の生ピクセルから計算されるため、表示だけを生に揃える

`AdjustParams` には:
- `is_identity()` = 色調 identity **かつ** `post_filter == None`
- `is_color_identity()` = 色調 identity のみ (バイパス中の早期 return 判定用)

### 2.4 元画像プレビュー

右 Ctrl を押している間だけ、補正 / ポストフィルタ / AI アップスケール・デノイズ /
消しゴム補完結果 / 隠蔽加工を表示選択から外し、元画像を一時表示する。これは「比較用の描画 override」
であり、`adjustment_cache` や `ai_upscale_cache` の無効化、DB 書き換え、AI ジョブの停止は
行わない。表示元は常に raw 専用の `fs_cache` で、消しゴム補完済みページでも
`erase_base_cache` は参照しない。

### 2.5 Auto モード

- **Auto**: ヒストグラムの 0.5/99.5 パーセンタイルでレベル補正
- **MangaCleanup**: 紙/インク検出 → グレースケール → S 字カーブ → γ=0.85 → コントラスト ≥15

---

## 3. キャッシュ構造 (フルスクリーン時)

| キャッシュ | 型 | 内容 |
| --- | --- | --- |
| `fs_cache` | `HashMap<idx, FsCacheEntry>` | 生デコード結果。Static / Animated / Failed |
| `adjustment_cache` | `HashMap<idx, FsCacheEntry>` | 補正適用済みテクスチャ |
| `ai_upscale_cache` | `HashMap<idx, FsCacheEntry>` | AI アップスケール/デノイズ適用済み |
| `erase_result_cache` | `HashMap<EraseResultKey, EraseResultCacheEntry>` | 消しゴム MI-GAN 確定結果。`idx + input_generation + erase_mask_generation` で識別 |
| `conceal_cache` | `HashMap<idx, ConcealCacheEntry>` | 隠蔽加工合成済みテクスチャ |

描画時 ([display-pipeline.md](display-pipeline.md) を参照) は:

```
conceal_cache > erase_result_cache > adjustment_cache > ai_upscale_cache > fs_cache
```

の優先順位で最も処理済みのテクスチャを選ぶ。
隠蔽加工のプレビュー合成は `current_conceal_source_pixels` で
`erase_result > adjustment_cache > ai_upscale_cache > fs_cache` を毎回解決する。
`conceal_base_cache` は隠蔽モード入場時ソースの退避で、編集中ページの現行ソースを
一時的に取得できない場合のフォールバックとしてだけ使う。AI アップスケール完了などで
編集中にソース解像度が変わった場合は、AI 完了 hook / 次回 compose 前 / モード終了保存前に
in-memory のビットマップと Shape をソース解像度へ一度だけスケールする。
ドラッグ中など中間座標が残る場合だけ compose 時の一時リスケールにフォールバックする。

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

### 3.3 AI 処理中の暫定補正 (pre-AI adjustment)

AI 処理には数秒〜数十秒かかるため、その間も `maybe_apply_adjustment` は **`fs_cache` を入力に
補正を同期適用して `adjustment_cache` を埋める**。これを省くと AI 完了までユーザーには
「補正が掛かっていない生の `fs_cache`」が表示され、AI 完了の瞬間に補正分 (特にモノクロ系
ポストフィルタなどコントラストが強いもの) が一気に乗って濃度が跳ねて見える。

- AI 完了時 (`poll_ai_upscale`): **無条件に `adjustment_cache.remove(idx)` して仮版を破棄**
  してから、表示中かつ bg 一致の場合のみ AI 結果に対して再度 `apply_sync_adjustment` を呼ぶ。
  表示中でないページで AI が完了したときは `adjustment_cache` を空にしておけば、次回来訪時に
  `maybe_apply_adjustment` が `ai_upscale_cache` から再生成する。
- 仮版は `fs_cache` の解像度で作られるため AI 結果と比べて低解像度。仮版がそのまま残ると
  表示優先順位 `adjustment > ai_upscale > fs` により AI 結果が覆い隠されてしまうので、
  上記の明示的な `remove` が必須。

---

## 4. キャッシュ無効化ルール (早見表)

**これを間違えると高確率でバグる**。変更する前に必ず以下を確認:

| 変更された内容 | `adjustment_cache` | `thumb_adjust_tex` | `ai_upscale_cache` | 実行中 AI ジョブ |
| --- | --- | --- | --- | --- |
| 色系パラメータ変更* (ページ個別) | 該当 idx のみクリア | 該当 idx のみクリア | 残す | 残す |
| **ポストフィルタ変更** (ページ個別) | **該当 idx のみクリア** (サンプラー切替のため再アップロードが必要) | 該当 idx のみクリア (実際は identity 差分なしで no-op、影響なし) | 残す | 残す |
| 消しゴム/隠蔽加工/分析モードの入出 (`post_filter_bypassed` 切替) | 該当 idx の描画用 cache のみクリア (`input_generation` は進めない) | 触らない (サムネは post-filter 非対象) | 残す | 残す |
| AI モデル変更 (ページ個別) | 全クリア | 該当 idx のみクリア | **全クリア** | **キャンセル** |
| 保存スロット読込 → 現ページに適用 | 全クリア | 該当 idx のみクリア | AI モデルが異なれば全クリア | 同上 |
| 「全画像に適用」 / 「全画像から削除」 | 全クリア | **全クリア** | AI 設定が変わる idx のみクリア + pending キャンセル | あり |
| 「標準にする」 (global_preset 更新) | 全クリア | **全クリア** (override 有無を判定するより単純) | global の AI 設定が変わった場合、継承ページ (override なし) の idx をまとめてクリア + pending キャンセル | あり |
| 「個別設定を解除」 (Ctrl+Backspace) | 該当 idx のみクリア | 該当 idx のみクリア | AI 設定が変わるなら該当 idx のみクリア + pending キャンセル | あり |
| スライダードラッグ中 | 毎フレーム適用 (fs のみ) | **抑制** (描画時 `adjusted_tex = None`) | 残す | 残す |
| スライダー release (true→false 遷移) | (変化なし) | **全クリア** → visible 優先で再生成 | — | — |
| フォルダ切替 | 全クリア | **全クリア** + `thumb_pixels` も全クリア | 全クリア | キャンセル |
| keep_range からの eviction | 該当 idx のみ evict | 該当 idx のみクリア + `thumb_pixels` も drop | 対象外 | — |
| 回転変更 | **クリアしない** (描画時の GPU 行列で回転) | **クリアしない** (同左) | クリアしない | — |
| 消しゴムマスク変更 | 残す | 触らない (`thumb_pixels` は元サムネソース、マスクで変わらない) | 残す | — |

*「色系」= brightness/contrast/gamma/saturation/temperature/levels/auto_mode
(ポストフィルタは AI 設定を変えないので `ai_settings_eq` には含まれず、色系変更と同じ扱い)

消しゴムマスク変更時は `erase_mask_generation[idx]` を進め、`erase_result_cache` と
`conceal_cache[idx]` を stale 化する。`fs_cache` / `ai_upscale_cache` /
`adjustment_cache` は下位入力として保持し、MI-GAN 結果だけを再生成する。

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

同じ考え方を bulk / global / favorite 系にも横展開している:
- `apply_params_to_all_pages(params)`: 書換前の各 idx の effective params と
  `params` を `ai_settings_eq` で比較し、一致しない idx だけ AI キャッシュを落とす。
- `clear_all_page_params()`: 個別削除後の effective params は `effective_default_for_idx`
  (お気に入り標準 or global_preset) になるため、書換前の effective params とその標準を
  比較して差がある idx だけ落とす。
- `copy_params_to_global(params)`: 旧 global と新 `params` を比較し、AI 設定が
  変わった場合のみ「個別 / お気に入り標準のどちらも持たない (= global 継承) 画像ページ」を
  対象に落とす。個別やお気に入り標準を持つページは effective params が変わらないので触らない。
- `set_favorite_default(fav_id, params)`: 旧そのお気に入り標準 (無ければ global) と新 `params`
  を比較し、AI 設定が変わった場合のみ「そのお気に入り傘下かつ個別を持たないページ」を対象に落とす。
- `clear_favorite_default(fav_id)`: 削除後は global にフォールバックするため、旧そのお気に入り
  標準と global を比較して差がある場合、同じ対象範囲で落とす。

---

## 5. 消しゴム (Erase) との関係

`ui_erase.rs` と `mask_db.rs` で実装された消しゴム機能は、補正パイプラインと連携している:

```
生画像 (fs_cache) ─▶ AI upscale (ai_upscale_cache) ─▶ 補正 (adjustment_cache)
                                                           │
                                                           ▼
                    mask_db (消しゴムマスク) ─▶ MI-GAN で inpaint
                                                           │
                                                           ▼
                                               erase_result_cache ─▶ conceal_cache ─▶ 画面
```

`fs_cache` は raw decode 専用で、消しゴム確定結果を書き戻さない。マスクが存在する画像は
表示時に `ensure_erase_result_texture` が現在の pre-erase 入力
(`adjustment_cache > ai_upscale_cache > fs_cache`) と保存マスクから inpaint を非同期起動し、
結果を `erase_result_cache` に載せる。入力またはマスクが変わると generation key が変わり、
古い MI-GAN 結果は採用されない。

### 透過画像の扱い (黒で不透明化)

MI-GAN は RGB 専用で alpha を扱えない。透過 PNG / WebP 等は透明部が premultiplied RGB=0
(= 黒) で格納されるため、そのまま渡すとモデルが透明部を「黒」として補完入力に使い、補完結果も
黒くなる。混乱を避けるため、消しゴム作業ベースは**黒で不透明化**して統一する:

- `App::black_flatten_if_transparent` (`ui_erase.rs`): 1 画素でも alpha<255 があれば、
  premultiplied RGB を保ったまま alpha=255 にした不透明コピーを返す (= 黒背景への合成と等価。
  全不透明なら `None` で no-op)。
- 適用点は 2 つ: `enter_erase_mode` の `erase_base_cache` 初回保存 (表示 =
  `ensure_erase_base_texture`) と、`resolve_erase_input_pixels` 系の返り値 (preview / apply /
  auto-apply / ensure-result の全 MI-GAN 入力)。これで**表示も入力も出力も黒不透明で揃う
  (WYSIWYG)**。
- AI 高解像度レイヤがあるページでは、消しゴム入場時の `erase_mask_size` も AI 側の
  解像度に合わせる。raw サイズのマスクで preview し、通常表示復帰後に高解像度へ
  リスケールして ensure-result すると、同じ見た目のマスクでも MI-GAN 入力が変わり、
  preview と確定結果が一致しなくなるため。
- AI アップスケール ON の透過画像では、B キーで白背景を選んでいても消しゴム用の
  composite-first cache は bg=0 (黒) を参照する。白背景に焼き込まれた `(idx,1)` や
  その派生 `adjustment_cache` を消しゴム入力へ流さないため。
- `fs_cache` の透明原本は無変更。マスクを 1 つも作らずに消しゴムを抜ければ通常表示は `fs_cache`
  に戻るので、**元の透明画像がそのまま保持される** (= 加工しなければ破壊しない)。
- 結果として MI-GAN と diffusion フォールバックの出力 alpha も一致する (P3-8 の非一貫も解消)。

### 5.1 マスクのデータ構造 (ビットマップ + ベクタ)

マスクは 2 つのレイヤで構成される:

- **ビットマップ** (`Vec<bool>`): 筆 (Brush) / 囲み (Lasso) でのストロークが直接ラスタライズされる。
  mask_db の `mask_data` 列 (1bit/pixel + deflate 圧縮) に保存。
- **ベクタ** (`Vec<Shape>`): 直線 / 縦線 / 横線 / 矩形 / 楕円はオブジェクトとして保存される。
  各 Shape は `op = add | subtract` を持つ。描画モードで作った Shape は `add`、
  消去モードで作った Shape は `subtract` として保存される。mask_db の `vectors` 列
  (JSON) に保存。

MI-GAN / diffusion に渡す最終マスクと、オーバーレイ描画に使うマスクは、
**毎回ビットマップとベクタを合成した結果 (`composite_mask`)**。
合成順序は「ビットマップ下地 → ベクタ Shape を作成順に適用」で固定。
`add` Shape はマスクを足し、`subtract` Shape はそれ以前のビットマップ/Shape 結果を
削り取る。消去モードで矩形や楕円を重ねても、既存 Shape 自体は削除されない。
ビットマップが空でも、ベクタが残っていればエントリは保存されたままになる
(`mask_db.set` の削除判定は「ビットマップ全 false **かつ** ベクタ空」)。

サイドカー (`mimageviewer.dat`) の `SidecarMask` も `vectors` フィールド (JSON 配列)
を持ち、中央 DB と同じ形式でミラーされる。

### 5.2 ベクタオブジェクトの編集

**選択ツール (`S` キー、または左パネル「選択」ボタン)** に切り替えてベクタ本体をクリックすると選択状態になり:

- 未選択のベクタも編集用アウトラインで表示する。`add` は橙、`subtract` は水色の枠で示す
  (合成マスク上では `subtract` が透明になるため、存在確認用の補助表示として別レイヤーに描く)
- ドラッグ: 平行移動 (Pan)
- 直線の両端: 端点ドラッグで始点/終点の位置変更
- 直線の中点付近の菱形ハンドル: 太さ変更
- 矩形/楕円の角・辺ハンドル: サイズ変更、回転ハンドル: 回転
- `Shift`+ハンドル: 端点角度 / 回転角スナップ、矩形/楕円の角リサイズを等比化
- `Alt`+ハンドル: 矩形/楕円を中心固定でリサイズ
- `Del`: 選択オブジェクトを削除
- `Esc`: 選択中は選択解除、未選択なら消しゴムは補完して終了、隠蔽加工は DB 保存して終了
- 他ツールに切り替えると自動で選択解除。ただし描画直後に `S` で選択ツールへ入る場合は、
  自動選択された Shape をそのまま保持してハンドル編集に移れる

描画系ツールでは任意のベクタ選択は行わない。ただし直近に描いた自動選択 Shape の
ハンドル上をクリックした場合だけ、そのツールのまま微調整できる。

消しゴム / 隠蔽加工の描画ツールでは、Ctrl+ドラッグやホイールで筆サイズ・
線幅・縦横線の傾きを変えない。画像上のホイールは修飾キーなしでも `Ctrl+ホイール`
でも表示ズームに使う。パネル上の修飾なしホイールはパネルスクロールに残す。
筆/線のサイズはパネルスライダー、作成後の形状調整は選択ツールのハンドルで行う。

### 5.3 マスク全体の平行移動・回転

選択が無い状態で矢印キー / `[` / `]` を押すと、**ビットマップとベクタすべてをまとめて**変換する。
偶数ページのスキャンゴミ補正で、位置/角度を微調整してから F7/F8 で再適用する用途。

| キー | 動作 |
| --- | --- |
| 矢印 | 1 px 平行移動 |
| Ctrl+矢印 | 10 px 平行移動 |
| `[` / `]` | ±0.1° 回転 (中心回り) |
| Ctrl+`[` / `]` | ±1° 回転 |

倍率キーは平行移動・回転とも **Ctrl** に統一している。`[` / `]` は Shift+ が
論理キー `{` / `}` に化けて `Key::OpenBracket` / `CloseBracket` にマッチしない
(JIS・US 共通) ため Ctrl にせざるを得ず、矢印キーの方も揃えた。

ビットマップは nearest-neighbor で回転するため、累積回転で若干劣化する (割り切り)。
ベクタは端点を厳密に回転させるので劣化しない。画像外にシフトした部分はクリップ。

### 5.4 マスクスロットと F7〜F10 クイック適用

スロット 1/2 に「現在のマスク (ビットマップ + ベクタ)」を保存できる
(`__slot_1` / `__slot_2` のキーで mask_db に格納)。

- 消しゴムモード内の「ロード」ボタン: 現在のマスク/ベクタを**スロットの内容で差し替える** (上書き)。
  直前の状態は Ctrl+Z で戻せる。OR マージにすると偶数/奇数ページを取り違えたときに
  旧マスクが残って過剰マスクになるため、差し替え仕様にしている (2026-04 仕様)。
- **フルスクリーン表示中の F7/F8**: `apply_slot_in_viewing_mode` — 消しゴムスロットの内容で現ページの
  マスクを**差し替えて** DB へ保存し、MI-GAN で inpaint → `erase_result_cache` に格納する。
  `fs_cache` は raw のまま維持し、generation key で古い inpaint 結果を stale 化する。
  消しゴムモードに入らずワンキーで適用できる経路。既存マスクがあっても上書きなので、
  別スロットを再 F7/F8 するだけで直せる。
- **グリッド画面での F7/F8**: `apply_slot_to_selection` — チェック中のサムネイル (なければ選択
  中 1 枚) にスロットを一括で配布。inpaint はその場で走らせず、**各ページを次回フルスクリーンで
  開いたときに `auto_apply_saved_mask` で自動適用**される。DB への書き込みはスロットの元サイズ
  のまま行い、読み出し側で `get_full` が画像サイズに合わせてリスケールする。
- **フルスクリーン表示中の F9/F10**: `apply_conceal_slot_in_viewing_mode` — 隠蔽スロットの内容で
  現ページの隠蔽マスクを差し替えて DB へ保存し、`conceal_cache` を破棄する。隠蔽加工は
  表示パイプラインで合成されるため worker は起動しない。
- **グリッド画面での F9/F10**: `apply_conceal_slot_to_selection` — チェック中のサムネイル
  (なければ選択中 1 枚) に隠蔽スロットを一括で配布する。
- **Shift+F7/F8 / Shift+F9/F10**: 対象ページから適用済みの消しゴム / 隠蔽マスクを削除する。
  スロットそのものは削除しない。

### 5.5 マスク保有ページのバッジ

サムネイル左上に、ページ個別補正の青「補」バッジと並んで、消しゴムマスクがあるページには
オレンジ系の「消」バッジが表示される。

判定用の `App::mask_pages: HashSet<usize>` をフォルダロード時に `mask_db::load_mask_keys` で
一括取得する (スロットキー `__slot_*` は除外)。`save_mask_with_sidecar` /
`delete_mask_with_sidecar` / `apply_slot_to_selection` の書き込み経路でこの集合も
同時に更新する。

---

## 6. 新しい補正項目を追加する時

チェックリスト:

- [ ] `AdjustParams` に新フィールド追加 (デフォルト値は「効果なし」)
- [ ] `apply_adjustments_fast` の適用順序を決めてその中に挟む
- [ ] LUT で済むか、f32 パイプラインが必要かを判断 (必要なら `temperature != 0` 判定に合流)
- [ ] UI パネル (`ui_adjustment_panel.rs::draw_sliders`) にスライダー追加
- [ ] 変更検出は `draw_sliders` が `(changed, dragging)` を返すので追加対応不要
- [ ] `AdjustParams` の JSON シリアライズ互換性を確認 (`#[serde(default)]` 等)
- [ ] **サムネイルにも色調として乗る**: `apply_adjustments_fast` に相乗りするため、
  色系であれば `thumb_adjust_tex` 経由で自動適用される。`is_color_identity()` の
  判定に参加するよう、新フィールドのデフォルトは「効果なし」にしておくこと
  (そうでないとサムネが常時再生成される)。

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

## 8.X 画像補正の Undo / Redo (v0.8.1)

`Ctrl+Z` / `Ctrl+Y` (`Ctrl+Shift+Z`) でフルスクリーン中の画像補正操作を取り消せる。
履歴は [`crate::undo_stack::UndoStack`](../src/undo_stack.rs) に積まれ、
レーティング・タグの Undo と同じスタックを共有する (型は `UndoEntry::Adjustment`)。

### 取り消し対象

- 左パネルの全スライダー / ラジオ / コンボボックス / リセット↩ボタン (= `set_page_params` 経由のページ個別更新)
- アクションボタン: 「お気に入り標準にする / 解除」「標準にする」「個別設定を解除」
  (= `set_favorite_default` / `clear_favorite_default` / `copy_params_to_global` /
  `clear_page_params` 経由)
- ホットキー: U / Shift+U / Alt+U (アップスケール循環)、N (デノイズトグル)、
  T / Shift+T / Alt+T (ポストフィルタ循環)、Q / Ctrl+Backspace (個別解除)
- 保存スロット適用: Ctrl+1〜9 / Ctrl+0、左パネルのスロットボタン

### スコープ表現 ([`AdjustUndoScope`](../src/undo_stack.rs))

```rust
enum AdjustUndoScope {
    Page(usize),       // adjustment_page_params[idx]
    Favorite(Uuid),    // adjustment_favorite_params[uuid]
    Global,            // settings.global_preset
}

struct AdjustmentChange {
    scope: AdjustUndoScope,
    before: Option<AdjustParams>,  // None = エントリ無し (Page / Favorite のみ)
    after: Option<AdjustParams>,
}
```

`Global` スコープは常に `Some` (`settings.global_preset` は Optional ではない)。
`Page` / `Favorite` の `None` は「そのスコープにエントリが存在しない =
下層 (Favorite default / Global) にフォールバック」を表す。

### 適用ロジック (`apply_adjustment_change_to_app`)

スコープごとに既存の書き込み API を再利用するだけ — `set_page_params` /
`clear_page_params` / `set_favorite_default` / `clear_favorite_default` /
`copy_params_to_global`。これらは DB 更新・サイドカー更新・キャッシュ無効化を
すべて内部で行うので、Undo 用に副作用を再実装する必要はない。

### スライダードラッグの取り扱い (drag-release granularity)

旧実装はスライダードラッグ中に毎フレーム `set_page_params` を呼んでいたため、
60 frames/sec の DB UPSERT + サイドカー XMP 書き込み + キャッシュクリアが発生して
いた。Undo 機能と一緒に以下のように改修:

- `App::adjustment_drag_session: Option<AdjustmentDragSession>` を追加
- 状態遷移:
  - drag 開始フレーム (`prev_dragging=false && curr_dragging=true`):
    `before = adjustment_page_params.get(&fs_idx).cloned()` をスナップショットして
    session を立てる。
  - drag 中 (`is_dragging=true`): `adjustment_page_params[fs_idx] = edit_params`
    だけ更新 (DB / サイドカー書き込みなし)。色調キャッシュは毎フレームクリアして
    リアルタイムプレビューは維持。
  - drag 終了フレーム (`prev_dragging=true && curr_dragging=false`): session を
    `take()` し、`set_page_params` を **1 回だけ** 呼んで永続化 + Undo エントリを 1 件
    プッシュ。
- 非ドラッグ変更 (ラジオ / コンボ / リセット↩ボタン) は drag セッション無視で
  即時 `set_page_params` + Undo プッシュ (1 操作 = 1 エントリ)。

これにより、CRT エミュレーションのような細かい調整も「1 つ前に戻す」が直感的に
効く + DB 負荷も大幅減 (60 writes/sec → 1 write/release)。

### 履歴クリア境界

`App::clear_meta_undo` で undo / redo 両方を破棄。呼び出し箇所:

- `load_folder` (フォルダ移動)
- `open_fullscreen` (グリッド → フルスクリーン、フルスクリーン中の画像移動)
- `close_fullscreen` (フルスクリーン → グリッド)
- `enter_erase_mode` / `reset_erase_mode` (消しゴムモード遷移)

消しゴムモード中は `erase_undo_stack` (バイトマップスナップショット) が
`Ctrl+Z` を担当するため、行き来の境界で履歴をクリアして文脈を分離する。

`clear_meta_undo` は `adjustment_drag_session` も `None` にリセットするので、
ドラッグ中に境界を跨いでも進行中セッションが残骸として残らない。

### 副作用に乗る点

- `set_page_params` の **「ページ個別が effective_default と一致するなら個別を削除」**
  正規化は Undo 経路でも有効。`before` に `Some(p)` で記録された値を再適用しても、
  もし `p` が現在の effective_default と一致するなら個別エントリは作られず削除される。
  この振る舞いは normal 操作と同じなので一貫性がある。
- `set_favorite_default` / `clear_favorite_default` は内部で「冗長になった個別を削除」
  処理を行う (Codex P2 対応)。Undo で巻き戻したときも同じロジックが走るので、
  状態は always 正しい不変条件を保つ。

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
