# stage-r2e-1 — viewer context registry の状態機械を、production の保管を切らずに作る

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
(第 3 版、Codex レビュー 6 巡で BLOCKER 0)。本書はその **§7 ステージ①** の実装指示書。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-1)` を含める。

---

## 1. このステージで作るもの

新しいモジュール **`src/app/viewer_context_registry.rs`** を 1 本追加し、その中に
viewer context の **identity / 所在 / 投影 / window binding** を扱う状態機械
`ContextTable<P>` を作る。**payload `P` に対してジェネリック**で、
`ViewerContextBundle` を一切知らない。

**production の挙動は 1 mm も変わらない。** このモジュールは誰からも呼ばれない。
`src/app.rs` に `mod viewer_context_registry;` の **1 行だけ**を足す。

### 1.1 このステージで作らないもの

- `ViewerContextBundle` の移設 (ステージ②-b)
- `App::active_detached_viewer_context` / `DetachedImageWindowSnapshot::paused_bundle` の削除 (②-d)
- production の実行器 (②-d。①で確定するのは**実行器が満たすべき契約**まで)
- `syn` 監査ツール (②-c)
- 手書き mount の helper 化 (②-pre。①とは独立で、どちらが先でもよい)
- **fork の payload 分割そのもの** (224 → 225 フィールドの 3 分類は②-b/②-d)。
  ①が決めるのは「fork という遷移が table 状態と binding に何をするか」まで。

## 2. なぜこの切り方なのか

第 2 版のステージ分割は **コンパイルできない**という理由で落ちた
(設計 §7 / 付録 A の BLOCKER 3)。保管フィールドを消すと、それを直接触っている消費者が
同時に全部壊れる。したがって「registry を入れる」と「保管を切る」は分けられない。

**分けられるのはその手前だけ**であり、それが本ステージである。
`ContextTable<P>` をジェネリックにすることが、production から切り離してコンパイルさせる仕掛け。
②-d では `P = Box<ViewerContextBundle>` として実体化するだけで、
**状態遷移の設計判断は①で終わっている**状態にする。

## 3. 型

すべて `src/app/viewer_context_registry.rs` に置く。
**このステージではモジュール外へ何も公開しない** (`pub` / `pub(crate)` / `pub(super)` を
1 つも付けない)。テストは子モジュールなので private を見られる。
②-e で入れる監査 A4 の allowlist は、この時点では**空**である。

```rust
/// context の identity。payload 1 個に 1 個。OS ウィンドウとも「main かどうか」とも独立。
/// 生成は `ContextTable::allocate_id` (private) だけ。フィールドも private。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct ViewerContextId(u64);

/// ある id の payload が今どこに在るか。設計 §3.2。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextResidence {
    Mounted,   // 投影がこの context
    AtRest,    // slot にある。マウントできる
    Building,  // build 中。予約済みで未 commit。slot にはまだ無い
    Retiring,  // retire の digest 実行中。読めるが mount も bind もできない
    Retired,   // 払い出されたことがあり、その後 retire (build abort を含む) された
    Unknown,   // 一度も払い出されていない id。バグの疑い
}

/// 投影が今何を写しているか。設計 §3.3。
enum Projection {
    Mounted(ViewerContextId),
    Building {
        reserved: ViewerContextId,
        previous: ViewerContextId,
        /// commit と同時にだけ公開される窓の予約 (I8)。
        pending_bind: Option<u64>,
    },
}

enum Slot<P> {
    AtRest(P),
    /// digest 実行中。mount / bind を拒否する。
    Retiring(P),
}

/// fork の種別。①では **binding への影響と、実行器へ渡す分割種別**だけを表す。
/// payload の分割そのものは②-b/②-d。設計 §4.3。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForkPolicy {
    /// live-park。マウント中の context が今持っている窓が fork 側へ移る。
    /// **`finish_fork` の中で `transfer_window_binding` を実行する**
    /// (分割と binding の移動を別呼び出しにすると、その隙間で食い違う)。
    LiveMediaPark { window_id: u64 },
    /// materialize 済み物理一覧からの独立静止画窓。窓は新規なので binding は
    /// 呼び出し元が `bind_window` で結ぶ。
    MaterializedStillOpen,
}

/// 実行器 (呼び出し側) が順に実行する payload 操作。設計 §7 ①。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableOp {
    /// 新しい空 payload を作って投影と交換し、元の投影の中身を transient にする。
    /// **「取り出す」と「空を据える」を割ってはならない** (投影は空にできない)。
    ReplaceProjectionWithFreshEmpty,
    /// 投影から 2 個目の payload を policy に従って派生させ transient にする。投影は変わらない。
    ForkProjectionIntoTransient(ForkPolicy),
    /// transient を id の slot へ預ける。
    DepositInto(ViewerContextId),
    /// id の slot から payload を取り出して transient にする。
    WithdrawFrom(ViewerContextId),
    /// transient を投影と交換し、押し出された空 payload を drop する。
    RestoreProjectionAndDropDisplacedEmpty,
    /// **id の binding を外してから** transient を drop する
    /// (production では drop で worker cancel が走る)。unbind が先。
    DropTransientAsRetired(ViewerContextId),
}

enum BindError {
    WindowOwnedBy(ViewerContextId),   // その窓は別の生きた context が持っている
    ContextOwnedBy(u64),              // その context は既に別の窓を持っている
    WrongOrigin(Option<ViewerContextId>), // transfer の from が実際の持ち主と違う
    NotBindable(ContextResidence),    // 対象 id が bind できる状態にない
}

/// mount が失敗した理由。**`Option` を返さない** — 「できなかった」を
/// 無言で読み飛ばす今の `else { continue }` を再生産しないため (設計 §4.1)。
struct MountError {
    id: ViewerContextId,
    residence: ContextResidence,
}

/// retire が失敗した理由。`MountError` では
/// 「AtRest だが main なので retire 禁止」を表せないので別の型にする。
enum RetireError {
    /// App が組み立て中。何も retire できない。
    Building,
    /// その id は `AtRest` ではない。
    NotAtRest(ContextResidence),
    /// **`main` が指している context は retire できない。** 先に `promote` すること (I4)。
    IsMain,
}

/// plan_* が積み、finish_* が消費する。**これが「op 列を実行中」の唯一の表現**で、
/// 別に `in_transit: bool` は持たない。
enum PendingTransition {
    BeginBuild { reserved: ViewerContextId, previous: ViewerContextId },
    CommitBuild { reserved: ViewerContextId, previous: ViewerContextId, pending_bind: Option<u64> },
    AbortBuild { reserved: ViewerContextId, previous: ViewerContextId },
    Mount { from: ViewerContextId, to: ViewerContextId },
    Fork { policy: ForkPolicy, new_id: ViewerContextId, from: ViewerContextId },
    Promote { stashed: ViewerContextId, fresh: ViewerContextId },
    Retire { id: ViewerContextId },
}

struct ContextTable<P> {
    projection: Projection,
    slots: HashMap<ViewerContextId, Slot<P>>,
    /// main 窓の一覧を担う context。identity ではなく binding (設計 §3.1)。
    main: ViewerContextId,
    /// context → 窓。部分写像 (どの窓にも結ばれない期間がある)。
    window_of: HashMap<ViewerContextId, u64>,
    /// 窓 → context。`bind` / `unbind` / `transfer` だけが更新する派生 index。
    context_of: HashMap<u64, ViewerContextId>,
    /// 次に払い出す serial。`highest_reserved_serial == next_serial - 1`。
    next_serial: u64,
    /// plan と finish の間だけ `Some` (§5.2)。
    pending: Option<PendingTransition>,
}
```

`ArrayVec` は依存に無いので op 列は `Vec<TableOp>` で返す (1 transaction 数個なので問題にならない)。

### 3.1 初期化と id の払い出し

```rust
/// main context の payload は呼び出し元が投影に持っている前提で、table は id だけを持つ。
fn new() -> Self;
```

- `new()` は serial 1 を main へ払い出し、`next_serial = 2`、
  `projection = Mounted(main)`、`slots` / `window_of` / `context_of` は空、`pending = None`。
- したがって `next_serial >= 2` が常に成り立ち、**`next_serial - 1` は underflow しない**。
- `allocate_id` は **private**。**`plan_begin_build` / `plan_fork` / `plan_promote` が
  自分で払い出して返す。** 呼び出し元から id を受け取らないので、
  「retire 済み id を渡されて再利用される」経路が構造的に存在しない。
- overflow: `next_serial` は `u64` なので実用上到達しない。
  `checked_add` で `expect("viewer context serial exhausted")` にする (無言 wrap を作らない)。

## 4. API

### 4.1 問い合わせ (副作用なし)

```rust
fn main(&self) -> ViewerContextId;
/// Building 中は None (「今マウントされている context」は存在しない)。
fn mounted_id(&self) -> Option<ViewerContextId>;
fn residence(&self, id: ViewerContextId) -> ContextResidence;
fn locate_window_context(&self, window_id: u64) -> Option<(ViewerContextId, ContextResidence)>;
/// payload を持っている全 id (投影中のものを含む)。**id 昇順で決定的**に返す。
fn ids(&self) -> Vec<ViewerContextId>;
```

`residence` の判定順:

1. 投影が `Mounted(id)` → `Mounted`
2. 投影が `Building { reserved: id, .. }` → `Building`
3. `slots[id] == Slot::Retiring` → `Retiring`
4. `slots[id] == Slot::AtRest` → `AtRest`
5. `id.serial() <= next_serial - 1` → `Retired`
6. それ以外 → `Unknown`

**問い合わせ API はすべて先頭で `assert!(self.pending.is_none())`** (§5.2)。

### 4.2 binding (設計 §3.5)

```rust
fn bind_window(&mut self, id: ViewerContextId, window_id: u64) -> Result<(), BindError>;
fn unbind_window(&mut self, window_id: u64) -> Option<ViewerContextId>;
fn transfer_window_binding(&mut self, window_id: u64, from: ViewerContextId, to: ViewerContextId)
    -> Result<(), BindError>;
```

- `bind_window` が `Ok` を返す条件: `id` の residence が `Mounted` か `AtRest`
  (**`Building` も不可** — build 中の予約は `reserve_window_binding_for_build` を使う)。
  同じ `(id, window_id)` の組み合わせは冪等で `Ok`。
  窓が別の生きた context にあれば `WindowOwnedBy`、
  `id` が既に別の窓を持っていれば `ContextOwnedBy`、
  residence が `Building` / `Retiring` / `Retired` / `Unknown` なら `NotBindable`。
- `transfer_window_binding`: **`from` も `to` も生きたまま**窓を移す。
  `from` が実際の持ち主でなければ `WrongOrigin`。`to` が既に別の窓を持っていれば `ContextOwnedBy`。
- この 3 本も先頭で `assert!(self.pending.is_none())`。
  **`finish_*` と op から呼ぶのは assert しない private の core** で、公開側はその薄い
  ラッパにする (§5.2)。core は**鍵が違う 2 本の unbind を別関数**にする
  (Rust はシグネチャのオーバーロードができない):

  ```rust
  fn bind_core(&mut self, id: ViewerContextId, window_id: u64) -> Result<(), BindError>;
  /// 窓を鍵にする。公開 `unbind_window` が包む。
  fn unbind_window_core(&mut self, window_id: u64) -> Option<ViewerContextId>;
  /// context を鍵にする。`DropTransientAsRetired(id)` が使う。
  fn unbind_context_core(&mut self, id: ViewerContextId) -> Option<u64>;
  fn transfer_core(&mut self, window_id: u64, from: ViewerContextId, to: ViewerContextId)
      -> Result<(), BindError>;
  ```

### 4.3 transaction — plan / execute / finish の 3 相

**`plan_*` は op 列と `pending` を積むだけで、投影も binding も動かさない。**
op 列を全部実行し終えてから `finish_*` を呼び、**そこで初めて**投影が動き binding が公開される。
plan の時点で投影や binding を進めると、op の途中で panic したときに
I8 (binding は commit と同時にだけ公開) が破れる。
**op は panic し得る** (production の swap は rating 同期と visible index 再構築を通る)。

```rust
// build
fn plan_begin_build(&mut self) -> (ViewerContextId /* reserved */, Vec<TableOp>);  // 2 op
fn finish_begin_build(&mut self);
/// build 中の窓の予約。commit まで公開されない。同じ窓の再予約は冪等。
/// **別の窓を 2 回目に予約したら panic** (1 build = 1 窓)。
/// 投影が `Building` でなければ panic。
fn reserve_window_binding_for_build(&mut self, window_id: u64);
fn plan_commit_build(&mut self) -> Vec<TableOp>;                                  // 4 op
fn finish_commit_build(&mut self) -> ViewerContextId;
fn plan_abort_build(&mut self) -> Vec<TableOp>;                                   // 6 op
fn finish_abort_build(&mut self);

// mount
fn plan_mount(&mut self, id: ViewerContextId) -> Result<Vec<TableOp>, MountError>; // 0 or 4 op
fn finish_mount(&mut self);

// fork
fn plan_fork(&mut self, policy: ForkPolicy) -> (ViewerContextId /* new */, Vec<TableOp>); // 2 op
fn finish_fork(&mut self) -> ViewerContextId;

// promote (投影を退避し、新しい空 context を投影にする)
fn plan_promote(&mut self) -> Vec<TableOp>;                                       // 2 op
fn finish_promote(&mut self) -> ViewerContextId;   // 戻り値 = 退避された旧 main の id

// retire
fn begin_retire(&mut self, id: ViewerContextId) -> Result<(), RetireError>;        // AtRest -> Retiring
fn plan_finish_retire(&mut self, id: ViewerContextId) -> Vec<TableOp>;             // 2 op
fn finish_retire(&mut self);
```

**受け付ける事前状態** (満たさなければ panic。無言で no-op にしない):

| 関数 | 事前状態 |
| --- | --- |
| `plan_begin_build` | `Mounted(previous)` |
| `plan_commit_build` / `plan_abort_build` | `Building { .. }` |
| `plan_mount(id)` | **panic しない。失敗はすべて `Err(MountError)`**。投影が `Building` なら `Err(residence: Building)` (App が組み立て中なので何もマウントできない)。投影が `Mounted(current)` なら、`id` の residence が `Mounted` (再入 = 空 op 列) か `AtRest` のとき `Ok`、それ以外は `Err(その residence)` |
| `plan_fork` | `Mounted(current)`。`LiveMediaPark { window_id }` は `window_of[current] == Some(window_id)` |
| `plan_promote` | `Mounted(current)` かつ **`current == main`** |
| `begin_retire(id)` | `id` の residence が `AtRest` **かつ `id != main`**。投影が `Building` なら `Err(Building)`、`AtRest` でなければ `Err(NotAtRest(..))`、main なら `Err(IsMain)`。⚠ **main を retire できてしまうと I4 が回復不能に壊れる** (main() が Retired な id を指し、mount も promote もできなくなる) |
| `plan_finish_retire(id)` | `slots[id] == Slot::Retiring` |
| 各 `finish_*` | `pending` が対応する variant。違えば panic |
| **すべての `plan_*` と `begin_retire`** | **`pending` が `None`**。既に transaction が進行中なら panic (前の op 列を実行し終えていない)。⚠ この行は `pending` の話だけで、`plan_mount` / `begin_retire` の**対象 id の状態は panic ではなく `Err(MountError)`** で返す (設計 §4.1 の「`Option` を返さない」= 呼び出し元に理由を渡す) |
| `retiring_slot_mut` | `pending` が `None` (digest は op 列の外) |

op 列の中身 (設計 §7 ①の表と一致させること):

```text
begin_build   : ReplaceProjectionWithFreshEmpty        // app.rs:40545 に対応
                DepositInto(previous)

commit_build  : ReplaceProjectionWithFreshEmpty        // app.rs:40654
                DepositInto(reserved)
                WithdrawFrom(previous)
                RestoreProjectionAndDropDisplacedEmpty // app.rs:40655

abort_build   : ReplaceProjectionWithFreshEmpty        // startup_ops.rs:652
                DepositInto(reserved)
                WithdrawFrom(previous)
                RestoreProjectionAndDropDisplacedEmpty // startup_ops.rs:653
                WithdrawFrom(reserved)
                DropTransientAsRetired(reserved)       // startup_ops.rs:654 (return で drop)

mount(id)     : ReplaceProjectionWithFreshEmpty
                DepositInto(current)
                WithdrawFrom(id)
                RestoreProjectionAndDropDisplacedEmpty
                （id == current なら空の op 列。finish_mount も no-op = 再入 mount）

fork(policy)  : ForkProjectionIntoTransient(policy)
                DepositInto(new)

promote       : ReplaceProjectionWithFreshEmpty
                DepositInto(current)
                （実行後、投影には新しい空 payload が載っている）

retire(id)    : WithdrawFrom(id)
                DropTransientAsRetired(id)   // unbind してから drop
```

**abort の 6 op を 4 op に縮めないこと。** 現行の
`let _failed_context = self.take_current_viewer_context_bundle();`
([startup_ops.rs:652](../src/app/startup_ops.rs:652)) は**名前付きの束縛**なので、drop は
`swap_viewer_context_bundle(&mut main_context)` ([:653](../src/app/startup_ops.rs:653)) より後、
`return false` ([:654](../src/app/startup_ops.rs:654)) の時点で起きる。drop は worker cancel と
condvar notify を伴う ([app.rs:2743](../src/app.rs:2743)) ので、**順序を入れ替えると cancel の
timing が変わる**。transient を 1 個に保ったままこの順序を再現するには 6 op が要る。

**retire は unbind が drop より先** (設計 §4.4)。`DropTransientAsRetired(id)` の中で
`unbind_context_core(id)` を実行してから transient を drop する。abort でも同じ op を使うが、
そのとき `reserved` の binding はまだ公開されていないので unbind は no-op になる。

`finish_*` が行うこと:

| finish | 投影 | binding | その他 |
| --- | --- | --- | --- |
| `finish_begin_build` | `Mounted(previous)` → `Building { reserved, previous, pending_bind: None }` | — | — |
| `finish_commit_build` | `Building{..}` → `Mounted(previous)` | `pending_bind` を **公開** (`bind_core`) | `reserved` を返す |
| `finish_abort_build` | `Building{..}` → `Mounted(previous)` | `pending_bind` を **捨てる** | `reserved` は以後 `Retired` |
| `finish_mount` | `Mounted(from)` → `Mounted(to)` | — | 再入なら no-op |
| `finish_fork` | 変わらない | `LiveMediaPark` は **`transfer_core(window_id, from, new)` を実行**。`MaterializedStillOpen` は何もしない | `new` を返す |
| `finish_promote` | `Mounted(stashed)` → `Mounted(fresh)` | — | `main = fresh`。`stashed` を返す |
| `finish_retire` | 変わらない | (unbind は op 側で済んでいる) | **entry が既に無いことを assert** して `pending` を消すだけ (削除は `withdraw` が済ませている) |

`finish_commit_build` の**内部順序は載っている** (実装で入れ替えないこと):

1. `projection = Mounted(previous)` を先に据える
   (この時点で `reserved` の residence が `Building` → `AtRest` になり、bind 可能になる)
2. `bind_core(reserved, window_id)` で `pending_bind` を公開する
3. `pending = None` にする

逆順にすると 2 が `NotBindable(Building)` で失敗する。

`finish_commit_build` / `finish_fork` で `bind_core` / `transfer_core` が `Err` を返したら、
**握りつぶさずに panic する** (成立しない窓へ commit / fork した = 呼び出し元のバグ)。

## 5. 実行器の契約 (①では定義とテスト実装だけ)

### 5.1 責務

実行器は **transient を高々 1 個** (`Option<P>`) 持ち、op を順に実行する。

| op | 事前条件 | 動作 |
| --- | --- | --- |
| `ReplaceProjectionWithFreshEmpty` | transient が `None` | 空 payload を作り投影と交換。元の投影の中身が transient |
| `ForkProjectionIntoTransient(policy)` | transient が `None` | 投影から policy に従って 2 個目の payload を派生させ transient へ。投影は変わらない |
| `DepositInto(id)` | transient が `Some`、`slots` に `id` が無い | `table.deposit(id, payload)` |
| `WithdrawFrom(id)` | transient が `None`、`slots[id]` に payload がある | `table.withdraw(id)` → transient |
| `RestoreProjectionAndDropDisplacedEmpty` | transient が `Some` | transient を投影と交換し、押し出された値を drop |
| `DropTransientAsRetired(id)` | transient が `Some` | `table.unbind_context_core(id)` の後、transient を drop |

事前条件違反はすべて panic (debug / release とも)。

payload に触る table のメソッドは **モジュール private**:

```rust
/// slots に `id` の entry が既にあれば panic。
fn deposit(&mut self, id: ViewerContextId, payload: P);
/// **entry ごと除去して** payload を返す。entry が無ければ panic。
/// `Slot::AtRest` / `Slot::Retiring` のどちらからでも取り出せる。
fn withdraw(&mut self, id: ViewerContextId) -> P;
```

⚠ **`withdraw` は entry を残さない。** `Slot<P>` は `AtRest(P)` / `Retiring(P)` の 2 変種しか
無いので、payload を抜いたまま entry を残すことが safe Rust ではできない。
「`Vacant` 変種」「placeholder payload」「unsafe move」のどれも**入れないこと**
(②-d で撤去する羽目になる)。全 op 列がこれで整合する: mount は current を deposit した後に
target を除去、commit / abort は previous / reserved を除去、retire は `Retiring` の entry を除去。

digest 用の貸し出しは op 列の**外**なので、executor 用ヘルパとは別扱いにする (§5.2):

```rust
/// digest 用。**`Slot::Retiring` のときだけ** `Some`。`AtRest` には貸さない。
/// op 列の外から呼ぶので `assert!(self.pending.is_none())` を持つ。
fn retiring_slot_mut(&mut self, id: ViewerContextId) -> Option<&mut P>;
```

②-d の production 実行器も**同じモジュール内に書く**ので、モジュール外へ生 payload が出る
API は最後まで作らない (設計 §3.7)。

**op 列の実行中に利用者コードを 1 行も走らせない。** `f` (build の本体) が走るのは
`begin` の op 列を実行し終えて `Building` が確定した後、`commit` / `abort` の op 列を
始める前だけ。**retire の digest は op 列の外**で、`begin_retire` の後
`plan_finish_retire` の前に `retiring_slot_mut` を使って行う。

### 5.2 plan と finish の間 (`pending`)

この窓の間、table の `projection` はまだ古い値を指しており**実体とずれている**。
ここで問い合わせ API を呼んではならない。

- `plan_*` が `pending = Some(..)`、`finish_*` が `None` に戻す。
- **問い合わせ API (§4.1) と公開 binding API (§4.2) は先頭で
  `assert!(self.pending.is_none())`。** debug 限定にしない (release でも守る)。
- `finish_*` と op は `pending` が立ったまま binding を触るので、
  **assert を持たない private の `bind_core` / `unbind_window_core` /
  `unbind_context_core` / `transfer_core`** を使う。
  公開 API はこの core に assert を足しただけの薄いラッパにする。
- `deposit` / `withdraw` は op 実行中に呼ばれるので assert しない。
- **`retiring_slot_mut` は op 列の外 (digest) から呼ぶので `assert!(self.pending.is_none())` を持つ。**
  executor 用ヘルパと同じ扱いにしないこと。

### 5.3 op panic の扱い (保証しないことを明記する)

保証するのは 2 つだけ:

- **I1b**: 投影が payload を持たない瞬間は存在しない。
- **I8**: abort / panic で binding を 1 つも公開しない。

**op panic の完全なロールバックは保証しない。** transient が中身を持っていれば drop されるが、
`DepositInto(reserved)` の後で落ちた場合は組み立て済み payload が slot に残る。
doc comment にこの限界を書くこと (無いことにしない)。

## 6. テスト要件

`#[cfg(test)] mod tests` をモジュール内に置く ([viewer_session.rs](../src/app/viewer_session.rs) と同じ形)。
テスト用実行器は **table 自身の slots** に対して op を実行する (別の store を作らない)。
`TestPayload` は識別できる値 (`u32` の tag など) と「空かどうか」が分かるもの。
`ForkProjectionIntoTransient` は tag を派生させた新しい payload を返す実装にする。

必須テスト:

1. **build commit**: `reserved` が `AtRest` になり、投影が `previous` に戻る。
2. **build abort**: 投影が `previous` に戻り、`reserved` が `Retired` になり、
   **binding が 1 つも公開されていない**。
3. **払い出したが commit されずに消えた id が `Retired`**、払い出していない id が `Unknown`。
   (`highest_committed` ではなく `next_serial - 1` で判定していることを固定する)
4. **build 中に予約した binding が commit まで公開されない**
   (`locate_window_context` が commit 前は `None`、commit 後に `reserved` を返す)。
5. `reserve_window_binding_for_build` の同じ窓の再予約が冪等。別の窓の 2 回目は panic
   (`#[should_panic]`)。`Building` でない投影で呼ぶと panic。
6. **mount**: 再入 (`id == current`) が空の op 列で finish が no-op。
   別 id の mount → mount(元) で元に戻る。
7. **`Building` 中の `plan_mount` が `Err(MountError { residence: Building })`**。
   `Retiring` 中の `plan_mount` / `bind_window` が Err。
8. **binding**: 同じ組み合わせは冪等。別の生きた context が持つ窓へ bind → `WindowOwnedBy`。
   既に別の窓を持つ context の bind → `ContextOwnedBy`。`Building` の id の bind → `NotBindable`。
9. **transfer**: 両方生きたまま窓が移る。`from` 不一致 → `WrongOrigin`。
10. **unbind → bind** で同じ context を別の窓へ結べる (always-new の park→再割り当て相当)。
11. **promote**: `main` が新しい id に入れ替わり、退避された旧 main が `AtRest` になる。
    退避された id を窓へ bind できる。`current != main` の投影で `plan_promote` は panic。
12. **retire**: `begin_retire` で `Retiring` になり、`retiring_slot_mut` で payload を読めて、
    `AtRest` の id では `retiring_slot_mut` が `None`。
    **`finish_retire` 後に `Retired` かつ binding が消えている**。
12b. **main は retire できない (I4)**: fork → 別 context を mount → 旧 main は `AtRest`
    になる。この状態で `begin_retire(main)` が `Err(RetireError::IsMain)` を返し、
    **table が健全なまま**であること (main はまだ `AtRest` で `ids()` に居り、mount できる)。
12c. **promote した後なら旧 main は普通に retire できる**。
13. **retire は unbind が drop より先**: `Drop` から table を覗くことは
    (自己参照 / unsafe / 二重帳簿のどれかが要るので) しない。**実行トレース**で見る。
    `Rc<RefCell<Vec<Event>>>` を `TestPayload` に持たせ、
    実行器が `unbind_context_core` の直後に `Event::Unbind(id)` を、
    `TestPayload::drop` が `Event::Drop(id)` を push する。
    **`Unbind` が `Drop` より先**であること、および `unbind` 直後に
    `window_of` / `context_of` から実際に消えていることを assert する。
14. **fork**: **両 policy とも op ベクタを丸ごと `assert_eq!` する**
    (`LiveMediaPark { window_id }` を渡したのに
    `ForkProjectionIntoTransient(MaterializedStillOpen)` を返す実装が全テストを通ってしまう。
    ②-d では policy が 225 フィールドの move / clone 分けを決めるので、間違えると静かに壊れる)。
    派生した payload が**実行器に渡された policy どおりに作られている**ことも確認する。
    `MaterializedStillOpen` は新 id が `AtRest` になり投影は不変、binding は動かない。
    **`LiveMediaPark { window_id }` は `finish_fork` を抜けた時点で窓が fork 側へ移っており、
    元の context も生きている** (`residence(from) == Mounted`)。
    `window_of[current] != window_id` の状態で `plan_fork(LiveMediaPark)` は panic。
15. **op 列の完全一致**: `TableOp` に `Debug` / `PartialEq` を derive し、
    begin / commit / abort / mount / fork / promote / retire の**返り値ベクタを丸ごと
    `assert_eq!`** する (設計 §7 ①の表との一致を文章確認ではなくテストで固定する)。
16. **failpoint sweep (I8 の機械的検証)**: 実行器に「n 個目の op の後で panic する」フックを
    持たせ、**begin / commit / abort それぞれで n を全通り**回して、
    **どこで落ちても binding が 1 つも公開されていない**ことを確認する。
    `catch_unwind` + `AssertUnwindSafe` で受ける。
    ⚠ panic 後は `pending` が立ったままなので **問い合わせ API は assert で落ちる**。
    検証は `window_of` / `context_of` を**直接**見ること。
    ⚠ **この sweep は op の「境目」しか見ない。** op の内部で早く binding を公開し、
    正常終了までに戻す実装は検出できない。①の table は binding を finalizer でしか
    触らないので現状は問題ないが、**②-d の production 実行器では
    `swap_viewer_context_bundle` の内側にも failpoint が要る** (設計 §7 ②-d)。
17. `ids()` が投影中のものを含み、順序が決定的 (id 昇順)。
18. `residence()` が 6 状態すべてを返し分ける。

## 7. スコープ外 (本ステージでやらない)

- `src/app.rs` の `mod` 宣言 1 行以外の production 変更
- `ViewerContextBundle` / `swap_viewer_context_bundle` / `paused_bundle` /
  `active_detached_viewer_context` に触ること
- `DetachedWindowManager` / `DetachedWindowRuntime` に触ること
- 既存テストの変更

## 8. 触ってはいけないもの

- **憲法 3**: App に新しい `bool` / `Option` を足さない。本ステージは App に**フィールドを
  1 つも足さない** (`ContextTable` を `App` に持たせるのは②-d)。
- **憲法 4**: placement の保存先を作らない。`ContextTable` は placement を持たない。
- **憲法 5**: 時間窓を使わない。`residence` は事実。debounce / grace / settle を導入しない。
- **憲法 7**: 本書に書かれていないファイル・機構を「ついでに」直さない。
- **憲法 8**: 既存の detached テスト (約 207 本) を削除・弱体化しない。

## 9. 完了条件

1. `cargo check -p mimageviewer --bin mimageviewer-core` が通る。
2. `cargo test -p mimageviewer --lib viewer_context_registry::` が緑 (§6 の 18 項目すべて)。
3. `cargo test -p mimageviewer --lib` が緑。
4. **既存テストに一切手を入れていない**:
   `git diff --stat HEAD -- src/app/tests.rs src/app/viewer_session.rs` が**空**。
   (「本数が減っていない」より強く、差分ゼロで機械的に確認する)
5. **非 Windows 対応**: モジュール内に**プラットフォーム条件を 1 つも書かない**。
   `#[cfg(test)]` (テストモジュール) は当然必要なので、禁止するのはそれ以外である:

   ```bash
   # cfg / cfg_attr / cfg! のどの形でも、cfg(test) 以外は 1 件も無いこと
   grep -nE 'cfg!?\(|cfg_attr' src/app/viewer_context_registry.rs | grep -v '#\[cfg(test)\]'
   # -> 0 件であること
   # プラットフォーム条件は形を問わず 0
   grep -cE '(cfg!?\(|cfg_attr\()[^)]*(windows|unix|target_os|target_family|target_arch)' \
       src/app/viewer_context_registry.rs
   # -> 0 であること
   ```

   実際の非 Windows コンパイル確認は **CI の ubuntu ジョブ**が担う。
   ローカルの `--features portable` は portable cfg のガードであって非 Windows の確認ではない。
6. `cargo fmt` 済み (pre-commit フックが `cargo fmt --check` を回す)。
7. **モジュール外への公開が 0 件**:
   `grep -cE '^\s*pub(\(|\s)' src/app/viewer_context_registry.rs` が 0。
   `src/app.rs` の追加は `mod viewer_context_registry;` の 1 行だけ
   (`git diff HEAD -- src/app.rs` が 1 行の追加のみ)。
8. モジュール先頭に `#![allow(dead_code)]` 相当を置き、
   **「ステージ②-d で結線する」というコメント**を添える (無言の死蔵にしない)。
9. 完了報告に次を書く:
   - §6 の 18 項目それぞれに対応するテスト関数名
   - failpoint sweep が回した n の総数 (begin / commit / abort 別)
   - 完了条件 4 / 5 / 7 の grep・git コマンドの実際の出力

## 10. 実機 smoke

**不要。** production から呼ばれないコードなので実行時挙動が変わらない。
実機 smoke はステージ②-d 以降で行う。
