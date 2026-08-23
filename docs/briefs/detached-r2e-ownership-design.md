# R2e 設計 — viewer context の所有を型にする (第 3 版)

対象: detached リワーク R2 の残件のうち「context の所有を型にする」部分。
正本プラン: `docs/detached-rework-plan.md` (§2 憲法 / §4 R2 / §9.1 現況 / §9.5 / §11)。
第 1 版は BLOCKER 5 件、第 2 版は 4 件で破棄。本書は第 3 版で、**まだ実装指示書ではない**。

行番号はすべて **2026-08-23 時点の master** で確認したもの。第 2 版の行番号は既に腐っていたので、
本書は全件を取り直している (付録 B に一覧)。

---

## 0. 第 3 版が第 2 版から変えたこと

| # | 第 2 版 | 第 3 版 | 由来 |
| --- | --- | --- | --- |
| 1 | `MountPhase { Mounted / Vacant / Building }` — App の投影側だけを表す | **所在 (`ContextResidence`) を一級の問い合わせにする**。投影は registry の内部状態へ降格 | §6.5 / BLOCKER 1 |
| 2 | `extract_mounted_viewer_context() -> ViewerContextBundle` | **生 bundle を返す API を 1 つも置かない**。build / fork / mount / retire / promote の 5 transaction だけ | BLOCKER 1 |
| 3 | 保管先は `slots` + 「押しのけられた bundle」 | **保管先は slots だけ**。`active_detached_viewer_context` と `paused_bundle` の 2 箇所を 1 箇所へ統合し、active か parked かは `DetachedWindowRuntime.state` だけが持つ | 本書 §1.5 |
| 4 | `ViewerContextId::{ Main, Detached(serial) }` | **`Main` 変種を廃止**。`Main` は identity ではなく binding (役割)。promote 経路が「main の bundle が detached になる」ので、`Main` を identity にすると id を改名する羽目になる | 本書 §3.1 (今回新たに判明) |
| 5 | window 対応表なし | **registry が `window_of` / `context_of` を所有**し、`bind` / `unbind` / `transfer` の順序制約を `Result` で強制 | BLOCKER 2 |
| 6 | 「Building には identity が無い」 | **予約済みだが未 commit**。`begin` で serial を先に払い出す | BLOCKER 4 |
| 7 | R2e-1 で保管を切り、R2e-2 で消費側 (コンパイルできない) | **①抽象データ構造 → ②-pre helper 化 → ②一括切替 → ③巡回 → ④非同期 identity**。①は production を 1 行も切らない | BLOCKER 3 |
| 8 | 完了確認は「プリミティブが registry 経由」 | **第一の門は Rust の可視性**。型を registry モジュールへ移してフィールドを module-private にすると、生成 / destructure / 生 swap がモジュール外で**言語仕様として書けなくなる**。syn 監査は可視性で表せない残り 4 種だけを見る | BLOCKER 3 |

**Codex レビューの反映 (2026-08-23、2 巡)**。第 1 巡: BLOCKER 3 件 (build 中の binding と
I5 の矛盾 / abort された予約 id の分類 / 監査の公開面が狭すぎる) と P1 2 件 (終端 digest の欠落 /
serial 払い出しと投影への焼き込みの順序)、P2 2 件 (②の追加分割 / 件数の誤り)、P3 1 件 (行番号)。
第 2 巡: **folder-nav reopen を「同じ窓・新しい context」と書いていたのは誤り**だったので
§3.5 の根拠を descriptor resume へ差し替え、`rebind_window` と `BuildCommit.displaced` を撤回
(`#[must_use]` は義務を強制できない)。`BuildOutcome::Abort` も、現行に失敗経路が無いので撤回。
`ProjectionSwap` の trait コールバックは借用が重なるため plan を返す形へ変更。監査 A1 / A4 / A7 の
穴を塞ぎ、A1 の有効化を②-c から②-d へ移した。該当箇所には ⚠ 付きで根拠を残した。
第 3 巡: **「生きた context 同士で窓が移る経路は無い」も誤り**だったので (live-media fork が
まさにそれ)、`transfer_window_binding` を追加し `bind_window` に `ContextOwnedBy` を足した。
`BuildOutcome::Abort` は、**build 経路がもう 1 本ある** (`open_bookmark_media_in_detached_context`)
と分かったので復活。監査 A2 / A7 に import 正規化と値位置 move を追加。`TableOp` の
protocol を transient 1 個の形で確定させた。
**アーキテクチャは 3 巡とも変えていない** (Codex も「別の所有モデルは要らない」と判定)。

---

## 1. 一つの説明 — 4 件の失敗は同じ形

### 1.1 4 件

| # | どこ | 単一値が答えていた 2 つの問い | 症状 |
| --- | --- | --- | --- |
| 1 | `active_detached_viewer_context.is_none()` (keep-alive backstop、§11 2026-08-20) | 「所有者が居ない」/「所有者は今マウント中」 | 別 context の題と texture で描画 |
| 2 | `right_drag_pointer_pos(owner)` ([gamepad_input.rs:1189](../../src/app/gamepad_input.rs:1189)) | 「誰の操作か」/「どの producer が供給しているか」 | egui 側が native 側の開始したドラッグを `ButtonStateLost` で cancel |
| 3 | `snapshot.paused_bundle.is_none()` ([app.rs:706](../../src/app.rs:706)) | 「bundle が無い」/「parked-live poll がマウント中」 | 成立コマンドが `viewer_identity_unavailable` で捨てられる |
| 4 | `DetachedViewerWindowPlacement` (レーン A-0、97d1ee98) | 「現在のジオメトリ」/「復元用のジオメトリ」 | 最大化中に placement を毎フレーム書き戻し、82/82 フレームで同一内容。収束しない |

4 件目はレーン A-0 の成果で、R2e とは別の値だが**同じ形**である。

さらに、同じ形が**もう 1 箇所、機能の判定に載っている**ことを今回確認した。

> `should_promote_active_detached_video_for_main_context_change`
> ([app.rs:42720](../../src/app.rs:42720)) は `self.active_detached_viewer_context.is_none()`
> **かつ** `viewer_session_is_detached_or_switching()` で「detached context が holder ではなく App へ
> 直接マウントされている」を表している。ここでの `is_none()` は「居ない」ではなく
> 「ここではない場所に居る」であり、main のフォルダ移動時に再生中メディアを畳まないという
> 実機の挙動がこの読み替えの上に載っている。

### 1.2 共通形

> **1 個の保管場所の「値の有無」で、identity 以外の軸 (所在 / 駆動者 / 時制) を答えている。**

`Option<T>` の `None` は「その場所に無い」しか言えない。ところが実際の問いは
「**どこに** あるか」であり、候補地が 2 つ以上ある。候補地の 1 つを覗いて空だったことは、
残りの候補地について何も言わない。placement (4 件目) も同型で、1 個の struct が
「今の値」と「戻すべき値」という 2 つの時制を持ち、最大化中は前者を書く場所が無い。

### 1.3 3 つの規則

- **規則 A (所在)**: 所在は保管フィールドの `Option` で答えない。**所有者に問い合わせて、
  取り得る所在をすべて列挙した enum を返させる。** 候補地が複数ある限り `is_none()` は嘘をつく。
- **規則 B (駆動者)**: 「誰の状態か (owner)」と「誰が今それを動かしているか (producer)」は
  別の軸。**両方を鍵に含める。** owner だけで絞ると、別 producer の cancel が刺さる。
- **規則 C (時制)**: 「現在値」と「復元値」を 1 個の値に同居させない。片方を書く場所が無くなる
  局面 (最大化・全画面) が必ず来て、書き込みが収束しなくなる。

### 1.4 どの規則を、どこで実装するか

**過剰主張を避けるために切り分けを明示する。R2e が直すのは規則 A だけである。**

| 規則 | 対象 | どこで直すか |
| --- | --- | --- |
| A (所在) | §1.1 の #1 / #3 と、上の promote 判定 | **本書 (R2e)**。候補地を 1 つに統合し、`ContextResidence` を一級の問い合わせにする |
| B (駆動者) | #2 | **R2f (R2b 残件)**。`RightDragOwner` を `(owner, producer)` にする。registry とは独立 |
| C (時制) | #4 | **レーン A-0**。placement を「現在ジオメトリ」と「復元ジオメトリ」に分ける |

3 件が「同じ説明で覆えるか」への答えは、**診断は 1 つ、修理は 3 箇所**である。
3 件全部を R2e に持ち込むと、また範囲が破裂して落ちる。

### 1.5 なぜ「候補地を 1 つに統合する」が効くのか

規則 A の実装は 2 段ある。

1. **候補地を減らす**: 今、1 個の context は活性化 / park のたびに
   `App::active_detached_viewer_context` ([app.rs:10095](../../src/app.rs:10095)) と
   `DetachedImageWindowSnapshot::paused_bundle` ([app.rs:433](../../src/app.rs:433)) の
   **2 つの保管フィールドを往復する**。だから「どちらを見るか」の分岐が全消費者に生える。
   → **slot は 1 種類にする。active か parked かは `DetachedWindowRuntime.state` だけが持つ。**
2. **残った 2 状態 (slot にある / 今マウント中) を型で返す**。マウント中の実体は App の
   フィールドそのものなので、これは保管フィールドを覗いても永久に分からない。
   → `ContextResidence` と、mounted / at-rest を同じ形で読む `ContextRef`。

---

## 2. 4 つの軸に分解する

| 軸 | 問い | 現行の表現 | 第 3 版の表現 |
| --- | --- | --- | --- |
| identity | どの context か | `items_generation` (stamp) / 暗黙の「holder に入っている方」 | `ViewerContextId` (bundle 単位に払い出す serial) |
| residence | その context のバイト列が今どこにあるか | `Option` 2 個の有無の組み合わせ | `ContextResidence` (registry が答える) |
| projection | App の 225 フィールドが今どの context を写しているか | 暗黙 | registry の `Projection` (private) |
| binding | どの窓 / どの役割に結び付いているか | 「その snapshot が bundle を持っている」という**包含関係** | `window_of` / `context_of` / `main` (registry 所有) |
| driver | 今その状態を誰が動かしているか | `RightDragOwner` のみ | **R2f の範囲** (本書では扱わない) |

`ViewerContextBundle` ([app.rs:2465](../../src/app.rs:2465)、225 フィールド) が context の状態を
まとめた入れ物であること、ジェスチャ状態と音楽解析は意図的に App-global のままであることは
第 2 版から変わらない。

---

## 3. 型

置き場所は `src/app/viewer_context_registry.rs` (新設)。`DetachedWindowManager` には置かない
(あちらの責務は window runtime / HWND / activation で、context identity は別境界)。
先例は [`src/app/viewer_session.rs`](../../src/app/viewer_session.rs) — 「per-context 状態の一部を
別モジュールへ出し、交換境界を 1 本にする」を既にやっている。本書はそれを 1 段上でやる。

### 3.1 `ViewerContextId` — `Main` 変種を作らない

```rust
/// context の identity。bundle 1 個に 1 個。OS ウィンドウとも「main かどうか」とも独立。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ViewerContextId(u64);
```

**第 2 版の `ViewerContextId::Main` は採れない。** 理由は今回確認した promote 経路である。

`promote_active_detached_video_for_main_context_change` ([app.rs:42734](../../src/app.rs:42734)) は、
**今マウントされている context をそのまま detached の active context にし、投影を空にする**
(`take_current_viewer_context_bundle` [app.rs:42741](../../src/app.rs:42741) →
`active_detached_viewer_context = Some(..)` [app.rs:42753](../../src/app.rs:42753))。
呼び出し元 ([app.rs:17110](../../src/app.rs:17110) 他 3 箇所) はその空投影へ新しい main を読み込む。

つまり **「main」は context の属性ではなく、その時点でどの context が main 窓の一覧を担っているかという
binding** である。identity にすると、promote のたびに `Main` → `Detached(n)` の**改名**が要る。
改名した id は非同期の相関鍵として使えない (第 1 版が window_id で踏んだのと同じ罠)。

したがって registry が `main: ViewerContextId` を binding として持つ (§3.4)。
`metadata_import_refresh::ContextSlot::Main`
([metadata_import_refresh.rs:63](../../src/app/metadata_import_refresh.rs:63)) は
`registry.main()` の解決結果へ写す (ステージ④)。

**`items_generation` との関係**: detached の generation は
`BASE(1<<63) | serial<<32` ([app.rs:394](../../src/app.rs:394) / [:38094](../../src/app.rs:38094)) なので
serial を復元できるが、**復元を機構として使わない**。promote された context は main 由来の
generation を持ったまま detached になる (今日の挙動) ので、encoding は identity の代用にならない。
`items_generation` は BLOCKER 3 の指示どおり **staleness stamp として据え置き**、
`ViewerContextId` は要求に明示的に載せる (ステージ④)。encoding からの復元は
detached id に限った `debug_assert` の相互チェックにだけ使う。

### 3.2 `ContextResidence` — 要求 7 の一級の問い合わせ

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextResidence {
    /// App の 225 フィールドがこの context を写している。`self.<field>` で直接読み書きできる。
    Mounted,
    /// registry の slot にある。`with_viewer_context` でマウントできる。
    AtRest,
    /// `build_viewer_context` の実行中。id は予約済みで未 commit。slot にはまだ無い。
    Building,
    /// `close_and_retire_context` の digest 実行中。読めるが、マウントも bind もできない。
    Retiring,
    /// **払い出されたことがあり**、その後 retire された (build abort を含む)。
    /// 遅れて届いた非同期結果はここで**静かに**捨ててよい。
    Retired,
    /// 一度も払い出されていない id。**バグの疑い**。捨てる前にログを出す。
    Unknown,
}
```

`Retired` と `Unknown` を分ける理由: 遅延結果の破棄が正常なのか退行なのかを、捨てる側が
判定できるようにするため (無言の fallback を作らないという憲法の要請)。

⚠ **判定基準は「commit 済み」ではなく「払い出し済み」でなければならない**
(Codex 第 3 版レビュー BLOCKER 2)。`build` が abort すると、その `reserved` は
**払い出し済みだが未 commit** のまま消える。`highest_committed_serial` で判定すると
abort された id は「commit 済みより大きい」ので `Unknown` に落ち、`Retired` に
build abort を含めるという上の定義と矛盾する。

正しくは `highest_reserved_serial` (= `next_serial - 1`) を使う。
`ViewerContextId` の生成は registry モジュール private で、払い出しは単調増加なので、
**「払い出し済みなのに今どこにも無い」= `Retired`** が安全に言える。
tombstone 集合は要らず、O(1) 比較のままで足りる。

窓から引く場合:

```rust
/// 窓に紐づく context と、その所在。窓が未 bind なら None。
fn locate_window_context(&self, window_id: u64) -> Option<(ViewerContextId, ContextResidence)>;
```

これが **§6.5 の #1 / #3 と §1.1 の promote 判定を、同じ 1 本で置き換える**。

### 3.3 `Projection` — 要求 6

```rust
/// App の 225 フィールドが今何を写しているか。registry モジュール private。
enum Projection {
    Mounted(ViewerContextId),
    /// 予約済み・未 commit の context を組み立てている。
    /// `previous` は commit / panic 時に投影へ戻す先。
    /// `pending_bind` は commit と同時にだけ公開される窓の予約 (I8)。
    Building {
        reserved: ViewerContextId,
        previous: ViewerContextId,
        pending_bind: Option<u64>,
    },
}
```

第 2 版の `Vacant` は**消える**。`Vacant` は「投影に空 bundle が載っている」という実装の
中間状態にすぎず、transaction の中にしか存在しない。外から観測できない状態を型に出すと、
それを見に来る消費者が生えて第 2 版の失敗を繰り返す。
`promote` の「投影を空にして呼び出し元が新しい main を読む」形も、専用 transaction (§4.5) に
することで `Vacant` を露出せずに表せる。

`Building` は「identity が無い」ではなく「**予約済みだが未 commit**」(BLOCKER 4)。
`begin` が最初に serial を払い出し、`items_generation` を投影へ焼いてから本体へ入る。
組み立て中に開始された worker は、その時点で予約済み identity の generation を持つ。

### 3.4 `ViewerContextRegistry`

```rust
pub(crate) struct ViewerContextRegistry {
    projection: Projection,
    /// マウントされていない context。**保管先はここだけ。**
    #[cfg(windows)]
    slots: HashMap<ViewerContextId, Slot>,
    /// main 窓の一覧を担う context (§3.1)。
    main: ViewerContextId,
    /// context → detached 窓。context の生存期間に従う。
    #[cfg(windows)]
    window_of: HashMap<ViewerContextId, u64>,
    /// 窓 → context。`bind` / `unbind` だけが更新する派生 index。
    #[cfg(windows)]
    context_of: HashMap<u64, ViewerContextId>,
    /// 次に払い出す serial。`highest_reserved_serial` は `next_serial - 1` (§3.2)。
    next_serial: u64,
}

/// slot の中身。`Retiring` は digest 実行中の一時状態。
enum Slot { AtRest(Box<ViewerContextBundle>), Retiring(Box<ViewerContextBundle>) }
```

**不変条件**

| # | 不変条件 | 守り方 |
| --- | --- | --- |
| I1 | context の bundle は `slots` か投影のどちらか一方にだけ在る | 生 bundle を返す API が無い (§3.7) |
| I2 | `window_of` と `context_of` は互いの逆写像 | 更新は `bind` / `unbind` の 2 本だけ (module private の共通実装) |
| I3 | 1 つの窓に context は高々 1 つ | `bind` は別の生きた context に bound の窓を `Err` で拒否する。置換したければ先に `retire` |
| I4 | `main` は常に存在し、`slots` か投影のどちらかに在る | `promote` は新しい main を作ってから古い main を bind する |
| I5 | `Building` 中は mount / retire を受け付けない。**binding は即時公開せず transaction に積む** (§4.2) | `Projection` の match で typed error を返す。binding は `Building` が保持する `pending_bind` |
| I6 | `Retiring` 中の id は mount / bind できない | `Slot::Retiring` への遷移 |
| I7 | 投影は毎 root pass の末尾で `Mounted` | 定義した継ぎ目での assert (時間窓ではない。憲法 5) |
| I8 | binding は commit と同時にだけ公開される。panic unwind では 1 つも公開されない | `pending_bind` を commit 経路でのみ適用 |

⚠ I5 の「bind を受け付けない」を素直に読むと §4.2 の build と矛盾する
(Codex 第 3 版レビュー第 1 巡 BLOCKER 1)。実際の build は **load を始める前に窓を確保し、
runtime と session を立てる** ([app.rs:40565](../../src/app.rs:40565) /
[:40573](../../src/app.rs:40573) / [:40593](../../src/app.rs:40593)) ので、
`f` の中で窓が決まる。解決は「bind を許す」ではなく **「bind を予約として積み、
commit と原子的に公開する」** である (§4.2)。

### 3.5 `window_id → ViewerContextId` をどこに置くか — 要求 4

**決定: registry が所有する。`DetachedWindowRuntime` にも `DetachedImageWindowSnapshot` にも置かない。**

根拠:

1. **窓は context より長生きする。** 1 個の窓が、生涯のうちに**別の context を受け取る経路が
   実在する**。passive 窓の activation で bundle が既に無い場合、descriptor から
   **新しい context を組み立てて同じ `snapshot.id` へ結ぶ**
   ([app.rs:41244](../../src/app.rs:41244)〜[:41250](../../src/app.rs:41250) →
   [`resume_active_detached_book_context_from_descriptor`](../../src/app.rs:40480) →
   build へ `resume_window_id = Some(window_id)` [app.rs:40566](../../src/app.rs:40566))。
   窓側に context を持たせると、この差し替えのたびに runtime を書き換えることになり、
   所有者が 2 つになる (BA-6 と同じ形)。
2. **main には窓が無い。** runtime 側に持たせると main だけ表現できず、また分岐が生える。
3. **activation は window_id から始まる** ([app.rs:41013](../../src/app.rs:41013)) が、
   registry が逆引き index を持てば O(1) で足りる。runtime に持たせる理由にならない。
4. 憲法 3 に抵触しない: App に新しい `bool` / `Option` を足すのではなく、**既存の 2 つの保管
   フィールドを 1 つの typed owner に畳む**方向である (憲法 3 が指している「R2 で導入する
   state owner へ足す」そのもの)。

⚠ **第 3 版の初稿はここで folder-nav reopen を根拠に挙げていたが、それは誤りだった**
(Codex 第 3 版レビュー第 2 巡 BLOCKER)。`close_fullscreen_for_folder_nav_reopen`
([app.rs:52211](../../src/app.rs:52211)) は **マウント中の context をそのまま保ったまま**
fullscreen セッション状態だけを畳み、`detached_viewer_window_id` と `fs_viewport_generation` を
復元して次フォルダを読む。つまり Ctrl+↑↓ の reopen は「**同じ窓・同じ context・新しいフォルダ**」で、
context は差し替わらない。`ensure_detached_viewer_window_id` の window_id 再利用
([app.rs:38304](../../src/app.rs:38304)〜) は **egui の ViewportId を安定させる**ためのもので
(毎回新しい id を振ると OS 窓が破棄→再生成され、既定サイズ 822x656 の小窓がカスケードする)、
context の差し替えを意味しない。窓側に置けない根拠は上の 1 (descriptor resume) に置き換えた。

**binding を動かす操作は 3 つだけ。それ以外の形は書けない。**

```rust
/// 窓へ context を結ぶ。
/// - 未 bind の窓 & 未 bind の context → Ok
/// - **同じ組み合わせ**で既に bound → Ok (冪等)
/// - 窓が**別の生きた context** に bound → Err(BindError::WindowOwnedBy(other))
/// - context が**別の窓**に bound → Err(BindError::ContextOwnedBy(other_window))
fn bind_window(&mut self, id: ViewerContextId, window_id: u64) -> Result<(), BindError>;

/// 窓の binding を外す。context は生き残る (park の「窓を手放す」に対応)。
fn unbind_window(&mut self, window_id: u64) -> Option<ViewerContextId>;

/// **生きている 2 つの context の間で窓を移す。** `from` は移した後も生き続ける。
/// `from` が実際にその窓を持っていなければ `Err` (期待値照合)。
/// fork の直後にしか呼ばれない (§4.3)。
fn transfer_window_binding(
    &mut self,
    window_id: u64,
    from: ViewerContextId,
    to: ViewerContextId,
) -> Result<(), BindError>;
```

⚠ **`transfer_window_binding` が要る理由** (Codex 第 3 版レビュー第 3 巡 BLOCKER)。
第 3 版の第 2 稿は「生きた context 同士で窓が移る経路は無い」と書いたが、**live-media fork が
まさにそれ**である。`park_current_viewer_context_as_live_media_inner`
([app.rs:41868](../../src/app.rs:41868)) は `preserve_main_context = true` で
`split_current_context_preserving_main_grid` を呼び、**マウント中の context を生かしたまま**
fork する。ところが `viewer_session` は `move_to_parked` 側なので
([app.rs:42297](../../src/app.rs:42297) の `swap_with_mounted`)、`detached_viewer_window_id` は
fork へ移り、snapshot は同じ窓 id で push される ([app.rs:41889](../../src/app.rs:41889))。
つまり `W: C → C2` で **`C` も `C2` も生きている**。`bind_window` だけでは
`WindowOwnedBy(C)` で弾かれ、`C` を retire するのは誤りである。

**`rebind_window` は置かない。** 第 2 稿が撤回した理由はそのまま有効で、`transfer` はその
代替ではない。`transfer` は「**両方生きている**ことが前提の、fork 専用の原子操作」であり、
「旧 context を押し出して破棄の責務を呼び出し元へ渡す」形ではない。破棄が要るなら
先に `retire` する。`#[must_use]` の戻り値で義務を表す形は採らない
(`let BuildCommit { id, .. } = commit;` で合法に捨てられるため)。

**`ContextOwnedBy` が要る理由**: always-new の静止画 open は、古い窓を park してから
([app.rs:42579](../../src/app.rs:42579)) 同じマウント中の context に**新しく確保した窓**を
割り当てる ([app.rs:42794](../../src/app.rs:42794))。park が `unbind_window` を先に呼ぶことが
前提条件で、忘れると 1 context が 2 窓に結ばれる。`ContextOwnedBy` はそれを弾く。
descriptor resume の「その窓は context を持っていない」という前提も、park 時の
`unbind_window` が効いていて初めて成り立つ。

`DetachedImageWindowSnapshot` は bundle を手放し、**park 中に描くための凍結表示データと
`id` だけ**になる ([app.rs:415](../../src/app.rs:415)〜)。これは責務として正しい分離で、
`can_activate()` / `has_paused_bundle()` / `right_drag_context()` / `right_drag_viewer_identity()`
([app.rs:697](../../src/app.rs:697)〜[:730](../../src/app.rs:730)) は registry への問い合わせになる。

### 3.6 `ContextRef` / `ContextMut` — mounted も at-rest も同じ形で読む

```rust
/// 1 つの context への読み取り。マウント中なら App のフィールド、そうでなければ slot を読む。
/// 呼び出し側はどちらか知らなくてよい。
pub(crate) struct ContextRef<'a> { /* private: Mounted(&App) | AtRest(&ViewerContextBundle) */ }

impl ContextRef<'_> {
    pub(crate) fn id(&self) -> ViewerContextId;
    pub(crate) fn items_generation(&self) -> u64;
    pub(crate) fn fullscreen_idx(&self) -> Option<usize>;
    pub(crate) fn item_at(&self, idx: usize) -> Option<&GridItem>;
    pub(crate) fn current_folder(&self) -> Option<&Path>;
    // ... 外から実際に読まれている ~26 種だけ (付録 B-3)
}
```

これが効く具体例: 今は**同じ述語が bundle 版と App 版で二重定義**されている。

- `viewer_context_bundle_contains_video(&ViewerContextBundle)` ([app.rs:39104](../../src/app.rs:39104))
- `current_viewer_context_contains_video(&self)` ([app.rs:39197](../../src/app.rs:39197))

本文はほぼ同一で、片方だけ直せば静かに乖離する。`ContextRef` 版 1 本に畳める。
同型の bundle 引数ヘルパーは非テストで 12 本ある (付録 B-4)。

`ContextMut<'_>` は retire の digest 専用 (§4.4)。

### 3.7 生 bundle を渡す境界は 1 つも作らない — 要求 1

registry の公開面に、次のシグネチャを**置かない**。これは §6.3 の監査 allowlist にも反映する。

- `-> ViewerContextBundle` / `-> Box<ViewerContextBundle>` / `-> Option<Box<ViewerContextBundle>>`
- `-> &mut ViewerContextBundle` / `-> &mut Box<ViewerContextBundle>`
- `-> &mut HashMap<ViewerContextId, Slot>` (slot map の貸し出し)

第 2 版の `extract_mounted_viewer_context()` はここに該当したため、I1 が最初から破れていた。

---

## 4. API — 5 つの transaction

context の所有を動かす操作は、次の 5 種**しかない**。それ以外の形は書けない (§6.1)。

| transaction | 現行の対応 | 所有の動き |
| --- | --- | --- |
| **mount** | `with_active_detached_viewer_context` ([app.rs:16698](../../src/app.rs:16698)) と手書き 18 箇所 | slot → 投影 → slot (必ず戻る) |
| **build** | `start_active_detached_book_context_with_start` ([app.rs:40530](../../src/app.rs:40530)) | 投影を新 context にして組み立て、slot へ入れ、直前を戻す |
| **fork** | `split_current_context_preserving_main_grid` ([app.rs:41922](../../src/app.rs:41922)) | 投影を保ったまま 2 個目を作り、slot へ入れる |
| **retire** | `take_and_close_...` ([app.rs:38577](../../src/app.rs:38577)) / `teardown_paused_media_bundles_for_window_ids` ([app.rs:39424](../../src/app.rs:39424)) | 読んでから drop。外へは出さない |
| **promote** | `promote_active_detached_video_for_main_context_change` ([app.rs:42734](../../src/app.rs:42734)) | 投影中の context を slot へ退避し、新しい空 context を投影にする |

### 4.1 mount

```rust
/// id の context をマウントして f を実行し、必ず元へ戻す (panic 時も)。
/// 既にマウント中ならそのまま f を実行する (再入は swap しない)。
fn with_viewer_context<R>(&mut self, id: ViewerContextId, f: impl FnOnce(&mut Self) -> R)
    -> Result<R, MountError>;

/// 窓から引く版。activation 経路 (window_id 始まり) 用。
fn with_window_viewer_context<R>(&mut self, window_id: u64, f: impl FnOnce(&mut Self) -> R)
    -> Result<R, MountError>;
```

`MountError` は `ContextResidence` を持つ (`Building` / `Retiring` / `Retired` / `Unknown`)。
**`Option` を返さない**のは、「マウントできなかった」を無言で読み飛ばす今の
`else { continue }` を再生産しないため (付録 B-2 に 6 箇所)。

panic 安全は既存 `with_active_detached_viewer_context` と同じ `catch_unwind` +
`resume_unwind` で担保する。**今の手書き 18 箇所にはこれが無く、panic すると押しのけられた
bundle が drop され、`Drop for ViewerContextBundle` ([app.rs:2743](../../src/app.rs:2743)) が
その context の worker を cancel する。**

### 4.2 build — 要求 1 / 要求 6

```rust
/// 新しい context を投影上で組み立てる。id は begin で予約済み。
/// f が Commit を返せば slot へ入れ、予約された binding を公開し、直前の context を投影へ戻す。
/// Abort / panic なら組み立て中の投影を retire 扱いで畳み、**binding は 1 つも公開せず**、
/// 直前を戻す (panic はその後 resume_unwind)。
fn build_viewer_context(
    &mut self,
    reason: &'static str,
    f: impl FnOnce(&mut Self, ViewerContextId) -> BuildOutcome,
) -> Option<ViewerContextId>;

pub(crate) enum BuildOutcome { Commit, Abort(&'static str) }
```

⚠ **`Abort` は実在する経路である** (Codex 第 3 版レビュー第 3 巡 BLOCKER)。
第 3 版の第 2 稿は「現行 build に失敗経路が無い」として `Abort` を落としたが、
**build 経路は `start_active_detached_book_context_with_start` だけではない**。
`open_bookmark_media_in_detached_context`
([startup_ops.rs:608](../../src/app/startup_ops.rs:608)〜) は同型の手書き build transaction で、
`load_folder_or_convert_archive_with_auto_fullscreen` の `FolderOpenOutcome`
([app.rs:262](../../src/app.rs:262)、`ConversionDialogOpened` / `Ignored` を含む) が
目的のメディアを開けなかった場合、**窓と session を畳み、組み立て中の context を捨て、
main を戻して `false` を返す** ([startup_ops.rs:652](../../src/app/startup_ops.rs:652)〜
[:654](../../src/app/startup_ops.rs:654))。これが `Abort` そのものである。

`Abort` では窓側の後始末 (`begin/finish_active_detached_session_close` /
`remove_detached_window_runtime`) は**呼び出し元が今までどおり明示的に行う**。
registry が肩代わりしない — 下の ⚠ のとおり窓側の副作用は transaction の外にあり、
それを暗黙に巻き戻す機構を R2e で新設しない。

**`f` の中で窓が決まる問題 (第 1 巡 BLOCKER 1) の扱い。** 現行の build は load を始める前に
window_id を確保して runtime / session を立てるので、binding は `f` の中で決まる。
これを即時 bind にすると I5 と衝突する。そこで `f` は次の 1 本だけを呼び、
**registry は予約を `Building` に積む**。

```rust
/// build 中に、この context を結ぶ窓を予約する。commit まで公開されない。
/// 同じ窓の再予約は冪等。**別の窓を 2 回目に予約したら panic** (build は窓 1 つに対応する)。
fn reserve_window_binding_for_build(&mut self, window_id: u64);
```

descriptor resume ([app.rs:40480](../../src/app.rs:40480)) は
`resume_window_id = Some(window_id)` で既存の窓を渡してくるが、その窓は
**その時点で context を持っていない** (bundle があれば resume 経路が先に走る) ので、
commit 時の公開は素直な `bind_window` になる。

⚠ **窓側の副作用は transaction の外にある** (Codex 第 3 版レビュー第 2 巡 BLOCKER)。
`f` は load を始める前に viewport runtime を reset し
([app.rs:40561](../../src/app.rs:40561))、placement を seed し
([app.rs:40573](../../src/app.rs:40573))、`Opening` へ遷移させ、
`begin_active_detached_session` を呼び、`last_active_detached_window_id` を更新する
([app.rs:40593](../../src/app.rs:40593))。**これらは bundle の一部ではないので、
transaction のロールバックでは戻らない。** descriptor resume はさらに、build を呼ぶ前に
`remove_detached_window_runtime_preserving_activation`
([app.rs:41244](../../src/app.rs:41244)) で旧 runtime を外している。

第 3 版はこれを**直さない**。`Abort` を返す既存経路
([startup_ops.rs:636](../../src/app/startup_ops.rs:636)〜) は窓側の後始末を自分で書いており、
その形を維持する。panic 時の窓側状態は**今日も復元されていない**が、R2e はそれを悪化させない。
第 3 版が改善するのは「panic で押しのけられた bundle が drop され worker が cancel される」
という今日の実害の方である (§4.1)。窓側の副作用まで transaction に取り込むなら、先に
40561 / 40573 / 40593 を commit フェーズへ移せるかを確定する必要がある (§10 の質問 10)。

**token 型にせず closure にする理由**: token (`BuildTxn`) が registry を借りると、本体で
`&mut App` を使えない。token を値にすると未 commit のまま落ちる経路を `Drop` で救えない
(`Drop` から App へ戻れない)。現行の build 経路は**すべて 1 関数の中で完結している**ので
closure で足りる。既存 `with_detached_viewer_main_history_suppressed`
([app.rs:16682](../../src/app.rs:16682)) と同じ形。

**現行 `start_active_detached_book_context_with_start` との対応**:

| 現行 | 行 | 第 3 版 |
| --- | --- | --- |
| `let mut main_context = self.take_current_viewer_context_bundle();` | 40545 | `begin` の中 (slot へ退避)。スタックローカルに出さない |
| `self.navigation_scope = DetachedPhysical;` ほか | 40548〜40559 | `f` の中 (投影への通常の書き込み) |
| `let context_serial = self.assign_next_detached_viewer_context_generation();` | 40560 | **2 つに割って `begin` の中へ。** ①serial と generation の払い出し (純粋、投影に触らない) → ②main を slot へ退避 → ③**新しい空投影へ** generation を焼く → ④`f` へ。`f` は `reserved` を受け取る |
| `reset_active_detached_viewport_runtime_for_new_window(context_serial, ..)` | 40561 | `f` の中 (`reserved` を渡す) |
| `let window_id = ... allocate_detached_viewer_window_id()` | 40565 | `f` の中。直後に `reserve_window_binding_for_build(window_id)` (**即時 bind ではない**。公開は commit 時) |
| 各 `load_*` / `start_*` | 40598〜40634 | `f` の中 |
| `let active_context = self.take_current_viewer_context_bundle();` | 40654 | `commit` の中 |
| `self.swap_viewer_context_bundle(&mut main_context);` | 40655 | `commit` の中 (slot から復帰) |
| `self.active_detached_viewer_context = Some(..)` | 40656 | 消える (slot が保管先) |

⚠ **払い出しと「焼く」を分けることが必須** (Codex 第 3 版レビュー P1)。
`assign_next_detached_viewer_context_generation` ([app.rs:38100](../../src/app.rs:38100)) は
払い出した generation を**その場でマウント中の投影へ焼く**。
`set_items_generation` ([app.rs:24864](../../src/app.rs:24864)) は generation 不一致の
cache 項目を捨てるので、**これを現行の `take` より前へそのまま動かすと main の context が壊れる**。
上の表の①〜④の順序 (払い出しは純粋、焼くのは退避後の新しい空投影に対して) を守ること。

40546〜40559 の間に `items_generation` を読む処理が無いことは Codex 側でも確認済み
(`PendingBookOpen::begin_page_wait` は stage と時刻しか触らない、
[bookmark_browser.rs:899](../../src/bookmark_browser.rs:899))。

**abort の規約**: `Abort` / panic では、組み立て途中の投影を `retire` と同じ経路で畳む
(worker cancel が走る)。`reserved` は `Retired` になるので、遅れて届いた結果は静かに
捨てられる。abort は理由をログに出す (無言 fallback を作らない)。
**窓側の後始末は呼び出し元の責務**で、registry は binding を公開しないことだけを保証する。

### 4.3 fork — 要求 2

```rust
/// 投影 (= マウント中の context) を保ったまま、2 個目の context を作って slot へ入れる。
/// 生 bundle は返らない。返るのは id だけ。
fn fork_mounted_context(&mut self, policy: ForkPolicy, reason: &'static str) -> ViewerContextId;

pub(crate) enum ForkPolicy {
    /// live-park。main grid が使う一覧 identity / worker 複合体は投影に残し、
    /// viewer の一時状態だけを新 context へ移す。
    LiveMediaPark,
    /// materialize 済みの物理一覧から独立静止画窓を開く。LiveMediaPark に加えて
    /// 表示・編集系 ~40 フィールドを複製する。
    MaterializedStillOpen,
}
```

現行の 3 分類マクロ (`duplicate_for_parked!` / `move_to_parked!` / `keep_in_main!`,
[app.rs:41924](../../src/app.rs:41924)〜) はそのまま registry モジュールへ移す。
**225 フィールドの exhaustive destructure がフィールド追加時にコンパイルエラーになる性質は
維持する** (今の設計の一番良い部分で、壊してはいけない)。

**「fork してから条件を見て捨てる」を前置き判定へ直す。**
現行 `park_current_viewer_context_as_live_media_inner` ([app.rs:41868](../../src/app.rs:41868)) は
fork した後に `viewer_context_bundle_contains_video(&parked_bundle)` を見て、偽なら捨てる。
fork した側 (`preserve_main_context = true`) では、捨てた bundle へ `move_to_parked!` で移した
状態がそのまま失われる。判定材料 (`fullscreen_idx` / `items` / `fs_cache` /
`vst3_deferred_media_open`) は**分割前の投影にすべて揃っている**ので、fork の前に
`current_viewer_context_contains_video()` ([app.rs:39197](../../src/app.rs:39197)) で判定できる。
呼び出し元 `park_detached_session_for_stack_aggregation` ([app.rs:41854](../../src/app.rs:41854)) は
既にそうしている。

⚠ これも**挙動同値性の検証項目**。ステージ②で「前置き判定と後置き判定が一致すること」を
テストで固定してから入れ替える。一致しないケースが見つかったら、消さずに報告する。

fork の直後の binding も呼び出し元が行う (窓を作るのは registry の責務ではない)。
`LiveMediaPark` は**マウント中の context が持っていた窓を fork へ移す**ので
`transfer_window_binding(window_id, from = mounted, to = forked)`、
`MaterializedStillOpen` は新しい窓なので `bind_window`。§3.5 参照。
現行 `route_materialized_physical_still_open_to_active_context` ([app.rs:42485](../../src/app.rs:42485)) が
serial を `let (_context_serial, items_generation) = ...` ([app.rs:42514](../../src/app.rs:42514)) と
**捨てている**のは、今 context に id を持たせる場所が無いからで、fork が id を返せば解消する。

### 4.4 retire — 要求 3

```rust
/// 終端削除。bundle はこの関数の中で drop され、外へは出ない。
///
/// - `finish`: **その context をマウントした状態**で走る終了処理
///   (viewport への Close 送信 / `close_fullscreen`)。
/// - `digest`: アンマウント後、drop の直前に中身から所有値を作る。
///   `ContextMut` なので、drop 前に畳んでおきたいフィールドの片付けもここで行う。
fn close_and_retire_context<D>(
    &mut self,
    id: ViewerContextId,
    reason: &'static str,
    finish: impl FnOnce(&mut Self),
    digest: impl FnOnce(ContextMut<'_>) -> D,
) -> Result<D, MountError>;

/// `finish` の要らない版 (parked / media teardown)。
fn retire_context<D>(&mut self, id, reason, digest) -> Result<D, MountError>;
```

**2 つの現行経路がそのまま乗る。**

*(a) ブックマーク照合* ([app.rs:41497](../../src/app.rs:41497)〜[:41509](../../src/app.rs:41509))

```
現行: take_and_close_...() が生 bundle を返す
      → reconcile_closed_bookmark_detached_context(&closed) が
        bookmark_view_state / selected / items[idx] / archive_source_override /
        current_folder を読み、main をマウントした状態で load_folder... まで実行する
      → closed が drop
第3版: let summary = self.close_and_retire_context(id, reason,
           |app| { /* viewport Close + close_fullscreen */ },
           ClosedBookmarkSummary::read)?;
       self.reconcile_closed_bookmark_detached_context(&summary);   // main がマウントされた状態
```

`ClosedBookmarkSummary` は所有値だけを持つ小さな struct。**借用衝突が起きない**のがこの形の要点で、
`&mut App` と bundle の同時借用を避けるために生 bundle を外へ出す必要がなくなる。
第 2 版が「アンマウント後に slot を消す」で足りないと言われたのは、この読みが drop の前に
必要だからである。

⚠ **digest に含めるものを取りこぼさない** (Codex 第 3 版レビュー P1)。上に挙げた
`bookmark_view_state` / `selected` / `items[idx]` / `archive_source_override` / `current_folder` に加えて、
現行の終端 close は **`closed_context.pdf_password_request.is_some()`** を見て、main 側に
request が無ければ PDF パスワードダイアログの App-global state を畳んでいる
([app.rs:38595](../../src/app.rs:38595)〜[:38605](../../src/app.rs:38605))。
これを落とすと viewport 作成前の終端 close で orphan ダイアログが残る。
`ClosedBookmarkSummary` にこの 1 つの `bool` を含める。

*(b) メディア teardown* ([app.rs:39424](../../src/app.rs:39424))

```
現行: dropped_bundles: Vec<Box<ViewerContextBundle>> を作り、plan を計算し、
      save/cleanup し、normalize_* を clear してから drop
第3版: let plans: Vec<_> = ids.iter()
           .filter_map(|&id| self.retire_context(id, reason, |mut cx| {
               let plan = viewer_context_media_teardown_plan(cx.as_ref());
               cx.clear_normalize_state();
               plan
           }).ok())
           .collect();
       self.save_viewer_context_media_teardown_resumes(&plans);
       self.cleanup_viewer_context_media_teardown_globals(&plans, reason);
```

⚠ **teardown の前段 2 つを落とさない** (Codex 第 3 版レビュー P1)。上の疑似コードは
`retire_context` のループだけを書いているが、現行は bundle を外す**前に**
`clears_tile_companion` と `clears_mode_switch` を決めている
([app.rs:39429](../../src/app.rs:39429)〜[:39452](../../src/app.rs:39452))。
どちらも**閉じる側と生存側の両方の context を見る**判定
([app.rs:39322](../../src/app.rs:39322)〜[:39417](../../src/app.rs:39417)) なので、
retire ループより前に `any_viewer_context` で確定させ、その結果を所有値として持ち回る。
順序を変えると、生存側の video を見落として tile overlay や mode-switch を誤って畳む。

**生存側を見る読みも registry が答える。**
`closing_parked_windows_own_native_video_mode_switch` ([app.rs:39396](../../src/app.rs:39396)) は今、
「マウント中」「active holder」「parked 群」の **3 経路**を別々に見ている
(`current_viewer_context_contains_video` / `active_detached_viewer_context_contains_video` /
`detached_image_windows` の走査)。第 3 版では 1 本になる。

```rust
// retire ループより前に評価する。
let others_have_video = self.any_viewer_context(|id, cx|
    !closing.contains(&id) && cx.contains_video());
```

### 4.5 promote

```rust
/// 投影中の context を slot へ退避し、新しい空 context を投影にする。
/// 戻り値は退避された側の id (呼び出し元が bind_window する)。
#[must_use]
fn stash_mounted_and_start_fresh(&mut self, reason: &'static str) -> ViewerContextId;
```

`promote_active_detached_video_for_main_context_change` はこれを呼び、返った id を
`bind_window(id, window_id)` し、新しい投影 (= 新しい main) を `registry.main` にする。
**`Vacant` を型に出さずに済む**のはこの transaction があるからである。

### 4.6 巡回

```rust
/// 現在 context を持っている全 id (マウント中のものを含む)。
fn viewer_context_ids(&self) -> Vec<ViewerContextId>;

/// 順にマウントして f を呼ぶ。マウントできない id は typed reason 付きでスキップし、ログを出す。
fn for_each_viewer_context(&mut self, reason: &'static str, f: impl FnMut(&mut Self, ViewerContextId));

/// マウントせずに読むだけの走査 (mounted / at-rest を同じ形で見る)。
fn any_viewer_context(&self, f: impl FnMut(ViewerContextId, ContextRef<'_>) -> bool) -> bool;
fn with_viewer_context_ref<R>(&self, id: ViewerContextId, f: impl FnOnce(ContextRef<'_>) -> R) -> Option<R>;
```

置き換え対象 (ステージ③): [app.rs:30417](../../src/app.rs:30417) `rebuild_all_viewer_context_visible_indices` /
[app.rs:54317](../../src/app.rs:54317) `clear_all_edit_preview_materializations` /
[app.rs:19795](../../src/app.rs:19795) `consume_deferred_vst3_media_open_in_all_contexts` /
[app.rs:28993](../../src/app.rs:28993) rename 後の rehydrate。

**非 Windows**: `for_each_viewer_context` / `any_viewer_context` / `with_viewer_context_ref` /
`residence` は cfg 中立にし、非 Windows では投影 1 個だけを回す。これで呼び出し側から
`#[cfg(windows)]` ブロックが消え、ubuntu の `cargo check` ジョブ (CLAUDE.md リリース手順 Phase 2) が
そのまま番人になる。slot / binding / detached transaction は `#[cfg(windows)]`
(`ViewerContextBundle` 自体が `#[cfg(windows)]` のため)。

---

## 5. 不変条件を何が守るか

| 不変条件 | 型 | 借用 | 可視性 | 監査 | テスト |
| --- | --- | --- | --- | --- | --- |
| I1 bundle は slot か投影のどちらか一方 | ○ (生 bundle を返す API が無い) | — | ○ | ○ (A1/A4) | ○ |
| I2 window_of / context_of が逆写像 | — | — | ○ (private field) | — | ○ |
| I3 1 窓 1 context | ○ (`bind` が Err) | — | — | — | ○ |
| I4 main は常に存在 | ○ (`main: ViewerContextId` は非 Option) | — | — | — | ○ |
| I5 / I6 Building / Retiring 中の禁止操作 | ○ (`MountError`) | — | — | — | ○ |
| I7 root pass 末尾で Mounted | — | — | — | — | ○ (継ぎ目の assert) |
| 所在を `is_none()` で答えない | ○ (`ContextResidence`) | — | ○ | ○ (A5) | ○ |
| 225 フィールドの取りこぼしが無い | ○ (exhaustive destructure) | — | — | — | コンパイルエラー |
| mount が panic 安全 | — | ○ (`catch_unwind` を 1 箇所に集約) | ○ | ○ (A2) | ○ |

---

## 6. 完了確認 — コンパイラを第一の門にする

BLOCKER 3 の「grep では不十分」への答えは 2 段構えである。

### 6.1 Rust の可視性だけで弾けるもの (監査コード不要)

`src/app/viewer_context_registry.rs` へ次を移し、**`ViewerContextBundle` の全 225 フィールドを
そのモジュール private にする**。

- `struct ViewerContextBundle` (型自体は `pub(in crate::app)`、**フィールドは private**)
- `impl Drop for ViewerContextBundle` / `empty()` / `set_items_generation()`
- `swap_viewer_context_bundle`
- fork の 3 分類 destructure (両 policy)
- `ContextRef` / `ContextMut` と accessor 群
- `ViewerContextRegistry` / `ViewerContextId` / `ContextResidence` / `Projection` / `Slot`

Rust の可視性規則 (子モジュールは親の private を見られるが、親は子の private を見られない) により、
**registry モジュールの外では次が言語仕様として書けなくなる**。lint も grep も要らない。

| 書けなくなるもの | 理由 |
| --- | --- |
| `ViewerContextBundle { .. }` (struct literal) | private field |
| `let ViewerContextBundle { a, b, .. } = x` (destructure) | private field |
| `bundle.items` / `bundle.fs_cache` などの直接アクセス | private field |
| `std::mem::swap(&mut self.items, &mut bundle.items)` | private field |
| `ViewerContextBundle::empty()` | private fn |

現在この 3 種は非テストで **3 箇所しかない** (destructure :16189 / `empty()` :16667 / fork :41939) ので、
移設そのものは小さい。効くのは「**将来もう書けない**」という性質のほうである。

**代償**: 外から読まれている ~26 フィールドに accessor が要る (付録 B-3)。
`src/app/vram_accounting.rs:70` のような子モジュールからの読みも accessor 経由になる。

### 6.2 可視性で弾けない残り

| 残る穴 | 例 | 監査 |
| --- | --- | --- |
| 型名を使って**モジュール外に保管場所を作る** | `struct Foo { b: Option<Box<ViewerContextBundle>> }` — 型は nameable | A1 |
| **App のフィールドを直接 `mem::swap`** して context を手で動かす | `mem::swap(&mut self.items, &mut stash)` — bundle 型を一切使わない | A2 |
| registry モジュール**内部**の逸脱 | slot map を貸す accessor を足す | A4 |
| **公開面が静かに育つ** | `pub(super) fn active_bundle_is_none()` / closure 境界に生 bundle を混ぜる / `Deref` を生やす | A4 |
| **registry 自体を動かす** | `mem::take(&mut self.viewer_contexts)` して別の所有者へ渡す | A7 |

### 6.3 syn 監査が許可するもの / 弾くもの

`tools/viewer_context_audit` (workspace member、`syn` の `full` + `visit` を使う source-only の bin)。
vendor 資産を要らないので ubuntu CI でそのまま走る。

| # | 規則 | 対象 | 弾く例 | 許可する例 |
| --- | --- | --- | --- | --- |
| **A1** | `ViewerContextBundle` が **型位置** (struct / enum のフィールド型、fn の引数・戻り値、`let` の型注釈、ジェネリック実引数、`impl` の対象) に出てよいのは registry モジュールだけ。**registry モジュール外で、この型への `use ... as X` / 型エイリアス / 再エクスポートを定義してはならない** (別名を作れば A1 を素通りできるため) | `src/**/*.rs` 全部 (tests.rs 含む) | `Vec<Box<ViewerContextBundle>>` を関数の戻り値にする / `use ...::ViewerContextBundle as B; struct Foo { b: B }` | doc comment / 文字列リテラル中の言及 |
| **A2** | registry モジュール外で `mem::swap` / `mem::replace` / `mem::take` の実引数に **任意の receiver のフィールドアクセス `<expr>.<F>`** が現れてはならない (`self.<F>` だけでなく `app.<F>` も)。`F` は監査が **`ViewerContextBundle` の定義そのものを syn で読んで**得た 225 フィールド名。呼び先の解決は **A7 と共通の import 正規化**を通す (`std::mem::` / `core::mem::` / `use std::mem::take as pull` などの別名) | `src/**/*.rs` | `mem::take(&mut self.fs_cache)` / `use std::mem::take as pull; pull(&mut app.fs_cache)` | 同名でも bundle に無いフィールド / registry 内部 / 理由付き allowlist (行単位) |
| **A3** | registry モジュール外で `ViewerContextBundle::` の関連関数呼び出しが無いこと | `src/**/*.rs` | `ViewerContextBundle::empty()` | — |
| **A4** | registry モジュールの**公開面は allowlist と完全一致**。比較するのは**正規化した API 指紋**であって生の文字列ではない (下記) | `viewer_context_registry.rs` | allowlist に無い公開項目を足す / 既存関数の戻り値を `Box<ViewerContextBundle>` に変える / 引数の closure 境界を `FnOnce(&mut ViewerContextBundle)` に変える | allowlist を**同じコミットで明示的に更新**する (= レビューの可視点を強制する) |
| **A5** | 識別子 `paused_bundle` / `active_detached_viewer_context` が存在しないこと (ステージ②完了後) | `src/**/*.rs` | 復活 | — |
| **A6** | `#[cfg(test)]` 以外から `viewer_context_registry::test_access::` を呼ばないこと | `src/**/*.rs` | production からの呼び出し | `#[cfg(test)]` 配下 |
| **A7** | registry モジュール外で **`App::viewer_contexts` そのものを動かさない**。禁止するのは (a) `mem::swap` / `mem::replace` / `mem::take` の対象にすること (import 正規化を通す)、(b) **代入の左辺にすること**、(c) **`&mut` で他所へ渡すこと**、(d) `App` を値で destructure して取り出すこと、(e) **値位置でフィールドごと move すること** (`let r = app.viewer_contexts;`)、(f) **戻り値の型が `ViewerContextRegistry` を含む**関数を定義すること (`Option<_>` / `(_, _)` / `Box<_>` などの内側も含む) | `src/**/*.rs` | `self.viewer_contexts = ViewerContextRegistry::new()` / `helper(&mut self.viewer_contexts)` / `let App { viewer_contexts, .. } = app;` / `let r = app.viewer_contexts;` / `fn take_registry() -> Option<ViewerContextRegistry>` | registry 内部 |

**import 正規化 (A2 / A7 共通)**: 監査はファイルごとに `use` 木を読み、
`std::mem::{swap, replace, take}` / `core::mem::...` への**別名を含む全経路**を 1 つの正規名へ畳んでから
呼び出しを照合する。`App` の型推論はしない (syn にはできない) ので、
A2 は**フィールド名だけで過剰検出**し、誤検出は行単位の理由付き allowlist で外す
(`check_ui_glyphs.py` の `// glyph-lint:skip` と同じ運用)。

**A4 の「正規化した API 指紋」に含めるもの** (Codex 第 3 版レビュー BLOCKER 3)。
名前 + 引数型 + 戻り値型だけでは足りない。次を全部含めて初めて漏れが無くなる。

- 項目種別 (fn / struct / enum / type / const / mod / macro) と receiver (`&self` / `&mut self` / なし)
- **正確な可視性** (`pub` / `pub(crate)` / `pub(super)` / `pub(in path)` を区別する)
- **ジェネリックパラメータ、trait 境界、`where` 節**
  — allowlist 済みの `fn with_viewer_context<F>(.., f: F)` が、名前も引数名も戻り値も変えずに
  `F: FnOnce(&mut ViewerContextBundle)` を獲得して生 bundle を漏らせてしまう
- **公開型に対する trait 実装すべてと関連型** — `impl Deref<Target = ViewerContextBundle>` /
  `AsRef` / `AsMut` は impl 項目に `pub` トークンが無いので、可視性だけを見る監査を素通りする
- **公開フィールドと enum variant**、関連定数、関連型
- **`pub use` / 再エクスポート / use rename / 型エイリアス**、`#[macro_export]` マクロ
- **`unsafe` / `extern` などの修飾子**と、`#[cfg]` / `#[cfg_attr]` など**公開面を変え得る属性**
  (cfg 違いで別の実体が露出するのを指紋の差として見せる)

**syn にできないこと**: 到達可能性の推論はできない。指紋が一致していても、
中身が意味的に何を漏らしているかは分からない。監査が保証するのは
「**公開面の形が黙って変わらない**」ことだけで、意味のレビューは人間が行う。

**A7 が必要な理由**: A2 は bundle の 225 フィールド名しか見ないので、
`ViewerContextRegistry` 自体を `mem::take` して別の場所へ持たせる新コードを弾けない。
それをやられると、**bundle は registry の中に居るが registry が追跡されていない producer の
手にある**という、§1.2 の共通形がそのまま再発する。

**allowlist 更新と承認の関係を過大に言わない**: 同じコミットで allowlist を更新させると
**差分としてレビューに必ず現れる**が、レビュアーが承認したことを CI が保証するわけではない。
機械が保証するのは「気付かないうちに公開面が育たない」ところまでである。

**A4 が監査の要**である。「所在を `bool` で答える関数を作るな」のような**意味**の規則は機械化できないが、
「公開面は列挙されたものだけ」なら機械化できて、しかも**新しい所在述語を足そうとした瞬間に
CI が落ちてレビューの可視点を作る**。第 2 版が `extract_mounted_viewer_context` を通してしまったのは、
公開面の増加を誰も止めなかったからである。

**A2 のフィールド名リストが腐らない理由**: 監査が別に持つ定数ではなく、
`ViewerContextBundle` の定義を syn でパースして得る。フィールドを足しても監査は自動で追随する。

**A1 が拾えないもの (明記)**: 型エイリアス経由 (`type B = ViewerContextBundle;`) と、
マクロ展開後にだけ現れる型位置。前者は A1 に「`ViewerContextBundle` への型エイリアス定義を
registry 外で禁止」を含めて塞ぐ。後者は syn がトークン列しか見られないので**塞げない**。
これは監査の既知の穴として書き残し、レビュー時の目視項目にする (無いことにしない)。

**tests.rs の扱い**: 監査は tests.rs も対象にする (例外を作らない)。ただし現状
`active_detached_viewer_context` 系の参照が 225 件あり、うち ~27 件は
`is_none()` / `is_some()` による**所在の表明**なので `residence()` の assert へ素直に写る。
残りのフィールド読みは `#[cfg(test)] pub(in crate::app) mod test_access` を registry に置き、
**読み取りだけ**を露出する。書き込みと生成は test_access にも置かない。
これは監査の抜け穴になり得るので A6 で `#[cfg(test)]` 外からの使用を禁じる。

**走らせる場所**: `.git/hooks/pre-push` に 1 行追加 + `.github/workflows/ci.yml` に
新ジョブ (ubuntu、`cargo run -p viewer-context-audit`)。CLAUDE.md の
「開発中のビルド・テスト選択」に従い、通常の編集ループでは走らせない。
workspace member を 1 つ増やすので、`test-full.ps1` の `--workspace` 対象に入る点は
指示書で明示する (vendor 資産に依存しないことが条件)。

---

## 7. ステージ分割 — 各段でコンパイルが通る状態

BLOCKER 3 の指摘どおり、**保管フィールドを消した瞬間に全消費者が壊れる**ので、
「registry を入れる段」と「保管を切る段」は分けられない。分けられるのは**その前**である。

### ステージ① — registry の状態機械 (production を 1 行も切らない)

- 新モジュール `src/app/viewer_context_registry.rs` を追加。**`ViewerContextBundle` はまだ移さない。**
- 状態機械を **payload に対してジェネリック**に定義する:

  ```rust
  struct ContextTable<P> {
      projection: Projection,
      slots: HashMap<ViewerContextId, SlotOf<P>>,
      main: ViewerContextId,
      window_of: HashMap<ViewerContextId, u64>,
      context_of: HashMap<u64, ViewerContextId>,
      next_serial: u64,
  }
  ```

  production はステージ②-d で `P = Box<ViewerContextBundle>` として実体化する。
  **ジェネリックにすることが、①を production から切り離してコンパイルさせる仕掛け**である。
- `ContextTable` は「bundle を投影へ swap する」実務を知らないが、**知らないままだと
  id の遷移しかテストできず、build transaction の中身を検証できない**。かといって
  **swap を trait のコールバックで受けると借用が重なる**: production の投影は
  `App` の残り全体で、table は `App::viewer_contexts` の中に居るので、
  `&mut self.viewer_contexts` を保持したまま `&mut self` を swapper として渡せない
  (Codex 第 3 版レビュー第 2 巡 P1)。
- **そこで table はコールバックを持たず、「次に何をすべきか」を plan として返す。**
  payload の実移動は呼び出し側 (App 側の薄い実行器) が行うので、**借用が重ならない**。

  ```rust
  /// table が要求する payload 操作。実行器は **transient を高々 1 個**持ち、
  /// op を 1 つ実行するたびに table への借用は終わっている。
  enum TableOp {
      /// 投影から payload を取り出して transient にする。
      TakeProjection,
      /// 空 payload を作って transient にする。
      CreateEmpty,
      /// transient を id の slot へ預ける。
      DepositInto(ViewerContextId),
      /// id の slot から payload を取り出して transient にする。
      WithdrawFrom(ViewerContextId),
      /// transient を投影へ据える。
      InstallProjection,
  }

  impl<P> ContextTable<P> {
      /// 状態を進め、呼び出し側が順に実行すべき op 列を返す。
      /// slots は table が所有し続ける (実行器は別の store を持たない)。
      fn begin_build(&mut self, reserved: ViewerContextId) -> ArrayVec<TableOp, 4>;
      fn commit_build(&mut self) -> ArrayVec<TableOp, 4>;
      // ...
  }
  ```

  例: `begin_build` は
  `[TakeProjection, DepositInto(previous), CreateEmpty, InstallProjection]`、
  `commit_build` は
  `[TakeProjection, DepositInto(reserved), WithdrawFrom(previous), InstallProjection]`。

- **I1 の補足**: payload の所在は「slot / 投影 / **実行中の実行器の transient 1 個**」の 3 択になる。
  transient は `residence()` から見えないが、**op 列の実行中に利用者コードは 1 行も走らない**
  ので観測されない。`f` が走るのは `begin` の op 列を実行し終えて `Projection::Building` が
  確定した後、`commit` の op 列を始める前だけである。したがって
  **transient が panic を跨いで残ることはない** (op の実行自体は payload の move だけで panic しない)。
- ①のテストは **table 自身の slots** に対して op を実行する 20 行程度の実行器を持つ
  (production と同じ store。第 2 稿にあった「テスト用の別 `HashMap`」は撤回)。
  これで「build の途中で投影が確かに新しい空 payload になっている」「`f` の panic で
  直前の payload が投影へ戻る」まで確認できる。
  production の実行器は `swap_viewer_context_bundle` と `ViewerContextBundle::empty` を使う。
- **ステージ②-d は、この実行器を production 側に 1 本書くだけで結線が済む**。
  結線の形が①の時点で確定しているので、②-d の設計判断が残らない。
- ①の間このモジュールは production から呼ばれない。`#[allow(dead_code)]` に
  「ステージ②で結線する」というコメントを付ける。これは無言の死蔵ではなく、
  BLOCKER 3 が禁じた「コンパイルできない分割」を避けるための明示的な段取りである。
- **終了時にコンパイルが通る状態**: production は完全に現状のまま。
  `cargo test -p mimageviewer --lib viewer_context_registry::` が通る。ubuntu の
  `cargo check` も通る (cfg 依存が無い)。
- **払い出したが commit されずに消えた id が `Retired` に分類されること**もここでテストする
  (第 1 巡 BLOCKER 2)。
- **テスト** (`ContextTable<TestPayload>`):
  build commit で slot に入る / **build abort と panic unwind の双方で previous が投影へ戻り
  binding が 1 つも公開されない** / mount 再入 / `Building` 中の mount が Err /
  **build 中に予約した binding が commit まで公開されない** /
  `Retiring` 中の bind が Err / **別の生きた context に bound の窓への `bind` が
  `WindowOwnedBy`** / **既に別の窓に bound の context の `bind` が `ContextOwnedBy`** /
  同じ組み合わせの `bind` が冪等 / **`transfer_window_binding` が両方生きたまま窓を移し、
  `from` 不一致なら Err** / `unbind` 後に同じ context を別の窓へ bind できる /
  各状態の `residence()` /
  **払い出したが commit されずに消えた id が `Retired`**、払い出していない id が `Unknown` /
  promote 後に main が入れ替わる。

### ステージ②-pre — 手書き mount の helper 化 (保管は変えない)

- 既存 `with_active_detached_viewer_context` の対になる
  `with_paused_detached_context(window_id, f)` を足し、**どちらも panic 安全**にする。
- 手書きの `take → swap → 実行 → swap → 戻す` **18 箇所**を helper 呼び出しへ置換
  (付録 B-2)。1 箇所ずつコミットできる。
- **これは規模削減のためだけの段ではない**: 「取り出せなかった (= 今マウント中)」の分岐が
  helper 1 箇所に集まるので、②で `residence()` へ差し替える差分が機械的になる。
- ⚠ **挙動が変わる点が 1 つある**: 手書き 18 箇所は panic 安全でないので、helper 化すると
  panic 時に bundle が drop されなくなる (= worker が cancel されなくなる)。
  これは症状パッチではなく構造的修正だが、**憲法の適用範囲どおり ClaudeCode と Codex の
  双方で「症状パッチではない」ことに合意し、`docs/detached-rework-plan.md` §11 に記録する**。
- **終了時**: 挙動不変 (panic 経路を除く)。既存テスト 207 本が緑。

### ステージ②-a — 終端経路を「所有値の digest」へ (保管は変えない)

- ブックマーク照合 ([app.rs:41497](../../src/app.rs:41497)) と メディア teardown
  ([app.rs:39424](../../src/app.rs:39424)) を、現行の保管フィールドのまま
  「読む → 所有値 → drop → main をマウントして使う」形へ組み替える (§4.4)。
- digest の中身 (`pdf_password_request` / tile-companion / mode-switch) をここで確定させる。
- **終了時**: 挙動不変。コンパイルが通る。

### ステージ②-b — 型の移設 (可視性はまだ緩い)

- `ViewerContextBundle` + `Drop` + `empty` + `set_items_generation` + `swap` + fork destructure を
  registry モジュールへ移す。**フィールドは一時的に `pub(in crate::app)` のまま**にする。
- **終了時**: 挙動不変。コンパイルが通る。この時点ではまだ何も弾けていない。

### ステージ②-c — accessor 移行と監査ツールの導入

- 外部フィールド読み ~26 種を `ContextRef` accessor へ (付録 B-3)。
- `tools/viewer_context_audit` を追加し、**A2 / A3 / A7 だけを有効化する**。
  ⚠ **A1 はここでは有効化できない** (Codex 第 3 版レビュー第 2 巡 BLOCKER)。
  `ActiveDetachedViewerContext::bundle` ([app.rs:2258](../../src/app.rs:2258)) と
  `DetachedImageWindowSnapshot::paused_bundle` ([app.rs:433](../../src/app.rs:433)) が
  まだ registry モジュール外の型位置に `ViewerContextBundle` を出しているので、
  A1 は必ず落ちる。A1 は保管が消える②-d で有効化する。
- **終了時**: 挙動不変。コンパイルが通る。

### ステージ②-d — 保管・binding・transaction の一括切替 (1 コミット)

**ここだけは分けられない。** コンパイラが作業リストを列挙してくれるので手探りにはならない。

- `App::active_detached_viewer_context` ([app.rs:10095](../../src/app.rs:10095)) と
  `DetachedImageWindowSnapshot::paused_bundle` ([app.rs:433](../../src/app.rs:433)) を削除し、
  `App::viewer_contexts: ViewerContextRegistry` へ統合。
- 生プリミティブ 4 種を 5 transaction へ (§4)。
- window binding を、window_id を確定している箇所へ入れる
  (build 経路は `reserve_window_binding_for_build`、activation 経路は `bind_window`)。
- **A1 / A5 をここで有効化する** (保管フィールドが消えて初めて通る)。
- **§6.5 の暫定回避策を撤去する** (これが②-d の完了条件の一部):
  - `right_drag_viewer_identity_for_window_id` の
    `native_video_parked_live_input_window_id == Some(window_id)` 分岐
    ([app.rs:1249](../../src/app.rs:1249)〜[:1264](../../src/app.rs:1264)) → `residence()` の match
  - keep-alive backstop の 3 分岐 ([ui_fullscreen.rs:13126](../../src/ui_fullscreen.rs:13126)) → 同上。
    「別の detached sentinel を足すな」というコード中のコメントに対する正解がこれである
  - `should_promote_active_detached_video_for_main_context_change` の
    `active_detached_viewer_context.is_none()` ([app.rs:42720](../../src/app.rs:42720)) → 同上
- **終了時**: コンパイルが通り、既存 detached テスト 207 本が緑。**挙動不変**
  (ステージ②-pre の panic 経路を除く)。
- **規模** (②-pre / ②-a / ②-c 後に残る量、2026-08-23 実測): 非テストの
  `swap_viewer_context_bundle` 呼び出し 52、`paused_bundle` 参照 104、
  `active_detached_viewer_context` 参照 158、tests.rs 側 294 行。install 17 箇所 / take 13 箇所。

### ステージ②-e — フィールドを完全 private にし、allowlist ゲートを入れる

- `ViewerContextBundle` のフィールドを registry モジュール private へ落とす (§6.1)。
- A4 / A6 を有効化する (A1 / A5 は②-d で有効化済み)。
- **終了時**: 挙動不変。ここで初めて「モジュール外では書けない」が成立する。

### ステージ③ — 巡回の単純化

- 手書き巡回 4 箇所 (§4.6) を `for_each_viewer_context` / `any_viewer_context` へ。
- bundle 版 / App 版で二重定義されている述語 (`contains_video` ほか) を `ContextRef` 版 1 本へ。
- **終了時**: 挙動不変。個別にコンパイルが通るので複数コミットに割れる。

### ステージ④ — 非同期要求の identity 変換

- `metadata_import_refresh::ContextSlot`
  ([metadata_import_refresh.rs:61](../../src/app/metadata_import_refresh.rs:61)) を
  `ViewerContextId` へ。**`PausedDetached { index: usize }` の Vec index 依存が消える**。
- ⚠ `ContextSlot::Main` は **要求を組み立てる時点の `registry.main()` を焼き付ける**。
  結果が返ってきた時点で「main」を解決し直すと、その間に promote が走った場合に
  別の context へ適用してしまう (§3.1 の「main は binding」の帰結。Codex 第 3 版レビュー B)。
- 適用側 ([app.rs:28321](../../src/app.rs:28321)〜[:28405](../../src/app.rs:28405)) の
  `take()` が `None` のときの silent skip を `residence()` の match へ。
- `items_generation` は staleness stamp として据え置く (BLOCKER 3)。
- **挙動が変わる**: 今日は index ずれ / マウント中で捨てていた結果が正しく届く。
  → 実機 smoke 対象。テスト ([tests.rs:647](../../src/app/tests.rs:647) /
  [:845](../../src/app/tests.rs:845) 付近) を id ベースへ。

### R2f (別件)

純粋 reducer + 合法遷移制約 + 散在 pending / flag の typed 集約 (R2b 残件)。
§1.4 の規則 B (`RightDragOwner` に producer を含める) もここ。

---

## 8. テスト計画

| 段 | 追加するテスト |
| --- | --- |
| ① | §7 ①のリスト (状態機械の単体テスト、`ContextTable<TestPayload>`) |
| ②-pre | helper 化した 18 箇所のうち、parked-live poll 中に走り得る経路で「マウント中の context がスキップされない」ことを 1 本 |
| ②-a | 終端 digest が `pdf_password_request` / tile-companion / mode-switch の判断を落としていないこと |
| ②-d | build の abort / panic → 直前の context が復帰し binding が 1 つも公開されない / bookmark メディアの open 失敗 ([startup_ops.rs:636](../../src/app/startup_ops.rs:636)) が abort として通る / live-media fork が `transfer_window_binding` で窓を移し両 context が生きている / park の `unbind` を飛ばすと `ContextOwnedBy` で弾かれる / mounted と at-rest を跨ぐ `any_viewer_context` / fork の前置き判定と後置き判定の一致 / promote 後に main が入れ替わり旧 main が窓へ bind される |
| ③ | 巡回が「マウント中の 1 個 + slot の全部」を過不足なく回ること |
| ④ | 窓の並び替え / 削除の後でも非同期結果が正しい context へ届くこと (今の index 依存では落ちるテスト) |

既存 207 本は**削除も弱体化もしない** (憲法 8)。`is_none()` / `is_some()` による所在の表明は
`residence()` の assert へ写すが、表明の内容は変えない。

**実機 smoke**: 挙動が変わるのは②-pre の panic 経路と④だけなので、②-e までは
smoke-matrix の通常セットで足りる。④は「メタ情報 import 中に別ウィンドウを開閉する」ケースを
追加する。②-d は挙動不変だが範囲が広いので、folder-nav reopen (window_id 再利用) と
live-park の往復を smoke に含める。

---

## 9. 憲法チェック

| 条 | 判定 |
| --- | --- |
| 1 rect 一致捕捉に条件を足さない | 触れない |
| 2 geometry 由来の host_lost を recreate トリガにしない | 触れない |
| 3 App に新しい detached 用 bool / Option を足さない | **足さない。逆に 2 個の保管 `Option` を 1 個の typed owner へ畳む。** 3 が指す「R2 で導入する state owner」がこれ |
| 4 placement の新しい保存先を作らない | registry は placement を持たない。placement は runtime 所有のまま (レーン A-0 の担当) |
| 5 時間窓で競合を吸収しない | `residence()` は事実。debounce / grace / settle を 1 つも導入しない。I7 の assert は「定義した継ぎ目での状態検査」であって時間窓ではない |
| 6 実機で新症状が出てもその場でヒューリスティックを入れない | 設計段階では該当なし。②で挙動同値性が崩れたら報告して止まる |
| 7 指示書に無いファイルを「ついでに」直さない | 各段の触ってよい範囲は実装指示書 (`docs/detached-rework-stage-r2e-1.md` 以降) で列挙する |
| 8 既存テストを削除・弱体化しない | §8 のとおり |

---

## 10. 未決事項 (Codex への質問)

1. **§3.1 の `Main` 変種廃止**に同意するか。promote 経路 ([app.rs:42734](../../src/app.rs:42734)) が
   「main の bundle が detached になる」ことを根拠にしているが、他に `Main` を identity として
   扱っている経路が残っていないか。
2. **§4.2 の serial 前倒し** (40560 → `begin` の先頭) は挙動不変か。40546〜40559 に
   `items_generation` を読む処理が無いことを目視で確認したが、間接呼び出しで読む経路はないか。
3. **§4.3 の fork 前置き判定**は後置き判定と同値か。
   `viewer_context_bundle_contains_video` ([app.rs:39104](../../src/app.rs:39104)) が見る材料が
   分割前の投影にすべて揃っている、という読みで正しいか。
4. **§6.3 の A4 (公開面 allowlist)** は運用に耐えるか。allowlist の粒度 (名前だけ / 名前 + 型) は
   どちらが良いか。
5. **§6.3 の test_access** は監査の抜け穴として許容範囲か。tests.rs の 294 行を
   accessor へ全面移行する方が良いか。
6. **§7 のステージ②を 1 コミットにする以外の分割**で、各段がコンパイルできるものはあるか。
   ②-pre で 18 箇所を helper 化してもなお大きい。
7. **§1.4 の切り分け** (規則 B = producer を R2f、規則 C = placement をレーン A-0) に同意するか。
   R2e に持ち込むべきものが混ざっていないか。
8. **§3.4 の I7** の assert を、`eframe::App::update` ([app.rs:66549](../../src/app.rs:66549)) の
   薄い wrapper (内側の root-pass メソッドから戻った直後) に置く案でよいか。
   `update` 本体には early return が複数あるので、本体末尾に足すだけでは成立しない
   (Codex 第 3 版レビュー D)。
9. **§3.5 の binding 操作 3 種 (`bind` / `unbind` / `transfer`) で全経路を覆えているか。**
   live-media fork ([app.rs:41868](../../src/app.rs:41868)) が `transfer`、
   always-new の park→再割り当て ([app.rs:42579](../../src/app.rs:42579) →
   [:42794](../../src/app.rs:42794)) が `unbind` → `bind`、descriptor resume が `bind`、という
   割り当てで漏れがないか。`transfer` を必要とする経路が他にもあれば挙げてほしい。
10. **§4.2 で窓側の副作用 (40561 / 40573 / 40593) を transaction の外に置いたまま**にする
   判断に同意するか。`Abort` 経路 ([startup_ops.rs:636](../../src/app/startup_ops.rs:636)) は
   今も自分で後始末しているので現状維持でよい、という読みで正しいか。
   将来これを commit フェーズへ移すなら挙動不変か。

---

## 付録 A: 第 1 版 / 第 2 版の BLOCKER (保存)

第 3 版の各節が対応しているので、参照用に残す。

### 第 1 版 (BLOCKER 5 件、単一スカラーで「今の所有者」を表す案)

- `Vacant`: `take_current_viewer_context_bundle` は App に空 bundle を残す
- `Building`: 新しい detached context は App のフィールド上で組み立てられる
- 押しのけられた bundle がスタックローカルにあり `self` から到達できない
- owner に window_id は使えない (フォルダ再オープンで再利用される)
- 巡回・消費者の一括移行が 1 ステージに収まらない

### 第 2 版 (BLOCKER 4 件、phase + slot map + 復元スタック案)

1. **所有の transaction が無い。** `extract_mounted_viewer_context` が生の bundle を返す時点で
   不変条件が破れる。`begin_build(reserved_id)` → `commit_and_restore_previous` が要る。
   加えて (a) main をマウントしたまま 2 個目を作る atomic な fork / insert_unmounted、
   (b) 終端削除は drop 前に中身を読む必要がある (bookmark 照合 / media teardown)。
   → **第 3 版 §3.7 / §4.2 / §4.3 / §4.4**
2. **window_id → ViewerContextId の対応表が無い。** アクティブ化は window_id から始まるので
   対応表が無いと slot を選べない。window_id からは推論できない。
   → **第 3 版 §3.5**
3. **R2e-1 / R2e-2 の切り方はコンパイルできない。** 消費側が保管フィールドを直接触っているので、
   保管を切ると同時に移行するしかない。完了確認は grep ではなくコンパイラ + AST allowlist。
   → **第 3 版 §6 / §7**
4. **「Building には identity が無い」は誤り。** 正しくは「予約済みだが未 commit」。
   → **第 3 版 §3.3 / §4.2**

### §6.5 実務で 3 回踏んだ同型の失敗

→ **第 3 版 §1.1 / §1.4** (診断は 1 つ、修理は R2e / R2f / レーン A-0 の 3 箇所)。
第 3 版で撤去する暫定回避策は §7 のステージ②に列挙した。

---

## 付録 B: 事実 (2026-08-23 の master でコード確認)

### B-1 保管と所有権プリミティブ

| | 場所 |
| --- | --- |
| `ViewerContextBundle` (225 フィールド、`#[cfg(windows)]`) | [app.rs:2465](../../src/app.rs:2465)〜[:2740](../../src/app.rs:2740) |
| `Drop for ViewerContextBundle` (worker cancel) | [app.rs:2743](../../src/app.rs:2743) |
| `ViewerContextBundle::empty()` | [app.rs:2795](../../src/app.rs:2795) |
| `swap_viewer_context_bundle` (destructure は :16189) | [app.rs:16175](../../src/app.rs:16175) |
| `take_current_viewer_context_bundle` (空 bundle を残す) | [app.rs:16666](../../src/app.rs:16666) |
| `with_active_detached_viewer_context` (唯一の panic 安全 mount) | [app.rs:16698](../../src/app.rs:16698) |
| `split_current_context_preserving_main_grid` | [app.rs:41922](../../src/app.rs:41922) |
| `split_materialized_physical_context_for_independent_still_open` | [app.rs:42421](../../src/app.rs:42421) |
| `App::active_detached_viewer_context` | [app.rs:10095](../../src/app.rs:10095) |
| `ActiveDetachedViewerContext` (bundle 1 フィールドだけ) | [app.rs:2258](../../src/app.rs:2258) |
| `DetachedImageWindowSnapshot` / `paused_bundle` | [app.rs:415](../../src/app.rs:415) / [:433](../../src/app.rs:433) |
| `DetachedWindowRuntime` (window_id / state / hwnd / placement / intent) | [detached_window_manager.rs:460](../../src/app/detached_window_manager.rs:460) |
| `ViewerSession` (per-context 分離の先例) | [viewer_session.rs:9](../../src/app/viewer_session.rs:9) |

件数 (非テスト): `swap_viewer_context_bundle` 呼び出し **52**、`paused_bundle` 参照 **104**、
`active_detached_viewer_context` 参照 **158**、`with_active_detached_viewer_context` **12**。
tests.rs 側は `active_detached_viewer_context` **225 行** / `paused_bundle` **69 行** (合計 294 行)。
非テストの `ViewerContextBundle` 生成・destructure は **3 箇所だけ** (:16189 / :16667 / :41939)。

### B-2 手書き mount (ステージ②-pre の対象、18 箇所)

active holder 経由 9: [19807](../../src/app.rs:19807) / [27884](../../src/app.rs:27884) /
[27931](../../src/app.rs:27931) / [28150](../../src/app.rs:28150) / [28223](../../src/app.rs:28223) /
[28373](../../src/app.rs:28373) / [28993](../../src/app.rs:28993) / [30421](../../src/app.rs:30421) /
[54321](../../src/app.rs:54321)

paused_bundle 経由 9: [19843](../../src/app.rs:19843) / [27891](../../src/app.rs:27891) /
[27950](../../src/app.rs:27950) / [28169](../../src/app.rs:28169) / [28231](../../src/app.rs:28231) /
[28397](../../src/app.rs:28397) / [29016](../../src/app.rs:29016) / [30429](../../src/app.rs:30429) /
[54328](../../src/app.rs:54328)

うち **6 箇所は `else { continue }` で「取り出せなかった context」を無言で飛ばす**。
到達可能性は経路ごとに異なるが、`None` が「無い」と「今は別の場所にある」を区別していない点は同じ。
`consume_deferred_vst3_media_open_in_parked_contexts` ([app.rs:19819](../../src/app.rs:19819)) は
その区別のために `native_video_parked_live_input_window_id` を自分で立てて回している。

### B-3 registry モジュール外から読まれている bundle フィールド (accessor 化の対象)

出現回数の多い順: `fullscreen_idx` (17) / `items` (15) / `fs_cache` (12) / `viewer_session` (8) /
`tag_prewarm_pending` (4) / `pdf_password_request` (4) / `fs_pending` (4) / `video_audio_mode` (3) /
`selected` (3) / `items_generation` (3) / `fs_open_intent_from_grid` (3) / `current_folder` (3) /
`pending_return_to_parent` (2) / `pending_auto_fs_open` (2) / `pdf_prefetch_grace_until` (2) /
`fs_lanczos_cache` (2) / `visible_indices` / `navigation_scope` / `normalize_ui_states` /
`normalize_auto_scan_suppressed` / `vst3_deferred_media_open` / `native_video_in_window_active` /
`music_bookmarks` / `music_bookmarks_loaded_for` / `top_level_grid_view` / `final_ai_pending`。

`split_materialized_physical_context_for_independent_still_open` ([app.rs:42425](../../src/app.rs:42425)〜)
が**書く** ~40 フィールドは accessor にしない。関数ごと `ForkPolicy::MaterializedStillOpen` として
registry モジュールへ移す。

### B-4 `&ViewerContextBundle` を引数に取るヘルパー (非テスト 12 本)

網羅ではない。`teardown_paused_media_bundles` ([app.rs:39465](../../src/app.rs:39465)、
`Vec<Box<..>>` を取る) と `reconcile_closed_bookmark_detached_context`
([app.rs:41262](../../src/app.rs:41262)) は終端消費者として §4.4 で別に扱う。

[9213](../../src/app.rs:9213) / [9255](../../src/app.rs:9255) / [9287](../../src/app.rs:9287) /
[9331](../../src/app.rs:9331) / [9368](../../src/app.rs:9368) / [9403](../../src/app.rs:9403) /
[9444](../../src/app.rs:9444) / [16175](../../src/app.rs:16175) / [39104](../../src/app.rs:39104) /
[39125](../../src/app.rs:39125) / [39137](../../src/app.rs:39137) / [39312](../../src/app.rs:39312)

`viewer_context_bundle_is_music_consumer` は free fn ([9255](../../src/app.rs:9255)) と
`impl App` の関連関数 ([39312](../../src/app.rs:39312)) で**二重に定義**されている。
`contains_video` は bundle 版 ([39104](../../src/app.rs:39104)) と App 版
([39197](../../src/app.rs:39197)) で本文がほぼ同一。どちらも `ContextRef` 版 1 本に畳める。

### B-5 identity の払い出し

| | 場所 |
| --- | --- |
| `DETACHED_VIEWER_CONTEXT_GENERATION_BASE = 1<<63` / `STRIDE = 1<<32` | [app.rs:394](../../src/app.rs:394) / [:396](../../src/app.rs:396) |
| `allocate_detached_viewer_context_generation` → `(serial, items_generation)` | [app.rs:38088](../../src/app.rs:38088) |
| `assign_next_detached_viewer_context_generation` (投影へ焼く) | [app.rs:38100](../../src/app.rs:38100) |
| `allocate_detached_viewer_window_id` | [app.rs:38107](../../src/app.rs:38107) |
| `ensure_detached_viewer_window_id` (**folder-nav reopen で window_id を意図的に再利用**) | [app.rs:38277](../../src/app.rs:38277)、再利用の条件は [:38304](../../src/app.rs:38304)〜 |
| `bump_items_generation` (context 内で +1) | [app.rs:24873](../../src/app.rs:24873) |

### B-6 非同期の要求 identity (ステージ④の対象)

| | 場所 |
| --- | --- |
| `ContextSlot { Main, ActiveDetached(Option<u64>), PausedDetached { index, window_id } }` | [metadata_import_refresh.rs:61](../../src/app/metadata_import_refresh.rs:61) |
| 要求の組み立て (main / active / parked を手書きで 3 経路) | [app.rs:28137](../../src/app.rs:28137)〜[:28190](../../src/app.rs:28190) |
| 結果の適用 (`take()` が `None` なら無言で捨てる) | [app.rs:28321](../../src/app.rs:28321)〜[:28405](../../src/app.rs:28405) |

`ActiveDetached(Option<u64>)` が `Option` なのは、「bundle を holder へ入れる時点で window_id が
未確定な経路がある」ことの既存の証拠である。§3.5 で `window_of` を**部分写像**にしてよい
(context が一時的にどの窓にも結ばれない状態を許す) 根拠がこれである。
`bind_window` 自体は `u64` を取る。
