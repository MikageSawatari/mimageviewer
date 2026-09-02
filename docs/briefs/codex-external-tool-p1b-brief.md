# ブリーフ: 外部ツール起動 P1b — 右クリックへの差し込みと旧 UI の載せ替え

正本: [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md) の §4.9 と §3.1。
P0 (型・DB・移行) と P1 (設定ページ・引数展開・起動) は実装済み。この phase で
**利用者が普段の右クリックから外部ツールを起動できる**状態にする。

作業場所: この worktree (`C:\home\mimageviewer-extlaunch`, ブランチ `external-tool-launch`)。

## 0. P1b の範囲

**やること**

1. ネイティブコンテキストメニューの mIV 差し込み領域へ、外部ツールを平坦に足す
2. mIV 独自 (フォールバック) メニューの「アプリケーションで開く…」を外部ツールへ載せ替える
3. 旧 `custom_open_with_apps` への書き込みを止める (載せ替えたので不要になる)

**やらないこと** (後続 phase)

- 複数選択 (`SelectionPolicy`) の適用 (P2)
- キー割り当て・ツールバー (P2)
- 仮想ページ / 動画フレームの実体化 (P3 / P4)

**対象は右クリックした実項目 1 件だけ。** 複数選択されていても、この phase では
右クリックした項目を対象にする (P2 で `checked` 優先へ広げる)。

## 1. ネイティブメニューへの差し込み

**差し込み口は既にある。** [`native_context_menu.rs:387`](../../src/native_context_menu.rs:387) が
`request.miv_items` を `AppendMenuW` で先頭へ積み、セパレータを挟んで Shell 項目を続けている。
足りないのは `NativeMivCommand` に外部ツールを表す variant が無いことだけ
([native_context_menu.rs:38](../../src/native_context_menu.rs:38))。

- `NativeMivCommand` に `ExternalTool(ExternalToolId)` を足す。この enum は `Copy` なので、
  `ExternalToolId` が `Copy` な newtype であることを前提にしてよい (P0 でそうなっている)。
- `miv_items` を組み立てている箇所 ([context_menu.rs:1387](../../src/ui_dialogs/context_menu.rs:1387) 付近) で、
  `show_in_context_menu` が立っているツールを**登録順に**積む。ラベルは `display_name()`。
- **既存の mIV 項目との間にセパレータを 1 本入れる。**
- **無効なツールは積まない** (右クリックが伸びるのを避ける)。この phase で無効になるのは
  「対象が仮想ページ (ZipImage / PdfPage / Stack)」と「`payload` が `RealFileOnly` でない対象」。
  **例外は編集用ツール** (`for_editing`) で、こちらは `MF_GRAYED` + 理由のツールチップで出す
  (正本 §4.8 / §4.9)。理由の文面は「圧縮ファイル内のページは編集用ツールで開けません。
  書き出してから編集してください (フルスクリーンで Ctrl+E)」。
- **表示時に打ち切らない。** `show_in_context_menu` が立っているツールは全部積む。
  利用者の要望は「設定した数のアプリだけメニューを追加する」なので、設定した物が
  出ないのは期待に反する。**件数の抑制は登録時の既定値だけで行う** (正本 §4.9):
  新規登録時の `show_in_context_menu` は ON、ただし**既に ON の総数が 10 を超えている
  場合だけ OFF で追加**する。この判定は設定ページ側 (P1 で入った追加経路) に置く。
- `NativeMivCommand::ExternalTool(id)` を受けたら、`id` からツールを引き、
  P1 の `queue_external_tool_launch` を呼ぶ。**ID が見つからない場合は黙って無視せず通知する**
  (メニュー構築と実行の間に設定が変わった場合)。

**メニュー項目の組み立ては純関数に切り出してテストする。** 入力 = ツール一覧 + 対象の種別、
出力 = 積む項目の一覧 (ラベル・有効/無効・理由)。Win32 呼び出しと混ぜない。

## 2. フォールバックメニューの載せ替え

[`context_menu.rs:2037`](../../src/ui_dialogs/context_menu.rs:2037) の「アプリケーションで開く…」を次にする。

- 「登録アプリ」の一覧を `settings.custom_open_with_apps` から **`settings.external_tools`** へ変える。
  クリックしたら `queue_external_tool_launch` を呼ぶ (引数テンプレートが効く)。
- 「参照…」からの登録先も `external_tools` へ変える。`ExternalTool::defaults_for_viewing()` +
  `next_id()` を使い、`name` は選んだ EXE の file_stem、`show_in_context_menu` は true。
- **「最近使ったアプリ」と「関連付けアプリ」の 2 群はそのまま残す。** これは履歴と OS 由来の
  一覧であって、登録ツールではない。`recent_open_with_apps` も従来どおり更新する。
- 「外部ツールの設定…」を末尾に足し、環境設定の外部ツールページを開く。

## 3. 旧 `custom_open_with_apps` の書き込みを止める

§2 で登録先が `external_tools` へ移るので、**もう誰も `custom_open_with_apps` を増やさない**。
[settings_db.rs](../../src/settings_db.rs) の `save_full` から `write_recent_apps(&tx, "custom_open_with_apps", ...)`
を落とす (P0 で「P1b まで残す」とコメントを付けてある。そのコメントも一緒に更新すること)。

- **テーブルと行は消さない。** 移行後の突き合わせ用に残す (正本の決定どおり)。
- `Settings.custom_open_with_apps` フィールドは読み取り専用の移行元として残し、
  その旨をコメントに書く。
- 既存の移行テストが「書き戻さなくなったこと」を含めて通ることを確認する。

## 4. 守ること

- コミット前に `cargo fmt` (引数なし)。テストは `cargo test -p mimageviewer --lib`。
- UI 文言を足したら `python scripts/check_ui_glyphs.py` を通す。
- スナップショットテストが落ちたら [docs/ui-snapshot-policy.md](../ui-snapshot-policy.md) に従い、
  **勝手に `UPDATE_SNAPSHOTS=1` で上書きせず差分の理由を報告**すること。
- UI スレッドで同期 I/O を足さない。メニュー構築時に EXE の存在確認やファイル I/O をしない
  (有効 / 無効の判定は、対象の種別とツール定義という**同期で分かる情報だけ**で決める。正本 §4.8)。
- 範囲を広げない。複数選択・キー割り当て・実体化は後続 phase。
- 正本と食い違ったら実装を止めて報告すること。

## 5. 完了報告に含めること

- 変更ファイル一覧、追加した純関数とテスト、テスト結果の件数
- ネイティブメニューに積む条件の実装場所
- 実機で確認すべき操作 (利用者に渡す手順)
- 迷った点
