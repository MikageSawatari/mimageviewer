# UI スナップショットテスト運用方針

v0.7.0 で導入した `egui_kittest` ベースの UI 回帰テストの運用ルール。

## 目的

egui の描画結果を PNG スナップショットとして保存し、意図しない見た目の変化を
`cargo test` の段階で検出する。「ダイアログの色が変わった」「ラベルが折り返して
レイアウトが崩れた」「テーマ切替で文字が読めない配色になった」などを早期にキャッチする。

**目的外の用途:**

- 機能テスト (値の正しさ、ロジックの挙動) → 通常のユニット/統合テストで行う
- パフォーマンステスト → `bench_scroll` 等のベンチマークで行う
- E2E シナリオテスト → `docs/e2e-smoke-test.md` の Computer Use 手動実行で行う

## 実行

```bash
# 通常実行 (既存スナップショットと比較)
cargo test --test ui_snapshot

# 意図的に見た目を変更した場合のスナップショット更新
UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot
```

更新後は `tests/snapshots/*.png` の差分を必ず目視確認してからコミットする。
git diff のバイナリ比較では変化の意図 (改善なのか回帰なのか) は判断できない。

## ディレクトリ構成

```
tests/
├── ui_snapshot.rs              # 全スナップショットテストの定義
└── snapshots/                  # 期待スナップショット PNG (コミット対象)
    ├── smoke_label_and_button_light.png
    ├── smoke_label_and_button_dark.png
    ├── susie_diagnostic_*.png  # Susie プラグイン診断 UI
    └── ...
```

## 設計方針

### 1. 再利用可能な UI 関数として切り出したものを対象にする

`App` を丸ごと構築するスナップショットは書かない (状態の組み合わせ爆発でメンテが
破綻する)。代わりに、以下のような純粋な描画関数を対象にする:

- `ui_susie_diagnostic::render_diagnostic(ui, status, plugins)`
- 必要に応じて `ui_helpers` などから切り出した整形系関数

UI コードをスナップショットしたい場合は、まずロジックを `fn foo(ui, args) -> ()`
の形に切り出してから、その関数を `Harness::builder().build_ui(|ui| foo(ui, ...))`
でラップするのが基本パターン。

### 2. ハーネスは `tests/ui_snapshot.rs` 内のヘルパーを使う

- `snapshot_with_theme(name, theme, build_ui)`: 任意 UI + テーマ指定
- `snapshot_diagnostic(name, status, plugins)`: Susie 診断専用

これらはすべて `install_japanese_font()` を呼んで、本体と同じ日本語フォント
(YuGothM / meiryo / msgothic) を登録する。豆腐化したスナップショットは
「描画が崩れている」のか「フォントが無いだけ」のか区別できないため常に登録する。

### 3. サイズは固定

デフォルトは 480 × 360 px。理由:

- サイズ可変だと差分検出が過敏になる (描画誤差の累積)
- 小さすぎるとラベルが折り返しで消えて検出不能になる
- 大きすぎると PNG が肥大化してリポジトリが重くなる

### 4. 文字列や色のリテラルはフィクスチャに集約

複数テストで共通するプラグイン情報などは `*_fixture()` ヘルパー関数に切り出す。
同じ入力・別テーマのようなパターンで一貫性を保つ。

## 新しいスナップショットテストを追加する手順

1. 対象の UI コードが `fn(ui: &mut egui::Ui, ...)` の形になっていることを確認。
   そうでなければ先に `ui_*` モジュールに切り出す (ui_susie_diagnostic.rs を参考に)。
2. `tests/ui_snapshot.rs` に新しい `#[test]` を追加。
3. `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot` で PNG を生成。
4. 生成された PNG を目視で確認 (想定通りの描画か)。
5. PNG と `.rs` の変更を一緒にコミット。

## リポジトリサイズ管理

PNG は現状 1 ファイル 5〜10 KB 程度。100 ファイル程度までは許容する想定。
肥大化してきた場合:

- サイズを縮小 (例: 480×240 に変更)
- 冗長なテーマ差し替え分の統合 (Light / Dark のうち代表 1 枚のみ残す)
- git LFS 化 (最終手段)

## CI での扱い

現時点では CI 環境を持たないためローカル実行のみ。将来 GitHub Actions 等で
自動化する場合、Windows runner が必要 (本体と同じ日本語フォントを登録するため
`C:\Windows\Fonts\*.ttc` を参照している)。Linux runner に移行する際は、
CJK フォントを自前で vendored するか `Noto Sans CJK` をインストールする必要がある。

## 既知の制限

- **フルスクリーンビューポート** (eframe の `show_viewport_immediate`) の単体
  スナップショットは現状対応外。メインビューポートと別管理のためハーネスで
  扱いづらい。必要になったら `Harness` の `run_steps` を複数サイクル回す方式で
  試す。
- **画像コンテンツ**を描画する UI (グリッドセル等) は、画像パスが通らないため
  スナップショット対象外。モックテクスチャを渡すテストは将来的に検討。
