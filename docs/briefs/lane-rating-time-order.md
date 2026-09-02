# レーン 1: 一覧の時刻ソートと、編集の一括操作

v3.5.0 の並行レーンの 1 本。**このブリーフを最初に読み、そのうえで
[docs/README.md](../README.md) から該当領域の設計ドキュメントを開くこと。**

## 作業ツリーとブランチ

- 作業ツリー: `C:\home\mimageviewer-r2e` (このツリー)
- ブランチ: `rating-time-order` (master `ea233160` から分岐)
- **master へ merge しない。** master では別セッションがリリース作業中。
  区切りごとにこのブランチへコミットし、完了したら報告するところまでが担当。
- 他の worktree (`-extlaunch` / `-pano` / `-video-strip` / `-export`) と
  `C:\home\mimageviewer` のファイルは**読むのも書くのも行わない**。
- `git worktree remove` を使わない (junction 再帰削除の事故があるため)。

## 担当する項目 (この順で)

正本は [docs/next-release-backlog.md](../next-release-backlog.md) の各節。**着手前に必ず
その節を読む** (ここは要約であって正本ではない)。

### 1. §1.142 ★時刻ソートを選んでもカテゴリ再配置が並びを組み替える (最優先)

- ソート自体は正しい。壊しているのは直後の `grid_item::arrange_grid_items` 再配置。
  `app.rs` の `install_rating_view_rows` (21279 行付近) と
  `install_bookmark_view_rows` (30269 行付近) の 2 か所が同型。**両方直す。**
- 方針は利用者と合意済み: **時刻ソート
  (`RatingViewSort::RatedAtDesc/Asc`・`BookmarkViewSort::CreatedAtDesc/Asc`) の間は
  再配置を通さない。** `Normal(SortOrder)` は従来どおり据え置く。
- **やらないこと**: 再配置の後ろに「フォルダを後方へ動かす」並べ直しを足さない。
  `grid_display_order` の意味も通常フォルダの挙動も変えない。分岐 1 つで足りる。
- 閲覧履歴 (`install_reading_history_entries`) と mIV Remote
  ([remote_ipc/collections.rs](../../src/remote_ipc/collections.rs)) は元から再配置しない。
  直すと本体とリモートの並びが一致する側へ寄る。**リモート側を変える必要があるかを確認し、
  無ければ触らない。**
- テスト: rating / bookmark それぞれで「時刻ソート時の行順が `sort_rows` の結果と一致」
  「`Normal` では従来どおりカテゴリ順」を純関数レベルで固定。フォルダ / アーカイブ / 画像を
  混ぜ、**時刻 NULL の行を 1 件入れる**。

### 2. §1.143 詳細表示に★設定時刻が無く、列ソートが★時刻順を黙って上書きする

2 つある。**(a) を先に、(b) は後述の「A 待ち」に該当する。**

- **(a) 列ソートがビュー固有ソートに勝つ**: `rebuild_details_order` (`app.rs` 47124 行付近)
  の除外が「本として表示中」と「閲覧履歴」だけ。加えて、ツールバーは
  `details_header_sort_active` で無効化されるのに、[ui_main.rs](../../src/ui_main.rs) の
  メニュー「ソート順」は同じ述語を見ておらず選べてしまう。**まず 2 つの入口を同じ述語に揃える。**
- **(b) ★設定時刻の列と行**: `DetailsColumnId` / `DetailsSortKey::RatedAt` の追加、
  `selection_info_content` へ 1 行 (ブックマークの「登録日時」・閲覧履歴の「最終閲覧」が前例)。
  列と sort key は**レーティング一覧の中だけ**に出し、ビューを抜けるとき選択中なら
  `Toolbar` へ戻す。通常フォルダに常に空の列を残さない。
- **データの現実**: 実環境の `rating.db` は ★2 が 7017 行中 6087 行 (87%) で `rated_at_ms`
  が NULL。空欄が大量に出る前提で、空欄表示と「NULL は末尾」規約を明示する。

### 3. §1.150 + §1.151 編集内容の一括貼り付け / 編集内容のリセット (A 着地後に配線)

- 決めることが共通なので 2 件まとめて設計する: 対象集合の作り方 / 途中失敗を残すか戻すか
  (トーストで「成功 N 件 / 失敗 M 件」が自然) / 対象外 (動画・音声・フォルダ) の扱い /
  Undo に含めるか。
- §1.151 は**何を消すかを決めるのが本体**。編集は 7 種類 (補正 / 消しゴム / モザイク /
  補正レイヤー / 注釈 / 切り取り / 回転)。**★ とタグは含めない。**
  モーダル確認を必須にし、「何件の、どの種類を消すか」を出す。
- 入口は現状 [context_menu.rs](../../src/ui_dialogs/context_menu.rs) の 3 か所
  (1078 / 1476 / 1940)。**同型の入口をすべて塞ぐ** (片方だけ直すと経路で挙動が変わる)。
- **`context_menu.rs` はレーン A が全面書き換え中** (下記)。エンジン (選択集合 → N 件適用 →
  集計) を先に作り、**メニュー配線は A が master へ着地してから**行う。

## 共有登録簿 — A が着地するまで触らない

レーン A (`external-tool-launch` worktree、右クリックメニューと外部ツール起動) が、
以下を全面的に書き換えている。**先に触ると解決不能なコンフリクトになる。**

| 場所 | 状況 |
| --- | --- |
| `src/ui_dialogs/context_menu.rs` + 新設 `context_menu_model.rs` | 全面書き換え (1746 行) |
| `src/ui_dialogs/preferences.rs` / `preferences/pages.rs` | ページ追加 |
| `src/settings.rs` / `src/settings_db.rs` | 旧 `custom_open_with_apps` の廃止と移行 |
| `src/keymap.rs` / `docs/keymap.ini.default` | action 追加 |

このレーンで該当するのは **§1.143(b) の `DetailsColumnId` / `DetailsSortKey` 追加**と
**§1.150/§1.151 のメニュー配線**。どうしても先に必要なら、**そのレーンの最後に専用コミット
1 本**へまとめ、機械的なコンフリクト解決で済む形にしておくこと。

`src/app.rs` は A も触るが、A のハンクは 10701–15786 / 26212 / 60220–60549 / 66930–67594 に
限られる。このレーンの対象 (21279 / 30269 / 47124) とは**重ならない**。
`src/ui_main.rs` は A も触るが領域が違う (A は右クリック / メニュー登録)。

## 進め方

- 修正前に、観測された失敗・守るべき不変条件・違反を作った経路を特定する。症状を消す
  guard / delay / retry / 一括 reset / silent fallback を根本原因の代わりに置かない
  (CLAUDE.md「バグ修正の一般原則」)。
- 同じ状態の producer / consumer を数えてから直す。§1.142 が rating と bookmark の
  2 か所なのはその一例。
- 実装を Codex へ出すなら**出す前にコミットする** (未検証の差分が混ざると切り分け不能)。
  **1 worktree につき Codex は 1 本まで。**
- コミット前に `cargo fmt` (引数なし・ワークスペース全体)。
- テストは最小ターゲットで回す: `cargo test -p mimageviewer --lib <filter>`。
  全体は区切りでだけ (`.\scripts\test-full.ps1`)。

## 実機確認の頼み方

`.\scripts\build-dev.ps1` を回し、
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` を利用者へ渡す。
**エージェント自身は起動しない** (実利用中の `%APPDATA%\mimageviewer` を触るため)。
確認シナリオを具体的に書き、起動前にインストール版 / 常駐 tray 版を終了してもらう旨を添える。

**実機確認は利用者 1 人しかいない直列資源で、いま 4 レーンが並行している。**
細かく何度も頼まず、区切りでまとめて 1 回にする。

## 他のレーン (参考)

| レーン | ツリー | 中身 |
| --- | --- | --- |
| A | `-extlaunch` | 外部ツール起動 §1.117 (進行中) |
| 1 | **`-r2e` (ここ)** | §1.142 → §1.143 → §1.150/§1.151 |
| 2 | `-pano` | 表示ジオメトリの丸め §1.161/§1.159/§1.154 → 右パネルロック §1.158 / 360 §1.145 |
| 3 | `-video-strip` | 動画シークストリップ §1.155 |
| 4 | `-export` | エクスポート §1.144 → §1.148 |
