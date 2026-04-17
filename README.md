# mImageViewer

Windows 向け高速サムネイルビューワー

[![GitHub release](https://img.shields.io/github/v/release/MikageSawatari/mimageviewer)](https://github.com/MikageSawatari/mimageviewer/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

## こんな方におすすめ

- **大量の写真を素早く確認したい** — RAW・HEIC にも対応。フォルダを開くだけでサムネイル一覧
- **AI 生成画像のプロンプトを確認しながら閲覧したい** — PNG に埋め込まれた Stable Diffusion / ComfyUI のメタデータを自動表示
- **ZIP にまとめた漫画や同人誌をそのまま読みたい** — 展開不要で ZIP 内をブラウズ。見開き表示・右→左読みに対応
- **大量の PDF をサムネイルで見比べたい** — PDF をページごとにサムネイル表示。パスワード付きにも対応
- **AI アップスケールで綺麗な画像で見たい** — Real-ESRGAN / Real-CUGAN / NMKD-Siax を内蔵。自動選択モードや用途別のモデル切り替えに対応
- **スキャンの汚れを補修したい** — 消しゴムツール（MI-GAN）で非破壊修復。元ファイルを書き換えずにマスクで補修

## 主な機能

- GPU アクセラレーションによるサムネイルグリッド表示
- SQLite + WebP サムネイルキャッシュ（2回目以降は瞬時表示）
- フルスクリーン表示（前後画像の先読み付き）
- 見開き表示（左→右 / 右→左、表紙単独表示対応）
- 画像分析モード（カラーピッカー、ヒストグラム、色差強調、グレースケール等）
- AI アップスケール（Real-ESRGAN / Real-CUGAN / NMKD-Siax、DirectML）
- AI JPEG ノイズ除去（RealPLKSR / OmniSR）
- AI 画像修復（消しゴムツール、MI-GAN）— 非破壊マスク保存、タイル処理
- 画像分類（風景・人物・ドキュメント等の自動判定）
- 画像補正（明るさ・コントラスト・色調）＋フォルダ/ZIP/PDF 単位の 4 プリセット
- AI 画像メタデータ表示（A1111/Forge、ComfyUI、NovelAI）
- EXIF 表示（日本語タグ名対応）
- 非破壊回転
- ZIP 内画像のブラウズ（展開不要）
- PDF 表示（ページごとレンダリング、パスワード付き対応、PDFium 内蔵）
- RAW / HEIC / AVIF / JPEG XL 対応（Windows WIC 経由）
- 動画サムネイル表示（MP4, AVI, MKV 等）
- お気に入りフォルダ、カスタマイズ可能なツールバー
- メタデータキーワード検索、スライドショー

## 対応フォーマット

| 種類 | フォーマット |
|------|-----------|
| 静止画（内蔵） | JPEG, PNG, GIF, WebP, BMP |
| 静止画（WIC） | HEIC, AVIF, JPEG XL, TIFF, 各社 RAW |
| アニメーション | GIF, APNG（Animated WebP は先頭フレームのみ表示） |
| ドキュメント | PDF |
| アーカイブ | ZIP |
| 動画（サムネイルのみ） | MP4, AVI, MOV, MKV, WMV, MPEG |

## 動作環境

- Windows 10 / 11（64bit）
- DirectX 12 対応 GPU
- メモリ 4GB 以上推奨

## ダウンロード

[Releases ページ](https://github.com/MikageSawatari/mimageviewer/releases/latest) から以下のいずれかをダウンロードできます。

- **インストーラ版** (`mImageViewer_setup.exe`) — スタートメニュー登録・アンインストール機能付き
- **単体 exe 版** (`mimageviewer.exe`) — 任意のフォルダに置いて実行するだけ

設定は `%APPDATA%\mimageviewer\` に保存されます。

## マニュアル

[オンラインマニュアル](https://mikage.to/mimageviewer/manual/) — インストール方法・操作方法・キーボードショートカット・設定リファレンス

## 技術情報

- **言語**: Rust (edition 2024)
- **GUI**: eframe / egui (wgpu バックエンド)
- **JPEG 高速デコード**: TurboJPEG (libjpeg-turbo, SIMD スタティックリンク)
- **PDF エンジン**: PDFium（exe に埋め込み、3 プロセス並列レンダリング）
- **サムネイルキャッシュ**: SQLite + WebP

## 更新履歴

### 次期バージョン (開発中)
- レーティング機能（★1〜★5）を追加：F1〜F5 で付与、F6 で解除、ツールバーから★でフィルタ
- Ctrl+A でフィルタ中の全アイテムを一括チェック
- 通常画像・ZIP 内画像・PDF ページに対応、データは `rating.db`（SQLite）に非破壊保存

### v0.6.0
- AI アップスケール（Real-ESRGAN、DirectML）
- AI JPEG ノイズ除去（RealPLKSR / OmniSR）
- AI 画像修復（MI-GAN、DirectML）：消しゴムツールでの範囲指定消去、タイル処理、非破壊マスク保存
- 画像分類（風景・人物・ドキュメント等の自動判定）
- 画像補正機能（明るさ・コントラスト・色調）＋フォルダ/ZIP/PDF 単位の 4 プリセット
- プリセットシステム刷新（グローバルプリセット、保存スロット、グリッド連携）
- AI モデルを exe に埋め込み（ダウンロード不要）
- 環境設定ダイアログを統合（ツリー形式の単一ダイアログに再編）
- ヘルプメニュー・バージョン情報ダイアログを追加
- 1 カラム表示＋ Alt + 数字でカラム数切り替え
- フルスクリーンで P キーによるスライドショー切り替え
- ホバーバー・見開きポップアップにショートカットツールチップを表示
- AI スキップ閾値（アップスケール・ノイズ除去）を設定可能に
- クラッシュログを `%APPDATA%\mimageviewer\logs\` に移動
- 8192px 超の画像表示時のクラッシュを修正

### v0.5.0
- TurboJPEG (SIMD) による JPEG 高速デコード（1.5〜2.4 倍高速化）
- PDF マルチプロセス並列レンダリング（3 ワーカープロセス、99% 高速化）
- フルスクリーンでズーム / パン / 任意角度回転に対応
- フルスクリーンで右クリック長押しコンテキストメニュー
- サムネイル読み込みの優先度キュー最適化（可視範囲を最優先）
- ZIP / PDF の一括キャッシュ作成に対応
- フォルダサムネイルの再帰解決（空フォルダでも子孫の画像を表示）

### v0.4.0
- PDF 表示（ページごとレンダリング、パスワード付き対応、PDFium 内蔵）
- 見開き表示（左→右 / 右→左、表紙単独表示対応）
- 画像分析モード（カラーピッカー、ヒストグラム、色差強調、グレースケール等）
- フォルダサムネイルプレビュー（1枚目の画像＋名前バッジ）
- ZIP/PDF ファイルバッジ表示
- 「アプリケーションで開く」コンテキストメニュー
- PDF/ZIP キャッシュオプション
- EXIF 日本語タグ名表示
- インストーラ版を追加

### v0.3.0
- AI 画像メタデータパネル（Stable Diffusion / ComfyUI / Midjourney プロンプト表示）
- EXIF 表示（タグフィルタ設定付き）
- スライドショー（間隔設定可能）
- 非破壊回転（SQLite 保存）
- メタデータキーワード検索（Ctrl+F）
- 右クリックコンテキストメニュー
- 複数選択（Space / Ctrl+クリック）
- 同名ファイル処理設定

## ライセンス

[MIT License](LICENSE) — 無料でご利用いただけます。詳細は LICENSE ファイルを参照してください。
