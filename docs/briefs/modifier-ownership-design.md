# 修飾キー所有権の再設計 (design proposal / 合意用) — 第 2 版

**状態: 提案。第 1 版は Codex Sol の批評により中核が否決された。本版はその批評を全面的に採り入れた
改訂。実装は再度の合意後。** 凍結領域 ([detached-rework-plan.md](../detached-rework-plan.md) §2)
に触れるため、合意が取れたら §11 へ記録する。

## 0. 第 1 版が否決された理由 (記録として残す)

第 1 版は「プロセス全体の modifier transition timeline を作り、`GetMessageTime()` 順に並べる」
だった。これは成立しない。

- `GetMessageTime()` は**共通のタイムスタンプであって順序プリミティブではない**。ms 精度なので
  同一 ms が普通に起き、異なるキュー間の tie-breaker を OS は公開していない。値は wrap もする。
- **late publication を解けない**。presenter のキューに `CtrlUp(t=100)` が残っているのに
  presenter スレッドがまだ dequeue しておらず、UI が先に `BS(t=101)` を処理したら、UI が
  timeline を引いた時点で `CtrlUp` は存在しない。後から挿入しても、**刻み終わった BS は直らない**。
- `SendMessage` 等の nonqueued message はキューを迂回し、その WndProc の `GetMessageTime()` は
  当該メッセージの時刻ですらない。`WM_SETFOCUS` も nonqueued なので、focus seed を同じ timeline
  に自然に並べることもできない。

この欠落は実装漏れではなく、**owner が正しい状態を生成できるという不変条件の欠落**である。
そのまま実装すれば、同一 ms と late publication に対して後から grace / retry を足したくなり、
禁止された症状パッチへ滑る。第 1 版は破棄する。

## 1. 現状 — 修飾キーの出所が **4 つ**あり、正を持つ主体がいない

| # | 出所 | 実体 | 使っている所 |
| --- | --- | --- | --- |
| A | egui イベントの刻印 | winit | `consume_key` (固定キー)、keymap の egui fallback ([keymap.rs:7497](../../src/keymap.rs:7497)) |
| B | Win32 `KeyEdge` の刻印 | `GetKeyState` ([key_input.rs:2007](../../src/key_input.rs:2007)) | keymap の主経路 ([keymap.rs:7472](../../src/keymap.rs:7472)) |
| C | OS レベル読み | `GetAsyncKeyState` | ホールド系、一部レベル判定、`resync_egui_modifiers_from_os` |
| **D** | **presenter 独自の刻印** | **presenter スレッドの `GetKeyState`** ([native_window.rs:1939](../../src/video/native_window.rs:1939)) | `NativeVideoKeyEvent`、overlay 自前の modifier state と合成 egui event ([render_core.rs:4320](../../src/video/native_presenter/render_core.rs:4320)) |

D は第 1 版で見落としていた。「presenter の WndProc が publish する」だけでは D が残る。

## 2. 目標とする構造 — timeline をやめ、**入力キュー単位の状態 + focus epoch**

Windows のキーボードメッセージは、**その時点で focus を持つウィンドウを作ったスレッドのキュー**へ
送られ、focus が移るまでそこに留まる。したがって**同一キュー内の FIFO 順序が権威**であり、
キューをまたぐ全順序は要らない。

問題が起きるのは focus 遷移の一点だけで、そこには `WM_SETFOCUS` という明示的な修理点がある。

**owner は 1 つ。ただし提供する事実は 1 個の bool ではなく、型の異なる 2 つ。**

| 型 | 意味 | 出所 |
| --- | --- | --- |
| `DeliveryModifiers` | **そのメッセージが自分のキュー内で占める位置**での修飾キー状態 | キュー順に更新した owner の状態 |
| `CurrentModifiers` | **今**の物理状態 | `GetAsyncKeyState`。`Known(..)` / `Unavailable` |

`docs/keymap-spec.md` が既に区別している delivery-time / handling-time の 2 つを、owner が別の型で
出すだけである。「真実を 1 つの bool にする」ことが目的ではない。

### なぜこれで今回の故障が直るか

Ctrl 押下は UI キューへ、focus が presenter へ移る、**Ctrl 離しは presenter のキューが消費**、
focus が UI へ戻る。ここで `WM_SETFOCUS` が**新しい focus epoch を開始し、物理状態から seed する**。
キューをまたいで失われた遷移は、この 1 点で回収される。stale な思い込みは epoch 境界で捨てられる。

### なぜ高速 Ctrl+BS が壊れないか

CtrlDown、BS、CtrlUp はすべて**同一キュー・同一 epoch**で、間に seed が入らない。BS のエッジは
自分のキュー順の状態から刻まれ、`GetKeyState` の現在の挙動と一致する。`GetAsyncKeyState` へ
単純に差し替えた場合に壊れるケースが、ここでは壊れない。

### 表現

- 修飾キーは**左右別の bitset** (LCtrl / RCtrl / LShift / RShift / LAlt / RAlt) で持ち、集約は OR で導く。
  **AltGr** は配列によって RAlt が `LCtrl + RAlt` として見えるため、集約 bool では表現できない。
  現 `KeyEdge` は集約のみ ([key_input.rs:243](../../src/key_input.rs:243))。
- seed は bool ではなく `Known(state)` / `Unavailable`。`GetAsyncKeyState` は inactive desktop や
  UIPI で失敗時にも 0 を返すため、**失敗を「全部上がっている」と読まない**。`Unavailable` のときは
  前 epoch の状態を保持する。

## 3. 接続すべき producer (完全な棚卸しを実装前に確定する)

| producer | 現状 | 対応 |
| --- | --- | --- |
| UI スレッドのキーボードメッセージ全体 | per-HWND サブクラスのみ。**passive detached viewport は未登録** ([ui_fullscreen.rs:11666](../../src/ui_fullscreen.rs:11666))、IME / common dialog の HWND も漏れる | 既存の `WH_GETMESSAGE` hook ([lib.rs:571](../../src/lib.rs:571)) を producer 境界にする |
| presenter スレッド | pump が `MSG` を保持している ([native_window.rs:1103](../../src/video/native_window.rs:1103)) | `PeekMessageW` 直後を producer 境界にする |
| `native_key_event` の刻印 (出所 D) | presenter スレッドで独自に `GetKeyState` | owner から導く |
| overlay の modifier state と合成 egui event | 独自 ([render_core.rs:4320](../../src/video/native_presenter/render_core.rs:4320)) | owner から導く |
| test-script の synthetic timeline | `KeyInputState` が所有し armed 中は OS reader を差し替え ([key_input.rs:1072](../../src/key_input.rs:1072)) | 同じ chokepoint の arming override として維持 |

**nonqueued (`SendMessage`) のキーボードメッセージは位置を捏造しない。** `InSendMessageEx` で
判別し、**キーボード注入の対応範囲外であることを明記する**。

## 4. 段取り (第 1 版から変更)

| 手順 | 内容 | 挙動変更 |
| --- | --- | --- |
| **S0** | 契約を型とテストで固定する。2 つの型、左右 bitset、focus epoch、seed の可用性 | **なし** |
| **S1** | 全 producer を owner へ接続。presenter の離しを捨てない。D と overlay も owner から導く | **ここで今回の故障が直る** |
| **S2** | consumer を owner へ寄せる。固定キー (Esc / Delete / BS / 編集モードの矢印)、keymap、一覧の矢印。双子除去が再び stale な刻印を見ている件 ([keymap.rs:7495](../../src/keymap.rs:7495)) も直す | 一貫性 |
| **S3** | 残存する直接 `GetKeyState` / `GetAsyncKeyState` / egui resync を監査し、撤去するか owner API 配下へ封じる | 掃除 |

**第 1 版の「手順 1」(子 viewport の closure に `GetAsyncKeyState` resync をもう 1 つ足す) は取り下げる。**
S1 が owner の current-level API で置き換える対象を先に増やすだけであり、**別の真実を残すため症状パッチ寄り**
という批評を受け入れる。ホイール症状は S1 / S2 で直る。

consumer 統合 (S2) を producer 統合 (S1) より先にやらない点は第 1 版のまま。汚染され得る経路へ
全部を寄せることになるため。

## 5. 症状パッチではないと考える根拠 (合意を取りたい点)

- 真実の出所を **4 から 1 owner / 2 型**へ減らす。guard / delay / retry / fallback / reset を足さない。
- **時間窓を使わない** (§2 規則 5)。順序は各キューの FIFO、修理点は `WM_SETFOCUS` という
  OS のイベント、レベルは物理状態。ms の tie も watermark も要らない。
- 直す対象は「あるキューが知っている事実が別のキューから見えない」ことで、修正はその事実を
  epoch 境界で回収させること。症状の側 (キーが効かない) に条件を足すのではない。
- App に新しい detached 用 bool / Option を足さない (§2 規則 3)。owner は `key_input` に置く。

## 6. 実装前に決着させる論点

- **StickyKeys**: 修飾キーは物理的に離れた後も latch / lock される。「focus 時に物理状態から seed」を
  そのまま正としてよいかは**実機観測で確定してから**決める。推測で書かない。logical と physical を
  型で区別することは S0 に含める。
- **注入入力**: `SendInput` は input stream へ入るので通常経路に来る。`PostMessage` は queued なので
  扱えるが物理状態は変わらないため、seed が論理状態を上書きする規則が要る。`SendMessage` は対象外と明記。
- **`AttachThreadInput`**: OS 側でキュー結合する案は存在するが、attach 時に key state が reset され、
  focus / activation の所有権まで結合する。現コードは foreground claim の間だけ transient に
  attach / detach しており ([native_window.rs:1001](../../src/video/native_window.rs:1001))、**この操作自体が
  key state reset を起こすことを新設計の脅威モデルへ入れる**。恒久 attach は採らない方針だが、
  transient attach の影響は S0 で観測する。

## 7. 検証 — 第 1 版の目標は誤りだった

**「`diverged == 0`」を目標にしない。** 正しい高速 Ctrl+BS では
「delivery 時は Ctrl=true、現在レベルは false」が**意図どおり食い違う**。現在の
`input/modifier_probe` は frame 単位の egui / OS 比較なので、エッジ刻印の正しさも late publication も
証明しない。

代わりに:

- エッジ単位で **delivery 修飾キーと epoch id** を記録し、**epoch 境界をまたいで**物理状態と
  食い違うエッジを異常とする。同一 epoch 内の食い違いは正常。
- 高速 Ctrl+BS は「食い違うのが正しい」ケースとしてテストに固定する。

回帰テストの最低線:
フルスクリーンで Ctrl 押下、presenter で離す、戻る、Esc / BS / 矢印 / ホイール。
main / fullscreen / detached の各 viewport。フレーム前に離される高速 Ctrl+BS。
AltGr 配列の RAlt。StickyKeys ON。seed が `Unavailable` のとき。
Shift / Alt のベクター編集。同一フレーム内の同一キー複数回。兄弟 viewport が消費しないこと。
