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

### 3.3 取り込みの確認 (ack)

台本の各命令は、積んだ入力を App が取り込む (`GamepadRuntime::drain` がキューから取り出す) まで待ってから戻る。

- 積んだ時点の通し番号 `injected` と、drain が取り出した件数の累計 `drained` を持つ
  (3.2 により `drained` は台本の入力だけ)。命令は `snapshot().gamepad.drained >= 自分の番号` を待つ。
  期限 (5 秒) を過ぎたら台本の失敗にし、`enabled`・`foreground`・`injected`・`drained` を添える。
- これで「押下と解放が別の logic 呼び出しで取り込まれる」ことが保証され、`pad_hold` の保持時間は
  App が押下を取り込んでからの時間になる。

### 3.4 前提条件

各命令は送る前に、最新の snapshot で `gamepad.enabled` と `gamepad.foreground`
(`foreground_app_hwnd().is_some()`) を確かめ、満たさなければ理由つきのエラーにする
(`wait_until(|s| s.gamepad.foreground, ms)` を促す)。黙って捨てない。
egui の `focused` は要求しない (本物のパッドも見ない)。

無効中・背景中に入力が捨てられる挙動は、既存のユニットテストが見ている。台本の命令は
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
5. X を押したまま十字キー上 (X リングの上 = 既定で「親フォルダへ」) → X を離す → `current_folder_path` が
   `library` になる。リングの確定に要る実際の手順 (方向の入れ方・離す順) はコードで確かめて台本に書く。
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

## 6. 実装記録

(実装後に追記)
