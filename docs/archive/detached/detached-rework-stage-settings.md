# Stage SETTINGS: ビューワモード設定の再構成 (ユーザー決定 2026-07-07)

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md) — §2 (憲法) を先に読むこと。
目的: 全体設定の「ZIP/PDF/対応アーカイブ」「画像ビューア」2 セクションを
「ビューワモード」1 構造に再構成し、**複数ウィンドウモードでは ZIP/PDF/本を常に
直開き (ページ一覧を経由しない) に固定**する。検証パターンの削減 (ON×ページ一覧の
組み合わせを設計から削除) が狙い。

## 1. 新しい設定 UI (ユーザー指定の構造・文言をベースにする)

現在の `page_*` (src/ui_dialogs/preferences/pages.rs の「ZIP/PDF/対応アーカイブ」radio +
「画像ビューア」checkbox) を以下に置き換える:

```
ビューワモード

◯フル機能ウィンドウ（編集機能あり）
　フル機能を使えますが、画像/動画はメインウィンドウまたは１つの別ウィンドウ表示
　(F12で切り替え)します。フルスクリーンへの切り替え(F11)も可能です。

　本の表示モード
　◯開いたとき、ページ一覧を表示
　◯開いたとき、ページを表示（１ページ目・続きはライブラリ・履歴と復元から設定可能）
　　□画像のみのフォルダは、PDF/ZIPのように本として扱う

◯複数ウィンドウ（編集機能なし）
　画像を開くたびに、新しいウィンドウで開きます。閲覧中心の方のためのモードです。
　動画/音声は１つのメディアウィンドウで再生します。フルスクリーンへの切り替え(F11)も可能です。

　□画像のみのフォルダは、PDF/ZIPのように本として扱う
```

- ビューワモードの radio = `detached_viewer_open_images_in_window`
  (フル機能 = false / 複数ウィンドウ = true)。
- 本の表示モードの radio = `auto_fullscreen_zip_pdf` (ページ一覧 = false /
  ページを表示 = true)。**フル機能側のサブ項目としてのみ表示** (複数ウィンドウ
  選択時は disabled または非表示。disabled 推奨 = 設定値が保持されることが見える)。
- 「画像のみのフォルダは…」checkbox = 既存 `auto_fullscreen_image_folders` を
  **両モードのサブ項目として同じ bool にバインドして 2 箇所に表示**する。
  enabled 条件: フル機能側 = (mode=フル機能 かつ ページを表示 選択時) /
  複数ウィンドウ側 = (mode=複数ウィンドウ選択時)。
- モード切替時の「開いている別ウィンドウは自動で閉じます」注記と toast 挙動
  (CUT §6) は維持する。
- ⚠ 文言メモ: ユーザー原案は「動画/音声は１つのウィンドウに表示されます」だが、
  音声ファイルの detached は Stage AUDIO で解除し、動画と同じメディアウィンドウへ合流する。**実挙動に
  合わせて上記のとおり「動画は…」とし、音声には言及しない**。将来音声 detached を
  解除したステージで文言を更新する。

## 2. 保存キーの方針 (マイグレーション不要の根拠付き)

| キー | リリース状況 | 方針 |
| --- | --- | --- |
| `auto_fullscreen_zip_pdf` | **v2.2.0 リリース済み** (git show v2.2.0 で存在確認済み) | キーと保存値の意味は**変えない** (フル機能モードでの意味は従来どおり)。UI 上の見せ方だけ変更 |
| `auto_fullscreen_image_folders` | v2.2.0 リリース済み | 同上 |
| `detached_viewer_open_images_in_window` | 未リリース (v2.2.0 に存在しない、リワーク中に導入) | そのまま radio にバインド |

新キーの追加・既存キーのリネームは行わない。スキーマ変更ゼロ。

## 3. 挙動変更: 複数ウィンドウモードは常に直開き

- `Settings` に実効値ヘルパーを追加する:
  ```rust
  pub fn effective_auto_fullscreen_zip_pdf(&self) -> bool {
      self.detached_viewer_open_images_in_window || self.auto_fullscreen_zip_pdf
  }
  ```
  `auto_fullscreen_image_folders_enabled()` も
  `self.effective_auto_fullscreen_zip_pdf() && self.auto_fullscreen_image_folders`
  に変更する。
- **`auto_fullscreen_zip_pdf` の全読み取りサイトを実効値ヘルパー経由に置換**する
  (grep で直接読みが設定 UI 以外に残っていないことを完了条件にする)。
  保存値は書き換えない (モードを フル機能 に戻したときにユーザーの
  ページ一覧/直開き選択が復元される。決定性・非破壊)。
- 対象は「開いたときの初期挙動」のみ。**Backspace でページ一覧へ戻る等の
  ナビゲーション経路は変更しない** (findings-7 の detached book 経路もそのまま)。

## 4. テスト

- findings-7 (519f8faa) の 4 象限 (mode ON/OFF × 直開き ON/OFF) テストのうち、
  **ON×直開き OFF の象限は「ON では強制直開き」の新仕様に合わせて書き換えてよい**
  (憲法 8 の明示リスト。これ以外の既存 detached テストは弱体化しない)。
- 新テスト: `effective_auto_fullscreen_zip_pdf` の真理値表 (mode ON なら保存値に
  よらず true / OFF なら保存値どおり) + 保存値がモード切替で変異しないこと。

## 5. ドキュメント / マニュアル同時更新

- `htdocs/mimageviewer/manual/settings.html` — 新しい設定構造の説明に差し替え
  (バージョン表記・内部用語なし。「ビューワモード」「フル機能ウィンドウ」
  「複数ウィンドウ」のユーザー向け説明)。
- `htdocs/mimageviewer/manual/fullscreen.html` — 直開き関連の記述があれば整合させる。
- `docs/spec.md` — 設定項目の変更を反映。
- `docs/detached-viewer-implementation-plan.md` — モード×直開きの分岐表があれば
  「複数ウィンドウ = 強制直開き」に更新。
- [ship-checklist](../../detached-rework-ship-checklist.md) §1 と
  [smoke-matrix](detached-viewer-smoke-matrix-20260630.md) §2 の設定セット表:
  S3 の「ZIP/PDF 直開き」列は強制 ON になる旨の注記を追記 (S2 = フル機能×ページ一覧は
  存続)。

## 6. 完了条件

- [ ] UI 再構成 (§1 の構造・文言。既存の説明文体・weak テキストの流儀に合わせる)
- [ ] 実効値ヘルパー + 全読み取りサイト置換 (設定 UI 以外の
      `auto_fullscreen_zip_pdf` 直接読み grep 0 件)
- [ ] §4 のテスト。既存テスト + full test 緑
- [ ] §5 のドキュメント更新
- [ ] `cargo fmt --check` / `python scripts/check_ui_glyphs.py` /
      `.\scripts\build-release.ps1`
- [ ] コミットは `(detached-rework stage-settings)` を含める。UI 文言変更で
      egui_kittest スナップショットが赤くなる場合は docs/ui-snapshot-policy.md の
      手順で更新し PNG を目視確認してからコミット

## 7. 実機確認 (ユーザー、次回チェックリストと合流)

1. 設定画面が §1 の構造で表示され、モード切替で全窓クローズ + toast (従来どおり)
2. 複数ウィンドウモード: ZIP/PDF/画像フォルダ (checkbox ON 時) を開くと常に直開き
   (ページ一覧を経由しない)
3. フル機能モード: 本の表示モードの選択が従来どおり効く。モードを往復しても
   ページ一覧/直開きの選択が保持されている
4. ship-checklist の S2/S3 ケースが新仕様の記述と一致
