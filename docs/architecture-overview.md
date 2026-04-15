# アーキテクチャ概観

mimageviewer 全体の構造を俯瞰するための入口ドキュメント。**修正作業の前に必ず目を通すこと**。
個別の詳細は下の「関連ドキュメント」にある専用ページに任せる。

---

## 1. レイヤー構造

```
┌──────────────────────────────────────────────────────────────┐
│  UI 層 (eframe + egui, wgpu バックエンド)                     │
│   - メインビューポート: グリッド (ui_main.rs)                 │
│   - フルスクリーンビューポート (ui_fullscreen.rs)              │
│   - オーバーレイ: 補正 / 分析 / 消しゴム / メタデータパネル    │
│   - ダイアログ群 (ui_dialogs/)                                 │
└───────────────┬──────────────────────────────────────────────┘
                │ App の public メソッド経由で状態を更新
┌───────────────▼──────────────────────────────────────────────┐
│  アプリ状態層 (src/app.rs の App 構造体)                       │
│   - items / thumbnails / fullscreen_idx …                     │
│   - 各種キュー (reload_queue, heavy_io_queue)                  │
│   - 各種キャッシュ (fs_cache, adjustment_cache, ai_upscale_…)  │
│   - 通信チャネル (tx/rx, cancel_token, scroll_hint)            │
└───────────────┬──────────────────────────────────────────────┘
                │ LoadRequest を push / ワーカースレッド spawn
┌───────────────▼──────────────────────────────────────────────┐
│  非同期ワーカー層                                              │
│   - サムネイルワーカー (通常/重 I/O の 2 系統)                 │
│   - フルスクリーンロードスレッド (1 画像ごとに spawn)          │
│   - PDF ワーカープロセス (--pdf-worker サブプロセス × 3)       │
│   - AI 推論スレッド (ort + DirectML)                           │
│   - 動画サムネイルスレッド, フォルダナビゲーション, etc.       │
└───────────────┬──────────────────────────────────────────────┘
                │ デコード結果を mpsc で返す
┌───────────────▼──────────────────────────────────────────────┐
│  データソース                                                  │
│   - ファイルシステム (画像 / ZIP / PDF)                        │
│   - image crate + turbojpeg (JPEG) + WIC (HEIC/AVIF/JXL/RAW)   │
│   - PDFium (pdfium-render、別プロセス)                         │
│   - SQLite DB 群 (catalog / rotation / adjustment / mask /     │
│     spread / pdf_passwords)                                    │
│   - ONNX モデル (upscale / denoise / inpaint / classify)       │
└──────────────────────────────────────────────────────────────┘
```

**鉄則**: 上位レイヤーから下位レイヤーへの呼び出しは OK。逆方向（ワーカーが UI を直接触るなど）は禁止。
ワーカーから UI への通知は必ず mpsc チャネルで行う。

---

## 2. モジュールマップ

### コア

| モジュール | 役割 |
| --- | --- |
| `main.rs` | エントリポイント。フォント設定、logger 初期化、eframe 起動、`--pdf-worker` サブコマンド分岐 |
| `lib.rs` | モジュール宣言のみ (ベンチマーク・テスト用に公開) |
| `app.rs` | `App` 構造体と `eframe::App` 実装。**3900 行超** — 状態遷移の中心 |
| `settings.rs` | 設定の JSON 永続化 (`%APPDATA%/mimageviewer/settings.json`) |
| `data_dir.rs` | `%APPDATA%/mimageviewer/` のパス解決 |
| `logger.rs` | パフォーマンス分析用ファイルロガー (`mimageviewer.log`) |
| `stats.rs` | 読み込み統計の集計 |

### グリッド / サムネイル / フルスクリーン

| モジュール | 役割 |
| --- | --- |
| `ui_main.rs` | メイン画面のグリッド描画とクリック/ドラッグ処理 |
| `ui_fullscreen.rs` | フルスクリーンビューポート (`show_viewport_immediate`)。描画テクスチャの優先順位はここで決定 |
| `ui_helpers.rs` | メニューバー、ツールバー、アドレスバー等の共通 UI |
| `grid_item.rs` | `GridItem` 列挙型と `ThumbnailState` (Pending/Loaded/Failed/Evicted) |
| `thumb_loader.rs` | サムネイル並列ロード (WebP キャッシュ生成含む) |
| `catalog.rs` | SQLite サムネイルキャッシュ (`%APPDATA%/mimageviewer/catalog.db`) |

### 仮想フォルダ (ZIP/PDF) / フォーマット

| モジュール | 役割 |
| --- | --- |
| `zip_loader.rs` | ZIP 内の画像列挙、エントリバイト取得、先頭画像抽出 |
| `pdf_loader.rs` | PDFium ワーカープロセスプール。ページ列挙・レンダリング |
| `pdf_passwords.rs` | PDF パスワードの DPAPI 暗号化永続化 |
| `wic_decoder.rs` | HEIC/AVIF/JXL/TIFF/RAW のデコード (Windows Imaging Component) |
| `fs_animation.rs` | GIF / APNG アニメーションのフレーム展開 |
| `video_thumb.rs` | 動画サムネイル取得 (Windows Shell API) |
| `folder_tree.rs` | 深さ優先前順トラバーサル (Ctrl+↑↓ 用) |

### 補正 / 編集 / AI

| モジュール | 役割 |
| --- | --- |
| `adjustment.rs` | `AdjustParams` (輝度/コントラスト/γ/彩度/色温度…)、LUT 適用、オート補正 |
| `adjustment_db.rs` | フォルダ別プリセット・ページ別プリセットの SQLite 永続化 |
| `rotation_db.rs` | 非破壊回転の SQLite 永続化 |
| `rating_db.rs` | レーティング (★1〜5) の SQLite 永続化 |
| `search_index_db.rs` | お気に入り配下のフォルダ/ZIP/PDF 名索引 (検索インデックス) |
| `mask_db.rs` | 消しゴムマスクの SQLite 永続化 (1bit/pixel deflate 圧縮) |
| `spread_db.rs` | フォルダ別の見開きモード永続化 |
| `ai/` | ONNX Runtime (DirectML) によるアップスケール / デノイズ / Inpainting / 画像種別分類 |
| `png_metadata.rs` | PNG の tEXt/iTXt/zTXt に埋め込まれた AI メタデータ読み取り |
| `exif_reader.rs` | EXIF 読み取り (rexif) |

### UI オーバーレイ / ダイアログ

| モジュール | 役割 |
| --- | --- |
| `ui_adjustment_panel.rs` | 画像補正パネル (左端オーバーレイ)。プリセット切替・AI 設定・保存スロット |
| `ui_analysis_panel.rs` | 画像分析パネル (右端オーバーレイ)。色情報・ヒストグラム |
| `ui_metadata_panel.rs` | メタデータパネル (AI メタデータ + EXIF) |
| `ui_erase.rs` | 消しゴムモード (Lasso/縦線/横線/ブラシ → MI-GAN で inpaint) |
| `ui_dialogs/` | 環境設定・キャッシュ管理・お気に入り編集・スライドショー設定等 |

### その他

| モジュール | 役割 |
| --- | --- |
| `gpu_info.rs` | GPU 情報取得 (VRAM サイズ等、キャッシュ容量の自動決定に使用) |
| `monitor.rs` | モニター情報取得 (DPI 等) |
| `open_with.rs` | 外部アプリで開く |

---

## 3. データフロー (俯瞰)

詳細は [display-pipeline.md](display-pipeline.md) を参照。ここでは 1 画面分だけ:

```
ユーザー操作 (キー/マウス)
    │
    ▼
App::update() 内のハンドラ
    │  ├─ load_folder(path)         → フォルダ/ZIP/PDF 切替
    │  ├─ start_fs_load(idx)        → フルスクリーン画像ロード
    │  ├─ apply_rotation(idx)       → 回転の DB 更新
    │  └─ preset 切替 / 補正変更    → adjustment_cache クリア
    │
    ▼
各ワーカーに LoadRequest を投げる / テクスチャキャッシュを無効化
    │
    ▼
次フレームの poll_* / maybe_apply_adjustment() 等で結果取り込み
    │
    ▼
ui_fullscreen.rs / ui_main.rs が「表示用テクスチャ」を選んで描画
    (adjustment_cache > ai_upscale_cache > fs_cache の優先順位)
```

**「どのテクスチャを表示するか」の決定ロジックは `ui_fullscreen.rs` に集中している**。
補正や AI を追加する時は、ここの選択順序を必ず確認すること。

---

## 4. 永続化ストア一覧

すべて `%APPDATA%/mimageviewer/` 配下。バックアップ対象。

| ファイル | 内容 | 書き込むモジュール |
| --- | --- | --- |
| `settings.json` | アプリ全体設定・グローバルプリセット・保存スロット・お気に入り | `settings.rs` |
| `catalog.db` | サムネイル WebP キャッシュ (BLOB) + メタデータ | `catalog.rs` |
| `rotation.db` | 非破壊回転角 (0/90/180/270) | `rotation_db.rs` |
| `rating.db` | レーティング (★1〜5、0 は未登録) | `rating_db.rs` |
| `search_index.db` | お気に入り配下のフォルダ/ZIP/PDF 名索引 | `search_index_db.rs` |
| `adjustment.db` | フォルダ別プリセット 4 種 + ページ別プリセット割当 | `adjustment_db.rs` |
| `mask.db` | 消しゴムマスク (deflate 圧縮 1bit/pixel) | `mask_db.rs` |
| `spread.db` | フォルダ別見開きモード | `spread_db.rs` |
| `pdf_passwords` | PDF パスワード (DPAPI 暗号化) | `pdf_passwords.rs` |
| `pdfium.dll` | 初回起動時に exe から展開 | `main.rs` |
| `models/*.onnx` | 初回起動時に exe から展開 | `ai/model_manager.rs` |
| `mimageviewer.log` | 起動ごとに追記 | `logger.rs` |

**パスキーの正規化**: Windows は大文字小文字非区別なので、すべての DB は **小文字化 + バックスラッシュ→スラッシュ** に正規化してから格納する。新しい DB を追加するときも同じ規約に従う (`rotation_db.rs` / `adjustment_db.rs` を参照)。

---

## 5. Phase 区分 (現状)

| Phase | 内容 | 状況 |
| --- | --- | --- |
| 1 | コアビューワー (グリッド・フルスクリーン・設定永続化) | ✅ |
| 1.5 | サムネイルカタログ (SQLite + WebP) | ✅ |
| 2 | AI アップスケール / デノイズ / Inpaint (ONNX + DirectML) | ✅ (Real-ESRGAN/waifu2x 系, MI-GAN) |
| 3 | お気に入り・ツールバー・ZIP・WIC・動画・アニメーション | ✅ |
| 3.5 | 画像補正プリセット (フォルダ別 4 種 + グローバル + 保存スロット 10) | ✅ |
| 3.6 | 消しゴム (Lasso/ブラシ → MI-GAN) | ✅ |

---

## 6. 関連ドキュメント

| ドキュメント | 読むべきタイミング |
| --- | --- |
| [display-pipeline.md](display-pipeline.md) | サムネイル表示やフルスクリーン描画を触るとき。**補正・AI・回転がどこで適用されるかの決定版** |
| [async-architecture.md](async-architecture.md) | 並列処理・キャンセル・キャッシュ競合を触るとき。ワーカー構成の一覧 |
| [virtual-folders.md](virtual-folders.md) | ZIP/PDF 関連を触るとき。**通常画像パスと分岐する箇所のチェックリスト** |
| [preset-and-adjustment.md](preset-and-adjustment.md) | 補正・プリセット・AI キャッシュを触るとき。無効化ルールの早見表 |
| [spec.md](spec.md) | 機能仕様・設定項目の正式な定義 |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の詳細設計 |
| [thumbnail-memory-redesign.md](thumbnail-memory-redesign.md) | サムネイルメモリ管理の背景経緯 |

---

## 7. 修正時のチェックリスト

1. **触る機能の doc を必ず先に読む** (上の表から該当ページを選ぶ)
2. **通常画像 / ZIPImage / PdfPage の 3 分岐を忘れない** — ZIP/PDF 対応漏れは頻出バグ
3. **サムネイル経路とフルスクリーン経路の両方で整合性を保つ** — 片方だけ修正すると表示が食い違う
4. **テクスチャキャッシュの無効化タイミング** — 補正・AI・回転を変更したら正しいキャッシュをクリアしているか確認 (`preset-and-adjustment.md`)
5. **ドキュメント同時更新** — CLAUDE.md の「コード修正時のドキュメント同時更新」セクションに従う
