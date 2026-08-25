# stage-r2e-4 — 非同期要求を context の identity で相関させる

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §9.5 全体を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
**§3.1 (main は identity ではなく binding)**、§7 ④。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-4)` を含める。

⚠ **この段は挙動が変わる。** ②-d 以来 2 度目。**実機 smoke が要る。**

---

## 1. 何が壊れているか

メタ情報 import の再構築要求 (`metadata_import_refresh::ContextSlot`) は、
**context を identity ではなく「置かれている場所」で指している**。

```rust
pub(crate) enum ContextSlot {
    Main,
    ActiveDetached(Option<u64>),
    PausedDetached { index: usize, window_id: u64 },
}
```

これは R2e が繰り返し直してきた形そのもの — **場所を identity の代用にしている**。
結果、要求を出してから結果が返るまでの間に配置が変わると、正しい結果が捨てられる。

### 1.1 `PausedDetached { index }` — **Vec の添字**に依存している

適用側 ([app.rs:27351](../src/app.rs:27351)、[:27399](../src/app.rs:27399)) は
`self.detached_image_windows.get(window_index)` で引き直す。窓が 1 つ閉じただけで
添字がずれ、`window.id != window_id` に落ちて **stale 扱いで結果を捨てる**。
窓は生きていて context も生きているのに、**添字がずれたというだけで捨てている**。

さらに `None => continue` は**無言のスキップ**である (憲法 5)。

### 1.2 `Main` — 適用時に「今の main」を解決している

`ContextSlot::Main` は適用時に `self` (= 今マウントされている投影) へ書く。
要求を組み立ててから結果が返るまでの間に **promote が走ると、main は別の context に
なっている**。設計 §3.1 の「**main は context の属性ではなく binding**」の帰結で、
**別の context へ適用してしまう**。

### 1.3 `ActiveDetached(window_id)` — 窓 id での照合

active detached 窓が入れ替わると stale。context 自体は生きていることがある。

---

## 2. やること

**`ContextSlot` を `ViewerContextId` に置き換える。**

- 要求を組み立てる時点で、その context の `ViewerContextId` を焼き付ける。
- ⚠ **`Main` も要求時点の `registry.main()` を焼き付ける** (設計 §7 ④ の ⚠、
  Codex 第 3 版レビュー B)。適用時に解決し直さないこと。
- 適用側は `with_viewer_context(id, ..)` / `with_viewer_context_ref(id, ..)` で引く。
- **`items_generation` は staleness stamp として据え置く** (設計 BLOCKER 3)。
  identity と generation は別の問いである — 「どの context か」と「その context の
  一覧が入れ替わっていないか」。**両方要る。**

### 2.1 引けなかったときは、理由で分岐する

今の `None => continue` / `.unwrap_or(true)` をやめ、`viewer_context_residence(id)` で分ける。

| residence | 意味 | 扱い |
| --- | --- | --- |
| `Mounted` / `AtRest` | 生きている | 適用する |
| `Retired` | 払い出し済みで既に畳まれた | **正常**。遅れて届いた結果を捨ててよい。debug ログのみ |
| `Unknown` | 一度も払い出されていない id | **バグの疑い**。捨てる前に必ずログを出す |
| `Building` / `Retiring` | 遷移中 | 捨てる前にログを出す (起き得るなら理由も書く) |

設計 §3.2 が `Retired` と `Unknown` を分けているのは、**まさにこの判断のため**である。

---

## 3. 挙動がどう変わるか

**今日は捨てていた結果が届くようになる。**

- 窓の並びが変わっても、context が生きていれば適用される (§1.1)
- 要求後に promote が走っても、**要求した当の context** へ適用される (§1.2)
- active detached 窓が入れ替わっても、context が生きていれば適用される (§1.3)

⚠ **「届くようになる」は「上書きしてよい」ではない。** `items_generation` の照合は
そのまま残す。一覧が入れ替わっていれば従来どおり stale として捨てる。
**identity が合っていて generation も合っているときだけ**適用する。

---

## 4. テスト

既存のメタ import テストは `ContextSlot` を直接組んでいる (`tests.rs` に **19 箇所**)。
id ベースへ移すこと。既存の assertion は変えない。

**追加 (最低 3 本)。どれも「今日の実装で落ちる」こと:**

1. **窓の並びが変わっても結果が届く。** 要求を作った後に**前方の窓を 1 つ閉じ**、
   残った窓へ結果が適用されること。今日は添字がずれて捨てられる。
2. **promote 後も、要求した context へ適用される。** `Main` の要求を作った後に
   promote を走らせ、**新しい main ではなく元の context** へ適用されること。
3. **retire 済みの id 宛ての結果は静かに捨て、未払い出しの id 宛ては記録する。**
   `Retired` と `Unknown` が別扱いであることを見る。

⚠ **落ちようがないテストを書かない。** 1 と 2 は**現行コードで実際に落ちること**を
確かめてから報告する。落ちなければ、その形は現行でも通っているということなので、
**テストの作りが症状を再現できていない。**

---

## 5. やらないこと

- **`items_generation` を identity の代用にしない** (設計 BLOCKER 3)。
  detached の generation は `BASE | serial<<32` で serial を復元できるが、
  **promote された context は main 由来の generation を持ったまま detached になる**ので
  identity の代用にならない。`debug_assert` の相互チェックにだけ使ってよい。
- **guard / retry / 遅延で「届かない」を隠さない。**
- 他の非同期要求 (thumbnail / PDF / AI) には手を広げない。この段は metadata import だけ。

---

## 6. 完了条件

1. `cargo test -p mimageviewer --lib` が緑。**件数は増える** (§4 の追加分)。
2. `cargo test -p viewer_context_audit` が緑。
3. `cargo run -p viewer_context_audit` が exit 0。**公開面を増やしたなら A4 の allowlist を
   同じコミットで更新する** (更新しないと失敗する。それが A4 の役目)。
4. `cargo fmt --check` が無出力。
5. `cargo check -p mimageviewer --lib` の dead-code 警告が **9 件のまま**。
6. `rg -n "ContextSlot" src/ | wc -l` が **0**。
7. `git diff --numstat HEAD` を貼る。
8. `.\scripts\build-dev.ps1` で実機確認用バイナリを作る (**起動はしない**)。

---

## 7. 報告に必ず含めること

- §6 の 8 項目の実出力。
- **§4 の 1 と 2 が現行コードで落ちることを、どう確かめたか。**
- `Building` / `Retiring` が実際に起き得る経路かどうか。起き得るなら何が起きるか。
- **利用者に実機で確認してほしいシナリオ** (具体的な操作手順で)。
  最低でも「別ウィンドウを複数開いた状態でメタ情報 import」と
  「import 中に窓を閉じる / F12 で切り替える」を含めること。
