# MCPのtoolを常に一覧へ出す（backlog §1.786、区切り6）

2026-11-14。基点 `c78dc1449`、worktree `G:\home\mimageviewer-asr-t1`。
ASR T1とは別のコミット単位。コミット・製品バイナリの起動はしていない。

## 棚卸し（変更前）

### 一覧と要求の経路

- `crates/mcp-server/src/schema.rs` の `tool_names` は許可snapshotからtoolを絞り、`tools_for` はその名前を `tools_named` へ渡していた。
- `lib.rs` の `tools/list` は `Action::List` としてcoreへ `Command::Status`（`RequestKind::Internal`）を送り、
  `permission_tools` で表示集合を決めていた。Disabledは空、Unreachableは `miv_status` だけ、Status成功はsnapshotから導く集合。
  その他のStatus失敗はJSON-RPC error `-32603`。
- legacyの `PermissionObserver` は5秒ごとにStatusを照会し、`observe_permission` と実際のStatus/List応答も使って集合を比較。
  変化時に `notifications/tools/list_changed` を送る。古いpollが新しい集合を上書きしないようrevisionを持っていた。
- `tools/call` は一覧の集合による受付制限をしていない。既知toolの引数を検証し、すべてcoreへ送っていた。
  許可による拒否はcoreの `src/agent_ipc/mod.rs` の `Connection::request`（snapshot取得→Disabled→tool別の段/チェック）で行う。
  実行中と送信前のsnapshot/epoch再照合、縮小画像・文字起こしhandler冒頭の検査も既存のcoreの防御。

「基本9 tool」は `miv_status`、`search_items`、`list_tags`、`items_by_tag`、`list_collections`、`get_collection`、
`list_library`、`get_item_info`、`list_folder`。
「整理7 tool」は `create_collection`、`rename_collection`、`add_to_collection`、`remove_from_collection`、`add_tags`、`remove_tags`、`set_rating`。

| 段/チェック | 変更前に一覧へ出ないtool | 呼ばれた場合の変更前の扱い |
| --- | --- | --- |
| Disabled | 全19 tool（coreへ接続できる時） | adapterは転送。coreが全toolを `disabled`「AI 連携 (MCP) は無効です」で拒否 |
| ReadOnly | `open_in_viewer`、整理7 tool。縮小画像・文字起こしは各チェック次第 | openはcoreが「mIV で開く許可がありません」。整理は「コレクションを変更する許可がありません」または「タグ・★を変更する許可がありません」 |
| ReadAndOpen | 整理7 tool。縮小画像・文字起こしは各チェック次第 | 整理は上と同じcoreの `disabled`。基本9とopenは段の条件を満たす |
| Organize | 段による非表示なし。縮小画像・文字起こしは各チェック次第 | 基本9・open・整理7は段の条件を満たす |
| 縮小画像OFF（有効な段） | `get_thumbnail` | core受付で `disabled`「AI に縮小画像を見せる設定は無効です」。handler直接呼出しにも共有OFFの検査あり |
| 文字起こしOFF（有効な段） | `get_transcript` | core受付で `disabled`「AI に文字起こしを見せる設定は無効です」。handler直接呼出しにも共有OFFの検査あり |
| GPS OFF/ON | 対象外（toolの有無は変わらない） | item_infoのEXIF GPS欄の投影に使用。OFF時はGPS値を含めない |
| AI説明文 OFF/ON | 対象外（toolの有無は変わらない） | 説明文検索の対象とitem_infoの `not_permitted` を決める。OFF時は本文のDB参照前に判定 |

Disabledでもadapterのinitializeは成功し、coreのHelloは版と名乗りだけを受け付ける。
接続を拒む構造ではない。mIV未起動の場合だけは、許可を取得できないため旧一覧は `miv_status` だけになっていた。
拒否は通常のtool結果（JSON-RPC `result` 内の `isError=true`、structuredContentとtextに同じerror）へ変換される。
unknown tool/不正な引数/不正なプロトコルは引き続きadapterのJSON-RPC errorで、許可エラーとは別。

## 決定と実装

利用者決定（2026-11-14、依頼52・53のClaude Code 2.1.72での観測）を適用し、19 toolの説明・schemaを常に全部返す。
Disabled・未起動・一時的なcoreのエラーでも同じ。これらは公開の機能説明で、ファイル・ライブラリの中身ではない。
Disabled時のcoreの全tool拒否、既存のsnapshot/epoch・カーソル・送信前検査は維持する。

- `schema::tools` を一覧の唯一の生成入口にし、許可で絞る3関数を削除。
- `tools/list` はadapterの固定応答にし、coreへのStatus要求を削除。既存のtyped `Envelope::tools_list` を使い、
  discovery予算48,000とデータ応答40,000の区別をwriterまで保持する。modernのttlMs=0/cacheScope=privateは維持。
- 集合が変わらなくなったため、PermissionObserver・5秒timer・revision・permission_tools・observe_permission・通知イベントを削除。
  legacyの `tools.listChanged` 広告も削除。変更しない集合を監視するthreadや、成立しない通知分岐を残さない。
  内部agent wireの `RequestKind::Internal` 自体は互換のため維持し、版は変更しない。
- coreのtool別受付を `check_tool_permission` にまとめ、設定の表示名を含むエラーへ変更。
  adapterに同じ認可は足していない。低位handlerの縮小画像/文字起こし拒否も同じメッセージ定数を使う。
- 各toolのdescription冒頭に、mIVのRead / Read and Open / Organizeと追加の共有許可を短く明記。
  出荷済みの操作条件・untrustedの注意・cursor注意は維持する。

| 不足する許可 | 新しいcoreの案内（共通の設定場所「mIV の『設定』→『AI 連携…』」を含む） |
| --- | --- |
| Disabled | 「mIV の AI 連携が無効です。…許可の範囲を選んでください」 |
| 縮小画像 | 「縮小画像を AI に見せる」をONにする |
| 文字起こし | 「文字起こしを AI に見せる」をONにする |
| open | 許可の範囲を「読み取りと、mIV で開く」以上にする |
| 整理7 tool | 許可の範囲を「整理まで」にする |

## テストの置き換えと範囲

許可で隠れること・observerのpoll/revisionを期待したテストを、次の新しい不変条件へ置き換えた。
toolのtransport、payload/schema、入力検証、FIFO、背圧、取消、EOF、stdout失敗の既存回帰は維持。
先頭tools/listのStatusを止めていた直列化テストは、実際の `miv_status` toolを止める形に変更して同じ不変条件を検査する。
pipe終了109/233とaccess deniedの区別も、一覧でなくtool要求のエラー変換で検査する。

- `always_list_tests`: 4段×4チェックの全64通り×3プロトコルで同じ全19 tool。discovery中はbackend呼出し0。
  core未起動・Disabled・PermissionChangedからの回復も同じ一覧で、余計な通知なし。
- 同じstdio接続で `get_transcript` を拒否→許可→再拒否。一覧の再取得を挟まず、その時のcore結果を返す。
  open・整理の既存の許可切替テストも全toolの一覧と継続呼出しを検査する。
- coreの `permission_admission_tests`: 実受付で対象10 toolすべてが必要な許可を名指して拒否、Disabledの共通案内。
  有効な3段×16チェックの表、GPS/AI説明文がtoolの受付を変えないこと、同じConnectionのDisabled解除・
  縮小画像/文字起こしON・Organizeへの変更を再接続/一覧再取得なしで受け付けること。
- 既存の全権限・全プロトコル・ID別discovery予算検査を維持し、受付上限の256 byte ID（NULのJSON escapeが最長になるケース）も追加。

## 文書・UI

`docs/mcp-integration-plan.md` の現在の方針・legacyの一覧・snapshot・縮小画像・内部照会の説明を更新した。
過去の実測・検収履歴は残し、冒頭の2026-11-14の決定が旧一覧/通知の仕様を置き換えると明記。
マニュアルとチュートリアルへ即時の許可変更と固定の機能一覧を追記し、privacyの日英に許可OFFでも公開の機能名・説明が見えることを追記。
製品ページの安心セクションも照合し、ファイル情報の受け渡しの記述と矛盾が無いため変更していない。

`agent_integration.rs` には許可変更でAIアプリの機能一覧が変わるという文言は無かった。
「見せる範囲」はファイル・ライブラリの情報と実行できる操作の説明なので、その意味を維持してUIは変更していない。PNGの更新なし。

ASR計画§10の古い `tool_names` による出し分けの記述も今回の利用者決定で置き換わる。
同計画本文は設計担当が所有するため編集せず、変更先の正本をMCP計画§0へ記録した。

## 変更ファイル

| ファイル | 内容 |
| --- | --- |
| `crates/mcp-server/src/lib.rs` | 固定discovery、typed Envelopeでの返却、Observer/通知/Action::Listの撤去 |
| `crates/mcp-server/src/schema.rs` | 許可による3種の一覧関数を撤去、各toolの必要許可を冒頭へ |
| `crates/mcp-server/src/pipe.rs` | 役割の変わったreachabilityのコメントのみ |
| `crates/mcp-server/src/tests.rs`, `always_list_tests.rs`（新規） | 旧一覧・監視の期待を固定一覧・認可の継続・core非照会へ置き換え、既存のI/O/取消/上限回帰を維持 |
| `src/agent_ipc/mod.rs` | 既存の要求受付の許可分岐を関数へ集約し、設定名つきの共通errorメッセージ |
| `src/agent_ipc/thumbnail.rs`, `transcript.rs`, `collection_write.rs` | 低位の既存認可も同じerrorメッセージを使用 |
| `src/agent_ipc/permission_admission_tests.rs`（新規） | 実受付の拒否、段とチェックの表、同じConnectionでの段/チェック変更 |
| `docs/mcp-integration-plan.md` | 決定・根拠・日付・固定discoveryと旧監視の撤去を反映 |
| `htdocs/mimageviewer/manual/ai-integration.html`, `tut-ai-integration.html` | 許可は即時反映、一覧は同じで許可のない操作を拒否 |
| `htdocs/mimageviewer/privacy.html` | 日英に許可OFF時も見える公開の機能説明を追記 |
| `docs/mcp-always-list-report.md`（新規） | 本報告 |

## 検証コマンド・結果・文字数

Rustの実行はすべて次のwrapper経由（各commandのログは `target/mcp-always-list/`）。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner asr-t1 -WorkDir G:\home\mimageviewer-asr-t1 -Command 'python -B -I target/mcp-always-list/run.py core'
```

補助runnerはignored。環境は `CARGO_BUILD_JOBS=1`、`MSBUILDDISABLENODEREUSE=1`、`CMAKE_BUILD_PARALLEL_LEVEL=1`。
ロックへの書込不可警告が出たため、wrapperの規則どおりBelowNormalで続行した。

| 実行 | 結果 |
| --- | --- |
| `cargo nextest run -j 1 -p mimageviewer-mcp --lib --no-fail-fast --success-output final` 初回 | 80件中75成功。旧仕様の期待値など3件の更新漏れを修正。ほか2件は既知のnamed pipe client作成でOS error 5（sandbox制約）。`adapter-pre.log` |
| 同上、修正後 | **80件中78成功**（14.582秒）。残る2件は初回と同じnamed pipe client作成時のOS error 5。`adapter-final.log` |
| `cargo nextest run -j 1 -p mimageviewer --lib -E 'test(agent_ipc::)' --no-fail-fast --success-output final` 初回 | 194件中191成功。新規テスト1件で整理の受付準備をfixtureへ設定し忘れていたため修正。ほか2件はnamed pipeのOS error 5。`core.log` |
| 同上、`-E 'test(agent_ipc::) & not (test(stdio_exe_uses_real_core_fixture_handlers) \| test(shutdown_interrupts_an_idle_connected_reader_without_retry))'` | **192/192成功**（42.212秒、compile 4分31秒）。`core-final.log`。既知のpipe制約2件は繰り返さず、exeを使う試験も除外 |
| `python scripts/check_site_cards.py` | 53 cards、超過0、成功 |
| `python scripts/check_ui_glyphs.py` | 危険なglyph 0、成功 |
| `cargo fmt` | 最後に引数なしで実行し成功。`git diff --check` も成功 |

adapterの2件は `pipe::tests::real_pipe_cancel_reaches_core_while_stdio_stdout_is_blocked` と
`pipe::tests::real_pipe_receives_cancel_while_hello_or_tool_response_is_withheld`。どちらもfixture thread開始前のclient openで失敗。
coreの2件は上の除外名のとおり。`stdio_exe_uses_real_core_fixture_handlers` もfixture serverの生成（`tests.rs:763`）で止まり、
exeの解決・起動へ進んでいない。製品バイナリは起動していない。

| 実測 | 文字数/上限 |
| --- | --- |
| 全19 tool、modern、64字ASCII ID | 42,417文字 |
| 全段×16チェック×3プロトコル×IDの最大 | **43,889文字**（modern、256 byteのNUL IDがJSONでescapeされた場合） |
| 上の値 + 余裕512 | **44,401 / 48,000文字**、余裕込みで3,599文字残る |
| 最長description | **list_folder: 667文字 / 667 UTF-16 units**。4,096以内、全toolが目標2,048以内 |
| instructions | **852文字 / 852 UTF-16 units**、4,096以内 |

全ケースでserializerの拒否が無いこと、`MAX_FRAME_BYTES`（4MiB）に収まることも検査し成功。
データ応答40,000・discovery48,000（検査の余裕512）・tool descriptionとinstructions各4,096（UTF-16も）の予算は変更しない。
製品の起動・実AIアプリでのセッション確認・build-devは実施していない。最後にfmtだけの指定に従う。
