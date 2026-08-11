# アプリ内蔵テストスクリプト実行 — 設計の正本

対象: [next-release-backlog.md](next-release-backlog.md) §2.3 (P1)。
これができてから §1.58 (ページ送りの引っかかり) をやり直す。

判定の正本は [display-pipeline.md](display-pipeline.md) §2.5 (要件 R1〜R4 / 2 軸の分離 /
入力信号の安定性 §2.5.2.1 / トレース不変条件 I1〜I5)。**本書は「その判定を機械で回すための
入力側」だけを決める。判定ロジックはアプリに持たせない。**

## 0. なぜ作るか

2026-08-11 に §1.58 の対策を 5 回直して 5 回とも外し、削除して出荷した。失敗の根本は
設計ではなく**検証**で、自分で再現できないまま毎回実機確認に依存し、1 往復 1 仮説しか
試せなかった。

外部プロセスからのキー注入は試して駄目だった。`SendInput` が成功を返し
(`inserted=1 lastError=0`)、対象ウィンドウがフォアグラウンド (`fgPid == ourPid`) でも、
アプリが就寝から起きなかった (perf log で t=3.3 秒以降フレーム 0)。原因未特定。
フォアグラウンド / デスクトップ / セッションに依存する層を挟むこと自体が弱い。

## 1. 差し込む層 (最重要の決定)

### 1.1 実機のキー入力は 2 つの表現を同時に生む

[key_input.rs](../src/key_input.rs) の subclass proc は edge を積んだあと
**`DefSubclassProc` を呼んでいる**。したがって 1 回の物理 `WM_KEYDOWN` は必ず 2 経路に出る。

```
WM_KEYDOWN ├─ push_edge          → key_input queue   … keymap の consumer が読む
           └─ DefSubclassProc    → winit → egui      … egui event queue を読む consumer
```

**§1.58 の静止画 ←→ は後者を読む。** 実経路は
[ui_fullscreen.rs:14753](../src/ui_fullscreen.rs:14753) →
[keyboard_input.rs:172](../src/keyboard_input.rs:172) `consume_fullscreen_raw_key` →
`ctx.input_mut(|i| i.consume_key(..))`。
`ui_fullscreen.rs:28575` の `consume_fixed_chord` は**動画タイルの経路**であって静止画ではない。

> **この読み違いが最初の設計を壊しかけた。** key_input にだけ注入していたら、静止画の
> ページ送りは一度も発火せず、「合成入力は届いているのにページが動かない」を追うことになった。

### 1.2 押下状態 (level) の供給源

| 系統 | 経路 | 既存 seam |
| --- | --- | --- |
| edge | subclass → `pending` → `begin_frame()` → `frame` → `keymap::consume_*` | あり (`set_test_frame*`、`#[cfg(test)]`) |
| **level** | `GetAsyncKeyState` 直読み ([keymap.rs:7274](../src/keymap.rs:7274) / [:7315](../src/keymap.rs:7315)) | **無し** |

§2.5.2.1 は「edge ではなく押下状態を読め」と定めている。**テストが押下状態を作れないなら
§1.58 は検証できない。** ここが本計画の存在理由。

### 1.3 決定

**`key_input` が単一の typed timeline を所有し、そこから 2 表現を導出する。**

- 実入力の routing は**変えない**。テスト基盤を作るために、まだテストできない層に手を
  入れるのは §1.58 の反省 (先にテスト基盤) と矛盾する。
- 「両方に入れる」のは二重定義ではなく §1.1 の忠実な模写。片方にしか入れない形が実機と違う。

却下した案:

| 案 | 却下理由 |
| --- | --- |
| (b) `keymap::key_held_chord` に差す | `key_held_from_os_sources` の物理 slot routing を飛ばす = **壊れている層を迂回してテストする**。`modifier_held_via_os` の呼び出し元もカバーできない |
| (c) 自 HWND へ `PostMessage` | `GetAsyncKeyState` は動かないので level は別途必要になり、結局 seam が 2 モジュールに割れる |
| (A) 実入力の静止画矢印を先に key_input へ統合 | 方向としては正しいが、**harness ができる前に実入力 routing を変える**ことになる。`FullscreenRawKeyPermit` の `FocusedUi` passthrough 契約 ([keyboard_input.rs:108](../src/keyboard_input.rs:108)) を壊していないことを検証する手段がまだ無い。**harness の後に、harness で守りながら行う** |

## 2. 同期点は ROOT の input plugin

`begin_frame()` で materialize しては**間に合わない**。実行順は次のとおり。

```
① eframe が winit から RawInput を取得
② Context::run に入る
③ egui plugin の input_hook が RawInput を処理     ← ここで fan-out する
④ App::update
⑤   App::update 内で key_input::begin_frame()
⑥   show_viewport_immediate → 子 (fullscreen) viewport の pass → その viewport の input_hook
```

- `begin_frame()` (⑤) で作ると、ROOT の plugin (③) は既に通過済み。embedded fullscreen では
  **egui 側だけ 1 フレーム遅れる**。
- 子 viewport の plugin (⑥) で作ると、`begin_frame()` (⑤) が完了済みなので、**Win32 edge が
  次の ROOT frame まで pending に残る**。

したがって **③ (ROOT の `input_hook`) を外側フレームの入力 barrier** にする。

```
Rhai thread
  └─ 単調時計つき command を送信 + ROOT を wake

SyntheticInputPlugin::input_hook(ROOT, 最初の pass)      ← ③
  ├─ command を timeline へ適用
  ├─ 現在時刻までの Down / due repeats / Up を時系列順に materialize
  ├─ key_input.pending へ全 target 分の KeyEdge を enqueue
  ├─ viewport 別 egui event queue へ enqueue
  └─ ROOT 宛イベントを現在の RawInput へ注入

App::update → key_input::begin_frame()                   ← ④⑤

SyntheticInputPlugin::input_hook(fullscreen viewport)    ← ⑥
  └─ ROOT hook で準備済みの当該 viewport 宛を RawInput へ注入
```

### 2.1 plugin の登録順

plugin は登録順に `input_hook` が走る。IME plugin は
[lib.rs:1087](../src/lib.rs:1087) で登録されている。**合成 plugin はその前**に登録する
(でないと合成 Esc / Enter が [ime_focus.rs:67](../src/ime_focus.rs:67) の IME 正規化を通らない)。

```
SyntheticInputPlugin      ← 追加、最初
ImeInputPlugin
Tab/focus policy
```

### 2.2 同一 RawInput での多重 pass

egui は `request_discard` 時に**同じ RawInput で複数 pass を走らせる**。`input_hook` は pass
ごとに呼ばれるので、素朴に materialize すると二重注入になる。

**規約**: materialize は (viewport, `RawInput.time`) につき 1 回。同じ time で再度呼ばれたら、
**新規生成せず、同じ batch を再注入する**。

### 2.3 対象 viewport がその外側フレームで描かれなかった場合

Win32 edge だけ先に失効する (`begin_frame` が frame をクリアするため)。この場合は成功扱いに
せず、**script を不成立で失敗させる**。

## 3. timeline の規約

### 3.1 コマンドは時刻つき、materialize は時系列順

UI が就寝・停止している間に worker から Down と Up の両方が届き、最終状態の released だけを
適用すると、**本来 Down〜Up の間に出ていたはずの repeat が消える**。

```
Down → due repeats → Up
```

の順で必ず materialize する。時計はシステム時計ではなく `Instant` (単調)。

### 3.2 repeat

- 既定は初期遅延 **250ms** / レート **30Hz** (実測のキーリピート間隔 34ms に合わせる)。
  OS 設定 (`SPI_GETKEYBOARDDELAY` / `SPI_GETKEYBOARDSPEED`) は読まない (再現性優先)。script
  から変更可能にする。
- **狙いは「重いフレーム中に repeat が溜まる」§1.58 の核心条件の再現。** 460ms のフレームなら
  13 個前後が同一フレームに materialize される。1 フレーム 1 個しか作らない形にすると、この
  条件が消える。
- `egui::Event::Key.repeat` は**合成側で true にしない**。egui-winit 自身が全 Key event を
  `repeat: false` で作り、egui が viewport-local な `keys_down` から再計算する。合成も同じに
  する。Win32 `KeyEdge.repeat` は実機どおり立てる。
- **Down / repeat / Up は同じ target viewport へ順序どおり入れる。** Down を ROOT、repeat を
  fullscreen へ入れると後者が初回 press 扱いになる。

### 3.3 modifiers

egui-winit は `ModifiersChanged` で持続状態を更新し、Key event にはその時点の snapshot を
埋める。合成側も 2 つ必要:

- 各 `Event::Key.modifiers` … その edge 発生時点の timeline 状態
- `RawInput.modifiers` … materialize した全 event 適用後の最終 level 状態

Windows では `ctrl=true` なら `command=true` / `mac_cmd=false`。
Win32 `KeyEdge` 側の modifier snapshot も、実 OS ([key_input.rs:615](../src/key_input.rs:615)
の `GetKeyState`) ではなく**同じ timeline のその時点の状態**から作る。

子 viewport には [app.rs:34107](../src/app.rs:34107) の modifier resync が無いので、
**plugin による `RawInput.modifiers` 更新が必須**。

### 3.4 focus 喪失

script 開始時に target の `RawInput.focused` が false なら待機、時間切れなら失敗。
hold 中に focus を失ったら:

1. 合成 level を false
2. 対応する Up を対象 viewport へ注入
3. script run を**「環境不成立」として失敗**

focus 喪失中に repeat だけ注入すると、実機に存在しない入力になる。

## 4. level (押下状態) の chokepoint

```rust
// key_input.rs
pub struct PhysicalKeySlot { vk: u32, extended: bool }
pub fn physical_key_down(slot: PhysicalKeySlot) -> bool
```

armed なら合成 timeline、でなければ `GetAsyncKeyState`。

**seam の型は `bool down` + `vk` だけでは不足**: `VK_RETURN` は Enter/NumpadEnter で共有され
extended bit と viewport latch で分離されている ([keymap.rs:7266](../src/keymap.rs:7266))。
Backslash/IntlYen も scan code で区別する ([keymap.rs:7739](../src/keymap.rs:7739))。

### 4.1 経由させる呼び出し元

| 場所 | 理由 |
| --- | --- |
| [keymap.rs:7274](../src/keymap.rs:7274) `key_held_via_os` | 必須。§2.5.2.1 の押下判定そのもの |
| [keymap.rs:7315](../src/keymap.rs:7315) `modifier_held_via_os` | 必須。`key_held_chord_via_os` が ctrl/shift/alt の一致を要求する |
| [app.rs:34118](../src/app.rs:34118) `resync_egui_modifiers_from_os` | 必須。素通しだと egui 側の `input.modifiers` だけ実 OS を見て食い違う |
| [ui_fullscreen.rs:2476](../src/ui_fullscreen.rs:2476) / [:2488](../src/ui_fullscreen.rs:2488) | FS キャンバスの Ctrl/Shift |
| [native_video.rs:26](../src/app/native_video.rs:26) | stale キー再配送の棄却 |
| [key_input.rs:615](../src/key_input.rs:615) `GetKeyState` (real edge の modifier) | §3.3 |

### 4.2 対象外 (明記する)

- `VK_LBUTTON` の 3 経路 ([detached_window_manager.rs:330](../src/app/detached_window_manager.rs:330) /
  [context_menu.rs:17](../src/ui_dialogs/context_menu.rs:17) /
  [native_window_host.rs:433](../src/video/native_window_host.rs:433)) — マウス
- native presenter の WndProc 経路 ([native_window.rs](../src/video/native_window.rs)) — 別経路
- [app.rs:34160](../src/app.rs:34160) の Ctrl+V — `GetAsyncKeyState` の **low bit** `(v & 1)` を
  読んでおり、`bool down` の seam では再現できない。clipboard は初期スコープ外

## 5. 初期スコープ

**navigation key のみ。** 文字入力 / clipboard / IME / Numpad 区別は対象外と明記する。

egui-winit は全 `WM_KEYDOWN` を `Event::Key` にするわけではない:

- logical/physical どちらにも変換できないキーは Key event を作らない
- **Ctrl+C/X/V は `Copy`/`Cut`/`Paste` へ変換し、Key event を作らずに return する**
- focus 変更由来の synthetic press は無視する (release は処理する)
- printable key は Key に加えて `Text` も作る

Right/Left/Up/Down/PageUp/PageDown/Home/End/Enter/Esc なら `key` と `physical_key` を同じ値に
すれば足りる。文字キー・JIS 記号・Numpad は logical/physical/location/Text の扱いが違うので
初期 API から外す。

## 6. routing target の決め方

**synthetic Down を適用する時点で `GetForegroundWindow()` を key_input の HWND registry で
`ViewportId` へ解決し、Up まで固定する。**

- embedded なら main HWND → ROOT
- fullscreen / detached なら subclass 済み HWND → 対応 viewport
- **未登録なら ROOT へ黙って fallback せず、待機または script 失敗**

これは実際の `WM_KEYDOWN` が source HWND から routing される規則と一致する。hold 中に target が
切り替わっても、物理的な 1 回の Down の所有先を途中で変えない。

> App が毎フレーム publish する案は却下。`fullscreen_viewport_id()` は detached 切替中の複雑な
> 安定化規則を持つ ([ui_fullscreen.rs:7311](../src/ui_fullscreen.rs:7311))。別の場所で
> 「fullscreen なら fs_id、それ以外 ROOT」を再構成すると切替フレームで食い違う。

## 7. スクリプト API

Rhai (前例: [filename_stack_script.rs](../src/filename_stack_script.rs))。**別スレッドで回すが、
スレッドはコマンドを送るだけ。適用は UI スレッド。**

| API | 意味 |
| --- | --- |
| `hold_key(name, ms)` | Down → ms 待つ → Up。**§1.58 に必須** |
| `tap_key(name)` / `release_key(name)` | 単発 / 明示解放 |
| `run_action(ini_name)` | `KeyAction` 直呼び。**入力層のバグは再現できない**ので補助 |
| `sleep(ms)` | |
| `wait_until(cond, timeout_ms)` | UI スレッドが毎フレーム publish する snapshot を読む |
| `set_repeat(delay_ms, hz)` | §3.2 |
| `log(msg)` / `fail(msg)` | perf event + logger / script を不成立で終了 |

`wait_until` が読む snapshot (UI スレッドが publish): `is_fullscreen`, `fs_idx`,
`items_generation`, `focused`, `target_viewport`, `pending_thumbs`, `spread_mode`,
`continuous_reading`。

**command 送信時に worker 側から ROOT の wake (`request_repaint_of(ROOT)`) を必ず呼ぶ。**
Down/Up/Cancel のいずれでも。就寝中の App は command を drain できないため。armed hold 中は
次フレームを継続要求する。

`App::update` には `key_input::begin_frame()` より**前に early return がある**
([app.rs:61895](../src/app.rs:61895) 付近)。`begin_frame` の位置は動かさない (入力処理しない
フレームで edge を消費してしまう)。early return 側が repaint を保証する。

## 8. 有効化ゲート

- cargo feature **`test-script`** (既定 off) — Rhai engine / plugin / CLI 解析
- CLI `--test-script <path>` + **`--data-dir` 必須**
- `scripts/build-dev.ps1` に `-TestScript` スイッチ (既定の build-dev 出力は通常 feature set のまま)

**§4 の chokepoint は feature に関係なく常にコンパイルする。** ここを gate すると「テストして
いるバイナリと出荷するバイナリで入力層が別物」になり、テスト基盤としての意味が消える。
feature off なら armed は常に false で、`GetAsyncKeyState` を呼ぶだけ。

## 9. 判定 — アプリに持たせない

従来どおり `python scripts/analyze_perf.py <jsonl> page-turn --check`。

### 9.1 burst 分割を hold_id にする (必須)

現行は ready 間隔 **300ms 超**で別 burst に切るが ([analyze_perf.py](../scripts/analyze_perf.py)
の `PAGE_TURN_BURST_GAP_SECS`)、**5 回の失敗時の中央値は 463ms** だった。
遅いページが 1 件ずつ別 burst になり、**ページ間の I2/I3 が実質検査されない = false green**。

script の `hold_id` と Down/Up で切る。hold event が無いログは従来の時刻 gap で切る (後方互換)。

### 9.2 「検証不成立」条件

run が次を観測できなかったら、**成功ではなく不成立**として exit 非 0:

- 同じ hold 中に `held=true/edge=あり` と `held=true/edge=なし` の**両方**を観測した
  (§2.5.2.1 の 30Hz 振動を実際に通した証明)
- 複数 repeat が 1 フレームに materialize されたケースを観測した (§3.2 の蓄積条件)
- burst が 1 つ以上ある

### 9.3 アプリが出す事実 (判定しない)

`test_script` カテゴリ:

| kind | 内容 |
| --- | --- |
| `hold_begin` | `hold_id`, `key`, `target_viewport`, `repeat_delay_ms`, `repeat_hz` |
| `hold_end` | `hold_id`, materialize した down/repeat/up の数 |
| `frame_input` | `hold_id`, `held`, `edge_count`, `materialized_in_frame`, `frame_nr` |
| `step` / `precondition` / `fail` | script の進行と前提 |

`fs/page_turn_ready` 等の §1.58 側 event には `input_route` (`fullscreen_raw_arrow` 等) と
`script_hold_id` / `target_viewport` を足し、Python 側で「本当にその経路を通ったか」を必須確認
する。ユーザーが Right を別 `KeyAction` に割り当てていると先行 consumer が egui event を
claim し得る (実機どおりだが §1.58 テストとしては別経路の false green)。**隔離 data-dir は
default keymap で作る。**

## 10. script が満たすべき前提 (静止画 Right が発火する条件)

`wait_until` で確認する。1 つでも崩れると「押しているのにページが動かない」になる。

- target viewport が登録済みで、実際に描画中かつ focused
- 現在アイテムが静止画 (Video でも Audio でもない)
- `fs_music_view_active(fs_idx) == false`
- fullscreen modal / context menu / IME / TextInput / pending focus が無い
- erase / text / conceal / local-adjust / export-crop モードではない
- capture region selection 中ではない
- `FullscreenRawKeyPermit` が `Some` (`FocusedUi` は許可、Modal/TextInput/Unclaimed は不可)
- `continuous_reading_active_for_idx(fs_idx) == false`
  (**横連続読みでは Right はページ送りではなく scroll になる**)
- 1 ページずつ検証するなら spread mode は Single
- フォルダ端ではなく前後に十分なページがある

`video_horizontal_arrow_key` は静止画なら `is_video_fs=false` なので自動的に false。

## 11. この基盤で検証できないもの

合成 seam は Win32 subclass より内側なので、次は対象外:

- subclass の登録漏れ・解除ミス
- WndProc → `RawKeyEdge` 変換のミス
- OS / winit / egui の配送・focus・wake
- 実キーボードの repeat 設定とスケジューリング

つまり**「入力後の §1.58 ロジック」は自動化できるが、「実キーが key_input へ到達すること」は
証明しない**。`RawKeyEdge` 変換の単体テストと、最小限の手動 smoke は残す。

## 12. 実装ステージ

| S | 内容 | 完了条件 |
| --- | --- | --- |
| S1 | timeline + chokepoint + plugin fan-out | 単体テスト。テスト専用 API で Down/repeat/Up を流し、Win32 edge と egui event の**両方**が同じ順序で出ること、level が hold 中 true のままであること |
| S2 | Rhai runner + CLI + feature gate + snapshot publish | `--test-script` で起動して script が完走し、終了する |
| S3 | perf event + `analyze_perf.py` の hold_id 分割と §9.2 の不成立条件 | `scripts/test_analyze_perf.py` に回帰テスト |
| S4 | `page-turn-smoke.ps1` の中身差し替え + self-test script + 本書/backlog 更新 | **§13** |

## 13. §2.3 自体の完了条件

`fs/page_turn_ready` / `page_turn_decision` は現行コードに無いので、**この基盤だけでは
`--check` は `checked bursts=0` の no-op**。§1.58 を待たずに基盤の正しさを証明する必要がある。

self-test (`scripts/page-turn/selftest.rhai`) が次を示すこと:

1. FS に入って Right を 5 秒 hold → **ページが N 以上進んだ** (= egui 経路に届いている)
2. hold 中ずっと `key_held_chord` が true だった (= level 経路に届いている)
3. §9.2 の 2 条件を観測した (= 振動と蓄積を実際に通した)

**これが通るまで §1.58 に着手しない。**

## 14. 後続 (この基盤ができてから)

- **案 A**: 実入力の静止画固定矢印を Win32 KeySlot queue へ統合する
  (permit-aware な `consume_fullscreen_fixed_chord`)。§1.2 の非対称を恒久的に消す。
  本 harness の script timeline で変更前後の nav と描画 trace を比較しながら行う。
- §1.58 のやり直し (対策 3 → 対策 1 の順に評価)。
- タッチ対応 (§4.7、v2.13.0 出荷済み) の回帰も同じ手法で回す。
