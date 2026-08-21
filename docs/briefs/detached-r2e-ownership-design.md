# R2e 設計 — viewer context の所有を型にする (第 2 版)

対象: detached リワーク R2 の残件のうち「window ごとの所有を型にする」部分。
正本プラン: `docs/detached-rework-plan.md` (§2 憲法 / §4 R2 / §9.1 現況 / §11)。
依存するバグ: `docs/next-release-backlog.md` §1.99 / §1.100。

第 1 版は Codex レビューで **BLOCKER 5 件** を受けて破棄した。指摘はすべて実コードで裏が取れた。
本書はその指摘を取り込んだ第 2 版で、まだ実装指示書ではない。

---

## 1. 事実 (コードで確認済み)

`ViewerContextBundle` (`src/app.rs:2073`〜`2348`、フィールド約 220) は viewer context の状態を
まとめた入れ物。ただし**全部ではない**: ジェスチャ状態 (`src/app.rs:10010`)、音楽解析 /
タイムライン (`src/app.rs:11684`) は意図的に App-global のまま。

### 1.1 マウントされていない bundle の置き場

| 置き場所 | 何の bundle か | 宣言 |
| --- | --- | --- |
| `App::active_detached_viewer_context` | アクティブな独立 detached 窓の context (最大 1) | `src/app.rs:9602` |
| `App::detached_image_windows[i].paused_bundle` | park 済み各窓の context | `src/app.rs:402` |
| **スタックローカル** | mount 中に押しのけられた bundle | `src/app.rs:16103` ほか |

3 つ目が第 1 版で見落としていたもの。`with_active_detached_viewer_context` の実行中、
**押しのけられた main bundle は関数のローカル変数 `active.bundle` にあり `self` から到達できない**。
`active_detached_viewer_context` フィールド自体は `None` になっている。

### 1.2 所有権を動かすプリミティブは swap だけではない

- `swap_viewer_context_bundle` (`src/app.rs:15576`) — 非テスト呼び出し 52 か所
- `take_current_viewer_context_bundle` (`src/app.rs:16067`) — **App に空 bundle を残す**
- `split_current_context_preserving_main_grid` (`src/app.rs:40949`) — main と viewer に分割
- フィールドへの直接代入 (`src/app.rs:41563` / `40919`)

したがって「raw swap 呼び出しが grep 0 件」は所有の一元化を証明しない。

### 1.3 App が「有効な context をマウントしていない」時間がある

- **Vacant**: `pause_current_active_detached_viewer_context` は detached を mount
  (`src/app.rs:37630`) → `take_current_viewer_context_bundle` (`src/app.rs:37671`) で抜き取り、
  main を戻す (`src/app.rs:37678`) までの間、App のビューアフィールドは**空 bundle**。
- **Building**: `start_active_detached_book_context_with_start` は main を抜き
  (`src/app.rs:39778`)、**App のフィールド上で新しい detached context を組み立て**、
  最後に捕獲する (`src/app.rs:39885`)。この間、マウント中の context には**まだ identity が無い**。

### 1.4 context identity ≠ window identity

`active_detached_viewer_context` へ bundle を入れる時点では window_id が未確定な経路がある
(`src/app.rs:41563` → `open_fullscreen` が `41569` で identity を割り当てる)。
既存の `metadata_import_refresh::ContextSlot` も `ActiveDetached(Option<u64>)` を許している
(`src/app/metadata_import_refresh.rs:61`)。さらに **window_id はフォルダ再オープンで意図的に再利用**
される (`src/app.rs:37568`) ので、window_id は非同期完了の stamp にならない。

一方、context には既に serial がある: `allocate_detached_viewer_context_generation`
(`src/app.rs:37368`) が `context_serial` と一意な `items_generation` を払い出している。

### 1.5 既存の「所有者っぽい型」は同義ではない

`FolderOpenScanPurpose` (走査目的) / `ArchiveConvertCompletionPolicy` (完了後の振る舞い) /
`BookmarkOpenRequestOwner` (要求 identity と復帰先) / `OpenRequestOwner` (nav vs bookmark の裁定) は
**別々の次元**を表している。context 所有者の型はこれらを置き換えるものではなく、直交して足すもの。

---

## 2. 第 2 版の型

`src/app/viewer_context_registry.rs` (新設) に置く。`DetachedWindowManager` には置かない
(あちらの責務は detached window runtime / HWND / activation で、context identity は別境界)。

```rust
/// viewer context の identity。OS ウィンドウとは独立。
/// `Detached` の値は `allocate_detached_viewer_context_generation` が払い出す context serial。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ViewerContextId {
    Main,
    Detached(u64),
}

/// App のビューア投影が今どうなっているか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MountPhase {
    /// 既知の context がマウントされている。
    Mounted(ViewerContextId),
    /// 明示的な抜き取りで空になっている (take 後、戻すまで)。
    Vacant,
    /// App のフィールド上で新しい context を組み立てている最中。identity は commit 時に付く。
    Building,
}

pub(crate) struct ViewerContextRegistry {
    phase: MountPhase,
    /// マウントされていない bundle。**押しのけられた bundle もここに入る** (スタックローカルにしない)。
    slots: HashMap<ViewerContextId, Box<ViewerContextBundle>>,
    /// mount の LIFO 復元スタック。panic 時もここから戻す。
    stack: Vec<ViewerContextId>,
}
```

第 1 版との差:

1. スカラー 1 個 → **phase (3 状態) + slots + stack**。Vacant / Building / ネストを表現できる。
2. 押しのけた bundle を registry が持つので、**cross-owner ネストが成立する**
   (detached をマウント中に `Main` を引ける)。panic で main の bundle が drop されて
   worker が cancel される事故 (`ViewerContextBundle::drop` `src/app.rs:2351`) も起きない。
3. owner は window_id ではなく **context serial**。window_id 再利用の影響を受けない。

## 3. API

```rust
/// id の context をマウントして f を実行し、必ず元へ戻す (panic 時も)。
/// すでにマウント済みなら swap せずそのまま実行する。
/// slot に無ければ None を返し f を呼ばない。
fn with_viewer_context<R>(&mut self, id: ViewerContextId, f: impl FnOnce(&mut Self) -> R) -> Option<R>;

/// 現在 bundle を持っている全 id (マウント中のものを含む)。
fn viewer_context_ids(&self) -> Vec<ViewerContextId>;

/// 上記を順にマウントして呼ぶ。「main + active + 全 parked」の手書きループを置き換える。
fn for_each_viewer_context(&mut self, f: impl FnMut(&mut Self, ViewerContextId));

/// マウント中の context を抜き取り、phase を Vacant にする。
fn extract_mounted_viewer_context(&mut self) -> ViewerContextBundle;

/// Vacant / Building の App 投影を、指定 id の context として確定する。
fn commit_mounted_viewer_context(&mut self, id: ViewerContextId);
```

`f` の中で自分自身の owner を削除する経路 (terminal close、`src/app.rs:40523`) は、
**現行どおり「マウント中に判定 → アンマウント後に slot を消す」**を規約として明文化する。
`with_viewer_context` の中から slot を消してはならない (テストで固定する)。

---

## 4. ステージ分割 (Codex の助言を採用)

第 1 版の「1 ステージで 52 か所 + プリミティブ + 非同期消費者」は広すぎた。

| ステージ | 内容 | 挙動 |
| --- | --- | --- |
| **R2e-1** | registry / phase / stack / unwind 復元 の導入と、**プリミティブ 4 種** (swap / take / split / 直接代入) を registry 経由へ。Vacant / Building / ネスト / panic 復元のテスト | 不変 |
| **R2e-2** | 巡回・消費側 (約 52 か所) を `with_viewer_context` / `for_each_viewer_context` へ移す | 不変 |
| **R2e-3** | 非同期消費者を移す。`metadata_import_refresh::ContextSlot` → `ViewerContextId` を最初に | 不変 |
| **R2f** | 純粋 reducer + 合法遷移制約 + 散在 pending / flag の typed 集約 (R2b 残件) | 変わる |

---

## 5. 2 つのバグが必要とするもの (再評価)

Codex レビューで、**どちらも registry の完成を待つ必要が無い**ことが分かった。

### §1.99 (複数ウィンドウで RAR を開くとメイングリッドも書庫一覧になる)

必要なのは既存 context の所有者ではなく、**「要求 X のために新しい detached context を作る」という
型付きの宛先意図**。完了時点では detached 窓も bundle もまだ存在しないので、
`with_viewer_context(Detached(..))` は解決できない (= 第 1 版の誤り)。

正しい形は既にある: ブックマーク経路が `ArchiveConvertCompletionPolicy::Bookmark(owner)` を持ち、
変換完了後に `open_converted_bookmark_in_detached_context` (`src/app.rs:39993`) が
**新しい detached context を作って**着地する。グリッドからの ConvertibleArchive 開きにも
同型の completion policy と着地関数を用意すればよい。**registry 非依存。**

### §1.100 (非アクティブな別ウィンドウでジェスチャが効かない)

**確定した事実 (第 1 版になかった)**: 非アクティブな静止画窓は `show_viewport_deferred` で描かれ、
コールバックは描画 / focus / close / placement しか報告しない (`src/ui_fullscreen.rs:11622`,
`:11175`)。アクティブ化を担う OS watcher は **`VK_LBUTTON` しかサンプルしない**
(`src/app/detached_window_manager.rs:326`)。したがって**非アクティブな静止画窓上の右ドラッグは、
アクティブ化もせずジェスチャ状態機械にも届かない完全な no-op**。バックログの現行記述
「最初の右ドラッグが失われる」は `ParkedLive` の動画窓にだけ当てはまる。

**利用者決定 (2026-08-21)**: 「ジェスチャを認識し、ジェスチャされた場合は自動でアクティブ化した
上で、ジェスチャコマンドを実行する」。両方の窓が見えている以上、ジェスチャが受理されるのが期待動作。

必要なもの:

1. 非アクティブ窓のポインタ列を、その窓の identity 付きで root pass へ届ける
   (deferred の event に足す。`DeferredDetachedImageWindowEvent` は既に window id を持つ)
2. ジェスチャ状態を **window 所有**にする (現在は `App` に 1 個 `src/app.rs:10010`、
   識別は `RightDragContext` だけなので複数の画像窓を区別できない)
3. 成立時に「アクティブ化 → コマンド実行」を**型付きの順序**として表す (guard や遅延ではなく)

コマンドはアクティブ化後に実行するので、**マウントされていない context へ適用する必要は無い**
= registry 非依存。2 は R2b 残件の「散在 state の typed 集約」そのものなので、
**`DetachedWindowRuntime` へ載せれば憲法 3 に抵触しない**。

---

## 6. 第 2 版に残った BLOCKER (第 3 版で解く)

Codex 第 2 版レビュー (2026-08-21) の結果。**§4 のステージ分割ごと作り直す必要がある。**

1. **所有の transaction が無い。** `extract_mounted_viewer_context` が生の bundle を返す時点で、
   「マウントされていない bundle は必ず registry が持つ」という §2 の不変条件が破れる。
   `start_active_detached_book_context_with_start` は
   「main を抜く → App 上で detached を組む → detached を抜く → main を恒久復帰」
   ([app.rs:39778](../../src/app.rs:39778) → [39832](../../src/app.rs:39832) →
   [39885](../../src/app.rs:39885) → [39886](../../src/app.rs:39886)) という形で、
   `with_viewer_context` (常に元へ戻す契約) では表せない。
   **`begin_build(reserved_id)` → `commit_and_restore_previous` の transaction が要る**。
   `Vacant` / `Building` は transaction の内部状態にし、生 bundle を返す API にしない。
   さらに 2 つの正当な遷移に API が無い:
   - main をマウントしたまま 2 個目の bundle を作る `split_current_context_preserving_main_grid`
     ([app.rs:40949](../../src/app.rs:40949)) → **atomic な fork / insert_unmounted** が要る
   - 終端削除は drop 前に中身を読む必要がある (bookmark 照合 [app.rs:40523](../../src/app.rs:40523)、
     media teardown [app.rs:38725](../../src/app.rs:38725))。「アンマウント後に slot を消す」では足りない
2. **window_id → ViewerContextId の対応表が設計に無い。** 今は active holder も parked snapshot も
   bundle しか持たず ([app.rs:1866](../../src/app.rs:1866) / [app.rs:402](../../src/app.rs:402))、
   `ActiveDetachedSession` と `DetachedWindowRuntime` は window_id しか持たない。
   アクティブ化は window_id から始まる ([app.rs:40065](../../src/app.rs:40065)) ので、
   対応表が無いと slot を選べない。**window_id からは推論できない** (再オープンで再利用されるため、
   [app.rs:37568](../../src/app.rs:37568))。どこに置くかを設計で決めること。
3. **R2e-1 / R2e-2 の切り方はコンパイルできない。** 消費側が `active.bundle` や各
   `paused_bundle` を直接触っている ([app.rs:27609](../../src/app.rs:27609) /
   [app.rs:40523](../../src/app.rs:40523) / [app.rs:38725](../../src/app.rs:38725)) ので、
   R2e-1 で保管フィールドを消すと同時に移行するしかない。
   **正しい分割**: ①registry の状態機械と build transaction を production の保管を切らずに定義・テスト →
   ②保管・active/parked の owner 参照・生プリミティブ・終端 teardown・直接消費者を**一括で**切替 →
   ③残った手書き巡回の単純化 → ④非同期の要求 identity 変換 (`items_generation` は残す)。
   **完了確認は grep ではなくコンパイラ + AST allowlist**: 生成 / exhaustive destructure /
   生の抽出 / 所有を動かす `mem::swap` を registry モジュール private にし、
   `syn` ベースの CI 監査でモジュール外の `ViewerContextBundle` 生成・返却・保管を弾く。
4. **§1.3 の「Building には identity が無い」は誤り。** context serial は load 開始前に払い出され
   ([app.rs:39793](../../src/app.rs:39793))、window_id もその直後に確定する
   ([app.rs:39798](../../src/app.rs:39798))。正しくは **「identity は予約済みだが未 commit」**。
   build transaction は予約済み `ViewerContextId` を保持すること。

その他の訂正: 非テストの `swap_viewer_context_bundle` 呼び出しは 52 ではなく **50**。
§1.1 の保管一覧は、終端 close が生 bundle を返す ([app.rs:37853](../../src/app.rs:37853))、
media teardown が `Vec<Box<ViewerContextBundle>>` を持つ ([app.rs:38739](../../src/app.rs:38739))
といった一時所有を含めると網羅ではない。

**Codex の推奨作業順**: §1.99 → §1.100 → R2e (再設計後)。前 2 件は registry 非依存であることに
Codex も同意した。ただし両方とも §11 のリワーク外合意プロセスで記録すること。

---

## 6.5 実務で 3 回踏んだ同型の失敗 (2026-08-21、第 3 版の設計材料)

§1.100 の実装中に、**同じ構造の失敗を 3 回**踏んだ。いずれも
「`None` が『存在しない』ではなく『今は別の場所にある』を意味していた」ケースで、
**R2e が解こうとしている問題そのもの**である。第 3 版はこの 3 件を説明できる形にすること。

| # | どこ | `None` / 単一値の誤読 | 症状 |
| --- | --- | --- | --- |
| 1 | `active_detached_viewer_context.is_none()` (§11 の keep-alive backstop) | 「所有者が居ない」ではなく「今マウント中」 | 別 context の題と texture で描画 |
| 2 | `right_drag_pointer_pos(owner)` (`src/app/gamepad_input.rs:1161`) | 所有者でしか絞らず、**どの producer が持っているか**を区別しない | egui 側が native 側の開始したドラッグを `ButtonStateLost` でキャンセル |
| 3 | `snapshot.paused_bundle.is_none()` (`src/app.rs:707` 付近) | 「bundle が無い」ではなく「parked-live poll がマウント中」 | 成立したコマンドが `viewer_identity_unavailable` で捨てられる |

**3 件に共通する要求**: ある識別子 (window / context / owner) を渡したら、
**その bundle が今どこにあるか (parked / mounted / 別 producer が保持中) を型で返す**こと。
第 2 版の `MountPhase` は App の投影側しか表しておらず、
**「この window の bundle は今マウント中か」を問い合わせる API が無い**。
第 3 版ではこれを一級の問い合わせにする (2 の producer 所有まで含めるかは設計判断)。

暫定の回避策として、3 は `native_video_parked_live_input_window_id`
(`src/app.rs:1249`) を「その窓がマウント中である事実」として読んでいる。
**これは parked-live 経路にしか無い facts なので、一般解にはならない。**

---

## 7. 第 2 版で聞いたこと (回答済み)

1. §2 の phase / slots / stack で、§1.3 と §1.4 の状態を**過不足なく**表現できているか。
   まだ表現できない現行の正当な状態が残っていないか。
2. `commit_mounted_viewer_context` の契約 (Building → Mounted) は、`src/app.rs:39778`〜`39885` の
   実際の組み立て手順に**そのまま被せられるか**。途中で main を mount し直す経路は無いか。
3. §4 のステージ分割の粒度と順序は妥当か。R2e-1 の完了条件を「プリミティブ 4 種が registry 経由」
   とした場合、**それを機械的に確認する方法**は何が適切か (grep では不十分と指摘済み)。
4. §5 の「2 つのバグは registry 非依存」という再評価に同意するか。
   同意する場合、§1.99 / §1.100 を R2e-1 より**先に**着手してよいか
   (どちらも新しい state は `DetachedWindowRuntime` / 既存 completion policy に載せる)。
5. §1.100 の 3 要件で、`show_viewport_deferred` のコールバックからポインタ列を運ぶことに
   構造的な障害はあるか (deferred は root pass と別 pass で走る)。
