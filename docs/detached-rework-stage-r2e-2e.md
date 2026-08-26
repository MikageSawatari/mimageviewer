# stage-r2e-2e — フィールドを private にし、公開面を allowlist で固定する

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §9.5 全体を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
**§6.1 (可視性で弾ける表)**、**§6.3 の A4 / A6**、§7 ②-e。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2e)` を含める。

**この段は挙動不変。** 実機 smoke は不要な見込み。

---

## 0. これが R2e の完了点である

②-d までで**非テストのビルドは 225 フィールド private で通る**ようになった。
だが実際には `pub(in crate::app)` のままなので、**まだ誰でも書ける**。

この段で初めて、設計 §6.1 の表が成立する:

| モジュール外で書けなくなるもの | 効く仕組み |
| --- | --- |
| `ViewerContextBundle { .. }` (struct literal) | private field |
| `let ViewerContextBundle { a, b, .. } = x` (destructure) | private field |
| `bundle.items` などの直接アクセス | private field |
| `mem::swap(&mut self.items, &mut bundle.items)` | private field |
| `ViewerContextBundle::empty()` | private fn |

**lint も grep も要らない。言語仕様として書けなくなる。**
効くのは今の 1 回ではなく「**将来もう書けない**」という性質のほうである。

---

## 1. 規模 (実測、`9d0a2b44` 時点)

**大きい。1 回で終わらない可能性がある。**

| | 箇所 |
| --- | --- |
| `src/app/tests.rs` の `ViewerContextBundle::empty()` | **103** |
| `src/app/tests.rs` の bundle フィールド書き込み | **約 230** |
| registry モジュール内の `*_for_test` 入口 | 14 |

非テスト側は既に 0 件 (②-c1b で到達済み)。**残っているのはテストだけ。**

---

## 2. テストの書き換え方 — `test_access` の façade を作らない

230 個の setter を持つ façade は公開面が育つだけで、設計 §6.2 が名指しする形そのもの。

**正しい形は既に②-d が使っている**: **context を組み立てる closure は
`&mut App` を受け取り、マウント済みの投影に書く**。App のフィールドはテストから見えるので、
bundle のフィールドを private にしても壊れない。

```rust
// 今 (bundle を直接組む)
let mut bundle = ViewerContextBundle::empty();
bundle.items = vec![GridItem::Video(path.clone())];
bundle.fullscreen_idx = Some(0);
app.install_window_context_for_test(bundle, window_id);

// これから (マウントした投影に書く)
let id = app.build_window_context_for_test(window_id, |app| {
    let idx = push_video(app, path.to_str().unwrap());
    app.fullscreen_idx = Some(idx);
});
```

実例は②-d の
`deferred_detached_video_open_resumes_when_its_context_host_becomes_ready`
([tests.rs](../src/app/tests.rs)) と `build_active_context_for_test`
([viewer_context_registry.rs:3070](../src/app/viewer_context_registry.rs:3070))。

⚠ **入口を増やしすぎない。** 既存 14 個で足りるはずで、足りなければ**なぜ足りないか**を
報告してから足す。A4 の allowlist に載る面なので、1 つ増えるたびにレビューの対象になる。

⚠ **`ViewerContextBundle` を引数に取る `*_for_test` は消える**
(`install_window_context_for_test` / `install_active_context_for_test` など)。
closure 版へ置き換える。**A1 が型位置を弾くので、消さないと監査が通らない。**

⚠ **テストの assertion を変えない。** setup の形が変わるだけ。
値の期待値や検証内容を書き換えたら、それは別の変更なので報告する。

---

## 3. 手順

1. **テストを closure 形へ寄せる** (§2)。ここが作業量の大半。
2. **225 フィールドを private にする** (`pub(in crate::app) ` を落とす)。
   コンパイラが残りを列挙する。
3. **`empty()` / `set_items_generation` / その他の関連関数を module private にする。**
   `take_current_viewer_context_bundle` ([app.rs](../src/app.rs)) が `empty()` を呼ぶなら
   registry モジュールへ移す (②-b で「②-e で移す」と決めてある)。
4. **A3 のテスト除外を外す** ([tools/viewer_context_audit](../tools/viewer_context_audit/src/lib.rs))。
   ②-c2 では「tests.rs が 109 箇所で `empty()` を呼ぶから除外」としたが、
   この段でその 109 が消えるので**除外はもう要らない**。外して 0 件になることを確認する。
5. **A4 / A6 を有効化する** (§4)。

---

## 4. A4 / A6

### A4 — registry モジュールの公開面を allowlist と完全一致させる

設計 §6.3 が「**正規化した API 指紋**」と呼んでいるもの。**名前と型だけでは足りない。**
次を全部含めて初めて漏れが無くなる:

- 項目種別 (fn / struct / enum / type / const / mod / macro) と receiver (`&self` / `&mut self` / なし)
- **正確な可視性** (`pub` / `pub(crate)` / `pub(super)` / `pub(in path)` を区別する)
- **ジェネリックパラメータ、trait 境界、`where` 節**
  — allowlist 済みの `fn with_viewer_context<F>(.., f: F)` が、名前も引数名も戻り値も変えずに
  `F: FnOnce(&mut ViewerContextBundle)` を獲得して生 bundle を漏らせてしまう
- **公開型に対する trait 実装すべてと関連型** — `impl Deref<Target = ViewerContextBundle>` /
  `AsRef` / `AsMut` は impl 項目に `pub` トークンが無いので、**可視性だけを見る監査を素通りする**
- **公開フィールドと enum variant**、関連定数、関連型
- **`pub use` / 再エクスポート / use rename / 型エイリアス**、`#[macro_export]` マクロ

allowlist に無い公開項目を足したら**失敗する**。既存項目のシグネチャが変わっても失敗する。
**同じコミットで allowlist を明示的に更新する**のが正しい直し方 (= レビューの可視点を強制する)。

⚠ **`#[cfg(test)]` の項目は A4 の対象外**にしてよい。テスト入口まで allowlist に載せると
テストを 1 本足すたびに allowlist 更新になる。ただし A6 で別に守る。

### A6 — テスト専用入口が production から呼ばれないこと

設計は `viewer_context_registry::test_access::` を前提に書かれているが、
**実装はその名前空間を作っていない** (`#[cfg(all(test, windows))] impl App` の
`*_for_test` 群になっている)。**実装に合わせて規則を定義し直すこと**:

- 名前が `_for_test` で終わる項目は、**必ず `#[cfg(test)]` を含む cfg で囲われていること**
- そうした項目が `#[cfg(test)]` の外から呼ばれていないこと

**規則を実装に合わせて書き換えたことを、報告と allowlist のコメントに残す。**

### 両方に共通

②-c2 と同じ規律: **fixture テストで弾く例と弾かない例の両方**を書く。
とくに A4 は「trait 実装を足す」「ジェネリック境界を足す」「可視性を広げる」の 3 形を必ず覆う。
**既知の指摘 1 件 (`activate_snapshot`) を消さない。**

---

## 5. 完了条件

1. `cargo test -p mimageviewer --lib` が緑。**件数が減っていないこと** (6261 以上)。
   減っていたら理由を報告する。
2. `cargo test -p viewer_context_audit` が緑 (A4 / A6 の fixture を含む)。
3. `cargo run -p viewer_context_audit` が exit 0。**A1 / A3 / A4 / A5 / A6 / A2 が全部有効**で、
   既知の指摘は `activate_snapshot` の 1 件のまま。
4. `cargo fmt --check` が無出力。
5. `cargo check -p mimageviewer --lib` の dead-code 警告が **9 件のまま**
   (②-d の教訓。増えていたら 1 件ずつ「配線するか削除するか」を答える)。
6. **`ViewerContextBundle` のフィールドに `pub` が 1 つも付いていないこと。**

   ```
   rg -n "pub\(in crate::app\) [a-z_0-9]+:" src/app/viewer_context_registry.rs
   ```

   struct 本体で 0 件。出力を貼る。
7. `git diff --numstat HEAD` を貼る。

---

## 6. 報告に必ず含めること

- §5 の 7 項目の実出力。
- **テスト入口を増やしたなら、その一覧と「既存 14 個で足りなかった理由」。**
- **A6 を実装に合わせて定義し直した内容。**
- assertion を変えたテストがあれば、その一覧と理由 (無いのが正しい)。
- **終わらなかった場合は、終わらなかったと書く。** 途中経過と残りの一覧を出すこと。
  103 + 230 箇所は大きい。**コンパイルを通すためだけの妥協を入れない。**
