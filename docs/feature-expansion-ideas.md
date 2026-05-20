# mImageViewer 機能拡張仕様 (v0.10 候補)

Codex / Claude Code の提案と筆者意見、Codex 第 1〜2 ラウンドレビューを統合した実装仕様書。
このドキュメントをベースに Claude Code レビューを受け、問題がなければ各機能ごとに
実装へ入る。

## 採用する機能 (3 件 + 関連改善 1 件)

1. **キャプチャ保存** — 静止画 / 動画フレーム / ZIP 内画像 / PDF ページを統一フローで保存
2. **比較ビュー** — 静止画 / ZIP / PDF を X キーでピン留め、C / Shift+C / Alt+C で切替・ワイプ・差分
3. **動画連続再生** — ループとは独立した別ボタンで 3 モード (オフ / 連続 / 連続+ループ)
4. **スクロールバー視認性向上** (関連改善) — 既存スタイルを少しだけ目立つように調整

## 見送る機能 (記録)

以下は本ラウンドでは見送り、必要になれば改めて議論する。

- **既読位置の自動記憶** — エクスプローラ慣例 (下層に降りるときは位置を保存しない)、PDF の表紙重視
  (常に 1 ページ目から開くのが直感的)、Ctrl+↑↓ ナビゲーションとの一貫性、いずれもユーザー期待と
  衝突する。v0.8.2 で BS のカーソル復元、v0.9.0 でフルスクリーン閉じ時のカーソル復元は既に実装
  済みで、セッション内の再開ニーズはカバー済。残るのは「アプリ再起動を跨ぐ復元」だが mIV の
  起動時間は v0.8.1 で 2.5 秒まで短縮済で、価値が薄い
- バッチ書き出し (汎用版) — 再配布権を持たないユーザーが主体なので価値が薄い
- スマートコレクション — フォルダ整理運用で覆える
- 色管理 / HDR — 大改修が必要、HDR モニタも手元にない
- 漫画の縦スクロール連続表示 — 合法的なテスト素材の確保が現実的でない
- 「次の話」自動移動 — Ctrl+↓ 案内で十分
- A-B リピート (揮発) — ブックマーク 2 つで代替可能
- ドラッグ送出 / 画像データのペースト・ブラウザ画像ドロップ — 工数大、別議論として残す
  (ファイルパス経由の D&D / Ctrl+V は既存実装あり)
- 動画字幕 / 多重音声トラック — 別議論済、合法的テスト素材の確保が困難

---

## 共通方針

- 保存処理・エンコード・I/O は **必ず worker thread**。UI スレッドは進捗・キャンセル UI だけ
- 設定追加は `settings.db` 経由 (v0.9.0 で SQLite 永続化済)
- `Settings` の単純フィールド追加は `settings_kv` に JSON 値として保存されるため、DB schema 変更は不要。
  `serde(default)` を付ければ既存 `settings.db` では既定値で復元される。新しい専用テーブルや
  `COMPLEX_FIELDS` への追加を行う場合だけ、起動時の schema 初期化 / migration 方針を別途設計する。
- v0.9.0 で導入したトースト通知 UI を流用
- 既存の `display_pipeline.md` の合成順を尊重

---

## 1. キャプチャ保存

### 1.1 目的

フルスクリーン表示中に <kbd>Ctrl</kbd>+<kbd>S</kbd> で現在表示中の画像 / 動画フレーム /
ZIP 内画像 / PDF ページを、事前設定したフォルダにファイル形式の差なく保存する。
ダイアログを開かない一発保存 + トースト通知方式 (mpv 流) を取る。

「保存できるもの・できないもの」が出ると体験が崩れるので、4 種すべてを最初から対応する。

### 1.2 UI / 操作

- フルスクリーン中 <kbd>Ctrl</kbd>+<kbd>S</kbd>: 現在表示中のアイテムをキャプチャしてファイル保存
  - 静止画 / 動画 / ZIP 内画像 / PDF ページの全フルスクリーンで有効
- **上部ホバーバー右上にカメラアイコン** を配置 (動画 HUD と静止画フルスクリーンで統一):
  - 動画 HUD の既存カメラアイコン (クリップボードコピー) を静止画 / ZIP 内画像 / PDF ページにも展開
  - **クリック**: 現在表示中をクリップボードにコピー (動画の既存挙動と同じ)
  - **ホバー時のツールチップ**: 2 行構成
    - 「クリック: クリップボードにコピー」
    - 「<kbd>Ctrl</kbd>+<kbd>S</kbd>: ファイル保存」
  - このツールチップが Ctrl+S 機能の発見導線にもなる (隠れキー化を避ける)
- ファイルメニュー > **キャプチャ保存フォルダを開く** : エクスプローラで保存先フォルダを開く
  (ユーザーが保存物にアクセスする導線)
- 保存成功時: 画面右下にトースト「`movie_0042.png` を保存しました」を 3 秒表示
  - トーストクリックでエクスプローラを開いて該当ファイルを選択状態にする
- 保存失敗時: 赤色トーストで原因を表示 (権限・容量・パスなし)
- クリップボードコピー開始時: トースト「キャプチャをクリップボードへコピー中」を表示

### 1.3 設定

環境設定 > **キャプチャ** (新設タブ) :

| 項目 | 既定値 | 選択肢 |
|---|---|---|
| 保存先フォルダ | Windows Known Folder の Pictures 配下 `mimageviewer\` | 任意のフォルダ |
| キャプチャ形式 | PNG (可逆) | PNG (可逆) / JPEG 品質 95 / JPEG 品質 85 / JPEG 品質 75 |

- 保存先フォルダが存在しないときは初回保存時に `fs::create_dir_all` で自動作成
- 既定保存先は `SHGetKnownFolderPath(FOLDERID_Pictures)` で取得し、失敗時だけ
  `%USERPROFILE%\Pictures\mimageviewer\`、さらに失敗時はアプリ data dir 配下 `captures\` に
  フォールバックする
- 形式に WebP / AVIF は含めない (シンプル化、要望が出たら追加)

### 1.4 キャプチャ対象と内容 (MVP 方針)

Codex 第 1 ラウンドレビュー指摘: 「display pipeline の最終 RGBA」は実装難度が過小評価されている。
worker thread で GPU shader 後の見た目を RGBA 化するのは、補正・AI 経路をそのまま流用できる
領域と、表示時のジオメトリ変換で適用される領域とで難度が大きく異なる。

第 2 ラウンドレビューで判明: **ポストフィルタは表示時 GPU シェーダではなく、CPU 側で
`post_filter::apply` されて `adjustment_cache` にピクセルとして保持される設計**。
したがって通常表示中の静止画は「画面に出している補正済みピクセル」をそのまま保存に流用でき、
MVP で十分対応可能。一方、回転・ズーム・パンは描画時の座標変換なので、保存に反映するには
別レンダリングが必要で工数が大きい。

#### MVP に含める処理

| 処理 | MVP で反映 | 経路 |
|---|---|---|
| 補正 (明るさ / コントラスト / ガンマ / 彩度 / 色温度 / レベル / 漫画クリーンアップ) | ✅ | `adjustment_cache` のピクセル |
| AI アップスケール | ✅ | `ai_upscale_cache` のピクセル |
| AI ノイズ除去 | ✅ | 同上 |
| 消しゴム (MI-GAN マスク) | ✅ | 既存マスク適用済み cache pixels |
| **ポストフィルタ (38 プリセット)** | ✅ | `adjustment_cache` のピクセル (CRT / セピア / モノクロ / シャープ等込み) |

#### 対象外 (実装しない)

| 処理 | 対象外理由 |
|---|---|
| 非破壊回転 | 描画時のジオメトリ変換。「焼き付け保存」と意味重複するので別機能扱い |
| ズーム / パン | 表示時のジオメトリ。ファイル保存の意味として除外が自然 |
| ルーペ表示 | 同上 |
| 画像分析モード (Z キー) のオーバーレイ | 解析表示なので保存対象外 |
| HUD / UI 要素 | 当然対象外 |

第 2 段階の追加候補にもこれらは入れない方針 (回転は専用機能、ズーム/パンはユーザー期待と
合わない)。

#### 対象別の動作

| ソース | キャプチャ内容 |
|---|---|
| 通常画像 (JPEG/PNG/RAW/HEIC など) | 元画像座標で「補正 + AI + 消しゴム + ポストフィルタ」を適用した RGBA |
| ZIP 内画像 | 同上 |
| PDF ページ | PDFium レンダリング結果 (fs_cache のページ画像) + 補正 + ポストフィルタ。UI 上のズーム / パンは反映しない |
| アニメーション画像 (GIF / APNG) | 現在表示中フレームの RGBA。MVP では補正 + AI + ポストフィルタの対象外 |
| 動画フレーム | 既存 `video::screenshot::capture_frame` 経由。**動画にはポストフィルタが適用されない仕様のため、capture_frame の出力 (生フレーム) をそのまま保存**。動画 HUD / 字幕等の overlay も当然含まない |
| 見開き表示中 | 左右両ページを元画像座標のまま 1 枚に結合 (右綴じ / 左綴じ設定に従う)。各ページに補正・ポストフィルタが反映される |

PNG 保存ではアルファを保持する。JPEG 保存ではアルファを保持できないため、フルスクリーンの
透過背景モード (黒 / 白 / 市松) に合わせて合成してから保存する。

#### CRT 系ポストフィルタの解像度変化に関する注意

CRT 系ポストフィルタ (ブラウン管エミュ、機種別減色など) は小さい画像を 2 倍 / 4 倍に拡大して
走査線やシャドウマスクを描画する実装になっており、**保存画像の解像度が元画像と一致しない場合
がある**。これは「ポストフィルタを作品として保存する」用途では自然な挙動だが、ユーザー期待と
ずれる可能性があるので:

- マニュアル / 設定タブに「CRT 系ポストフィルタを適用した状態で保存すると、画像サイズが元画像
  と異なる場合があります」と明示
- ファイル名は元の解像度に依存しないため衝突問題なし

#### クリップボードコピー (カメラアイコン) との関係

- **Ctrl+S とクリップボードコピーは同じ出力に揃える** (Codex 推奨)
- 両者とも下記 §1.7 の共通ジョブ `prepare_capture_pixel_job(idx)` を経由
- 動画は既存 `capture_frame` ベース (ポストフィルタなし)
- 静止画 / ZIP / PDF は補正 + AI + 消しゴム + ポストフィルタが反映される

### 1.5 ファイル命名規則

基本形: `{source_basename}_{seq:04d}.{ext}`

| ソース | source_basename 例 |
|---|---|
| 通常画像 `IMG_1234.jpg` | `IMG_1234` |
| ZIP `manga01.zip` 内 `page042.jpg` | `manga01_page042` |
| PDF `document.pdf` の 42 ページ | `document_p0042` |
| 見開き `page042.jpg` + `page043.jpg` | `page042_page043` |
| 動画 `movie.mp4` のフレーム | `movie` (時刻は seq で表現) |

- 連番 (`_{seq:04d}`) は同じ basename のファイルが既に存在する場合に増やす
- 例: `IMG_1234_0001.png` が既にあれば次は `IMG_1234_0002.png`
- 連番枯渇 (10000 件) は当面考慮外 (実害無し)

### 1.6 メタデータの扱い (MVP 方針)

- **初期実装ではメタデータを引き継がない** (保存ファイルは display pipeline 結果のみ)
- 元画像の EXIF / XMP / AI プロンプトは保存ファイルに含めない
- 理由: メタデータ引き継ぎは形式間変換 (HEIC→JPG など) で複雑度が増す
- 将来要望が出たら段階追加 (PNG への AI プロンプト書き戻しなど)

### 1.7 実装方針

#### 共通ピクセル取得関数 (Codex 推奨)

静止画系で Ctrl+S 保存とクリップボードコピーの両方が同じ出力を得るため、
UI スレッドでは「保存に必要な軽量 snapshot を作るだけ」にし、重い変換・エンコード・I/O は
worker thread で実行する。

```rust
struct CapturePixelJob {
    idx: usize,
    basename: String,
    source: Arc<egui::ColorImage>,
    source_already_adjusted: bool,
    params: AdjustParams,
}

fn prepare_capture_pixel_job(&self, idx: usize) -> Result<CapturePixelJob, CaptureError> {
    // 1. adjustment_cache があり、かつ post_filter_bypassed でないならそれを使う。
    //    通常の全画面表示中はここでヒットし、worker は RgbaImage 変換だけで済む。
    if !self.post_filter_bypassed {
        if let Some(FsCacheEntry::Static { pixels, .. }) = self.adjustment_cache.get(&idx) {
            return Ok(CapturePixelJob::already_adjusted(idx, pixels.clone()));
        }
    }

    // 2. なければ ai_upscale_cache または fs_cache の CPU pixels と effective params を clone する。
    //    worker 側で apply_adjustments_fast() + post_filter::apply() を実行する。
    let source = self.capture_source_pixels(idx)?;
    Ok(CapturePixelJob::needs_adjustment(idx, source, self.effective_params(idx).clone()))
}
```

これにより以下が成立する:
- 通常表示中の Ctrl+S / クリップボードコピー: `adjustment_cache` ヒットで即時 snapshot (高速)
- 表示が間に合っていないタイミング: worker 側で保存用に補正・ポストフィルタを実行
- `post_filter_bypassed` 中 (消しゴム編集中 / 分析モード中) は bypass 済みの `adjustment_cache` を
  最終出力として信用せず、元ソース + `effective_params` から保存用に再生成する
- Ctrl+S とクリップボードコピーで出力が完全に一致

消しゴムについては「確定済みの inpaint 結果」を保存対象とする。編集中のブラシ / ラッソ /
分析オーバーレイのような一時 UI 表示は保存しない。

#### 表示モード別の経路

| モード | 取得関数 |
|---|---|
| 静止画フルスクリーン / ZIP / PDF | `prepare_capture_pixel_job(current_idx)` → capture worker |
| アニメーション画像 | `FsCacheEntry::Animated.frame_pixels[current_frame]` → capture worker |
| 動画フレーム | 既存 `video::screenshot::capture_frame` (one-shot worker)。本機能では拡張しない |
| 見開き表示中 | 左右ページそれぞれ `prepare_capture_pixel_job()` し、配置設定に従って worker で結合 |

#### エンコード・書き込み

- エンコード: PNG は `image` クレート、JPEG は `turbojpeg`
- JPEG は透過画像を `fs_transparent_bg_mode` に合わせた matte へ合成してからエンコードする
- 書き込みは worker thread (UI スレッドはキャッシュ参照・ジョブ作成のみ)
- ファイル名衝突回避は worker 側で `create_new(true)` を使い、同名チェックと作成の race を避ける
- トースト表示は既存 UI を流用

#### Ctrl+S 競合解消 (Codex 第 1 ラウンドレビュー対応)

- グリッドの <kbd>Ctrl</kbd>+<kbd>S</kbd> は既存「お気に入り名前検索」(`app.rs:520`, `4914`)。**変更しない**
- フルスクリーン中だけキャプチャに割り当てる:
  - `ui_fullscreen.rs` の root key relay の probe set に `Ctrl+S` を追加 (現状未登録)
  - フォーカスがメインビューポート側に行くケースの拾い漏れを防ぐ
  - native 動画 HWND 経路 (`app/native_video.rs::handle_native_video_key_event`) でも plain ↑↓
    と同じく `Ctrl+S` を直接拾う
- `keymap-spec.md` の「画像 フルスクリーン」「動画 フルスクリーン」両セクションに <kbd>Ctrl</kbd>+<kbd>S</kbd> = キャプチャ保存 を追記
- 既存テスト `app::tests::ctrl_s_closes_ctrl_f` 等は「グリッド時のみ」が前提なので破壊しない
  (フルスクリーン中の Ctrl+S はキャプチャに直行するため、検索バーへは流れない)

### 1.8 エッジケース

| ケース | 動作 |
|---|---|
| 保存先未設定 | 既定の `Pictures\mimageviewer\` に保存。必要なら環境設定で変更 |
| 保存先フォルダなし | `fs::create_dir_all` で作成、失敗ならエラートースト |
| 書き込み権限なし | エラートースト「保存できませんでした: アクセス拒否」 |
| ディスク容量不足 | エラートースト「ディスク容量不足」 |
| アニメーション画像の Ctrl+S | 現在表示中フレームを保存 |
| 動画再生中の Ctrl+S | 再生は止めない、その瞬間のフレームを保存 |
| AI アップスケール途中の Ctrl+S | AI 完了待ちはしない。現在表示中の暫定結果 (通常は fs_cache + 補正) を保存。AI 後が必要なら完了後に再保存 |
| 検索バー入力中の Ctrl+S | グリッドモード扱い (検索バー側を優先、キャプチャ発火しない) |

### 1.9 残課題 / レビュー観点

- [ ] 保存先フォルダのデフォルト位置 (現案: `Pictures\mimageviewer\`、取得不能時だけ data dir 配下)
- [ ] 動画再生中の Ctrl+S で再生を一時停止すべきか (現案: 止めない)
- [ ] 見開き表示中に片ページだけ保存する導線が必要か (現状: 左右結合 1 枚)
- [ ] グリッドモードからも Ctrl+S を受けるか (現案: フルスクリーンのみ。グリッドは選択複数になりがちで意味が変わる)
- [ ] AI アップスケール後の巨大画像 (例 4K → 16K) の保存は数秒かかり、PNG/JPEG 用に追加バッファも持つ。進捗 UI やメモリ上限ガードが必要か
- [x] CRT 系ポストフィルタ適用時に保存画像サイズが元と異なる挙動は、マニュアル / 設定タブ説明文で案内。保存時の都度警告は出さない
- [ ] `prepare_capture_pixel_job` のフォールバック経路 (キャッシュミス時) で `apply_adjustments_fast + post_filter::apply` を実行する際の worker 実行時間。巨大画像では進捗 UI が必要か
- [ ] カメラアイコンの Shift+クリックでファイル保存も拾うか (現状: クリック=クリップボード、Ctrl+S=ファイル保存と完全分離)
- [ ] 静止画フルスクリーンで上部ホバーバーが非表示の瞬間 (ルーペ表示中・分析モード中など) もアイコンを出すか (現案: ホバーバーと連動して出し入れ)

### 1.10 実装状況 (2026-05-19)

- 完了: 動画フレーム保存 MVP
  - `video::screenshot::capture_frame()` の RGBA を `capture.rs` の非同期保存 worker に渡す
  - Ctrl+S は egui fullscreen / native HWND の両経路で動画フレーム保存を発火
  - 保存先は既定 `Pictures/mimageviewer`、形式は PNG / JPEG 95 / 85 / 75 を環境設定から選択
  - filename は `{source_basename}_{seq:04d}.{ext}`、`create_new(true)` で衝突 race を避ける
  - 動画 HUD 既存カメラアイコンのツールチップに `Ctrl+S: ファイル保存` を追加
- 完了: 静止画 / ZIP / PDF / アニメーション現在フレーム保存 MVP
  - `prepare_capture_pixel_job()` で `adjustment_cache` → `ai_upscale_cache` → `fs_cache` の順に
    CPU pixels を snapshot し、補正・ポストフィルタ・エンコード・I/O は worker thread で実行
  - フルスクリーンの Ctrl+S で通常画像 / ZIP 内画像 / PDF ページ / アニメーション現在フレームを保存
  - JPEG 保存時は透過背景モードに合わせて黒 / 白 / 市松へ合成
- 完了: 静止画カメラアイコン + クリップボード経路統合
  - 静止画フルスクリーン上部ホバーバーにカメラアイコンを追加
  - クリック時は `prepare_capture_pixel_work()` 経由で Ctrl+S と同じ snapshot をクリップボードへコピー
- 完了: 見開き結合保存
  - `CapturePixelWork::Spread` で左右ページを worker 側で RGBA 結合してから保存 / clipboard へ渡す
- 完了: 保存先を開く導線
  - ファイルメニューと環境設定 > キャプチャから保存先フォルダを開ける
- 完了: 保存完了トーストから該当ファイルを表示
  - 保存成功トーストをクリックすると Windows Explorer で保存ファイルを選択状態にする
- 未着手: 保存先フォルダを開く処理の失敗を UI トーストで返す導線、巨大画像保存時の進捗 / メモリ上限ガード

---

## 2. 比較ビュー

### 2.1 目的と対象

ある画像をピン留めして「比較スロット」に置き、別の画像と素早く見比べる。
AI 処理前後、生成画像のパラメータ違い、補正差分、連写の選別などに使う。

**対象は静止画 / ZIP 内画像 / PDF ページのみ**。動画フレームのピン留め・比較は対象外
(動画フルスクリーンは native presenter 経路で UI 拡張がバグを生みやすい。VST3 統合で
苦戦した経緯を踏まえた判断)。動画フルスクリーンでは X / C / Shift+C / Alt+C は消費する silent no-op にする。

### 2.2 UI / 操作

| キー | 動作 | 既存衝突 |
|---|---|---|
| <kbd>X</kbd> | 現在表示中、またはグリッドで選択中の画像をピン留め (再押下で解除) | 静止画/動画 FS とも未割当 ✅ |
| <kbd>C</kbd> | Normal モード中: ピン画像 ↔ 現在画像をトグル表示。Wipe 中は Normal へ戻る | 同上 ✅ |
| <kbd>Shift</kbd>+<kbd>C</kbd> | ワイプ比較モードに入る (Diff 中なら Diff を抜けて Wipe へ切替)、もう一度で Normal へ戻る | 同上 ✅ |
| <kbd>Alt</kbd>+<kbd>C</kbd> | 差分強調モードに入る (Wipe 中なら Wipe を抜けて Diff へ切替)、もう一度で Normal へ戻る | 同上 ✅ |
| <kbd>Esc</kbd> | Wipe / Diff モードから Normal へ (ピンは保持)、Normal で Esc は通常通り FS 解除 | 既存の FS 解除より優先 |

#### モデル (排他)

```
比較状態:
  - Off (ピンなし)
  - Pinned
    └ View mode:
        - Normal (現在画像を表示、ピン留めインジケータあり)
        - Wipe   (左=ピン / 右=現在 を縦線で分割。Shift+C で開始)
        - Diff   (画面全体に RGB 差分を強調表示。Alt+C で開始)

排他: Wipe と Diff は同時 ON 不可。一方に入ると他方は自動解除
```

- ピン留め中は画面右下に小さなサムネ + 「比較中: filename」ラベルを表示
- 比較スロットは **1 枚のみ** (MVP)。複数スロットは将来検討

### 2.3 サイズ違いのハンドリング (筆者指定)

ピン留め画像と現在画像のサイズが異なる場合:

- アスペクト比を維持
- 現在表示中の画像のサイズに合わせてピン留め側をリサイズ
- 中央寄せで配置

すべてのモード (Normal / Wipe / Diff) で同じ見え方になるようにする。比較用に準備する
`ComparePreparedPair` で、ピン留め側を現在画像サイズのキャンバスへ Lanczos3 リサイズ +
中央寄せで配置する。Normal / C トグルも準備済み pair があれば同じ aligned texture を使う。

### 2.4 ワイプ比較モード詳細

- 起動時の縦線位置: 画面中央
- ドラッグ可能範囲: 画面端から 5% 〜 95%
- 縦線見た目: 半透明白 (alpha 50%)、線幅 2px。マウスホバー時に 4px へ
- 左側: ピン留め画像、右側: 現在画像 (固定)
- パン・ズーム操作中は両画像同期
- <kbd>Esc</kbd> または <kbd>Shift</kbd>+<kbd>C</kbd> 再押下で解除

### 2.5 差分強調表示モード詳細

- 画面全体に RGB チャンネルごとの差分を色付きで強調表示 (ワイプ + 差分の同時表示はしない、排他)
- WGSL シェーダで RGB 差分の絶対値をチャンネルごとに γ補正:
  - `out.rgb = sqrt(abs(pinned.rgb - current.rgb))`
  - 同じ箇所は黒、差分が大きい箇所ほど明るく、色味の違いは差分色として残す
- <kbd>Esc</kbd> または <kbd>Alt</kbd>+<kbd>C</kbd> 再押下で解除

### 2.6 ピン留めの永続化

- **揮発** (Codex / Claude 共通の見解)。アプリ再起動でクリア
- フォルダ移動・別アイテム表示・フルスクリーン出入りでは維持
- ピン留め画像は CPU 側の `Arc<ColorImage>` を正として保持し、GPU テクスチャは必要に応じて
  再作成できる派生物として扱う
- 元ファイルが削除/移動されても CPU ピクセルを保持している限り比較を続けられる
- GPU リソース再作成 / 最小化復帰 / cache eviction 時も、CPU ピクセルから texture handle を
  作り直せるようにする

### 2.7 補正操作の扱い

- 比較中の補正スライダ操作は **現在画像のみ** に効く
- ピン留め画像はピン留め時点のスナップショット (補正不可)
- 「ピン留め画像も今の補正で見たい」場合は X で解除して付け直す

### 2.8 実装方針 (案 B: シェーダ統一)

Codex 第 1 ラウンドレビュー指摘「Alt+C 差分は WGSL custom shader pass が要るので工数を
押し上げる」への対応として、**Wipe と Diff を 2 テクスチャ入力シェーダで一体実装** する。

理由:

- 案 A (ワイプを egui clip rect、Diff を別途シェーダ) では 2 種の経路を保守する必要
- 案 B (両方を 2 テクスチャ入力 WGSL シェーダで実装) なら、Diff の追加分は混合関数の
  数行差で済み、合計工数が下がる

#### 実装構造

- ピン留めスロットを App 構造体に追加: `pinned_compare_slot: Option<PinnedSlot>`
  - PinnedSlot: { pixels: Arc<ColorImage>, texture: Option<TextureHandle>, original_path,
    display_name, source_size, captured_params }
- 比較モードを App 構造体に追加: `compare_view_mode: CompareViewMode`
  - `Off`, `PinnedNormal`, `Wipe { wipe_x }`, `Diff`
- `ui_fullscreen.rs` の root key relay probe set と native key map に `X` / `C` を追加し、
  フォーカスがメインビューポート側へ移った場合でも X / C / Shift+C / Alt+C を拾えるようにする
- `display_pipeline` に「比較合成段」を追加 (egui-wgpu の `paint_callback` 経由で WGSL shader):
  - 入力: 2 テクスチャ (ピン / 現在) + uniform (モード、wipe_x)
  - 出力モード:
    - Normal: 現在画像のみ
    - Wipe: 2 枚を境界線 X で左右合成
    - Diff: 差分シェーダで合成
  - シェーダ内 if 分岐 + 1 uniform で 3 モードを切替
- 既存ポストフィルタ / 画像分析モードのシェーダ基盤 (1 テクスチャ入力) とは別パイプライン
- 既存の「右 Ctrl 長押しで元画像」は別経路として共存
- `paint_callback` の 2 テクスチャ入力は `src/compare_wgpu.rs` に比較ビュー専用 pipeline として実装する。
  GPU callback が使えない環境では prepared texture を egui 側で描画する fallback を持つ。

#### v0.10 MVP 実装状況

- `X` で現在表示中の静止画 / ZIP 内画像 / PDF ページ / 見開き合成を worker で CPU snapshot 化し、
  `PinnedSlot` として保持する。元ファイルが消えても比較は継続できる。
- `C` は現在画像サイズへ合わせた `ComparePreparedPair` があれば aligned pin texture を表示し、
  準備中だけピン留め snapshot の既存 texture に fallback する。
- `Shift+C` は 2 入力 WGSL shader で左=ピン / 右=現在の Wipe 表示を行う。GPU callback が
  使えない場合は prepared texture + egui clip rect に fallback する。縦線はドラッグで
  5%〜95% の範囲を移動できる。
- `Alt+C` は 2 入力 WGSL shader で RGB チャンネルごとの差分を γ=0.5 の色付き差分に変換して表示する。
  GPU callback が使えない場合は worker で生成した diff texture を表示する。
- 動画フルスクリーンでは `X` / `C` / `Shift+C` / `Alt+C` を silent no-op として消費する。

#### 工数感

- 2 入力シェーダパイプライン整備: ~300 行
- Normal / Wipe / Diff の 3 モード切替: ~50 行
- ピン留めスロット管理 / CPU snapshot / texture 再作成: ~150 行
- UI (キーバインド / 右下サムネインジケータ / トースト): ~100 行
- 合計: ~600 行 (Codex 第 1 ラウンドで「Alt+C 削除で軽くなる」と評価された工数とほぼ同じ)

### 2.9 エッジケース

| ケース | 動作 |
|---|---|
| ピン留め元ファイルが削除/移動 | CPU pixels 保持で比較継続、トーストで通知 |
| 見開き表示中の X | 見開きペア全体を 1 枚として扱いピン留め |
| 大きなサイズ差 (例: 4K 画像 vs サムネ画像) | アスペクト比維持リサイズで合わせる (画質劣化は許容) |
| ピン留め中にアプリ最小化 → 復元 | ピン留め維持 |
| アプリ起動直後の C キー押下 (ピン無し) | トースト「比較画像が未設定です。X で設定してください」 |
| 動画フルスクリーン中の X / C / Shift+C / Alt+C | すべて silent no-op (比較ビュー対象外) |
| Wipe 中に Alt+C 押下 | Wipe → Diff へ切替 (排他) |
| Diff 中に Shift+C 押下 | Diff → Wipe へ切替 (排他) |

### 2.10 残課題 / レビュー観点

- [ ] ピン留めスロット数 (現案: 1。複数スロット案 F1〜F4 の必要性)
- [x] ピン留め画像のリサイズアルゴリズム (Lanczos3 + 中央寄せ)
- [x] 差分強調のγ値・色付け (RGB チャンネル別の色付き差分、γ=0.5)
- [x] 案 B のシェーダ統一実装で本当に工数が下がるか、egui-wgpu の `paint_callback` で 2 テクスチャ
  入力を扱えるかの spike
- [ ] ピン留めインジケータ (右下サムネ) の表示位置・サイズ (邪魔にならないか。MVP 実装済み、実機確認待ち)
- [x] Wipe モードで縦線ドラッグ中のパン・ズーム操作の競合解消 (縦線近傍のドラッグを優先)

---

## 3. 動画連続再生

### 3.1 目的

フォルダ内の動画を順番に連続再生する。MV 集や撮影クリップを流し見する用途。
プレイリスト UI は作らず、フォルダ構造で整理する現運用と整合させる。

### 3.2 UI / 操作

- HUD の **ループボタンの隣** に「連続再生」ボタンを新設
- ボタン押下で 3 段循環:

| モード | アイコン (案) | 動作 |
|---|---|---|
| 連続再生オフ (既定) | リストのみ (青矢印なし) | EOF で停止 (現在のループモードに従う) |
| 連続再生 | リスト + 1 行目から 2 行目へ進む短い L 字の青矢印 | EOF で次の動画へ。フォルダ末尾で停止 |
| 連続再生 + ループ | リスト + 末尾から先頭へ戻る右側の大きなコの字青矢印 | EOF で次の動画へ。フォルダ末尾で先頭に戻る |

### 3.3 ループボタンとの関係

- 連続再生がオン (連続 or 連続+ループ) のとき、既存のループボタンは **disabled** (グレーアウト)
  - 押下不可、ツールチップ「連続再生中はループ無効」
- 連続再生をオフに戻すと、既存のループモード設定を復元
- キーボード <kbd>L</kbd> (ループトグル) も同様に disabled (キー / ボタン両方で同じ排他状態)

理由: ループ (= 同じ動画を繰り返す) と連続再生 (= 次の動画へ進む) が論理的に競合するため。
ループの 4 モード + 連続の 3 モードを掛け算すると認知負荷が高いので、片方を排他にする。

### 3.4 動作仕様

#### 連続再生

- 動画 EOF で、**現在の表示リスト (visible_indices) 内の次の動画** へ進む
- 非動画 (画像) は飛ばす (visible_indices 走査時に GridItem 種別で判定)
- フォルダ末尾で停止し、既存の Ctrl+↓ 案内トーストを出す
- 「現在の表示リスト」は以下のいずれか (`fullscreen-navigation-consistency.md` の Ctrl+↑↓ コンテキスト
  解決と同じスコープ):
  - 通常フォルダ: 現在開いているフォルダの visible_indices
  - Ctrl+F メタデータ検索結果中: 検索結果の visible_indices
  - Ctrl+S 名前検索結果中: 同上
  - Ctrl+G グローバル検索結果中: 同上
  - 動画タイル中: タイル表示のソース動画 1 本のみ (タイル内では連続再生発動しない、タイルを
    抜けたら再判定)

#### 連続再生 + ループ

- 上記と同じ動作で、フォルダ末尾に達したら先頭の動画へ戻る (visible_indices 単位の無限ループ)
- ループ回数上限なし

### 3.5 永続化

- セッション中はモードを維持
- `Settings.video_continuous_mode` に保存し、アプリ再起動後も復元する
- 2026-05-20 変更: ループ / 音量 / ミュート / 倍速 / Norm と同じく、
  HUD で選ぶ再生プリファレンスとして保存対象にした

### 3.6 エッジケース

| ケース | 動作 |
|---|---|
| 連続再生中に Ctrl+↓ で別フォルダへ | モード維持。新フォルダ内 visible_indices で連続再生継続 |
| 連続再生中にフィルタ ★ ON/OFF | visible_indices が変化した時点でリスト再評価 |
| 連続再生 + ループでリストに動画 1 本だけ | その動画を繰り返し再生 (= 既存ループと同じ挙動だが許容) |
| 連続再生中に手動で一時停止 | モード維持。再生再開後、EOF で次へ進む |
| 連続再生中に動画タイル表示 | タイル中は連続再生発動しない。タイルを抜けた時点で次へ |
| 連続再生中に現在動画のループモード変更 | できない (ループボタン / L キー disabled) |
| 連続再生で次動画へ自動遷移 | `video_autoplay` / resume 設定に関わらず、常に先頭から自動再生 |
| Ctrl+F / Ctrl+S / Ctrl+G 結果をドリルダウンして別動画を直接開いた | モード維持。新しい visible_indices で次の動画判定 |

### 3.7 実装方針

- 既存の `VideoLoopMode` enum とは別に、`VideoContinuousMode` enum を追加:
  - `Off`, `Continuous`, `ContinuousLoop`
  - `Settings.video_continuous_mode` に保存し、`App` の runtime state へ起動時に復元する
- `poll_video` で `video_continuous_mode != Off` の間は `player.set_loop_enabled(false)` を優先し、
  既存 `VideoLoopMode` は「設定値として保持するが、再生挙動には使わない」状態にする
- EOF は `VideoPlayer::tick()` が現在どおり drain 完了後に engine state を `Eof` に遷移させる。
  App 側は tick 後に `player.engine_state_code() == EOF` を検出し、Continuous モードなら次動画へ進む
  - 重複発火防止に `video_continuous_last_eof: Option<(usize, u64)>` 相当を持ち、
    同じ idx / seek serial の EOF を 2 回処理しない。ここで使う `u64` は native presenter の
    `source_epoch` ではなく、VideoPlayer / AvClock 側の seek serial
  - T14 の EOF 同期配線は 2026-05-18 時点で解決済みなので、`engine_state_code() == EOF` を
    連続再生の検出点として使える
  - `Continuous` で次動画が見つからない場合はそのまま EOF 停止し、既存の Ctrl+↓ 案内トーストを出す
  - `ContinuousLoop` で次動画が現在動画自身だけの場合は reopen せず `seek(0.0)` + `Play`
- 「次の動画」探索は **現在の表示リスト (visible_indices) 内** で行う:
  - `fullscreen-navigation-consistency.md` の Ctrl+↑↓ コンテキスト解決と **別経路**
    (Ctrl+↑↓ はフォルダ横断、本機能は同一リスト内)
  - `find_next_video_in_visible_indices(current_idx, wrap)` を App 側 helper として新設
  - `ZipSeparator` / 画像 / フォルダ / ZIP / PDF / `ZipImage` / `PdfPage` はスキップし、
    `GridItem::Video` のみ候補にする
- 動画切替は手動の動画→動画移動と同じ `open_native_video_fullscreen_from_navigation` /
  `SwitchSource` 経路を使う。native presenter HWND を破棄せず、既存 fast-swap の安全策
  (source_epoch / navigation preview / stale event 破棄) に乗せる
- HUD ボタン UI 拡張は `src/video/native_presenter/` 経由:
  - `NativeOverlayCommand::ToggleContinuous` を追加
  - `NativeVideoOutputEvent::ToggleContinuous` を App が受け、`VideoContinuousMode` を循環
  - `NativeVideoOutputCommand::SetContinuousMode` で overlay 表示状態を同期
- ループボタン / L キーの disabled 状態管理を追加:
  - overlay ではループボタンをグレーアウトし、クリックしても command を出さない
  - native HWND の `L` キー処理も Continuous 中は no-op + トースト「連続再生中はループ無効」
  - Continuous を Off に戻したら、保存済みの `settings.video_loop_mode` を再び player に反映する

### 3.8 残課題 / レビュー観点

- [x] HUD ボタンの位置: ループボタンの右隣に追加
- [x] 連続再生モードのアプリ再起動時の挙動: `Settings.video_continuous_mode` に保存して復元
- [x] 「連続再生中はループ無効」の disabled 表示: ループボタンをグレーアウトして残す
- [ ] フォルダ末尾の停止時に「Ctrl+↓ で次フォルダ」案内トーストは既存と同じか専用文言にするか
- [ ] フォルダに動画が 1 本だけのときの連続+ループは違和感あるか (現案: 違和感許容)
- [ ] 動画タイル中の連続再生発動を抑止する判断は妥当か (タイルから次動画へ自動遷移するべきとの逆案あり)

### 3.9 実装状況 (2026-05-19)

- `crate::video::VideoContinuousMode` (`Off` / `Continuous` / `ContinuousLoop`) を追加し、`App`
  の runtime state と `Settings.video_continuous_mode` に保持
- native HUD に連続再生ボタンを追加。押下で 3 モードを循環し、状態は
  `NativeVideoOutputCommand::SetContinuousMode` で overlay に同期
- アイコンは「プレイリスト + 現在行の再生マーク」を基本形にし、連続再生は次行への矢印、
  連続再生 + ループは末尾から先頭へ戻る矢印で表現
- 連続再生 ON 中は既存ループを `set_loop_enabled(false)` へ強制し、HUD ループボタンと
  <kbd>L</kbd> キーは no-op + 「連続再生中はループ無効」表示
- 連続再生で次動画へ自動遷移するときは `autoplay=true` / resume 無視で開く。手動の動画移動は
  従来通りユーザー設定と resume 位置を尊重
- EOF は `poll_video` で `engine_state_code() == EOF` を検出し、`(fs_idx, seek_serial)` で
  重複処理を抑止
- 次動画探索は `find_next_video_in_visible_indices_from()` で `visible_indices` 内の
  `GridItem::Video` のみを候補にする。画像 / ZIP / PDF / セパレータ / フォルダ等はスキップ
- 動画切替は既存 `open_native_video_fullscreen_from_navigation()` を利用し、native presenter の
  fast-swap / stale event 破棄 / source epoch 管理に乗せる
- Unit test: `next_video_search_uses_visible_indices_and_skips_non_video_items`

---

## 4. スクロールバー視認性向上 (関連改善)

### 4.1 目的

筆者指摘: 現在のグリッドビューのスクロールバーが細く、色も薄いため、フォルダ中の
どのあたりを見ているかが分かりにくい。

カラフルにすると mIV の落ち着いた配色を崩すので、**幅と不透明度の微調整** に絞って
目立たせる。

### 4.2 調整案

| 項目 | 現状 (推定) | 調整案 |
|---|---|---|
| スクロールバー幅 | 4-6px | 8-10px (倍弱) |
| 不透明度 (idle) | 20-30% | 40-50% |
| 不透明度 (hover) | 50% 程度 | 70-80% (既存ホバー強調を強める) |
| 色味 | テーマ既定 (ほぼ白 or 黒のグレースケール) | **変更しない** (カラフルにはしない) |

### 4.3 実装方針

- egui 0.33 の `style.spacing.scroll` (`ScrollStyle`) を調整する
  - `bar_width`: 8-10px
  - `floating_width`: 4-5px 程度
  - `handle_opacity` 系: idle 40-50%, hover 70-80%
- `style.visuals.widgets.inactive.bg_fill` / `hovered.bg_fill` はボタン等の全 widget に波及するため、
  スクロールバー視認性目的では触らない
- まず `os_theme::apply_resolved()` の直後に全体適用し、ダイアログ類への波及が気になる場合は
  グリッドの `ScrollArea` 周辺だけ `ui.scope` で style override する
- ライト / ダーク両テーマで見た目を確認
- 既存スクリーンショットテスト (`tests/ui_snapshot.rs`) は影響を受けるので `UPDATE_SNAPSHOTS=1` で
  更新 (`ui-snapshot-policy.md` 参照)

### 4.4 工数感

- 小 (50-100 行 + スナップショット更新)
- 単独でも価値があり、他機能と独立して着手可能

### 4.5 残課題 / レビュー観点

- [ ] 幅と不透明度の数値は実機確認後に微調整
- [ ] フルスクリーンビューポート内のスクロール (例: PDF 縦長ページのフルスクリーン表示時)
  にも同じ調整を適用するか (現案: グリッドビューのスクロールバーのみ)

### 4.6 実装状況 (2026-05-18)

- 完了: `os_theme::apply_resolved()` 後に `style.spacing.scroll` の幅 / opacity だけを調整
- 確認: `widgets.*.bg_fill` は変更しない unit test を追加済み

---

## 5. 実装設計メモ

この節は Claude Code レビュー後に実装へ入るための差し込み口メモ。詳細な作業分解は各機能の
実装時に行うが、ここでは既存アーキテクチャとの接続点を固定する。

### 5.1 キャプチャ保存

#### モジュール / 型

- 新規 `src/capture.rs` を追加し、保存・クリップボード共通の型と worker を集約する
  - `CaptureFormat`: `Png`, `Jpeg95`, `Jpeg85`, `Jpeg75`
  - `CaptureDestination`: `File`, `Clipboard`
  - `CapturePixelJob`: UI スレッドで作る軽量 snapshot
  - `CaptureResult`: 保存先 path / clipboard 成功 / error message
  - `CaptureError`: unsupported / source_not_ready / io / encode
- `App` に `capture_pending: Option<CapturePending>` を追加し、結果を毎フレーム poll する
- `Settings` に単純フィールドを追加する
  - `capture_output_dir: Option<PathBuf>`
  - `capture_format: CaptureFormat`
  - いずれも `settings_kv` 保存なので DB schema 変更は不要
  - `PathBuf` の serde / `settings_kv` 保存は既存のパス系設定と同じ表現に揃える

#### 静止画 / ZIP / PDF のピクセル取得

- UI スレッドで `prepare_capture_pixel_job(idx)` を呼び、`Arc<ColorImage>` と `AdjustParams` を
  clone して worker へ渡す
- 優先順位:
  1. `!post_filter_bypassed` かつ `adjustment_cache[idx]` がある → その CPU pixels をそのまま使う
  2. `ai_upscale_cache[(idx, bg)]` がある → worker で `apply_adjustments_fast` + `post_filter::apply`
  3. `fs_cache[idx]` がある → 同上
  4. どれも無い → `source_not_ready` トースト
- worker では App / egui::Context / TextureHandle を触らない。`ColorImage -> RgbaImage` 変換、
  必要なら補正・ポストフィルタ、PNG/JPEG エンコード、ファイル I/O / clipboard 書き込みだけを行う
- 見開きは左右それぞれ `CapturePixelJob` を作り、worker で結合する。結合順は現在の見開き設定
  (LTR / RTL / Cover) に従う
- 静止画系のカメラアイコン / clipboard 経路も `prepare_capture_pixel_job(idx)` を必ず経由する。
  Ctrl+S だけ別実装にせず、保存と clipboard の snapshot 内容を一致させる

#### 動画フレーム

- 既存 `video::screenshot::capture_frame(path, target_secs)` を使う
- Ctrl+S は `capture_frame` の RGBA を同じ `capture.rs` のエンコード worker に流す
- カメラアイコンの clipboard は既存 `copy_rgba_image_to_clipboard_async_seq` を流用しつつ、
  tooltip だけ「Ctrl+S: ファイル保存」を追加する
- 動画再生は停止しない。target 秒は `player.position()` または `last_displayed_pts_secs()` を使い、
  既存 clipboard の取り方と揃える

#### UI / キー

- 静止画フルスクリーン上部ホバーバーにカメラアイコンを追加する。描画 helper は
  `ui_fullscreen/draw_icons.rs` に寄せ、動画 HUD の camera icon と見た目を揃える
- `ui_fullscreen.rs` の root key relay probe set に `Ctrl+S` を追加する
- `app/native_video.rs::handle_native_video_key_event` に `Ctrl+S` を追加し、native HWND 側でも
  ファイル保存を発火する
- ファイルメニューに「キャプチャ保存フォルダを開く」を追加する。開く処理は保存先の存在確認と
  `create_dir_all` を worker / command 側に寄せ、UI スレッドで重い I/O をしない

#### テスト / 確認

- `Settings` serde roundtrip: 既定 capture 設定が欠落 DB / JSON から復元されること
- `capture.rs` unit test: ファイル名連番、PNG/JPEG エンコード、CRT 系でサイズ変化しても保存できること
- App-level test: グリッド Ctrl+S は従来どおり名前検索、フルスクリーン Ctrl+S は capture に分岐
- 手動確認: 通常画像 / ZIP 内画像 / PDF ページ / 動画で Ctrl+S と camera clipboard が動くこと

### 5.2 比較ビュー

#### 状態

- `App` に以下を追加する
  - `pinned_compare_slot: Option<PinnedSlot>`
  - `compare_view_mode: CompareViewMode`
  - `compare_prepared_pair: Option<ComparePreparedPair>`
- `PinnedSlot` は CPU pixels を正とする
  - `pixels: Arc<ColorImage>`
  - `texture: Option<TextureHandle>`
  - `display_name`, `source_key`, `source_size`, `captured_params`
- `ComparePreparedPair` は Normal / Wipe / Diff 用に、現在画像のサイズへ合わせた pin texture を持つ
  - current idx / current size / pin source generation / mode を key にし、画像切替や補正変更で破棄する

#### ピン作成

- X 押下時は現在表示できている CPU pixels から即時 pin を作る
  - `adjustment_cache` → `ai_upscale_cache` → `fs_cache` の順
  - cache miss なら「画像読み込み中です」とトーストし、重い decode は発火しない
- 見開き中は左右ページを worker で結合してから pin する。単ページより重いので、X 押下後に
  「比較画像を準備中」トーストを出し、完了時に pin する

#### 描画

- Normal / C トグルは `ComparePreparedPair` があれば aligned pin texture を表示する。
  準備中だけ pin snapshot の既存 texture に fallback する
- Wipe / Diff は egui-wgpu `paint_callback` の 2 texture WGSL pipeline を使う。GPU callback が
  使えない場合は prepared texture 描画へ fallback する
- サイズ違いは Normal / Wipe / Diff に入る時点で `ComparePreparedPair` を worker 作成する
  - 基準サイズは現在画像
  - pin 側を `crate::fast_resize` の Lanczos3 でアスペクト維持リサイズし、中央寄せで pad する
  - current 側は capture pipeline の出力 pixels を基準にする

#### キー / 入力

- `ui_fullscreen.rs` の root key relay probe set と native key map に `X` / `C` を追加する
- 動画フルスクリーンでは X / C / Shift+C / Alt+C は **消費する silent no-op** にする。
  passthrough させず、比較ビュー非対応であることを入力経路側で明示する
- Wipe の縦線 drag は pan/zoom より優先する。縦線 hit rect 内なら wipe drag、外なら既存 pan/zoom

#### テスト / 確認

- App-level test: X で pin 作成、C で表示切替、Esc で Wipe/Diff だけ解除
- Unit test: `CompareViewMode` の排他遷移、visible item 変更時の prepared pair invalidation
- 手動確認: サイズ違い、見開き、補正変更、ファイル削除後の pin 維持、最小化復帰

### 5.3 動画連続再生

#### 状態 / UI

- `VideoContinuousMode` は `crate::video` 側の public enum とし、App / native presenter の双方から使う
- `App` runtime state:
  - `video_continuous_mode: VideoContinuousMode`
  - `video_continuous_last_eof: Option<(usize, u64)>`
    - `u64` は native presenter の `source_epoch` ではなく、VideoPlayer / AvClock 側の seek serial。
      同じ動画・同じ seek 世代の EOF を二重処理しないためのキーで、動画を開き直した時点で clear する
- native presenter:
  - `NativeOverlayCommand::ToggleContinuous`
  - `NativeVideoOutputEvent::ToggleContinuous`
  - `NativeVideoOutputCommand::SetContinuousMode`
  - overlay state に continuous mode を持たせ、ループボタンの隣にボタンを描画する

#### EOF 処理

- Continuous Off:
  - 現行どおり `settings.video_loop_mode` から `loop_enabled` を算出し、VideoPlayer の EOF 処理に任せる
- Continuous On:
  - `poll_video` で `player.set_loop_enabled(false)` を強制し、VideoPlayer には EOF で停止させる
  - tick 後に App が `engine_state_code() == EOF` を検出して `handle_video_continuous_eof(fs_idx)` を呼ぶ
    - T14 の EOF 同期配線は 2026-05-18 時点で解決済み。`engine_state_code()` が EOF を publish
      する現行経路を前提に実装する
  - `find_next_video_in_visible_indices(fs_idx, wrap)` で次動画を決める
  - 次動画が別 idx なら既存 native navigation / fast-swap 経路で開く
  - 次動画が同 idx だけなら `seek(0.0)` + `Play` で同一動画を再開する

#### テスト / 確認

- Unit test: `find_next_video_in_visible_indices` の通常 / 末尾 / wrap / 動画 1 本 / 非動画 skip。
  `ZipImage` / `PdfPage` も画像扱いでスキップし、候補は `GridItem::Video` だけにする
- App-level test: Continuous On 中は L が `settings.video_loop_mode` を変えない
- 手動確認: 通常フォルダ、検索結果、★フィルタ、動画タイル中、動画→動画 fast-swap

### 5.4 スクロールバー視認性

- まず `os_theme::apply_resolved()` 後の共通 style hook で `style.spacing.scroll` だけを調整する
- 色はテーマ既定のまま、opacity と幅だけを変更する
- `widgets.*.bg_fill` は触らない。必要なら grid の `ScrollArea` 周辺だけ局所 override に切り替える
- `cargo test --test ui_snapshot` で差分を確認し、意図した変更なら `UPDATE_SNAPSHOTS=1` で更新する

---

## 着手順 (Codex 推奨ベース)

規模小・確実な勝ちから着手する。各機能は独立しているので並行作業も可。

1. **動画フレーム保存** — 既存 `video::screenshot::capture_frame` がそのまま使え、最短で価値が出る
2. **静止画キャプチャ保存 MVP** — 補正済みソース画像保存。MVP 範囲 (補正・AI・消しゴム・ポストフィルタ) を反映
3. **動画連続再生** — HUD ボタン追加 + EOF ハンドラ拡張、visible_indices 内探索
4. **比較ビュー** — X/C/Wipe/Diff と `ComparePreparedPair` + Lanczos3 のサイズ合わせまで実装
5. **スクロールバー視認性向上** — 単独で着手可、他のどこかに挟むか並行

すべて完了後に v0.10.0 リリース候補。

比較ビューは表示パイプラインに触るため一番工数が大きい (4 番目)。
スクロールバーは独立タスクなので、待ち時間に挟むか並行で進める。
