# 修飾キー所有権の再設計 (design proposal / 合意用) — 第 3 版

**状態: 提案。第 1 版は中核が否決、第 2 版は方向を承認されたが 6 点の未確定で保留。
本版はその 6 点を確定させたもの。実装は合意後。** 凍結領域
([detached-rework-plan.md](../detached-rework-plan.md) §2) に触れるため、合意後に §11 へ記録する。

## 0. 却下・修正の履歴 (同じ轍を踏まないための記録)

**第 1 版 (否決)**: プロセス全体の transition timeline を `GetMessageTime()` 順に並べる案。
`GetMessageTime()` は共通タイムスタンプであって順序プリミティブではない。ms 精度で同一 ms の
tie-breaker が無く、値は wrap し、`SendMessage` / `WM_SETFOCUS` は nonqueued でキューを迂回する。
決定的なのは **late publication**: 別スレッドのキューに未 dequeue の `CtrlUp` があると、
その時点の timeline に存在せず、**後から挿入しても刻み終わったエッジは直らない**。
owner が正しい状態を生成できるという不変条件そのものが欠けていた。

**第 2 版 (方向は承認、6 点保留)**: キュー単位状態 + `WM_SETFOCUS` epoch。方向は正しいが
epoch 境界が不足し、`Unavailable` の扱いが不変条件と矛盾し、故障が直る段の見積もりが誤っていた。

## 1. 現状 — 修飾キーの出所が 4 つあり、正を持つ主体がいない

| # | 出所 | 実体 | 使っている所 |
| --- | --- | --- | --- |
| A | egui イベントの刻印 | winit | `consume_key` (固定キー)、keymap の egui fallback |
| B | Win32 `KeyEdge` の刻印 | `GetKeyState` ([key_input.rs:2007](../../src/key_input.rs:2007)) | keymap の主経路 ([keymap.rs:7471](../../src/keymap.rs:7471)) |
| C | OS レベル読み | `GetAsyncKeyState` | ホールド系、レベル判定、`resync_egui_modifiers_from_os` |
| D | presenter 独自の刻印 | presenter スレッドの `GetKeyState` ([native_window.rs:1937](../../src/video/native_window.rs:1937)) | `NativeVideoKeyEvent`、overlay の modifier state と合成 egui event |

## 2. 故障の機構 (確認済み)

1. UI キューで CtrlDown を dequeue、UI スレッドの `GetKeyState` で `KeyEdge.ctrl=true` を刻印
2. focus が presenter へ移る (別スレッドの `PeekMessageW` pump、[native_window.rs:1088](../../src/video/native_window.rs:1088))
3. **CtrlUp は presenter のキューで dequeue される**
4. **Windows のキーボード状態テーブルは、そのスレッドが自分のキューから keyboard message を
   remove したときだけ更新される。**別スレッドの dequeue では更新されない。よって UI スレッドの
   状態は CtrlDown のまま
5. UI へ戻った後の Backspace が stale な `GetKeyState` で `ctrl=true` と刻印され、
   `FsBackToList` ではなく `FsClearAdjust` が成立する

## 3. 構造 — 入力キュー単位の状態 + **入力ルーティング epoch**

キーボードメッセージは focus を持つウィンドウのスレッドキューへ送られ、focus が移るまでそこに
留まる。**同一キュー内の FIFO が権威**であり、キューをまたぐ全順序は要らない。食い違うのは
ルーティングが移る瞬間だけで、そこを epoch 境界にする。

**owner は 1 つ。提供する事実は型の異なる 2 つ。**

| 型 | 意味 |
| --- | --- |
| `DeliveryModifiers` | そのメッセージが**自分のキュー内で占める位置**での修飾キー状態 |
| `CurrentModifiers` | **今**の物理状態 |

### 3.1 epoch 境界 = そのキューがキーボードルーティングを獲得したこと (第 2 版から変更)

`WM_SETFOCUS` 単独では足りない。以下すべてを境界とする。

| 境界 | 理由 | 観測手段 |
| --- | --- | --- |
| キュー内のいずれかの HWND が keyboard focus を獲得 | 対象は mIV の特定 HWND ではなく**キュー**。common dialog / IME HWND / 同スレッドの別 child も含む | `WH_CALLWNDPROC` (スレッド全体、sent message を見る) または `WH_CBT` の `HCBT_SETFOCUS` |
| キュー内のいずれかの HWND が **activate** された (最小化を除く) | **focus が無くても** active window のキューへ `WM_SYSKEYDOWN/UP` が届く。`WM_SETFOCUS` を待つと閉じられない経路が残る | 同上 (`WM_ACTIVATE` / `HCBT_ACTIVATE`) |
| **`AttachThreadInput` の成功した attach / detach** | Microsoft は attach / detach 後に `GetKeyState` / `GetKeyboardState` の状態が **reset される**と明記。attach 中は両スレッドが入力状態と focus を共有する | 呼び出しは自コード ([native_window.rs:985](../../src/video/native_window.rs:985), [:1001](../../src/video/native_window.rs:1001))。呼び出し側が直接 publish |

`WM_SETFOCUS` は **sent message** なので、現行の `WH_GETMESSAGE` hook
([lib.rs:570](../../src/lib.rs:570)) では観測できない。現行 key subclass も `WM_KILLFOCUS` しか
扱っていない ([key_input.rs:2034](../../src/key_input.rs:2034))。**focus / activation 用の producer を
別に立てる。**

attach は**キュートポロジの変更**として扱う。attach 相手が別の mIV 入力キューなら**両方を
invalidate** し、各 producer が次のローカル dequeue の前に invalidation generation を観測して
seed し直す。呼び出しスレッドが相手の状態を直接上書きしない。

### 3.2 seed の可用性 (第 2 版の記述を撤回)

**第 2 版の「`Unavailable` なら前 epoch の状態を保持する」は誤りだった。** stale な状態を新 epoch の
**既知値として延命**するもので、直そうとしている不変条件そのものに反する。撤回する。

`GetAsyncKeyState` の 0 は「上がっている」と「API 失敗」の両方を意味し、戻り値だけでは区別できない。
よって:

- sampling の**事前条件** (自プロセスが foreground / active、対象デスクトップで呼べる) が成り立つ
  ときだけ `Known(state)` とする。epoch 境界は獲得の瞬間なので、通常はここが成立する
- 成り立たないときは epoch を **`Unknown` で開始**する。**旧値を新 epoch の delivery truth として
  刻印しない**。last-known は診断用に保持してよいが、delivery には使わない
- `Unknown` は**修飾キーごと**に解消する。その epoch 内で当該修飾キーの transition を最初に
  観測した時点で `Known` になる
- 解消前の `Unknown` を chord 照合でどう扱うかは **not held を既定**とする。理由は失敗方向の
  非対称性: 誤って held とすると <kbd>BS</kbd> が `FsClearAdjust` (補正を破棄) になり、
  誤って not held とすると `FsBackToList` (一覧へ戻る) になる。**破壊的でない側へ倒す。**
  `Unknown` のまま消費されたエッジは perf event に必ず出し、実際に起きるかを観測する

### 3.3 表現と AltGr の照合方針 (S0 で確定)

- 修飾キーは**左右別の bitset** (LCtrl / RCtrl / LShift / RShift / LAlt / RAlt)。現 `KeyEdge` は
  集約のみ ([key_input.rs:243](../../src/key_input.rs:243))。
- **左右 bitset は必要条件だが十分条件ではない。**現在の exact match は集約 `ctrl/shift/alt` だけを
  見る ([keymap.rs:1216](../../src/keymap.rs:1216))。AltGr 配列では RAlt が `LCtrl + RAlt` として
  見えるため、**単純に OR すると `Ctrl+Alt` の chord に誤一致する**。
- **提案する方針**: RAlt が down のとき、chord 照合では **LCtrl の寄与を無視**する (AltGr とみなす)。
  RCtrl の寄与は残す。これにより AltGr 入力が `Ctrl+Alt` 割り当てを誤爆しない。
  `Ctrl+Alt` を意図する利用者は LAlt または RCtrl を使う。この方針は S0 でテストに固定する。

## 4. 段取り (第 2 版から変更)

| 手順 | 内容 | 挙動変更 |
| --- | --- | --- |
| **S0** | 契約を型とテストで固定。2 つの型、左右 bitset、epoch とその 3 境界、seed の `Known` / `Unknown`、AltGr 方針 | なし |
| **S1** | 全 producer を owner へ接続 (§5) | 単体では出荷しない |
| **S2** | 全 consumer を owner へ寄せる (§6)。双子除去 ([keymap.rs:7495](../../src/keymap.rs:7495)) も直す | **S1+S2 を 1 つの単位として着地させる** |
| **S3** | 残存する直接読みを監査し撤去または owner API 配下へ封じる。**既知の直接読みは実装前に一覧化する** (S3 で初めて発見する扱いにしない) | 掃除 |

**「故障が直るのは S1」という第 2 版の見積もりは誤りだった。** S1 が直すのは `KeyEdge` を主経路に
する keymap action だけで、観測された故障には egui modifier を読む Esc
([ui_fullscreen.rs:18304](../../src/ui_fullscreen.rs:18304))、fullscreen wheel
([ui_fullscreen.rs:22007](../../src/ui_fullscreen.rs:22007))、grid wheel
([app.rs:35962](../../src/app.rs:35962))、grid arrows ([app.rs:33723](../../src/app.rs:33723)) が
含まれる。**S1 と S2 は実装チェックポイントとしては分けるが、挙動変更としては 1 つの単位で着地させる。**

## 5. producer 一覧 (第 2 版から追加)

| producer | 現状 | 対応 |
| --- | --- | --- |
| UI キューの keyboard message | per-HWND subclass のみ | `WH_GETMESSAGE` を境界にする。**`wParam == PM_REMOVE` のときだけ publish** する (現行 hook は `wParam` を見ていない ([lib.rs:571](../../src/lib.rs:571))。`PM_NOREMOVE` でも呼ばれるため無条件 publish は同一 MSG の二重投入になる) |
| UI キューの focus / activation | 無し | `WH_CALLWNDPROC` / `WH_CBT` の新 producer (§3.1) |
| `AttachThreadInput` の attach / detach | 無し | 呼び出し側が topology invalidation を publish |
| presenter スレッド | pump が `MSG` を保持 | `PeekMessageW` 直後を境界にする |
| `native_key_event` の刻印 (出所 D) | 独自 `GetKeyState` | owner から導く |
| overlay の modifier state / 合成 egui event | 独自 ([render_core.rs:4320](../../src/video/native_presenter/render_core.rs:4320)) | owner から導く |
| **native mouse move / button / wheel** | overlay の modifier を `wParam` から作り **Alt を false 固定** ([render_core.rs:4457](../../src/video/native_presenter/render_core.rs:4457)) | owner から導く |
| **native backdrop の egui から `NativeVideoKeyEvent` への変換** | 独自 ([ui_fullscreen.rs:3565](../../src/ui_fullscreen.rs:3565)) | owner から導く |
| **presenter / HUD の `WM_APPCOMMAND` 由来の合成イベント** | 独自 ([native_window.rs:1601](../../src/video/native_window.rs:1601), [hud_window.rs:948](../../src/video/native_window_host/hud_window.rs:948)) | owner から導く |
| **passive detached HWND** | detached manager に HWND を保存するだけで、key subclass 登録は active fullscreen の rect 経路のみ ([app.rs:38115](../../src/app.rs:38115)) | owner の HWND から viewport への registry へ登録 |
| test-script の synthetic timeline | aggregate `ctrl/shift/alt` のみ ([key_input.rs:126](../../src/key_input.rs:126)) | 左右 bitset へ拡張し、同じ chokepoint の arming override として維持 |

**nonqueued (`SendMessage`) のキーボードメッセージは位置を捏造しない。** `InSendMessageEx` で判別し、
**キーボード注入の対応範囲外であることを明記する**。

## 6. consumer 一覧 (第 2 版から追加、実装前に完成させる)

固定キー (Esc / Delete / BS / 編集モードの矢印)、keymap 主経路と egui fallback、
grid の矢印と範囲選択 ([app.rs:33723](../../src/app.rs:33723), [:33893](../../src/app.rs:33893))、
wheel routing 3 箇所 ([app.rs:35962](../../src/app.rs:35962),
[ui_fullscreen.rs:18224](../../src/ui_fullscreen.rs:18224), [:22007](../../src/ui_fullscreen.rs:22007))、
drag / edit の modifier 読み ([ui_fullscreen.rs:22231](../../src/ui_fullscreen.rs:22231),
[ui_erase.rs:673](../../src/ui_erase.rs:673), [ui_conceal.rs:619](../../src/ui_conceal.rs:619))、
操作カスタマイズの chord capture ([pages.rs:3316](../../src/ui_dialogs/preferences/pages.rs:3316))、
`modifier_held_via_os` ([keymap.rs:8038](../../src/keymap.rs:8038)) と
`resync_egui_modifiers_from_os` ([app.rs:36194](../../src/app.rs:36194))、
clipboard の直接 `GetAsyncKeyState` ([app.rs:36248](../../src/app.rs:36248))、
`modifier_probe` と key-debug。

## 7. 検証 — 不変条件 (第 2 版の判定規則は誤りだった)

**「epoch をまたいで物理状態と違うエッジは異常」は誤り。** 旧 epoch で配送済みのエッジが新 epoch で
消費され、現在の物理状態と食い違うのは**正常**である。配送済みエッジが後の focus loss を越えて
dispatch 可能なのは既存の契約 ([keyboard_input.rs:106](../../src/keyboard_input.rs:106))。

正しい不変条件:

- `EpochStart(Known(seed))` がそのキューの初期状態を決める
- 各エッジの `DeliveryModifiers` は、**その epoch のローカル FIFO を fold した結果と一致する**
- `Unknown` epoch で旧値を既知として刻印しない
- UI 復帰シナリオ: Backspace は**新 epoch** かつ `Ctrl=false`
- 高速 CtrlDown から BS、CtrlUp: **同一 epoch**で、BS だけ `Ctrl=true`
- **現在の物理状態との食い違い自体を pass / fail 条件にしない**

回帰テストの最低線: 上記に加えて、main / fullscreen / detached の各 viewport、
AltGr 配列の RAlt、StickyKeys ON、seed が `Unknown` のとき、Shift / Alt のベクター編集、
同一フレーム内の同一キー複数回、兄弟 viewport が消費しないこと。

## 8. 実装前に実機で確定する論点

- **StickyKeys**: 修飾キーは物理的に離れた後も latch / lock される。獲得時 seed の物理サンプルが
  latch をどう投影するかを**実機で観測してから**決める。推測で書かない。
- **transient `AttachThreadInput`**: 現行の foreground claim ごとの attach / detach が、実際に
  key state reset を起こすかを実機で確認する。§3.1 の topology 境界の設計はこの観測に依存する。
