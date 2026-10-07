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

詳細一覧の名前色 (§1.346) は `details_name_colors_*` と `details_name_color_settings_*` の
Light／Dark × 標準／強い、計8枚を保持する。前者は六分類・長い日本語名・hover・選択・チェック・
切り取り・選択情報バー・カスタム・OFF、後者は本体と共通の環境設定表・比・警告・強い固定色を描く。
`cut_item_appearance_light/dark` は名前が不透明、他列とプレビューが薄い境界を合わせて確認する。
名前色の追加8枚は合計1,133,075 bytes (約1.08 MiB)。一覧620×580、設定表760×800で全項目を収める。
`preferences_favorite_view_state_dark` は同じページ全体を縦スクロールするため、名前色表を追加すると
スクロールバーのつまみが短くなる。既存本文が変わらないことを差分画像で確認して期待画像を更新する。

## ディレクトリ構成

```
tests/
├── ui_snapshot.rs              # 全スナップショットテストの定義
└── snapshots/                  # 期待スナップショット PNG (コミット対象)
    ├── smoke_label_and_button_light.png
    ├── smoke_label_and_button_dark.png
    ├── susie_diagnostic_*.png    # Susie プラグイン診断 UI
    ├── changelog_markdown_*.png  # 更新履歴 Markdown の整形描画
    └── ...
```

## 設計方針

### 1. 再利用可能な UI 関数として切り出したものを対象にする

`App` を丸ごと構築するスナップショットは書かない (状態の組み合わせ爆発でメンテが
破綻する)。代わりに、以下のような純粋な描画関数を対象にする:

- `ui_susie_diagnostic::render_diagnostic(ui, status, plugins)`
- `changelog_markdown::render(ui, body)` — 更新履歴 Markdown サブセットの整形描画
- 必要に応じて `ui_helpers` などから切り出した整形系関数

UI コードをスナップショットしたい場合は、まずロジックを `fn foo(ui, args) -> ()`
の形に切り出してから、その関数を `Harness::builder().build_ui(|ui| foo(ui, ...))`
でラップするのが基本パターン。

### 2. ハーネスは `tests/ui_snapshot.rs` 内のヘルパーを使う

- `snapshot_with_theme(name, theme, build_ui)`: 任意 UI + テーマ指定（文字コントラストは標準）
- `snapshot_with_theme_and_contrast(name, theme, contrast, build_ui)`: 任意 UI + テーマ + 文字コントラスト指定
- `snapshot_with_ui_font(name, settings, build_ui)` (Windows): 任意 UI フォント + 縦位置補正指定
- `snapshot_diagnostic(name, status, plugins)`: Susie 診断専用

これらはすべて `install_app_fonts()` を呼んで、本体と同じ `ui_fonts::configure_fonts()`
のフォント fallback (YuGothM / meiryo / msgothic + 記号・絵文字補完) を登録する。
豆腐化したスナップショットは「描画が崩れている」のか「フォントが無いだけ」のか
区別できないため常に登録する。

通常文字・薄い文字・ボタン状態の配色を変える場合は、Light / Dark の標準 snapshot に加えて
`TextContrast::Strong` の Light / Dark snapshot も更新し、背景色まで意図せず変わっていないことを
目視確認する。

UI フォント経路を変更する場合は `custom_ui_font_meiryo_bold_alignment_dark` に加え、
`recommended_ui_font_*_alignment_dark` (BIZ UDPGothic 9pt / Meiryo 10pt /
Meiryo UI 10pt) も更新し、通常文字、ツールバー専用 family、日本語 / CJK / 記号
fallback の縦位置を確認する。9/10pt は 96 DPI の typographic point を egui logical
point へ換算して描画する。同じ snapshot の「動画・音声 HUD」は `x1`、`Norm`、`0.0dB`、
再生時間を `miv-hud-text` で描き、選択フォントを変えても字面・位置が変わらないことを確認する。

### 3. サイズは固定

デフォルトは 480 × 360 px。理由:

- サイズ可変だと差分検出が過敏になる (描画誤差の累積)
- 小さすぎるとラベルが折り返しで消えて検出不能になる
- 大きすぎると PNG が肥大化してリポジトリが重くなる

起動時ダイアログの小画面回帰は例外として **1093×614 / 1366×728 logical points** を使う。
`startup_dialogs_small_viewport` は App を構築せず、本体と共有する Modal/Window 描画関数を
固定フィクスチャで呼び、本体のUI倍率100%/200%を固定画面へ適用する。
27状態×2サイズ×2倍率の108ケースすべてで、固定操作行の全ボタンとタイトル×を、
viewport/clip内の矩形・hover・mouse clickで検査する。到達性はこのgeometry検査で保証する。
Remote reader再接続、起動時書庫変換の各phase、disabled変換も含む。
共通本文の別の回帰検査で、小/大画面の100%/200%における長文の本文高（viewport高の45%以上）と
短文の自然高への縮小も確認する。操作ボタンだけ可視でも、本文が不必要に小さければ失敗する。
幅200ptへ縮めたWindowの長い操作ボタンと復元一覧の説明の折り返しも、100%/200%で
固定操作のviewport/clipを検査する。本文高は縮小後の実割当高を測り、操作行を含めない。
PNGは初回設定/書庫変換確認の1093×614・100%/200%の代表4枚と、
意図的に更新した変更点の100%・2サイズの2枚だけを保持する。
ほかの状態/サイズ/倍率にはsnapshot呼び出しもbaselineも増やさず、geometry検査を維持する。
対象と除外を [起動時画面の監査](startup-dialog-small-screen.md) に列挙する。
長いエラー文・ネットワークパス・全バージョンの累積告知を含める。
初回設定のクリック到達性と IME-safe Enter / Esc は `--lib first_setup` の回帰テストで検査する。
表示後の縮小/拡大は、同じContextを1920×1440→1093×614→1920×1440→1093×614と変更し、
全27状態の固定操作とタイトル×を最大2frame後に検査する。初回設定は本体フォントで見出しも
検査し、最終layout pass内でResponseを採取する。Context::run終了後のread_responseはdiscard
されたpassのwidgetを優先し得るため、過去passの矩形を現在の描画として検査しない。
通常の2passでは縮小したframe内に収まり、pass予算1の検査は強制frameを足さずrepaint scheduler
だけで再配置/hover/clickへ到達することを確認する。追加PNGは作らず代表6枚の比較を維持する。
実アプリや認定端末での観測とは区別する。ダイアログの設計要件は CLAUDE.md の
「ダイアログ (egui::Window)」節を参照。

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

PNG の容量は画面サイズや文字量によって変わるため、追加時に枚数と合計容量を確認する。
画面サイズ・倍率・状態の組合せはgeometry/操作の検査を中心にし、PNGは見た目を確認する
少数の代表画面へ絞る。起動時ダイアログは上記6枚を保持し、108条件へPNGを増やさない。
肥大化してきた場合:

- サイズを縮小 (例: 480×240 に変更)
- 冗長なテーマ差し替え分の統合 (Light / Dark のうち代表 1 枚のみ残す)
- git LFS 化 (最終手段)

## CI での扱い

現時点では CI 環境を持たないためローカル実行のみ。将来 GitHub Actions 等で
自動化する場合、Windows runner が必要 (本体と同じ Windows フォント fallback を登録するため
`C:\Windows\Fonts\*.ttc` を参照している)。Linux runner に移行する際は、
CJK フォントを自前で vendored するか `Noto Sans CJK` をインストールする必要がある。

## 既知の制限

- **フルスクリーンビューポート** (eframe の `show_viewport_immediate`) の単体
  スナップショットは現状対応外。メインビューポートと別管理のためハーネスで
  扱いづらい。必要になったら `Harness` の `run_steps` を複数サイクル回す方式で
  試す。
- **実ファイルの読み込みを伴う画像 UI** (グリッドセル等) は、この純描画 harness の対象外。
  読み込みを行わず固定の生成 texture を渡す純描画は検証できる。別バージョンの一時表示は
  `src/ui_fullscreen.rs::tests::similar_preview_navigator_snapshot_dark` で geometry と navigator
  paint を共有し、本文の clip と可視範囲枠を確認する。App・worker・native GPU の検証とは区別する。

## integration test と lib unit test の使い分け

`tests/ui_snapshot.rs` は外部の integration test なので、lib crate が公開する
`mimageviewer::*` の pub API だけを利用できる。現在の `src/lib.rs` は実際の `app`
module を含み、実行ファイルは薄い入口である。ただし `app` module は非公開で、
`App::draw_local_adjust_*` のような `pub(crate)` 関数は integration test から直接呼べない。

crate 内部の純粋な UI 描画をスナップショットしたいときは、**lib unit test
(`src/<module>.rs` 内 `#[cfg(test)] mod`) の中で直接 `egui_kittest::Harness` を使う**。
これは App 全体を構築する snapshot を許可するものではなく、対象は上記の純粋な描画関数とする:

```rust
// src/ui_adjustment_panel.rs::local_adjust_segmentation_tests 内
#[test]
fn local_adjust_panel_snapshot_empty_layer_list() {
    use egui_kittest::Harness;
    let mut fonts_ready = false;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(280.0, 200.0))
        .build(move |ctx| {
            crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
            if !fonts_ready {
                crate::ui_fonts::configure_fonts(ctx);
                fonts_ready = true;
                ctx.request_repaint();
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                super::draw_local_adjust_layer_list(ui, /* args... */);
            });
        });
    harness.run();
    harness.snapshot("local_adjust_panel_empty_layer_list");
}
```

スナップショットは **同じ `tests/snapshots/<name>.png`** に保存される (integration test と
lib unit test でディレクトリ共有)。**`UPDATE_SNAPSHOTS=1`** も同じ:

```bash
UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --lib local_adjust_panel_snapshot
```

実例: `src/ui_adjustment_panel.rs::local_adjust_segmentation_tests::local_adjust_panel_snapshot_*`
(5 件、補正レイヤーパネル + レイヤーリスト UI)。
