# 修飾キー所有権の再設計 (design proposal / 合意用) — 第 3 版

**状態: 提案。第 4 版。Codex は構造的修正であることに同意済み (§11 用の文面あり) だが、
実装前決定 6 件が残っていた。本版はそれを確定させたもの。末尾の第 4 版節が §3.1 / §3.2 /
§3.3 / §6 / §7 を上書きする。実装は §9.7 の実機観測 2 件と、本版への合意の後。** 凍結領域
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

---

# 第 4 版 — 実装前決定 6 件の確定

**本節は §3.1 / §3.2 / §3.3 / §6 / §7 の該当箇所を上書きする。** 第 3 版は「文言では答えたが
実質を外している」点が 4 つあり、うち 2 つは反証を伴っていた。以下が確定版。

## 9.1 epoch 境界の **FIFO 上の位置** (§3.1 を上書き)

第 3 版は境界を「観測」できるようにしただけで、**キュー内のどこか**を決めていなかった。
`WM_SETFOCUS` は sent message であり、`GetMessage` は queued input を返す前に sent message を
先に配送する。したがって次の順序が成立してしまう。

1. 旧 epoch のキーが既にキューにある (アプリが忙しく drain できていない)
2. sent `WM_SETFOCUS` を hook が観測し、新 epoch を開始
3. **その後で旧キーが dequeue され、新 epoch へ fold される** ← 誤り

**確定: 取得を観測したら、そのキューへ private な marker message を `PostMessage` する。**

- marker は**現時点のキュー末尾に入る**ので、marker より前 = 旧 epoch、後 = 新 epoch と
  FIFO 上で一意に決まる。時刻比較も watermark も要らない
- **物理状態のサンプルは marker を post する時点で取り、marker の payload として運ぶ**。
  dequeue 時点で取り直さない (取得から dequeue までの遷移は marker の後ろに並ぶため、
  seed に含めると二重適用になる)
- **同一キュー内**の focus 移動 (child / common dialog / IME HWND 間) は routing の再取得では
  ないので **marker を出さない**。判定は「routing owner が別のキュー / group からこのキューへ
  変わったか」で行い、重複する activation → focus 通知は 1 つの marker へ coalesce する

`WH_CALLWNDPROC` と `WH_CBT` は代替関係にない。キューをまたぐ `WM_ACTIVATE` は非同期で
ウィンドウは即時 activate され、`HCBT_*` は操作**前**の通知である。よって
**`HCBT_*` を候補通知、`WM_ACTIVATE` / `WM_SETFOCUS` を成功確認**として使い、marker の post は
成功確認側で行う。

## 9.2 `AttachThreadInput` 中の所有権 (§3.1 を上書き)

attach 中は 2 スレッドが**入力状態を共有し、両者のイベントを受信順に処理する**。この間に
2 つの独立したキュー状態をローカル FIFO だけで fold すると、片方が処理した Ctrl の遷移が
他方へ反映されない。「各 producer が次の dequeue 前に generation を見る」だけでは、
API 成功と invalidation の publish の間に相手が dequeue する race が残る。

**確定: attach 中は両キューを 1 つの topology component として扱い、単一の状態を共有する。**
`claim_foreground` は attach → focus API → detach を同期実行しており
([native_window.rs:1001](../../src/video/native_window.rs:1001))、この span は自コードで閉じている。
span の入口で component を統合し、出口で分離して**両キューへ marker を post する** (detach 後は
OS 側でキー状態が reset されるため、両方が新 epoch になる)。detach の戻り値は現在捨てているので、
**成功可否を見るよう変更する** (失敗したまま分離扱いにしない)。

## 9.3 `Unknown` の照合 — **not held 既定を撤回** (§3.2 を上書き)

第 3 版の「解消前の `Unknown` は not held として照合する」は**誤り**。反証がある。

`FsClearAdjust` は <kbd>Ctrl</kbd>+<kbd>BS</kbd> だけでなく **bare <kbd>Q</kbd>** にも割り当てられて
いる ([keymap.rs:5471](../../src/keymap.rs:5471))。Ctrl が `Unknown` のとき false と決めつけると、
実際には Ctrl を押している <kbd>Ctrl</kbd>+<kbd>Q</kbd> が **bare <kbd>Q</kbd> と誤認され、
補正が破棄される**。「破壊的でない側へ倒す」という私の論拠は、同じ action の別 chord で崩れる。
1 例からの一般化だった。

**確定: 照合結果を `Match` / `NoMatch` / `Indeterminate` の 3 値にする。**
`Indeterminate` は **action を発火させず、perf event に必ず出す**。特定の action だけ
fallback させたい場合は、action ごとの明示 policy として宣言する (既定では無い)。

**`Unknown` は epoch 全期間続き得る。** 修飾キーごとに最初の遷移でしか解消しないので、
Ctrl の遷移が起きなければ Ctrl は `Unknown` のままである。よって:

- sampling の事前条件が後から成立したら、**recovery として新しい epoch を開始してよい**
  (marker を post して seed し直す)。これは「推測で埋める」ことではなく、
  条件が整った時点で正しい seed を取り直す操作である

## 9.4 AltGr — **RAlt が LCtrl を抑制する案を撤回** (§3.3 を上書き)

第 3 版の案は**誤り**。AltGr を無害化せず **Alt-only の chord に変換してしまう**。
grid には `Alt+1..0` (列数変更、[keymap.rs:5292](../../src/keymap.rs:5292)) があり、
`Ctrl+Alt+1..0` は標準補正 ([keymap.rs:5458](../../src/keymap.rs:5458))。提案方式では
**AltGr+数字が列数変更を誤爆する**。意図的な LCtrl+RAlt も、Ctrl+Alt が成立せず Alt-only が
成立するという最悪の形になる。さらに「Ctrl / Alt は左右どちらでもよい」という現行の意味
([keymap.rs:110](../../src/keymap.rs:110)) を壊す。

Win32 に「これは synthetic AltGr」を無損失で示す標準フラグは無い。Windows Terminal は
LCtrl と RAlt の時間差 50ms と生成 codepoint を使うが、自ら heuristic と明記している。
Chromium は layout の `KLLF_ALTGR` を公開 API から取れず `ToUnicodeEx` で推定している。

**確定: AltGr は sided bit の書き換えではなく provenance として保持し、AltGr sequence 中は
通常の chord matching 全体を抑止する。** 「事前から LCtrl が押されていたら genuine Ctrl」という
timing heuristic は採らない (ほぼ同時の意図的 LCtrl+RAlt と完全には識別できず、
§2 規則 5 の時間窓禁止にも触れる)。**AltGr 中に chord を発火させないことは機能低下ではなく、
現状の誤爆を止める側である。**

## 9.5 consumer 分類 — call-site 単位で `Delivery` / `Current` を決める (§6 を上書き)

§6 は場所の列挙であって分類になっていなかった。**実装前に全 call-site を 2 分類する。**

**確定した分類方針**:

- **キーの chord 判定は `Delivery`**。「そのキーが来たとき何が押されていたか」が意味だから。
- **ホイール / クリック / ドラッグの修飾キーは `Current`**。「今 Ctrl を押しながら回している」が
  利用者の意味であり、配送位置ではない。これにより「1 frame 内で Ctrl 遷移と複数ホイールが
  交錯したとき各ホイールの配送位置を復元できない」という問題自体が消える
  (sidecar event を新設しない)。
- **ホールド系は `Current`** (現状どおり)。

未分類の残り (grid click [ui_main.rs:12795](../../src/ui_main.rs:12795)、rating click
[ui_main.rs:1481](../../src/ui_main.rs:1481)、erase / conceal の vector drag
[ui_erase.rs:1466](../../src/ui_erase.rs:1466)、local-adjust の Alt preview
[ui_adjustment_panel.rs:11387](../../src/ui_adjustment_panel.rs:11387)) はすべて pointer 由来
なので `Current`。

## 9.6 不変条件の追補 (§7 を上書き)

- **修飾キー自身のエッジは、自分の遷移を適用した後の snapshot を持つ** (Ctrl down のエッジは
  `ctrl=true`)。現行 `GetKeyState` の挙動と一致する。
- **pointer / wheel は `Current` なので FIFO fold の対象外**。
- **`WM_APPCOMMAND` 由来の合成イベントは、意味として modifier-none とする**。これらは
  key chord ではなくアプリコマンドであり、現在も全 false で作られている
  ([native_window.rs:1601](../../src/video/native_window.rs:1601))。「owner から導く」ではなく
  **明示的に modifier-none と宣言する**ことで意味を一意にする。

## 9.7 実機観測が先に要る 2 件 (S0 の前提)

- **StickyKeys**: latch / lock が獲得時 seed の物理サンプルにどう映るか。
- **transient `AttachThreadInput`**: 現行の attach / detach が実際にキー状態 reset を起こすか。

**この 2 件の観測結果が出るまで S0 の契約は確定しない。**観測は利用者の実機で行う。

---

# 実機観測の結果 (2026-08-20)

計装: [modifier-observation-probe.md](modifier-observation-probe.md)。§9.7 の 2 件に対する回答。

## Q1 StickyKeys — **確定。seed 規則はそのままでよい**

latch 中 (`sticky_keys_flags` に `SKF_LCTLLATCHED`) の heartbeat 4 件で、
**`GetAsyncKeyState` / `GetKeyState` / egui のすべてが `LCtrl = true`** を報告した
(t=76.06 / 78.06、fullscreen viewport)。

**したがって「取得時に物理状態から seed する」は StickyKeys 環境でも正しい。**latch は物理
サンプルに映る。§3.2 に特別扱いを足す必要はない。

## Q1 の副産物 — **delivery と current の区別が実測で裏付けられた**

latch 直後の <kbd>BS</kbd> (t=78.19) は **`FsClearAdjust` として成立した** =
Windows は Ctrl+BS として配送した。**同じ瞬間の probe は egui / async / sync すべて
`ctrl = false`** を報告している (latch は既に消費済み)。

- **配送時点では Ctrl が押されていた**。`KeyEdge` は WndProc 時点の `GetKeyState` で刻むので
  これを正しく捉えた
- **フレーム処理時点では Ctrl は上がっている**。これも正しい
- **両方正しい。**これは §7 が「正常」と定めた高速 Ctrl+BS と同型の事象が、StickyKeys によって
  自然に発生したもの

**帰結**: §9.5 の「キーの chord 判定は `Delivery`」は実測で裏付けられた。もしキーを `Current` で
判定していたら、**StickyKeys 利用者の Ctrl+BS が素の BS になる**。

**新たに生じた論点**: §9.5 はホイール / クリック / ドラッグを `Current` に分類した。StickyKeys で
「Ctrl を叩いてからホイールを回す」場合、latch が消費された後に `Current` を読むと Ctrl 無しに
なる。**Windows が latch をマウスメッセージへ適用するかは未観測**。次の観測に含める。

## Q2 `AttachThreadInput` — **未実施。attach が 1 度も起きなかった**

`claim_foreground` は 3 回走ったが、**3 回とも `attach_ok = false`**。理由はコードの短絡評価:

```rust
let attached = foreground_tid != 0
    && foreground_tid != this_tid          // ← ここで抜けた
    && AttachThreadInput(this_tid, foreground_tid, true).as_bool();
```

3 回とも `this_tid == foreground_tid == 125704` (= UI スレッド)、`partner_is_miv_ui = true`。
**動画を開く時点で既に mIV 自身が foreground だった**ため、attach は不要と判定された。
呼び出し元は UI スレッド側 (`claim_native_window_focus` /
presenter 破棄後の main foreground 再取得) だった。

### この観測は設計を止めるか — **止めない**

attach が実際にキー状態を reset するかは未確認のままだが、**設計はどちらでも成立する**。

- reset する → 境界にする必要がある (設計どおり)
- reset しない → 境界にしても、物理状態から seed し直すだけで同じ値になる

**唯一の副作用**は、不要な境界が**進行中の高速 chord を分断**し得ること (BS がキューにあり Ctrl が
既に離されている最中に marker が入ると、BS が `ctrl=false` で seed される)。ただし attach は
利用者のウィンドウ切替に伴う操作なので、**その最中に高速 chord が進行している確率は無視できる**。

**確定: 成功した attach / detach のみを境界とする** (§9.2 のまま)。reset の有無は観測できたら
記録するが、S0 の前提条件からは外す。

### 次の観測に回す項目

1. `foreground_tid != this_tid` となる `claim_foreground` (= 実際に attach が走る場合) の 4 点
2. StickyKeys latch 中のホイール (latch がマウスメッセージへ適用されるか)

どちらも既存の計装のまま拾えるので、追加実装は不要。
