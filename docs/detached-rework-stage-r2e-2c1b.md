# stage-r2e-2c1b — 外からのフィールド「書き」を名前の付いた操作へ

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §9.5 (作業環境・②-c1 の記録を含む) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
§4 (transaction)、§6.1、**§6.2 (「公開面が静かに育つ」)**。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2c1b)` を含める。

**この段は挙動不変。** 書き方が変わるだけで、**どのフィールドがどう変わるかは 1 つも変えない。**

---

## 1. やること

②-c1 で読みは `ContextRef` に寄せた。残っているのは**書き 27 箇所**。
これを registry モジュール内の**名前の付いた操作**に置き換える。

終わると、**非テストのビルドではフィールドを private にできる**ようになる
(`cargo check -p mimageviewer --bin mimageviewer-core` の E0616 が **0**)。
残るのはテストからの直接アクセスだけで、それは②-e の `test_access` の仕事。

### 1.1 ⚠ 名前を付けることと、挙動を揃えることは別

下の W3 / W5 は「同じ操作の 3 変種」に見えるが、**落とすフラグの集合が 3 通りとも違う**。

| 経路 | `fs_open_intent_from_grid` | `pending_auto_fs_open` | `pending_return_to_parent` | `pdf_prefetch_grace_until` |
| --- | --- | --- | --- | --- |
| W3a ParkedLive → active ([app.rs:39210](../src/app.rs:39210)) | ✓ | ✓ | ✓ | ✓ |
| W3b passive → active ([app.rs:40105](../src/app.rs:40105)) | ✓ | — | — | ✓ |
| W5 promote ([app.rs:41215](../src/app.rs:41215)) | ✓ | ✓ | ✓ | — |

**この差をこの段で埋めない。** 各呼び出し箇所の集合を**そのまま**保つこと。
1 つの操作に畳んで「ついでに揃える」のは挙動変更であり、この段の条件に反する。

そのうえで、**この 3 通りが意図的なのかを調べて報告する**。
`git log -S` / `git blame` で各行がいつ入ったかを見て、
「3 経路が別々の時期に別々に書かれた」のか「意図した差」なのかを述べる。
**結論を出すのは利用者であって、この段ではない。**

---

## 2. 束ごとの指示 (27 箇所)

行番号は `2918e639` 時点。

### W1 — tag prewarm 起動 (2 箇所: [27289](../src/app.rs:27289) / [27296](../src/app.rs:27296))

`items` が空でなく `tag_prewarm_pending` が無ければ spawn する、という**ガード付きの操作**。
今は `ContextRef` の読み 2 回 + 直接代入に分かれている。

→ `ViewerContextBundle::ensure_tag_prewarm_started(&mut self) -> bool` に畳む
(戻り値は「起動したか」。使わないなら返さなくてよい)。

⚠ **同じ形が mounted 側にも 3 つ目のコピーとしてある** ([app.rs:27280](../src/app.rs:27280)、
`self.` 版)。**この段では触らない。** 3 コピー目が存在することを報告に書く (②-d の材料)。

### W2 — normalize state の破棄 (2 箇所: [38495](../src/app.rs:38495) / [38496](../src/app.rs:38496))

②-a の teardown ループ。設計 §4.4 が既に `clear_normalize_state()` と命名している。

→ `ViewerContextBundle::clear_normalize_state(&mut self)`。**この名前を使う。**

### W3 — parked bundle を active detached にする (6 箇所)

[39210](../src/app.rs:39210)〜[:39213](../src/app.rs:39213) と
[40105](../src/app.rs:40105)〜[:40106](../src/app.rs:40106)。

どちらも直前に `activate_independent_detached_session(id)` (②-c1 で追加済み) を呼んでいる。

→ **落とすフラグを引数で明示する形にはしない。** 引数で挙動が変わる操作は
「名前の付いた操作」ではない。代わりに、**それぞれの集合に名前を付けた 2 つの操作**にする:

- `activate_parked_live_as_independent_detached(&mut self, window_id: u64)` — 4 フラグ
- `activate_passive_as_independent_detached(&mut self, window_id: u64)` — 2 フラグ

どちらも中で `viewer_session.activate_independent_detached(window_id)` を呼ぶ。
②-c1 で足した `activate_independent_detached_session` は**この 2 つに吸収して消す**
(呼び出し元が W3 の 2 箇所と W5 だけなら)。W5 が残るなら残してよい。

### W4 — fork した bundle を detached physical scope に絞る (9 箇所)

[40983](../src/app.rs:40983)〜[:41000](../src/app.rs:41000)。
**`split_materialized_physical_context_for_independent_still_open()` の戻り値に対する
一続きの初期化**であり、その関数は②-c1 で registry モジュールへ移してある。

→ **初期化ごとモジュール側へ入れる。** fork 関数を拡張して
`split_materialized_physical_context_for_detached_scope(&mut self, physical_context: &Path, idx: usize, items_generation: u64) -> Box<ViewerContextBundle>`
のような 1 本にする (名前は任せる)。呼び出し側には `bundle.<field> = ...` が 1 つも残らないこと。

⚠ **`details_order` の clear は②-c1 で `clear_details_order()` として分離済み**だが、
この束の一部である。**畳み直して、`clear_details_order` は消す** (他に呼び出し元が無ければ)。
`visible_indices` の計算 (`item_belongs_to_detached_physical_scope` を使う filter) も一緒に入る。

### W5 — promote: mounted context を detached にする (7 箇所)

[41210](../src/app.rs:41210)〜[:41217](../src/app.rs:41217)。
`promote_active_detached_video_for_main_context_change` の中。
`selected` / `fullscreen_idx` / `native_video_in_window_active` / session activate /
`viewer_session.last_sync_stamp = None` / 3 フラグ。

→ `ViewerContextBundle::become_independent_detached_viewer(&mut self, window_id: u64, idx: usize)`
1 本に畳む (名前は任せる)。**W3 の 2 つと共通化しないこと** — フラグ集合が違う (§1.1)。

### W6 — 生きている detached media context を別のブックマークへ向け直す (1 箇所)

[startup_ops.rs:591](../src/app/startup_ops.rs:591)。直前の
`adopt_bookmark_media_open_pending(pending)` (②-c1 で追加済み) と**同じ 1 つの操作の 2 行目**。

→ 2 行を 1 つの操作に畳む。例:
`retarget_bookmark_media_open(&mut self, pending: PendingMediaOpen, target: BookmarkViewReturnTarget)`。

---

## 3. 置き場所と可視性

- 操作はすべて **registry モジュール内の `impl ViewerContextBundle`** に置く。
  可視性は `pub(in crate::app)`。
- **`ContextMut` を作らない。** ②-d。
- **引数で挙動が分岐する汎用 setter を作らない** (`set_field(name, value)` 等)。
  設計 §6.2 の「公開面が静かに育つ」に当たる。

---

## 4. 完了条件

1. **フィールドを private にして `cargo check -p mimageviewer --bin mimageviewer-core
   --message-format short` の E0616 が 0 件。** これがこの段の到達点。
   出力を貼ること (0 件になった証拠として、E0616 を含まないビルド結果全体)。
2. **`pub(in crate::app)` に戻してからコミットする。** テストはまだ直接触る。
3. `cargo test -p mimageviewer --lib` が緑。**件数 6251 のまま**。
4. `cargo fmt --check` が無出力。
5. `git diff --numstat HEAD` を貼る。変更してよいのは
   `src/app.rs` / `src/app/viewer_context_registry.rs` / `src/app/startup_ops.rs` の 3 本。
   **`src/app/tests.rs` は無変更**のはず (変更が要るなら理由を報告する)。
6. **§1.1 の表のとおり、3 経路のフラグ集合が変わっていないこと**を、
   新しい操作の中身を並べて示す。

---

## 5. 報告に必ず含めること

- §4 の 6 項目の実出力。特に **E0616 が 0 になった `cargo check`**。
- **§1.1 の 3 通りの差について、`git blame` / `git log -S` で調べた結果。**
  いつ・どの順で入ったか。意図的に見えるか、直し残しに見えるか。**直さずに述べる。**
- W1 の 3 コピー目 (mounted 側) の扱い — 触っていないことの確認。
- 畳んだ操作それぞれについて、**元の site と 1 対 1 で同じ書き込みをしている**ことの確認方法。
- 途中で「これは挙動が変わる」と気づいたら、**直す前に報告する**。
