# 一時通知の表示時間（backlog §1.402）

## 所有と設定

`feedback::resolve_timing` だけが「通知の表示時間」の倍率を適用する。
App の発行時に egui の寿命、native の寿命と fade を `ResolvedFeedbackTiming` に確定する。
一覧・画像 viewer・F12/別窓は既存 toast state の秒数、native は同じ確定済み payload を使う。
描画時には設定を読まず、窓の切替・mount/deposit で再計算しない。
保存先を開くaction、発火面、連続通知による置換は維持する。§1.482段Dでactionは`fs_feedback_toast_action`（保存先の表示 / 安全なAI診断コピーの単一enum）へ一般化した。AI失敗も既存のError通知時間を使い、hover詳細だけでは時間延長・自動コピーをしない。

動画内部が発行する通知（フィルタ準備・失敗、再生位置操作を使えない案内）は、既存の
`NativeChromeSnapshot.policy` の選択値を使う。
`NativeRenderCore::show_overlay_toast` は未確定の通知だけを同じ関数で確定し、App から届いた
確定 payload は再計算しない。環境設定OK後の既存 `sync_all_native_chrome_policy` が全登録contextへ反映する。
新しい設定同期 channel、worker、context state は追加しない。

### 標準で維持する native の既存挙動（2026-10-20 設計判断）

利用者決定「既定＝標準＝現在の秒数・見え方そのまま」を、面ごとの時間統一や既存不整合の修正より優先する。
比較元は §1.402 実装前の `28e0aa575`。native は従来、状態の保持期限（linger）と
fade の時計が一致していなかった。描画は linger によらず、通常は1.45秒で fade 開始・1.8秒で透明、
中央は2.15秒で開始・2.5秒で透明になる（fade幅0.35秒）。状態だけは指定 linger まで残る。
したがって長時間指定の拒否案内も native では先に透明になり、逆に短い linger は fade 開始前に消える。
中央700msのシーク境界は、700msの間不透明のまま消える。

この不一致を今回は修正しない。標準では寿命と旧 fade の両方をそのまま渡す。egui側の最後0.3秒のfadeも従来どおり。
短い/長いでは適用対象の寿命・fade開始/終点・fade幅に同じ倍率を一度掛けるため、
従来の可視時間に対して設定が効き、警告/エラーの短い設定は標準のままになる。
`Immediate` は寿命・fadeとも倍率1とし、シーク境界に新しい fade を加えない。
長い拒否理由を読む時間の確保という §1-11 の判断は維持するが、nativeで指定時間まで見えない
既存問題の解消は別件として扱う。

キャプチャ保存/デバッグ出力完了も、egui1.2秒・native1.8秒という既存差を維持する。
この2入口だけ `SurfaceSeconds` に元の基本時間を明示し、共通ownerで倍率を適用する。
音声トラック切替失敗/保存トラックの代替案内は、動画では従来のnative通常1.8秒、
音楽ビューでは従来の本文長方式。種類は両方とも警告とする。

状態の組合せを増やさない簡素化として、既存通知 state と設定 snapshot の利用を採用した。
設定変更は次に発行する通知から反映し、表示済みの通知を延長・再生成する専用状態は設けない。
処理の進捗 overlay や入力所有、detached の述語・placement・viewport 経路は変えない。
まれな失敗は既定で割り切り（ログ＋通知）、回復機構は追加しない。

`Settings.notification_duration` は `Short / Standard / Long`（保存名 `short / standard / long`）。
リリース済み settings.db の既存 KV へ項目を追加する。欠落した旧設定は serde の既定値
`Standard` へ読み替える加算的移行で、既存行・schema version は変更しない。
将来の未知値も標準へ正規化する。全体設定ページの既存 anchor/helper、OK/キャンセル、
設定保存・復元・転送の経路を使う。

## 秒数

| 種類 | 短い | 標準（既定） | 長い |
| --- | --- | --- | --- |
| 情報・成功 | 基本時間×0.6 | 基本時間そのまま | 基本時間×1.5 |
| 警告・エラー・拒否理由・対処案内 | 基本時間そのまま | 基本時間そのまま | 基本時間×1.5 |

本文長方式は Unicode scalar value の数で、20文字以下1.2秒、超過1文字ごと0.12秒、上限5.5秒。
fade を含む。情報・成功の上限も同じ倍率（短い3.3秒、長い8.25秒）にする。
長い設定は上限も延ばさないと長文で選択の効果が消える。短い設定で短縮可能なのは、
対象名・件数を確認する成功/状態通知に限り、読解・判断が要る説明を保護する。
[右クリックメニュー一本化 §1-11](context-menu-unification-plan.md) の長文拒否理由は短縮しない。

| 入口・用途 | 標準の基本時間 | 適用/例外 |
| --- | --- | --- |
| `show_feedback_toast` / warning / error | 本文長方式 | 適用。種別は発行元で明示 |
| 面指定 `*_on` / `show_feedback_result_on` | 従来の1.2秒 | 適用。nativeの旧fadeも保持 |
| `*_with_duration` / `*_on_with_duration` | 呼び出し元の指定秒数 | 適用。nativeは指定lingerと旧1.8/2.5秒のfadeを別々に保持して倍率適用 |
| 索引 worker の再起動・失敗通知集約 | 5秒 | 警告として適用 |
| 動画ピン・未変換書庫の代表画像設定拒否 | 5秒（2箇所） | 警告として適用 |
| 本へのページ追加の完了（除外・簡易再補完・取り消せない旨を含み得る） | 4.5秒 | 警告として適用。注意事項は省略しない |
| メディア窓からメインへの切替と開き直し案内 | 3.5秒 | 警告として適用 |
| ゲームパッド X ピッカーのメタデータ入口案内 | 4秒（`X_PICKER_HINT_TOAST_SECS`） | 警告として適用 |
| パイプラインデバッグ出力開始 | 2秒 | 一時通知として適用。処理中 overlay の寿命とは独立 |
| キャプチャ保存・デバッグ出力完了 | egui1.2秒 / native1.8秒 | 適用。既存の面ごとの差を維持し、クリックで保存先/manifestを開く機能も保持 |
| 音声トラック切替失敗・保存トラックの代替案内 | 動画native1.8秒 / 音楽ビュー本文長方式 | 警告として適用。動画の基本時間を本文長へ変更しない |
| native の通常/中央 toast | 1.8秒/2.5秒、最後350msでfade | 適用。fadeの開始/終点/幅も同倍率。準備通知は情報、失敗・利用不能は警告 |
| native 相対シークの先頭/末尾 | 700ms | `Immediate` の例外。全設定で700ms不透明、fade追加なし。リピート更新する操作フィードバック |
| `FsBoundaryHint` のナビゲーション境界・該当フォルダなし | 従来の専用2.5秒/4秒、native は専用中央表示 | 専用の操作フィードバックとして例外。通常 toast の設定に混ぜない |
| 処理の寿命で表示/消滅する進捗、AI status、スキャン/保存/変換モーダル、seek中表示 | 処理の寿命 | 対象外。開始時に一度出る toast とは区別 |

## 呼び出し元の棚卸し

棚卸し対象の dotted call（`show_feedback_toast*` と `show_native_video_overlay_toast`）を、
文字列内の括弧を除いて引数末尾まで抽出した。専用 tests ファイルを除いた抽出は1,037箇所。
`rg` と引数本文の「失敗/エラー/できません/ありません/必要/拒否/対象外/してください/
error/warning/reason/failure」等の検索で481候補を得た後、残りの固定文言を読み、変数・
helper 本文を渡す箇所を producer の分岐/戻り値まで追った。生成時に種類を指定しており、
実行時に本文から種類を推測する処理はない。

警告/エラーはファイル操作、クリップボード、タグ・評価、読書位置、本の構成/追加/並替え、
検索/索引、書庫/EPUB、補正/LUT/マスク/キャプチャ、外部ツール、動画/VST、復元/設定の各入口を
対象にした。成功文は対象名・件数・完了操作、表示設定・スロット・ツール変更は情報を既定とした。
失敗した二度目の操作に「処理中です」と返す箇所、空スロット/対象なし、古い結果の破棄も保護する。

判断に迷った/複合結果の一覧:

| producer | 決定 |
| --- | --- |
| 手動クリップボード保存・選択画像保存 | 既存の metadata error / failed / metadata_failed / errors で種別を決める。成功だけなら情報 |
| 一括編集 | failed・対象外・cancelled があれば警告。適用済み保持の説明を残す |
| 外部ツール完了集約 | failures の有無で分ける |
| Undo/Redo | 追加 notice があるときだけ警告 |
| 動画/音楽のブックマーク一括登録 | errors の有無で分ける。重複件数自体は成功を妨げない |
| コレクション追加/操作結果 | 既存 is_error を使用。保存先 path が必要な export 完了文は維持 |
| 本の読書位置クリア | Result の成功/失敗で分ける |
| バッチ書庫変換 | cancelled・失敗・既存/同名除外があれば警告 |
| 本追加の4.5秒summary | 簡易補完・hidden除外・Undo不能が混ざるため警告として保守的に保護 |
| native フィルタ確定通知 | 適用不能時の代替処理・対処が同じproducerに混ざるため警告として保護 |
| 画像フォルダのZIP変換結果 | 元フォルダが残ることと作成先pathの注意を含むため警告として保護 |
| スタックまとめの複合案内、類似 preview の非同期 notice | 拒否/読込不能を含むため警告として保護 |
| ドラッグ終了の除外/解決失敗集約、検索の visibility notice | 実行対象の除外を伝えるため警告 |
| 360°開始ヒント・操作方法の案内 | 判断や次の入力が必要な案内は保護、単純な表示モード名は情報 |

## 簡潔化した成功文

| 場所 | 変更前 | 変更後・残す情報 |
| --- | --- | --- |
| `ui_dialogs/rename_item.rs` | `名前を変更しました: <フルパス>` | `名前を変更: <変更後のファイル/フォルダ名>`。日本語・拡張子・長い名前を省略しない |
| `ui_dialogs/new_folder.rs` | `フォルダを作成しました: <フルパス>` | `フォルダを作成: <フォルダ名>` |
| `ui_dialogs/archive_convert.rs` | `ZIP ファイルを作成しました: <出力名>` | `ZIP を作成: <出力名>` |
| `ui_adjustment_panel.rs` | `LUT読み込み完了: <LUT名> (<size>^3) / <フルパス>` | `LUT読み込み: <LUT名> (<size>^3) / <ファイル名>`。LUT内部名・格子寸法を保持 |

キャプチャ保存は既にファイル名方式なので本文を変えない。コレクション export の保存先、
部分失敗や設定引継ぎの注意は削らない。完了文を短くした結果、標準設定でも文字数方式の通知は
短くなるが、時間式・係数自体は変えない。

## 初回実装の検証台帳（a1b88e3e5 としてcommit済み）

対象は HEAD `28e0aa575` からの初回実装差分。通常feature / debug test、
`MSBUILDDISABLENODEREUSE=1`。実製品の起動・実機測定は行っていない。

- `cargo check -p mimageviewer --bin mimageviewer-core -j 1`: コンパイル成功。
  初回のturbojpeg依存ビルドが並列MSBuildで終了1となったため、同じ生成先を
  `cmake --build ... --target install --config Debug --parallel 1 -- /v:normal /nr:false`
  でビルドし直し、依存側0エラー・cargo check成功を確認した。製品コードのコンパイル失敗ではない。
- `cargo nextest run -p mimageviewer --lib -E 'test(/notification_duration|feedback_toast|rename|new_folder|batch_convert|settings_transfer|format_completion_toast|external_launch_completion_summary|native_command_bus|a_toast_does_not_swallow|chrome_suppression_command_bus/)' --no-fail-fast`:
  **188件成功・失敗0・exit 0**。`target/notice-audit/targeted-verified.{stdout,stderr}.log` と
  `verification.json`。時間の境界・警告/エラー下限・明示秒数・DB往復/項目欠落/未知値・
  名前変更の日本語/長い名前・native転送の一度だけの適用/設定snapshot・保存先リンク保持・
  toast置換/発火面・native fade・既存HUDクリック透過を含む。
- 初回は187件中186成功、設定転送の登録項目数の固定期待値1件だけが失敗。
  新しい項目を含む総数464 / 転送可148 / wire key146へ更新して上記で再確認した。
- `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --test ui_snapshot notification_duration_preferences -- --nocapture`:
  2件成功。更新フラグを外した通常比較も**2件成功・exit 0**。
  `target/notice-audit/snapshot-compare.{stdout,stderr}.log`。
- 新規PNGは `notification_duration_preferences_light_standard.png`（17,989 bytes）と
  `notification_duration_preferences_dark_short_narrow.png`（17,694 bytes）、計35,683 bytes。
  両方を目視確認し、3択の選択状態・全文の可読性・狭幅の折返しを確認した。
- ワークスペース全体の `cargo fmt` 実行後 `cargo fmt --check` 成功。
  `python scripts/check_ui_glyphs.py` は0件、`python scripts/check_site_cards.py` は49枚・上限超過0件。
- lib全件・full gate・独立レビューはbriefに従い後続担当へ引き継ぐ。
- `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`:
  **exit 0**。通常feature / dev-runtime、core・Remote・EPUB workerを生成。
  `target/notice-audit/build-dev.{stdout,stderr}.log` と `build-dev-result.json`。
  portable featureは使用していない。VCRT検査もruntime 4 / PE 3で成功。

未確認は実機の経過秒数・F12/複数窓/native overlayの見え方・保存通知のクリック動作。
通常profileをこちらから起動していない。利用者確認はインストール版/常駐版を終了後、
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
既定では実利用中の `%APPDATA%\mimageviewer` を開き、設定/データを更新し得る。
全体設定の3段階、長い親pathにある日本語名の名前変更、短い設定でも残る拒否理由、
動画の通常通知と700msのシーク境界を確認する。新たな範囲外問題は検出していない。


## 独立レビュー指摘対応（2026-10-20、a1b88e3e5 からの差分）

- 音声トラック通知と保存完了のnative基本時間を1.8秒へ戻した。UI更新が呼ぶ
  `poll_audio_track_notice_toast` を抽出し、実際の通知消費/発行経路で検証する。
- native payloadへ確定fadeを含め、標準の1.8/2.5秒fadeと700msの不透明な境界表示を維持した。
  長時間lingerにfadeを合わせる初回実装の変更は撤回した。
- `処理中/進行中/反映中/クリア中/お待ち/待ってから/完了まで/実行中/準備中/保存中/読み込み中` および
  `show_feedback_toast*` の引数を再検索し、拒否分岐と開始通知を照合した。
  読書位置クリアの重複受付（面指定/設定画面の両方）、タグ編集の適用中の再適用、
  コレクション一括出力の受付拒否を警告に変更した。追加の拒否取りこぼしは見つからなかった。
  タグ改名/統合の最初の「反映中」、タグクリア開始、スキャン開始、キャプチャ保存中、
  デバッグ出力開始、領域分割の `scope.pending_label()`、変換中止後のアップスケール削除予約は
  実行を受理した情報通知なので変更しない（迷った対象の判断）。
- この対応で通知本文・環境設定UI・永続化項目は変更しない。新規ファイル/PNGなし。
- 先の検証台帳は初回実装時の履歴であり、native fadeがlingerと一致するという
  当時の期待値は今回のテストで置き換える。

### 対応後の検証

- `cargo nextest run -p mimageviewer --lib -E 'test(/notification_duration|saved_position_book_clear_rejects_duplicate|all_export_keeps_its_owner|feedback_toast|native_command_bus|chrome_suppression_command_bus|a_toast_does_not_swallow|native_output_tests|audio_track.*toast|saved_audio_open_notice|native_audio_track_event|pipeline_debug|seek_relative/)' --no-fail-fast`:
  **32件成功・失敗0・exit 0**。通常feature/debug、`MSBUILDDISABLENODEREUSE=1`。
  `target/notice-review/focused.{stdout,stderr}.log` と `focused-result.json`。
  追加/更新した回帰は次のとおり。
  - `poll_audio_track_notice_toast` が実際の失敗/保存トラック代替通知を消費・発行し、
    動画では1.8秒、音楽ビューでは本文長、短い/標準は警告を維持・長いは1.5倍、一度だけ通知。
  - キャプチャ保存とデバッグ完了pollからegui1.2秒/native1.8秒、保存先リンク保持、倍率とfade。
    実際の相対シーク境界ハンドラから全設定で700ms不透明の通知。
  - 標準のnative通常/中央・指定linger0.7/1.2/1.8/2.5/3.5/5.5秒について、
    変更前のfade式と厳密比較。全段階のfade倍率と警告/エラーの短縮防止も検証。
  - 読書位置クリアのコマンド/設定画面からの二重受付と、コレクション一括出力の拒否ハンドラを
    全段階で検査。元の処理票/操作ownerを保持する既存検査も維持。
- 初回コンパイルでテストhelperの可視性とfixtureの借用を修正した後、23件中22件成功。
  残る1件はfade開始ちょうどの丸め差（2.15秒で1.0と0.99999976）を検出した。
  旧式の分岐をそのまま維持し、上記32件で再確認した。
- ワークスペース全体の `cargo fmt` 実行済み、`cargo fmt --check` 成功。
  `python scripts/check_ui_glyphs.py` は0件、`git diff --check` 成功。
- 環境設定レイアウト/静的描画は変更しないのでsnapshot更新なし。lib全件/full gateは後続担当へ。
- `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`:
  **exit 0**。通常feature/dev-runtimeのcore・Remote・EPUB workerを生成（core 2分54秒）。
  `target/notice-review/build-dev.{stdout,stderr}.log` と `build-dev-result.json`。
  VCRT検査 runtime 4 / PE 3 成功。portable feature不使用、既存の実行中プロセスは停止しない。
- 製品バイナリの起動なし。実機の秒数・fadeの見え方・F12/別窓・保存通知クリックは未確認。
  利用者起動は `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
  インストール版/常駐版を先に終了する（同じmutex）。既定の `%APPDATA%\mimageviewer` の
  実設定/データを更新し得る。確認対象は標準の動画保存通知とfade、短い設定の拒否案内、
  長い設定の延長、全設定で変わらない700msシーク境界。新たな範囲外問題なし。

### 今回の変更ファイル（16件、新規なし）

- `docs/architecture-overview.md`
- `docs/display-pipeline.md`
- `docs/notification-duration-policy.md`
- `docs/video-architecture.md`
- `htdocs/mimageviewer/manual/settings.html`
- `src/app.rs`
- `src/app/book_resume_meter.rs`
- `src/app/book_resume_meter_tests.rs`
- `src/app/native_video.rs`
- `src/app/tests/notification_duration.rs`
- `src/feedback.rs`
- `src/pipeline_debug.rs`
- `src/ui_dialogs/collections.rs`
- `src/ui_dialogs/tag_editor.rs`
- `src/video/mod.rs`
- `src/video/native_presenter/render_core.rs`
