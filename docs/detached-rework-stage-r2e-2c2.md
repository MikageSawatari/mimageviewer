# stage-r2e-2c2 — 可視性で弾けない残りを監査する道具

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §9.5 (作業環境・②-c1 / ②-c1b の記録) を読むこと。**
設計の正本: [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)
**§6.2 / §6.3 (監査規則 A1〜A7)**。

ブランチ: `r2e-ownership` (worktree `C:\home\mimageviewer-r2e`)。
コミットメッセージに `(detached-rework R2e-2c2)` を含める。

**この段は本体のコードを 1 行も変えない。** 新しい workspace member を足すだけ。

---

## 1. やること

`tools/viewer_context_audit` を作り、**A2 / A3 / A7 の 3 規則**を実装する。

②-c1b で「非テストのビルドは 225 フィールド private で通る」ところまで来た。
可視性で弾けるものはコンパイラが弾く。**残るのは可視性では表せない逸脱**で、それが §6.2 の表:

| 穴 | 規則 |
| --- | --- |
| bundle 型を一切使わずに、App のフィールドを直接動かして context を手で移す | **A2** |
| `ViewerContextBundle::` の関連関数をモジュール外から呼ぶ | **A3** |
| registry そのものを別の所有者へ渡す | **A7** |

⚠ **A1 / A5 はこの段で有効化できない** (設計 §7 ②-c、Codex 第 3 版レビュー BLOCKER)。
`ActiveDetachedViewerContext::bundle` と `DetachedImageWindowSnapshot::paused_bundle` が
まだモジュール外の型位置に `ViewerContextBundle` を出しているので必ず落ちる。②-d。
**A4 / A6 は②-e** (公開面 allowlist と `test_access` が無いと意味を持たない)。

---

## 2. 規則を実測してから決めた (設計から 2 点ずらしてある)

指示書を書く前に現行ツリーで数えた。**設計のまま実装すると使い物にならない規則が 2 つある。**

### 2.1 A2 — 行単位ではなく「関数あたりの異なるフィールド数」で判定する

設計は「`mem::swap` / `replace` / `take` の実引数に 225 フィールド名のフィールドアクセスが
現れたら弾く」と書いている。現行ツリーで数えると **62 箇所**当たる。内訳:

| 呼び先 | 箇所 |
| --- | --- |
| `mem::take` | 57 |
| `mem::replace` | 5 |
| `mem::swap` | **0** |

62 箇所は全部 false positive である (`App` の one-shot を消費する
`std::mem::take(&mut self.pending_auto_fs_open)` などや、`metadata` / `prepared` といった
別 struct が対象)。**62 行の allowlist は保守できないし、その中に本物が隠れる。**

一方、**関数あたりの異なるフィールド数**で見ると分離が非常に良い:

| 異なるフィールド数 | 関数 |
| --- | --- |
| 20 | `src/app/smart_folder.rs` `preserve_smart_folder_session_for_load` |
| 11 | `src/app.rs` `start_loading_items_inner` |
| 10 | `src/app.rs` `remove_items_batch` |
| 5 | `src/app/snapshot_ops.rs` `activate_snapshot` |
| **2 以下** | それ以外すべて |

**これが A2 の狙っている形そのものである** — 「bundle 型を名指しせずに context 相当の状態を
まとめて手で動かす」。1 つの one-shot を `take` するのは正常、10〜20 個をまとめて動かすのは
そうではない。よって規則を次の 2 本にする:

- **A2a**: `mem::swap` の実引数に bundle フィールド名のフィールドアクセスが現れたら弾く
  (設計が挙げている例 `mem::swap(&mut self.items, &mut stash)` そのもの)。**現在 0 件**。
- **A2b**: 1 つの関数の中で `mem::swap` / `replace` / `take` の実引数に現れる
  **異なる** bundle フィールド名が **3 つ以上**なら弾く。**現在 4 件**、上の表のとおり。

閾値 3 は実測の分離 (4 件が 5 以上、残りが 2 以下) から採った。**マージンがある。**

⚠ **allowlist の鍵は行番号ではなく `ファイル + 関数名 + 理由`** にする。
`src/app.rs` は 4 万行あり行番号は毎コミット動く。行キーの allowlist は即座に腐る。
上の 4 件は**コードを読んで理由を書く**こと (「なぜこれは context の手動移動ではないのか」)。
理由が書けないものは弾かれたままにして報告する。

### 2.2 A3 — テストコードはこの段では対象外

設計は対象を `src/**/*.rs` 全部としている。現行ツリーの `ViewerContextBundle::` は **110 箇所**:

| 箇所 | 場所 |
| --- | --- |
| 109 | `src/app/tests.rs` の `ViewerContextBundle::empty()` |
| 1 | `src/app.rs` の `take_current_viewer_context_bundle` 内 |

**109 行の allowlist は雑音でしかない。** テストが `empty()` を呼べなくなるのは②-e で
`test_access` が入ったときで、しかもそのときは**コンパイラが弾く**ので監査は要らない。
よってこの段では:

- `#[cfg(test)]` が付いた item の中は対象外にする (syn で属性を見る)
- `src/app/tests.rs` はファイルごと対象外にする (`#[cfg(test)] mod tests;` で取り込まれるため、
  ファイル単体を見ても `cfg(test)` 属性が見えない)
- 残る `src/app.rs` の 1 件は**理由付き allowlist**に入れ、**②-e で消えること**を理由に書く

②-e で `test_access` が入ったら、この 2 つの除外を外せるか見直す。

### 2.3 A7 — 対象がまだ存在しない。だから fixture で試験する

A7 は `App::viewer_contexts` を守る規則だが、**そのフィールドは②-d まで存在しない**。
つまり現行ツリーでは**何も検出しない = 一度も動いたことがない規則**になる。

**だから A7 は fixture テストで完全に覆うこと。** (a)〜(f) の 6 形すべてについて、
**弾く例と弾かない例の両方**を合成ソースとして書き、解析器に食わせて検証する。
「本番で 0 件でした」を根拠にしない。

---

## 3. 作るもの

```
tools/viewer_context_audit/
  Cargo.toml        # workspace member。mimageviewer に依存しない
  src/main.rs       # 解析 + 報告 + exit code
  src/...           # 規則ごとに分けてよい
```

- 依存は `syn` (`full` + `visit` + `extra-traits`) と、必要なら `walkdir` / `proc-macro2`。
  **`mimageviewer` に依存しない** (vendor 資産不要で ubuntu CI で走ることが要件)。
- 225 フィールド名は**ソースから読む**。
  `src/app/viewer_context_registry.rs` を syn で解析して `struct ViewerContextBundle` の
  フィールド ident を集める。**見つからなければ「規則を飛ばす」のではなく失敗させる。**
  調査対象の信号に抑制条件を依存させると出力ゼロになる (過去に 2 回踏んでいる)。
- import 正規化 (A2 / A7 共通): ファイルごとに `use` 木を読み、
  `std::mem::{swap, replace, take}` / `core::mem::...` への**別名を含む全経路**を
  1 つの正規名へ畳んでから照合する (`use std::mem::take as pull;` を素通りさせない)。
- 実行: `cargo run -p viewer_context_audit`。違反があれば**内容を印字して exit 1**。
  0 件なら無出力で exit 0。
- allowlist は**ソース内の静的テーブル**でよい (別ファイルでもよい)。
  各エントリに `ファイル / 関数 / 規則 / 理由` を持たせ、**理由は必須**にする。

---

## 4. テスト (この段の本体)

**規則ごとに、弾く例と弾かない例の両方**を fixture ソース文字列で書く。
とくに A7 は本番で 0 件なので、**fixture が唯一の証拠**になる。

| 規則 | 最低限の fixture |
| --- | --- |
| A2a | `mem::swap(&mut self.items, &mut stash)` を弾く / bundle に無いフィールド名の swap は通す |
| A2b | 3 フィールドを `take` する関数を弾く / 2 フィールドは通す / **別関数に分かれていれば通す** |
| A2 共通 | `use std::mem::take as pull; pull(&mut self.items)` を弾く (import 正規化) |
| A3 | `ViewerContextBundle::empty()` を弾く / `#[cfg(test)]` の中は通す |
| A7 (a) | `mem::take(&mut self.viewer_contexts)` を弾く |
| A7 (b) | `self.viewer_contexts = ViewerContextRegistry::new()` を弾く |
| A7 (c) | `helper(&mut self.viewer_contexts)` を弾く |
| A7 (d) | `let App { viewer_contexts, .. } = app;` を弾く |
| A7 (e) | `let r = app.viewer_contexts;` を弾く |
| A7 (f) | `fn take_registry() -> Option<ViewerContextRegistry>` を弾く |
| 全規則 | registry モジュール自身の中では**どれも弾かない** |
| フィールド抽出 | struct が見つからないとき**失敗する**こと |

⚠ **落ちようがないテストを書かない。** 「0 件でした」を assert するテストは、規則を
まるごと消しても通る。**必ず「弾くべき入力を与えて弾かれること」を見ること。**

---

## 5. CI

`.github/workflows/ci.yml` に **3 つ目の job** を足す。既存 2 job と違い
apt も FFmpeg ヘッダも要らない (本体をビルドしないため) ので軽い。

```yaml
  viewer-context-audit:
    name: viewer context audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - <rust toolchain>
      - run: cargo test --locked -p viewer_context_audit
      - run: cargo run --locked -p viewer_context_audit
```

⚠ **`master` では別のバグ修正が進行中なので、この branch は当面 merge しない。**
よって **CI はこの job を merge まで走らせない**。ローカルで両コマンドが通ることを
必ず自分で確認すること。

---

## 6. 完了条件

1. `cargo test -p viewer_context_audit` が緑。**§4 の fixture を全部含むこと。**
2. `cargo run -p viewer_context_audit` が **exit 0** (allowlist 込みで 0 違反)。
3. **allowlist の各エントリに、コードを読んで書いた理由が付いていること。**
   理由が「既存だから」「たぶん問題ない」では不可。理由が書けないものは
   **allowlist に入れず、違反のまま報告する。**
4. `cargo test -p mimageviewer --lib` が緑。**件数 6251 のまま**
   (本体を触っていないので変わらないはず)。
5. `cargo fmt --check` が無出力。
6. `git diff --numstat HEAD` を貼る。変更してよいのは
   `Cargo.toml` (members 追加) / `Cargo.lock` / `tools/viewer_context_audit/**` /
   `.github/workflows/ci.yml` の 4 つだけ。**`src/` は 1 行も変えない。**
7. `cargo run -p viewer_context_audit` の出力を、**allowlist を空にした状態**でも 1 度貼る
   (= 何を検出できているかの証拠)。§2.1 の 4 件と §2.2 の 1 件が出るはず。

---

## 7. 報告に必ず含めること

- §6 の 7 項目の実出力。
- **A2b の 4 件それぞれについて、コードを読んで書いた理由。**
  読んでみて「これは実際に context を手で移している」と思ったものがあれば、
  **allowlist に入れずに報告する** (この段では直さない)。
- 閾値 3 が現行ツリーで正しく分離しているか、自分でも数えて確認した結果。
- A7 の fixture が (a)〜(f) の 6 形すべてを覆っていることの対応表。
- 実装中に「設計の規則のままでは表現できない」と気づいた点があれば、
  回避する前に報告する。
