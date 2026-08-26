# stage-r2e-2b — `ViewerContextBundle` を registry モジュールへ移す

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §9.5 (作業環境を含む) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
§7 ②-b と **§6.1 (可視性だけで弾けるもの)**。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2b)` を含める。

**触ってよいファイルは `src/app.rs` と `src/app/viewer_context_registry.rs` の 2 本だけ。**
`src/app/tests.rs` は**変更不要**であること自体がこの段の確認項目 (後述 §6-4)。

**この段は純粋な移設。挙動は 1 ビットも変わらない。** 中身の書き換えもしない。

---

## 1. やること

`ViewerContextBundle` とその所有プリミティブを `src/app/viewer_context_registry.rs` へ移す。
**この段ではまだ何も弾けない** — フィールドは `pub(in crate::app)` のままにする。
弾き始めるのは②-e。

### 1.1 なぜ先に移すのか

設計 §6.1 の完了判定は **Rust の可視性規則そのもの**である。子モジュールは親の private を
見られるが、親は子の private を見られない。だから `ViewerContextBundle` を子モジュールへ
移してフィールドを private にすると、**モジュールの外では struct literal も destructure も
`empty()` も言語仕様として書けなくなる**。lint も grep も要らない。

移設と可視性の緊縮を 1 コミットでやると、225 フィールド分のコンパイルエラーと
移動した行が同じ diff に混ざってレビュー不能になる。**先に移すだけ移し、緊縮は②-e で
やる**。緊縮のときはコンパイラが作業リストを列挙してくれる。

---

## 2. 移す対象 (5 ブロック)

行番号は `3c435070` 時点。**doc comment と `#[cfg(windows)]` 属性行を含めて**移す。

| # | 対象 | 現在地 | 行数 |
| --- | --- | --- | --- |
| 1 | `struct ViewerContextBundle` (225 フィールド) | [app.rs:2503](../src/app.rs:2503)〜[:2779](../src/app.rs:2779) | 277 |
| 2 | `impl Drop for ViewerContextBundle` | [app.rs:2781](../src/app.rs:2781)〜[:2821](../src/app.rs:2821) | 41 |
| 3 | `impl ViewerContextBundle` (`set_items_generation` / `empty` / `pause_background_work_keep_current_frame`) | [app.rs:2823](../src/app.rs:2823)〜[:3112](../src/app.rs:3112) | 290 |
| 4 | `App::swap_viewer_context_bundle` (225 フィールドの exhaustive destructure) | [app.rs:16213](../src/app.rs:16213)〜[:16702](../src/app.rs:16702) | 490 |
| 5 | `App::split_current_context_preserving_main_grid` (fork の 3 分類 destructure、doc comment 含む) | [app.rs:42012](../src/app.rs:42012)〜[:42515](../src/app.rs:42515) | 504 |

4 と 5 は `App` のメソッドなので、registry モジュール側に `impl App { ... }` ブロックを作って
その中へ入れる。**inherent impl はどのモジュールに書いてもよい**ので合法である。
`App` の private フィールド (`self.items` など) にも、子モジュールからなので届く。

### 2.1 置き場所

stage ① の状態機械は**一切触らない**。`mod tests` の**手前**に区切りコメント付きで
挿入する。stage ① の塊が連続したまま残るので、この段の diff は「移動しただけ」だと
一目で分かる。

---

## 3. 可視性 (この段の値)

| 項目 | この段 | ②-e |
| --- | --- | --- |
| `struct ViewerContextBundle` | `pub(in crate::app)` | 同じ |
| 225 フィールド | `pub(in crate::app)` | **private** |
| `empty` / `set_items_generation` / `pause_background_work_keep_current_frame` | `pub(in crate::app)` | private (+ `test_access`) |
| `swap_viewer_context_bundle` / `split_current_context_preserving_main_grid` | `pub(in crate::app)` | 検討 (②-d で transaction へ吸収) |

⚠ **`impl App` の中のメソッドは「何も付けなければ private」であり、その private は
`impl` ブロックを書いたモジュールの private である。** 4 と 5 は今 `fn`(可視性なし) だが、
移設後も app.rs から呼べる必要があるので **`pub(in crate::app)` を付ける**。
付け忘れると「app.rs から見えない」というエラーになる。

⚠ **`#[cfg(windows)]` を移設で落とさないこと。** 5 ブロックすべてが Windows 限定である。
app.rs 側に置く再エクスポート `use` も **`#[cfg(windows)]` を付ける**。
落とすと ubuntu の `cargo check` (cfg(windows) 漏れの番人、CLAUDE.md リリース手順 Phase 2)
だけが落ち、Windows 機では気づけない。

---

## 4. 付随してやること (2 つだけ)

1. **app.rs に再エクスポートを足す。**

   ```rust
   #[cfg(windows)]
   use viewer_context_registry::ViewerContextBundle;
   ```

   これで `src/app/tests.rs` の `super::ViewerContextBundle` も従来どおり解決する
   (private import も子モジュールからは見える)。

2. **`#![allow(dead_code)]` の適用範囲を狭める。**
   [viewer_context_registry.rs:2](../src/app/viewer_context_registry.rs:2) の
   **inner attribute はファイル全体に効く**ので、そのままだと移設した production 型にも
   かかり、**使われなくなった bundle フィールドの検出が止まる**。
   inner attribute を削り、stage ① の**非テスト top-level 13 項目**
   (`ViewerContextId` / `ContextResidence` / `Projection` / `Slot` / `ForkPolicy` /
   `TableOp` / `BindError` / `MountError` / `RetireError` / `PendingTransition` /
   `ContextTable` と 2 つの `impl`) に `#[allow(dead_code)]` を個別に付ける。
   ②-d で registry が production に繋がったら、この 13 個も外す。

---

## 5. やらないこと

- **中身を 1 行も書き換えない。** 名前・順序・コメント・マクロをそのまま運ぶ。
  「ついでに」の整理を入れない。差分が「移動」に見えなくなる。
- **フィールドを private にしない。** ②-e。
- **accessor を作らない。** ②-c。
- **`take_current_viewer_context_bundle` ([app.rs:16705](../src/app.rs:16705)) は app.rs に残す。**
  `empty()` + `swap` を呼ぶだけで、この段では両方 `pub(in crate::app)` なので通る。
  移設が要るのは②-e。
- **`split_materialized_physical_context_for_independent_still_open` も app.rs に残す。**
  bundle のフィールドへ直接書いているが、`pub(in crate::app)` の間は通る。
- **stage ① のコードを触らない** (§4-2 の `#[allow(dead_code)]` 付与を除く)。
- `src/app/tests.rs` を触らない。

---

## 6. 完了条件

すべて満たしてから報告する。

1. `cargo test -p mimageviewer --lib` が緑 (**PowerShell から**。bash 経由だと
   FFmpeg DLL が解決できず `STATUS_DLL_NOT_FOUND`)。**件数が 6251 のまま**であること
   — この段はテストを増やしも減らしもしない。
2. `cargo fmt --check` が無出力。
3. `git diff --numstat HEAD` が **`src/app.rs` と `src/app/viewer_context_registry.rs` の
   2 本だけ**で、`app.rs` の削除行数と registry の追加行数が**おおむね釣り合う**こと
   (可視性キーワードと区切りコメントの分だけ registry 側が増える)。実際の数値を貼る。
4. **`src/app/tests.rs` が無変更であること。** 変更が要るなら再エクスポートか可視性が
   間違っている。テスト側を直して辻褄を合わせない。
5. 移設した 5 ブロックが**すべて `#[cfg(windows)]` を保っている**こと。
   `rg -n "#\[cfg\(windows\)\]" src/app/viewer_context_registry.rs` の出力を貼る。
6. registry モジュール外に `ViewerContextBundle` の定義・destructure・`empty()` が
   残っていないこと。

   ```
   rg -n "struct ViewerContextBundle|impl (Drop for )?ViewerContextBundle|let ViewerContextBundle \{|ViewerContextBundle::empty" src/app.rs
   ```

   期待される残りは **`take_current_viewer_context_bundle` 内の `ViewerContextBundle::empty()`
   1 件だけ** (§5 で残すと決めたもの)。それ以外が出たら報告する。

---

## 7. 報告に必ず含めること

- 上の 6 項目の実出力。
- 移設中に**そのままでは通らなかった箇所**があれば、何をどう直したか
  (import 不足 / 可視性 / マクロのスコープなど)。**中身の書き換えに当たるものは、
  直す前に報告する。**
- `use super::*;` を使ったか、明示 import にしたか。この repo の子モジュールは
  `use super::*;` が慣行 ([startup_ops.rs:1](../src/app/startup_ops.rs:1))。
- 「移動しただけ」であることを、どう自分で確かめたか。
