# §1.221 右クリックメニュー構成

> 状態: 実装・自動回帰検証済み（2026-09-13）。正本は `docs/next-release-backlog.md` §1.221。

## 目的と境界

グリッドとフルスクリーンが共有する mImageViewer の右クリック項目について、環境設定から
静的項目の表示 ON/OFF と同一階層内の順序を変更できるようにする。既定の空設定では、従来の
`MenuNode` の内容、section、表示順を完全に維持する。対象の種類や選択状態による可否判定、
混在選択の拒否理由、`MenuCommand` の dispatch、キーや上部メニューの入口は変更しない。

## 永続 ID と固定枠

- `ContextMenuItemId` は表示名や実行時 payload に依存しない静的 leaf の stable ID だけを持つ。
  Root と Open With の親 ID を別に保存し、submenu を平坦化しない。
- 登録済み外部ツール群、Open With submenu、関連付けアプリ群は、元の capability-filtered tree
  にある位置と内部順を保つ固定枠である。設定 catalog、非表示一覧、共有 JSON へは出さない。
- Windows Shell 群も従来どおり native renderer が末尾に置く固定枠で、サブメニュー / 同階層
  の既存設定と Windows が返す内部順を維持する。

## 解決規則

`context_menu_model` は従来どおり可否判定済みの tree を先に構築する。その後、同一階層の元の
unit 列を static configurable slot と fixed slot に分類する。保存済み static ID は configurable
slot の間だけで並べ替え、fixed slot の位置と内容は動かさない。非表示も static item にだけ適用する。

未知 ID、重複 ID、親階層違いは無視する。保存データに無い新規 ID は canonical 配列の位置を
空けたまま既知 ID だけを保存順で再配置するため、新項目が既定位置へ補完される。利用不能な
項目は元 tree に存在しないので復活しない。最後に `normalize_menu` を通し、leading / trailing /
連続 separator と空 submenu を除く。native Win32 と egui fallback はこの同じ解決済み tree を使う。

## 保存・共有・UI

`Settings.context_menu_layout` は `serde(default)` の独立設定で、旧 settings は空の標準設定として
読む。「表示 → メニュー構成」に右クリック専用 section を置き、Root / Open With を別々に編集し、
すべて表示と既定へ戻す操作を用意する。操作カスタマイズ共有 JSON にも同フィールドを含める。
旧 JSON で欠落した場合は標準値となり、replace-only 取り込みは右クリック設定も標準へ置き換える。

## 受入条件

- 空設定で Grid / Fullscreen / checked の代表 tree と section が従来どおり。
- unavailable item は復活せず、unknown / duplicate / wrong-parent を無視し、新 ID は既定位置へ補完。
- static reorder / hide 後も外部ツール、Open With、関連付けアプリ、Windows Shell の固定位置と内部順を維持。
- native / egui が同じ resolved preorder を使い、正規化後に不正 separator や空 submenu がない。
- 環境設定のスクロール、上下移動、非表示、すべて表示、既定戻しが実 widget から反映される。
- settings と操作カスタマイズ共有の round-trip、旧 payload の標準置換、sanitize を固定する。

## 検証記録（2026-09-13）

- focused: model 21 件、設定 UI / settings / snapshot 7 件、操作カスタマイズ共有 8 件、
  native menu 8 件、Grid / Fullscreen 共通設定の実配線 1 件が成功。
- 実 widget の長い設定一覧でスクロール、上下移動、非表示、すべて表示、既定へ戻す、
  Open With 階層の展開を確認。snapshot は
  `tests/snapshots/preferences_context_menu_layout{,_open_with}.png` に固定した。
- `RUST_TEST_THREADS=1` の `scripts/test-full.ps1 -SuppressCrashDialogs` は main 8349 件、
  UI snapshot 48 件を含む workspace / integration / doc / vendor 全 target が成功し、
  process error mode も元へ復帰した。完全ログは
  `target/section221-context-menu-layout-20260913/test-full.stdout.log`、
  `test-full.stderr.log`、`test-full.exit.txt`。
- `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、
  UI glyph、viewer context audit、diff check は成功。
- 同じ source freeze の `scripts/build-dev.ps1 -PreserveRuntime` は exit 0。core は
  SHA-256 `49D3454AC1A64FB6FDD2DE7808D1680158EE4C4CD6D0F765DB013B91E133C7D7`
  （2026-09-13 14:49:47 JST）、remote は既存 runtime を保持した。agent によるアプリ起動・停止と
  手動 UI 確認は行っていない。

### source freeze SHA-256

| path | SHA-256 |
|---|---|
| `src/context_menu_model.rs` | `C215E3A083F3233E57662A1A294B298A98636F56370A51B85A25381D7E9A9618` |
| `src/native_context_menu.rs` | `A30A58A2FF8EE1F954291E6AC274398C5CBA1C60E771357E17F97CD94903141E` |
| `src/operation_customize_share.rs` | `67BF5387B38A53CE16EF37E4C39FA1448D0823723002E85FD2482D0FB130586F` |
| `src/settings.rs` | `4E35D4A02E992ACEC09F5926E06F3E4B994D6A7894CA40270B2929C1ABFFB440` |
| `src/ui_dialogs/context_menu.rs` | `F14977D2421CDC5A64138071D84AF67FCFAAC372E8236F84D6D007786E106DF1` |
| `src/ui_dialogs/preferences.rs` | `6B4D145401347633DE3BBD0E4DF593629C4DD9C00D23A74FE234E6EA58FCB818` |
| `src/ui_dialogs/preferences/pages.rs` | `F1B2A96E3EAC85EAA2D0A84FEC90CDFF79C610DEF83365A7BBAA26071FA54DEE` |
| `docs/architecture-overview.md` | `25ABA52F6D91B2A80C1AA2B90EEF3F996226CD5FEF0E9D5BBEF5DD6178303FFD` |
| `docs/context-menu-unification-plan.md` | `D32EDE229024D8A6CAD719CA610703B2C02466EC2475680C0A979FC596D7FD2E` |
| `docs/operation-customize-share-plan.md` | `6F416952A691BCB9B249CD5EC2814BFBA4A222AC9B1986025557640C941D1517` |
| `docs/spec.md` | `92F9E94BF0C43CE4F6B3898FB476404CCBB5FA8472A492458FA6C22F6EAD3C05` |
| `htdocs/mimageviewer/manual/settings.html` | `F3C039503125E0640635710F332A0E89794C05A1CD7FD4D55C47FA8C09FFC21E` |
| `tests/snapshots/preferences_context_menu_layout.png` | `ACE17A74A96B92735EEB2B54DBD65CE6C54B3DE2695D6ED0E23A1D4312AE961C` |
| `tests/snapshots/preferences_context_menu_layout_open_with.png` | `DA75E5234AA7DDC42EA2EB877AEF456D5BE5FB16B79335588378A325BF1A4AE0` |

本書は上表の source / golden 14 path を freeze とする。検証記録自身の hash は本文追記後に
別途照合する。
