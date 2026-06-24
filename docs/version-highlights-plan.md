# 更新後 初回起動の「重要な変更点」表示 実装計画

ステータス: **v2.0.0 で仕組み実装済み / v2.2.0 entry 追加済み / ClaudeCode レビュー済み** (2026-06-25)。2026-06-19 起案。

## 実装状況 (2026-06-25)

- データ + 選択ロジック: [src/version_highlights.rs](../src/version_highlights.rs)。
  `VersionHighlights { version, must_read, highlights }` の exe 埋め込みテーブル `table()` +
  純関数 `highlights_to_show(prev, current, table)` (unit test 12 本) + ヘルプ再表示用 `for_version` +
  `latest_not_newer_than` + 描画 `render(ui, entries)` (App 非依存 → snapshot から呼べる)。
- ダイアログ: [src/ui_dialogs/whats_new.rs](../src/ui_dialogs/whats_new.rs)。⚠ 必読 → ・ 新機能 →
  「すべての変更を見る」(changelog.html) / 閉じる。display-only。
- トリガ: `App` 構築時に `meta.previous_last_seen_version` と現行版で `highlights_to_show` を
  評価し、非空なら表示。`last_seen_version` は `Settings::load` で更新・保存済みなので一度きり
  (閉じなくても次回は出ない)。新規インストール (previous=None) は出さない。
- 開発用 `--whatsnew-from <ver>` で任意の前バージョンから強制表示 (`cli_flag_takes_value` 登録済)。
  ヘルプメニュー「重要な変更点を表示」で再表示。
- テスト: `highlights_to_show` の unit test + egui_kittest snapshot `whats_new_multi_version_dark`。
- **v2.0.0 の must_read = ツールバー右クリック化 + 左右クリック統一** (③ の告知)。Cargo.toml が
  2.0.0 になった初回起動で自動表示される。
- **v2.2.0 entry** として、`?` コンテキストヘルプ、キー割り当て表示の設定追従、
  環境設定「表示 → メニュー構成」を告知する。Cargo.toml が 2.2.0 に上がる前に entry を
  埋め込んでも、ヘルプメニューの再表示は現行版以下の最新 entry にだけフォールバックするため、
  開発中の 2.1.0 ビルドで v2.2.0 の告知を先取り表示しない。

## 0. 背景と目的

標準動作が変わる変更 (例: ツールバー左右クリックの規約変更、
[toolbar-customization-plan.md](toolbar-customization-plan.md)) をユーザーへ確実に伝えたい。
前回のマウス進む/戻るのような**バージョンごとの個別ダイアログを増やすと破綻**するので、
**更新後の初回起動で、そのバージョンの重要な変更点 (主要部分) を 1 画面で表示する汎用の
仕組み**にする。

`update_check` ([src/update_check.rs](../src/update_check.rs)) とは**別物**:

| | update_check (既存) | 本機能 (新規) |
| --- | --- | --- |
| タイミング | 更新**前** (新版が出たと気づかせる) | 更新**後**の初回起動 |
| 取得元 | ネットワーク (GitHub Releases、本文 8KB cap) | **exe 埋め込み (オフライン)** |
| 内容 | 全文 changelog | **操作・既定の変更を中心に主要部分だけ** |
| 届く相手 | 更新通知を見た人 | インストーラ/ポータブルで黙って更新した人にも確実に |

## 1. 方針 (確定 2026-06-19)

- **スコープ = 主要部分のみ**。README の更新履歴は詳細なので、本画面は**操作・既定の変更を
  中心に短く**。末尾に「すべての変更を見る」→ マニュアルの changelog.html へのリンク。
- **無効化設定は付けない** (毎回、重要事項だけを出す前提)。あとから**ヘルプメニューで再表示**は可。
- **表示のみ (display-only) で統一**。インタラクティブな移行選択 (前回のマウス進む/戻るのような
  二択 UI) は**新規には設けない**:
  - 既定が変わるが**旧動作が設定で残る**場合 → 本文に**その設定の場所を明記** (「従来の動作は
    設定 > X で選べます」)。
  - **強制変更** (旧動作なし) → 新動作の説明だけ。
  - ※ 既にリリース済みのマウス進む/戻る移行 (v1.8.0) は**そのまま**。本方針は今後の変更に適用。
    - **2 ダイアログ並走の確認 (2026-06-19)**: マウス移行プロンプトは `mouse_nav_prompt_done`
      フラグで**一度きり** ([src/settings.rs](../src/settings.rs) `mouse_nav_upgrade_prompt_pending`、
      4041 付近)。一度答えれば以後どのバージョンでも出ない。よって本機能 ④ を載せた版に
      更新したとき、マウス案内 + ④ の**2 つが並ぶのは「v1.8.0 を一度も起動せず、それ以前から
      直接ジャンプした少数のユーザー」のみ** (v1.8.0 を通った大多数は ④ のみ)。一回限りで漸減
      するため**このまま許容**。④ は display-only でマウスの選択 (標準 vs 従来) の代替には
      ならないので、この少数ケースで対話プロンプトが別途必要なのは構造上やむを得ない。
      ④ の highlight テーブルに **v1.8.0 のマウス変更を重複して載せない** (対話プロンプトが担当)。
  - 利点: 画面が純粋な情報表示になり、§5 のテスト容易性がさらに高まる。

## 2. トリガ (既存資産を流用、新規検出ロジック不要)

- 既存の `settings.last_seen_version` ([src/settings.rs](../src/settings.rs)) /
  `SettingsLoadMeta.previous_last_seen_version` / `version_changed` 判定をそのまま使う。
- `previous == None` (新規インストール) → **何も出さない** (新規ユーザーを「変更点」で迎えない)。
- `previous == current` → 出さない。
- `previous < current` → previous より新しく current 以下の**全バージョンの highlight を累積**して
  1 画面に表示する (バージョンを飛ばした人も、途中で入った重要変更を見逃さない)。

## 3. データ (exe 埋め込み)

- バージョン別 highlight を**ソースに埋め込む** (オフライン・バイナリ一致)。構造例:

  ```rust
  struct VersionHighlights {
      version: semver::Version,
      must_read: Vec<HighlightItem>,   // ⚠ 操作・既定の変更 (必読)
      highlights: Vec<HighlightItem>,  // 主な新機能 (任意)
  }
  struct HighlightItem { title: String, body: String } // 短文。内部用語は出さない
  ```

- `update_check` のネットワーク changelog とは別管理 (役割が違う)。
- authoring はリリース手順に 1 ステップ追加 (§9)。

## 4. 画面

- `egui::Window` + スクロール可能リスト (既存ダイアログの idiom)。
- 2 段構成: **⚠ 操作・既定の変更 (必読)** → **主な新機能 (任意)** → 「すべての変更を見る」リンク。
- バージョンをまたぐ場合はバージョン見出しごとに項目を並べる。
- 閉じる/OK の 1 ボタン。閉じたら `last_seen_version` 更新済みなので次回は出ない
  (既存のバージョン記録更新に乗る)。

## 5. テスト容易性 (← 最重要。実機テストを最小化する設計)

「複数バージョンまたぎは実機で再現しにくい / 実機に時間をかけられない」懸念に対し、
リスクをほぼ全部 CI 側へ寄せる:

- **選択ロジックを純関数化**: `highlights_to_show(prev: Option<&Version>, current: &Version,
  table: &[VersionHighlights]) -> Vec<&VersionHighlights>`。**またぎ累積の核心**をここに集約し、
  unit test で網羅 (None / 同一 / 1 段 / 多段スキップ / 降格 / parse 不能 / 空テーブル)。**実機ゼロ**。
- **fail-safe**: parse 不能や空でも**黙ってスキップ** (起動を絶対止めない)。`update_check` の
  silent-fail と同じ哲学。unit test で担保。
- **描画は egui_kittest スナップショット**: 合成の多バージョン payload を食わせて
  [tests/ui_snapshot.rs](../tests/ui_snapshot.rs) に 1 本追加。レイアウト崩れを**実機なし**で検知。
- **開発用の強制表示フラグ** `--whatsnew-from <version>` (または env): 任意の「前バージョン」から
  画面を即表示。**ダウングレード/再インストール不要**で、どのまたぎ組合せも目視できる。
- **残る実機 smoke は 1 点だけ**: バージョンを上げた初回に 1 回出て閉じれること。マトリクス不要。

## 6. 既存資産との接続

| 必要な処理 | 流用元 |
| --- | --- |
| バージョン変更検出 / 前バージョン取得 | `last_seen_version` / `previous_last_seen_version` / `version_changed` ([src/settings.rs](../src/settings.rs)) |
| ダイアログ (Window + ScrollArea) | 既存ダイアログ ([src/ui_dialogs/update_notice.rs](../src/ui_dialogs/update_notice.rs) ほか) の idiom |
| ヘルプメニューからの再表示 | 既存メニュー構造 |
| 「すべての変更を見る」リンク | マニュアル changelog.html ([src/external_links.rs](../src/external_links.rs)) |

## 7. 他計画との関係

- **ツールバー左右クリック規約の変更告知**はこの仕組みで行う
  ([toolbar-customization-plan.md](toolbar-customization-plan.md))。強制変更なので display-only で
  「新しい規約の説明 + カスタマイズは右クリック/⚙ から」を出す。
- 今後の標準動作変更は全部この器に集約 (個別ダイアログを増やさない)。

## 8. 永続データへの影響
- **追加ストアなし** (`last_seen_version` は既存)。highlight はコード埋め込み。
  マイグレーション不要。

## 9. リリース手順への追加
- [CLAUDE.md] のリリースチェックリスト (Phase 1 付近) に「version highlight テーブルに
  このバージョンの主要/操作変更を追記する」を 1 ステップ追加 (authoring 忘れ防止)。

## 10. ドキュメント同時更新 (実装時)
- [spec.md](spec.md) / [architecture-overview.md](architecture-overview.md) /
  htdocs マニュアル (ヘルプから再表示できる旨。内部用語を出さない方針)。
