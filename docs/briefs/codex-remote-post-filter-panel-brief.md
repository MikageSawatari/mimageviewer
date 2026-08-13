# リモートにポストフィルタのパネルを足す

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

- **1 コミット**。
- `docs/briefs/HANDOFF.md` と他の brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、末尾のテストを走らせる。

---

## いま起きていること

本体で選んだポストフィルタは**リモートの表示にも出ている**のに、**リモートからは変えられない**。
CRT やファミコン風にしたまま外出すると、スマートフォン側でそれを解除できない。

壊れてはいない。`apply_remote_adjustment_values` ([mod.rs](../../src/remote_ipc/mod.rs)) は既存の
`AdjustParams` を受け取って列挙されたフィールドだけ書き換え、`post_filter` は
`RemoteAdjustmentValues` に無いので保持される (実機で確認済み)。**足りないだけ**。

## ポストフィルタは 2 種類ある (ここが仕様の芯)

`post_filter::apply` ([post_filter.rs](../../src/post_filter.rs)):

- **`None` / `Nearest` / `UpscaleSharp` / `UpscaleAnime` は `src.clone()`** — 画素を変えない。
  本体が描画時に GPU でどう拡大するかを決めるだけ。**リモートに送るものが存在しない**
- **それ以外はすべて CPU で画素を書き換える**。`execute_final_composite` の中で走るので
  リモートの表示にも出ている

**4 つも一覧に載せる。** CRT を解除するには「標準（補間あり）」を選ぶしかなく、それが 4 つのうちの
1 つだから、隠すと解除できなくなる。

### 注意書きの文言 (決定済み。この通りにする)

> 本体の画面での拡大方法を決める項目です。リモートの表示は変わりません。

**「効果なし」と書かない。**理由: ①リモートでの選択は本体に記録されるので**本体の表示は変わる**
②「標準（補間あり）」は既定値であって壊れた選択肢ではない。「効果なし」と書くと、何もしていない
通常状態が不具合に見えるうえ、利用者は本体の見え方を意図せず変えることになる。

---

## 実装

### 1. 分類表を 1 つにする (**3 つ目のコピーを作らない**)

`POST_FILTER_GROUPS` (`{label, filters}` の配列) が既に
[gamepad_input.rs](../../src/app/gamepad_input.rs) にあり、必要な形をしている。
**これを `adjustment.rs` へ移して `pub(crate)` にし、リモートはそこから組み立てる。**
`ui_adjustment_panel.rs` にも同じ分類がインラインで書かれているが、**今回そこは書き換えない**
(ローカル UI に snapshot テストがあり、範囲が広がる)。

### 2. 「画素を変えるか」を導出値にする

フラグを手書きの一覧で持たない。`PostFilter` に

```rust
/// 画素を書き換えるか。`false` は本体の描画時にだけ効く (拡大方法の選択)。
pub fn rewrites_pixels(self) -> bool
```

を足し、カタログのフラグはこれから作る。**`post_filter::apply` と同じ事実から導いていること**が
重要で、これがずれると注意書きが嘘になる。

### 3. IPC (既存の前例に合わせる)

`RemoteGridSortState` / `RemoteGridSortOption` と同じ形にする
(「値・ラベルは本体の `SortOrder` から毎回組み立てる」というコメントが付いている前例)。

- `RemoteAdjustmentState` に候補と現在値を足す。`ai_model_catalog` を既に送っているので置き場所は同じ
- 値は `PostFilter` の serde 表現 (`#[serde(rename_all = "snake_case")]`)、
  ラベルは `display_label()`。**JS 側に名前を書き写さない** (30 個以上あり、必ず腐る)
- 各項目に「リモートの表示は変わるか」のフラグを持たせる (上の `rewrites_pixels`)
- 書き込みは `RemoteAdjustmentValues` に **`post_filter: Option<String>`**。
  **`Option` にする理由**: 古い SPA の payload に欠けていても**保存済みの値を消さない**ため。
  既存の `ai` フィールドが同じ理由で `Option` になっている (「旧 SPA の payload では欠落する。
  `None` は AI 値を変更しない」)
- 未知の文字列は既存の値検証と同じ形で拒否する
- `read_only` の扱いは他の補正の書き込みと揃える

### 4. SPA

- タブを 4 つ目「フィルタ」として足す。タブ id は端末設定に保存されるが
  `normalizeAdjustmentTab` が未知値を弾くので既存端末は壊れない
- 本体から来たグループ順・ラベルのまま並べる
- 「基本」グループに上の注意書きを 1 度だけ出す

### 5. ドキュメント

- `htdocs/mimageviewer/manual/tut-remote.html` の「リモートでできること」表の
  「見え方を調整する」行に追記
- `htdocs/mimageviewer/manual/remote.html` の該当箇所も揃える
- 本文にバージョン番号や内部用語を書かない (CLAUDE.md「マニュアル・製品ページの記述方針」)

---

## やらないこと

- `ui_adjustment_panel.rs` のインライン分類の書き換え (別件)。
- 描画時の 4 つをリモートで再現すること。`Nearest` は CSS の `image-rendering` で近いことは
  できるが、サーバ側が既に縮小して JPEG にしているので届く時点で境界が失われている。
  シャープ / アニメ拡大は描画時シェーダなので再現には別の仕事が要る
- プリセットスロットや履歴のような新しい保存の仕組み

---

## テスト

1. **分類表が `PostFilter` の全 variant をちょうど 1 回ずつ含む。**
   新しいフィルタを足したときに、載せ忘れが必ず落ちること。
2. **`rewrites_pixels()` が `post_filter::apply` と一致する。**
   全 variant について、`apply` が入力と同じ画素を返すかどうかと一致すること。
3. **`post_filter` を含まない payload は保存済みの値を消さない** (古い SPA 互換)。
4. 未知の値は拒否される。
5. 設定した値が `RemoteAdjustmentState` の現在値として返る。
6. 描画時の 4 つだけにフラグが立ち、他には立たない。
7. `read_only` のときは他の補正と同じように書き込みを拒む。
8. JS: 本体から来たグループがその順で描かれ、注意書きが「基本」に 1 度だけ出る。

1・2・3 は今回の肝なので、**実装を潰したときに落ちることを自分で確かめてから**報告すること
(例: 表から 1 つ抜く / `rewrites_pixels` に `Nearest` を含める / `Option` をやめて必ず上書きする)。

---

## 実行するテスト

```
cargo test -p mimageviewer --lib remote_ipc
cargo test -p mimageviewer --lib adjustment
cargo test -p mimageviewer --lib post_filter
cargo test -p mimageviewer-remote
cargo fmt --all -- --check
node --test              (crates/remote-web/web で実行)
```

## 報告してほしいこと

- 何をしたか (コミットはこちらで行う)。
- 分類表をどこへ移し、`ui_adjustment_panel.rs` 側との重複を今どう扱ったか。
- `rewrites_pixels` をどう導出し、`apply` との一致をどう検証したか。
- 古い payload が保存済みの値を消さないことをどう保証したか。
- テストを潰したときに落ちることを確認した結果 (最低 3 つ: 1・2・3)。
- ブリーフと意図的に違えた点があれば、その理由。
