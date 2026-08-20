# 修飾キー再設計のための実機観測 (計装のみ、挙動は変えない)

正本の設計: [modifier-ownership-design.md](modifier-ownership-design.md)。
着手前に [CLAUDE.md](../../CLAUDE.md) の「バグ修正の一般原則」を読むこと。
**これは計装だけの作業。production の挙動 (consume / dispatch / guard / keymap / 描画 /
focus / foreground) は 1 行も変えない。**

## 1. 目的

修飾キー所有権の再設計は、**推測では決められない 2 点**で止まっている。実機で確定させる。

| # | 問い | なぜ推測で決められないか |
| --- | --- | --- |
| Q1 | **StickyKeys** で latch / lock された修飾キーは、`GetAsyncKeyState` / `GetKeyState` / egui の どれにどう映るか | 設計は「取得時に物理状態から seed する」。StickyKeys は**物理的に離した後も latch する**ので、物理サンプルが latch を映さないなら seed が利用者の意図と食い違う |
| Q2 | presenter の **transient `AttachThreadInput`** は、実際にキー状態を reset するか | Microsoft は attach / detach 後に `GetKeyState` / `GetKeyboardState` が reset されると明記している。現行コードは foreground 確保のたびに attach → detach しているので、**これが起きているなら epoch 境界の設計がそこに依存する** |

## 2. Q1 — StickyKeys

既存の `input/modifier_probe` ([modifier_probe.rs](../../src/modifier_probe.rs)) の snapshot を拡張する。
発火条件は**変えない** (key / wheel / 2 秒 heartbeat のまま。修飾キーの値に依存させない原則も維持)。

追加するフィールド:

- **sided の物理状態**: `GetAsyncKeyState` で `VK_LCONTROL` / `VK_RCONTROL` / `VK_LSHIFT` /
  `VK_RSHIFT` / `VK_LMENU` / `VK_RMENU`
- **同じ 6 つを `GetKeyState` でも**取る。**`GetKeyState` は呼び出しスレッドのキュー相対**なので、
  async との差そのものが観測対象である
- **StickyKeys の設定**: `SystemParametersInfoW(SPI_GETSTICKYKEYS)` の `STICKYKEYS.dwFlags`
  (`SKF_STICKYKEYSON` / `SKF_AVAILABLE` / `SKF_TRISTATE` などをそのまま数値で載せる)

既存の `egui_ctrl` / `os_ctrl` はそのまま残す。**3 つの出所が同じ瞬間に何と答えるかを 1 行で
比較できる**ことが目的。

## 3. Q2 — `AttachThreadInput`

`claim_foreground` ([native_window.rs:985](../../src/video/native_window.rs:985)) の attach span に、
**span 内 4 点**の snapshot を出す。イベント 1 件に 4 点をまとめてよい。

1. attach を試みる**直前**
2. `AttachThreadInput(.., true)` の**直後** (attach に成功した場合のみ)
3. `SetForegroundWindow` / `SetActiveWindow` / `SetFocus` の**直後**
4. `AttachThreadInput(.., false)` の**直後**

各点で §2 と同じ sided async / sync を取る。あわせて記録する:

- `this_tid`、`foreground_tid`、**partner が mIV 自身の UI スレッドか**
- `attach_ok` (既存の `attached`)
- **`detach_ok`** — 現在 `let _ =` で捨てている戻り値 ([native_window.rs:1008](../../src/video/native_window.rs:1008))。
  **記録のために受け取るだけで、分岐は足さない**

この span は foreground 確保のときだけ走るので出力量は小さい。**rate limit は付けない**
(頻度が低く、取りこぼすと観測にならない)。

## 4. 制約

- `crate::perf::is_enabled()` が false のときは何も出さない。
- **`request_repaint` / `request_repaint_after` を呼ばない** (静止時の完全 sleep を要求する
  リリース必須ゲート `check-idle-health.ps1` が落ちる)。
- production の分岐・consume・dispatch・focus / foreground の挙動を変えない。
  `detach_ok` は**記録するだけ**で、失敗時の処理を足さない (それは設計側の仕事)。
- 既存の `modifier_probe` の**発火条件を修飾キーの値に依存させない**原則を壊さない。

## 5. テスト

- 追加フィールドが snapshot に載ること (sided async / sync / sticky flags)。
- 発火条件が修飾キーの値に依存しないことを固定する既存テストを壊さないこと。
- `claim_foreground` の 4 点 snapshot が、attach 成功時と非成功時の両方で出ること
  (非成功時は 2 点目を省く)。
- perf 無効時に何も出ないこと。

## 6. 観測手順 (利用者に渡す)

**Q1 (StickyKeys)**

1. Windows の設定で StickyKeys (固定キー機能) を ON にする
2. mIV をフルスクリーンにして、<kbd>Ctrl</kbd> を **1 回だけ叩いて離す** (latch させる)
3. そのまま <kbd>BS</kbd> か矢印を 1 回押す
4. StickyKeys を OFF に戻す

**Q2 (`AttachThreadInput`)**

1. mIV 以外のアプリを前面にする
2. mIV の動画を再生して presenter に前面を取らせる (= `claim_foreground` を通す)
3. 数回繰り返す

どちらも**再起動せずに**知らせてもらう (perf ログは起動時にローテートするため)。
