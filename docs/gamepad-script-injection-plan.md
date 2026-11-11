# 操作台本からのゲームパッド入力の差し込み

2026-11-11。設計 = ClaudeCode Opus 5.5 / high、実装 = Codex GPT-6.1 Sol / high、
独立レビュー = 別の GPT-6.1 Sol / xhigh。ブランチ `feat-pad-inject`。

## 1. 目的と範囲

ゲームパッド操作は実アプリでほとんど確かめられていない (サブ PC に機材が無い)。§1.682+§1.688 の作業中に、
パッドの移動要求を黙って捨てる経路が見つかった。試験用ビルド (`test-script` feature) の操作台本から
`PadEvent` を差し込み、**OS からの読み取り (gilrs) 以外の全経路**を実アプリで通せるようにする。

通す経路: `GamepadRuntime` の共有キュー → `PadEventReceipt` (前面の観測) →
`GamepadPhysicalInput::drain/ingest` (前面判定・押下の重複排除・抑止) → UI 側のバッチ
(`sample_gamepad_input`: リピート・操作 ID・モーダル中の配送) → X リング / ピッカー / 操作割り当て → 各画面の処理。

範囲外: ViGEmBus などの仮想ゲームパッドドライバー、gilrs の OS 読み取り部分 (`map_gilrs_event` 等) の試験、
夜間の組み合わせ試験のキットへの登録 (別作業。台本は夜間 Sandbox でも動く形にしておく)。

## 2. 前提 (コードで確認したこと、master 68a8b6dbe)

- `src/gamepad.rs`: gilrs スレッドが `map_gilrs_event` で `PadEvent` に写し、
  `PadEventReceipt::new(mapped, foreground_app_hwnd())` を `Arc<Mutex<VecDeque<PadEventReceipt>>>` に積む
  (上限 256、満杯時は最古を捨てる)。積んだら `ctx.request_repaint()`。
- `foreground_app_hwnd()` は製品・試験用ビルドとも OS の前面が自プロセスかを見る。受理 (`accepted`) は
  押下だけに効き、解放・軸・切断は常に通る。
- `GamepadRuntime::drain(ctx, enabled)` は `enabled == false` で読み取りスレッドを止め、キューを捨てる。
  消費は `App::logic` の `drain_gamepad_physical_input` だけ (1 frame 1 回)。
- `PadEvent::Connected` は今は何もしない。`Disconnected` は保持中の状態を消して終端にする。
- 既存の台本のキー入力は `UiCommand::Key` → ROOT の処理で `key_input` の合成キューへ。
  `require_key_target` は `target_registered && focused` (egui の focus) を要求する。
- `gamepad_enabled` の既定は ON。X リング (`gamepad_ring_enabled`) は常に有効。一覧の X リング既定割り当ての
  先頭は `GridParentFolder`。

## 3. 設計

### 3.1 差し込む層: gilrs と同じ共有キュー

台本からの `PadEvent` は、gilrs スレッドが積むのと**同じキュー**に、**同じ作り方の受領票**
(`PadEventReceipt::new(event, foreground_app_hwnd())`、上限と最古捨ても同じ) で積む。
producer 側の「受領票を作って積む」処理を 1 つの関数にまとめ、gilrs スレッドと台本の両方がそれを呼ぶ
(書き方を 2 か所に持たない)。積んだら `request_repaint()` する (gilrs と同じ)。

これより下流は一切変えない。前面判定・押下の重複排除・リピート・抑止・リング・モーダル中の扱いは、
本物の入力と同じコードを通る。

### 3.2 台本が動いている間は gilrs を起動しない

`test-script` feature のビルドで台本が動いているときは、`ensure_started` が gilrs スレッドを立てない
(ログに 1 行残す)。キューへ積むのは台本だけになる。

- 理由: 開発機にパッドが挿さっていると、本物の入力が台本の入力に混ざって結果が決まらない。
  また 3.3 の「取り込み済みの件数」を台本の入力だけで数えられる。
- OS 読み取りは範囲外なので、止めても検証範囲は減らない。
- 製品ビルドと、台本なしで起動した試験用ビルドの挙動は変わらない。
- 読み取りの状態は `Physical` (gilrs スレッドあり) / `Script` (スレッドなし、台本だけが積む) / `Stopped` の 3 つで持つ。
  `Script` でも「動いている」扱いにし、無効化 (`enabled = false`) では `Stopped` へ移ってキューを必ず捨てる
  (現行の `stop()` は `started == false` だとキューを消さないので、`ensure_started` の早期 return にしない)。
- どの状態にするかの判定は純関数にし、テスト用の `ensure_started` と本番用の両方が同じ判定を使う。

### 3.3 取り込みの確認 (ack)

台本の各命令は、積んだ入力を App が取り込む (`GamepadRuntime::drain` がキューから取り出す) まで待ってから戻る。

- 差し込み口 (キュー・`injected`・`drained`・`discarded` の件数) は `GamepadRuntime` のインスタンスが持つ。
  台本の worker は UI 側の応答で差し込み口の共有ハンドル (件数の `Arc`) と自分の通し番号を受け取り、
  `drained` を**直接読んで**待つ。snapshot の公開 (ROOT の UI 処理) には依存しない。
  3.2 により `drained` は台本の入力だけを数える。
- 待ちの期限は 5 秒で、UI 側の応答待ちも含める。積む・待つの際に ROOT へ repaint を要求する
  (gilrs と同じく App の logic を起こす)。期限を過ぎたら環境不成立として台本を失敗にし、
  `enabled`・`foreground`・`reader`・`injected`・`drained`・`discarded` を添える。再試行はしない。
- 無効化で捨てた入力は `discarded` に数え、成功の ack にしない。`discarded` が増えていたら待ちを直ちに失敗にする。
- `drained` は「App が受け取った」ことの確認であり、操作が受理・実行されたことの証明ではない。
  結果は台本が snapshot の状態 (`selected_index` 等) で確かめる。
- これで「押下と解放が別の logic 呼び出しで取り込まれる」ことが保証され、`pad_hold` の保持時間は
  App が押下を取り込んでからの時間になる。

### 3.4 前提条件

確認は UI 側で入力を積む**その場で**行い、拒否は理由つきで worker へ返す (黙って捨てない)。
前面の観測は 1 回だけ行い、同じ値を前提の確認と受領票の両方に使う (確認と受領票の間で前面が変わる隙間を作らない)。

| 入力 | 前提 |
| --- | --- |
| 押下 (`pad_press`、`pad_tap`/`pad_hold` の押下、`pad_trigger` が出す押下) | `enabled` かつ OS 前面が自プロセス |
| 解放・軸・接続・切断 | `enabled` だけ (製品側も背景で解放・中立軸・切断を取り込むため) |

拒否されたら台本の失敗にし、`wait_until(|s| s.gamepad.foreground, ms)` を促す。
egui の `focused` は要求しない (本物のパッドも見ない)。

後始末 (中断時の解放、`pad_neutral`、台本の終了処理) は interrupt の検査を通さずに送る (`send_unchecked`)。
台本が例外・中断・期限切れで終わったときは、ランナーの終了処理 (キーの `Cancel` を送る所) が、台本が
1 件でも差し込んでいれば `PadEvent::Disconnected` を 1 件積む。切断は保持中の状態をすべて終端にする製品側の経路で、
押下・軸を 1 つずつ戻す仕組みは作らない。

無効中・背景中に押下が捨てられる挙動は、既存のユニットテストが見ている。台本の命令は
「本物の入力が受理される状況で、その先が正しく動くか」を確かめる道具にする。

### 3.5 snapshot

`snapshot().gamepad` (Rhai の map):

| 項目 | 意味 |
| --- | --- |
| `enabled` | `settings.gamepad_enabled` |
| `foreground` | `foreground_app_hwnd().is_some()` |
| `reader` | `"script"` (台本が入力を持つ) / `"physical"` (gilrs) / `"stopped"` |
| `injected` | 台本が積んだ件数の累計 |
| `drained` | drain が取り出した件数の累計 |
| `discarded` | 無効化で捨てた件数の累計 |

snapshot の値は診断と台本の条件用で、ack は 3.3 のとおり件数を直接読む。

### 3.6 台本の命令 (Rhai)

ボタン名 (大文字小文字は区別しない): `a`/`south`、`b`/`east`、`x`/`west`、`y`/`north`、
`up`/`down`/`left`/`right` (十字キー、`dpad_up` 等も可)、`lb`、`rb`、`lt`、`rt`、`back`/`select`、`start`。
軸名: `left_x`、`left_y`、`right_x`、`right_y`、`lt`、`rt`。スティックは -1.0〜1.0、トリガーは 0.0〜1.0
(範囲外はエラー。黙って丸めない)。`left_y` の正の向きは gilrs と同じ (上)。整数も受け付ける。

| 命令 | 動き |
| --- | --- |
| `pad_press(button)` / `pad_release(button)` | 押下 / 解放を 1 件 |
| `pad_tap(button)` | 押下 → 取り込み待ち → 解放 → 取り込み待ち |
| `pad_hold(button, ms)` | 押下 → 取り込み待ち → `ms` 待つ → 解放。待ちの途中で中断されたら解放を送ってから失敗する (`hold_key` と同じ) |
| `pad_axis(axis, value)` | 軸を 1 件 |
| `pad_stick(side, x, y)` | `side` = `left`/`right`。X 軸、Y 軸の順に 2 件 |
| `pad_trigger(side, value)` | `side` = `left`/`right`。実機で gilrs がトリガーを引いたときに出すのと同じ並びで出す (§4) |
| `pad_neutral()` | 台本が押したままのボタンを解放し、0 でない軸を 0 に戻す |
| `pad_connect()` / `pad_disconnect()` | 接続 / 切断。切断後は台本側が覚えている押下・軸も空にする |

保持中のボタンと軸は台本側 (worker) が覚え、`pad_neutral` と中断時の解放に使う。

### 3.7 製品ビルドに入れない

追加するコードはすべて `#[cfg(feature = "test-script")]` (ユニットテストに要るものは
`any(test, feature = "test-script")`)。feature なしの `cargo check -p mimageviewer --bin mimageviewer-core`
が通り、追加した関数・命令・snapshot 項目が製品ビルドに現れないことを確かめる。

### 3.8 試しの台本

`scripts/ui-smoke/gamepad-basic.rhai` と `ui-smoke.ps1` のシナリオ `GamepadBasic`。
使い捨ての `library/sub/` に PNG を数枚置き、`sub` を開いて始める。

1. 入力の前提 (`gamepad.enabled`・`foreground`・一覧が落ち着いた) を待つ。
2. 十字キー右 → `selected_index` が 1 増える。
3. A → 全画面が開き、選択中の画像を表示する。
4. B → 全画面が閉じて一覧に戻る。
5. X 押下 → 十字キー上 押下 → X 解放 → 十字キー上 解放 (X リングの上 = 既定で「親フォルダへ」、
   `gamepad_input.rs` の X リング処理で確認済みの順) → `current_folder_path` が `library` になる。
6. `pad_neutral()`、`pad_disconnect()` で終える。

## 4. 実装時にコードで確かめること

- gilrs 0.11.2 の Windows 経路で、アナログトリガーを引いたときに出るイベントの並び
  (`ButtonChanged` だけか、`ButtonPressed`/`ButtonReleased` も閾値で出るか、その閾値)。
  `pad_trigger` はそれと同じ並びを `map_gilrs_event` を通した後の `PadEvent` で出す。
- 台本が動いているかを `GamepadRuntime` から知る手段 (既存の test_script の状態を読む)。
- `snapshot` を作る場所と、`gamepad` の値を App から渡す経路。
- X リングを確定させる実際の手順。

## 5. 採らなかった案

- **仮想ゲームパッドドライバー (ViGEmBus)**: ドライバーの導入が要り、夜間 Sandbox に載せにくい。
  gilrs の OS 読み取りまで含めて確かめたいときの仕上げ用に残す。
- **`GamepadInputState` へ直接書く / `PadPhysicalEvent` を差し込む**: 前面判定と受領票を飛ばすので、
  本物と違う規則を通ることになる。
- **gilrs と台本の入力を混ぜて、混ざったら失敗にする**: 混ざりの検出と件数の数え分けが要る。
  起動しないほうが状態が少ない (CLAUDE.md「状態の組み合わせを減らす」)。

## 6. 設計レビュー (2026-11-11、GPT-6.1 Sol / xhigh、実装前)

ACCEPT WITH CHANGES、P1 なし。P2 4 件をすべて反映した。

1. snapshot での事前確認だけだと、確認後に前面が変わって拒否された押下も ack が成立する
   → 3.4: UI 側で積むその場で確認し、同じ前面の観測を受領票に使う。拒否は worker へ返す。
2. 解放・中立化まで前面を要求すると後始末できない → 3.4: 解放・軸・切断は `enabled` だけ。
   後始末は interrupt を迂回し、台本の終了時は切断 1 件で終端にする。
3. gilrs を立てない場合の停止・破棄が未定義 → 3.2: 読み取り状態を 3 つで持ち、無効化で必ず捨てる。
   捨てた件数は `discarded` に数え、ack を失敗にする。
4. ack が ROOT の UI 処理 (snapshot 公開) に依存する → 3.3: worker が件数を直接読む。ROOT に repaint を要求し、
   期限切れは環境不成立。
- そのほか: X リングの確定手順は「X 押下 → 上押下 → X 解放 → 上解放」。台本との接続は既存の Windows 条件と揃え、
  件数を扱うテストは既存の直列化を使う。

## 7. 実装記録

### 7.1 実装と §4 の照合 (2026-11-11、feat-pad-inject)

対象は base `4739dc589` 上の未コミット差分。実装・検証をこの worktree 内で行い、独立した
GPT-6.1 Sol / xhigh のコードレビューは修正後 ACCEPT (未解決 P1/P2 なし)。

- `gamepad.rs` の producer を `enqueue_pad_events` に集約した。OS 前面を一度だけ観測し、
  UI 側の前提確認と受領票へ同じ値を渡す。上限 256・最古捨ては実入力と共通。
  reader は Physical / Script / Stopped。Script の無効化でもキューを捨て、discarded に数える。
  台本用の件数・共有ハンドルは runtime インスタンスの所有で、並列テスト間で共有しない。
- **トリガー (§4):** 使用中の gilrs 0.11.2 と gilrs-core 0.6.8 のローカル依存ソースで確認。
  Cargo の既定 Windows backend は WGI。`windows_wgi/gamepad.rs` は LeftTrigger / RightTrigger の
  値変化を AxisValueChanged として送り、gilrs `gamepad.rs` のボタン軸変換は、未押下で
  `value >= 0.75` なら ButtonPressed、押下中で `value <= 0.65` なら ButtonReleased を返し、
  その後に ButtonChanged を返す。それ以外は ButtonChanged だけ。
  `map_gilrs_event` 後は「押下／解放 (閾値を跨ぐ場合) → AxisChanged」。`pad_trigger` はこの順と
  ヒステリシスを使用する。gilrs の OS 読み取り自体の検証には読み替えない。
- **台本の稼働状態 (§4):** 既存の `test_script::runtime()` が持つ Option<UiRuntime> の有無を
  `is_active()` で読み取る。起動時は `lib.rs` の App creator で `test_script::start` が完了してから
  App の logic が始まる。Script の選択はテストと通常の ensure_started が同じ純関数を通る。
  UI command 処理中の runtime lock を再取得しないよう、差し込み口は Script を明示して開始する。
- **snapshot (§4):** 既存の App adapter は `ui_fullscreen.rs` の `test_script_snapshot`。
  App 所有の `GamepadPhysicalInput` → runtime から診断を読み、`TestScriptSnapshot.gamepad` として
  Rhai map に投影する。UI command は既存の `ui_update` に App の差し込み callback を渡す。
  ack は snapshot 公開を待たず、UI 応答で受け取った Arc の件数を直接読む。
- **X リング (§4):** `gamepad_input.rs` の方向押下は West 保持中に方向を記録し、West の解放で
  `finish_gamepad_west_release` → Ring(direction) → 通常の割り当て実行へ進む。
  `ring_shortcut.rs` の既定 grid profile の先頭 (Up) は GridParentFolder。
  台本は X 押下 → 上押下 → X 解放 → 上解放の順を維持する。
- 全 pad 命令、名前／数値検査、保持状態、5 秒の UI 応答＋drain 待ち、unchecked の後始末を追加。
  通常入力は runtime.finish 後に積まない。終了時の Disconnected はキーの Cancel と同じ終端処理として、
  既に finish が立っていても一度だけ積む。未差し込みの台本では切断を積まない。
- GamepadBasic、使い捨て PNG fixture、runner の登録・証跡保存と台本 API 文書を追加。
  専用 fullscreen viewport は既存 root paint owner の対象外なので、自動検査は全画面状態・ページ番号・
  画像 identity・page_ready を使い、実表示は全 viewport の screenshot を証跡にする。
  root の paint フラグを全画面の描画成功判定には流用しない。

### 7.2 検証

- feature なしの core `cargo check`: 成功 (3m09s)。
- `portable,test-script` の core `cargo check`: 成功 (2m52s)。
  ログ: `target/padinject-check-script.log`。既存の警告あり。
- fixture の Python テスト: 4 件成功。PowerShell の構文検査はエラー 0 件、runner は ASCII のみ。
- 引数なし `scripts/test-focused.ps1`: 文書・runner・fixture の未対応パスを検出して非ゼロ終了。
  この実行ではテストを選択していない。Rust の対象パスを明示した実行を別途行う。
- 初回の Rust 焦点テストは TurboJPEG の依存ビルド (MSBuild) が非ゼロで止まり、テスト未実行。
  `-j 1` は nextest の実行並列数でありビルド並列数ではないため、その再試行でも同じ段階で停止した。
  ワークツリー内の同一 CMake build directory の単独・直列 build は成功。
  正しいビルド並列指定 `--build-jobs 1` では依存ビルドを通過した。
  ログ: `target/padinject-tests-narrow.log`、`target/padinject-tests-narrow-retry.log`、
  `target/padinject-turbojpeg-rebuild.log`、`target/padinject-tests-narrow-j1.log`。
- その後の本体テストコンパイルは、新設した pointer test の WorkerState 参照パスが一段浅く
  E0433 (1 件) になった。`super::super::gamepad_input` へ修正して再実行する。この段階もテスト未実行。
- 修正後の本体 test build は成功 (10m21s) したが、一覧取得時に依存 DLL 不足
  (`0xc0000135`) で停止し、テスト未実行。`development-build-and-test.md` の手順どおり、
  この worktree の `vendor/ffmpeg/bin` の 6 DLL を `target/debug/deps` へ実ファイルとしてコピーし、
  コピー元・先の SHA256 一致を確認した。既存の異なる DLL は上書きしない手順を使った。
  ログ: `target/padinject-tests-narrow-final.log`。
- `cargo nextest run -p mimageviewer --lib --features portable,test-script --build-jobs 1
  -E 'test(/test_script::gamepad_input/)'`: **8 passed / 0 failed** (0.380s)。
  ログ: `target/padinject-tests-narrow-run.log`。実 API を登録した Engine で GamepadBasic を
  コンパイルする 1 件を含む (台本・アプリは実行しない)。
- 同 command の `-E 'test(/gamepad|test_script|pad_/)'`: **344 passed / 0 failed** (19.880s)。
  ログ: `target/padinject-tests-related.log`。producer の観測・容量・状態・件数、背景押下の抑止、
  台本 API と既存 gamepad／test_script の回帰を含む。新規回帰は計 12 件。
- `.\scripts\test-focused.ps1 -Path src/gamepad.rs,src/test_script.rs,src/app/gamepad_input/physical.rs
  -Features portable,test-script`: **13 + 154 + 8 = 175 passed / 0 failed**。
  ログ: `target/padinject-test-focused.log` と `target/test-focused/runs/` の各 run の JUnit。
  上記 filter と重複するため、件数を合算して unique な検証件数とはしない。
- `cargo fmt` (引数なし) を実行済み。最終 `cargo fmt --check` と `git diff --check`: exit 0。
- 終了処理の修正を含む最終ソースの `portable,test-script` core check: exit 0 (23.71s)。
  ログ: `target/padinject-check-script-final.log`。
  製品ビルド対象の共有処理は最初の feature なし check 以後、整形以外の意味変更なし。

実アプリ・ui-smoke は未実行。本体 lib 全件は依頼どおり実行せず、設計担当のロック取得後の gate に残す。
コミットも行わない。製品機能を変更しない test-script 診断であり、通常 profile の build-dev 確認用 binary は
今回の差し込みを使えないため作成しない。設計の主要な前提との相違は見つかっていない。
