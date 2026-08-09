# ブリーフ: §1.61 見開きでページが 1 枚ダブる (寸法の知識が退去で消える)

対象: v2.13.0 バックログ §1.61。実装 = Codex Sol / レビュー・検収 = ClaudeCode /
実機確認 = 利用者。

正本: [docs/next-release-backlog.md](next-release-backlog.md) §1.61 (症状・報告内容・
再現データはそちらを参照)。優先度 P1 相当。

前提 (コミット済み): `f7a75662` まで。master の作業ツリーで作業する。

---

## 1. 何が壊れているか (原因は特定済み。再調査は不要)

見開きの組み方は毎フレーム `build_spread_display_units_with_predicates`
([src/ui_fullscreen.rs:5954](../src/ui_fullscreen.rs)) が **nav の先頭から歩き直して**決める。
横長ページは単独ユニットになるので、そこから後ろの偶奇が決まる。

その `is_landscape` ([src/ui_fullscreen.rs:5835](../src/ui_fullscreen.rs)) は
**生きているテクスチャにしか寸法を聞かない**:

1. `fs_cache` (先読み窓ぶんだけのフルサイズキャッシュ)
2. `thumbnails[idx]` が `ThumbnailState::Loaded` のとき (`source_dims` かテクスチャ寸法)
3. どちらも無ければ **`false` (= 縦長)** を返す

`fs_cache` は読み進めると窓から落ち、`evict_grid_thumbnail`
([src/app.rs:50253](../src/app.rs)) は `Loaded` → `Evicted` へ落とす。
**`ThumbnailState::Evicted` は `source_dims` を持たない**ので、そこで寸法の知識が消える。
消えた瞬間に「横長 → 縦長」へ**後退**し、単独だったページが対になって以降の偶奇が
1 つずれる = 直前のページがもう一度出る。

**壊れている不変条件: 一度分かったページの寸法を忘れないこと (既知 → 未知へ後退しない)。**

### 1.1 影響範囲は詳細表示だけではない (ClaudeCode 確認済み、修正方針の前提)

バックログには「詳細表示だけの問題ではない可能性 (要確認)」と書いてあるが、
**ソース上で確定した**。`evict_grid_thumbnail` は表示モードを問わず `Loaded` → `Evicted`
にするので、**サムネイル表示でも横長ページが keep 範囲から外れれば同じ後退が起きる**。
報告が「詳細表示だけ」に見えたのは、詳細表示が `keep_range = (0,0)` で寸法ソース 2 を
最初から潰すため 11 枚でも再現したからで、枚数が多ければ通常表示でも起きる。

したがって**詳細表示に限定した対処をしないこと**。寸法の知識そのものを永続化する。

---

## 2. 直し方 (この設計で実装すること)

### 2.1 判明した寸法を保持する専用ストアを作る

`src/page_dims.rs` を新設する。

```rust
/// 一度判明したページのピクセル寸法を、テクスチャの生存期間と切り離して覚えておく。
pub struct PageDimsCache { /* generation: u64, dims: HashMap<usize, (u32, u32)> */ }

impl PageDimsCache {
    /// 記録する。`generation` が保持中のものと違えば、先に中身を捨ててから記録する。
    pub fn record(&mut self, generation: u64, idx: usize, dims: (u32, u32));
    /// 読む。`generation` が保持中のものと違えば **None** (= 今までどおり不明扱い)。
    pub fn get(&self, generation: u64, idx: usize) -> Option<(u32, u32)>;
    pub fn clear(&mut self);
}
```

- **`items_generation` の刻印が安全装置**である。items が差し替わったり、削除で idx が
  ずれたりしたときに、掃除を書き忘れても**別ファイルの寸法を返さない** (fail-closed =
  不明に戻るだけ = 今までの挙動)。`AutoAspectState.items_generation`
  ([src/auto_aspect.rs:36](../src/auto_aspect.rs)) と同じ手であり、この repo の既存作法。
- 記録は**上書き可**。より確かな値 (`source_dims`) が後から来たら置き換えてよい。
  禁止するのは**消すこと**だけ (generation 変更を除く)。
- 値は**ソースが渡してきた値をそのまま**入れる。EXIF orientation を適用するか等の
  判断をこの層でしない (今の `is_landscape` と同じ値になることが要件)。
- メモリ: 10 万ページで数 MB 以内。上限は設けない。

### 2.2 置き場所は `ViewerContextBundle` (App global にしないこと)

`ViewerContextBundle` ([src/app.rs:2039](../src/app.rs)) に `items` / `thumbnails` /
`fs_cache` / `rotation_cache` と並べて持たせ、App 側にも同名フィールドを持つ
(他の per-context フィールドと同じ swap 対象にする)。

**App global + generation 刻印では不十分**である。`items_generation` は context ごとの
カウンタで、**別 context 間で世代番号を比較できない** (同ファイル `facet_name_cache` の
コメントが同じ理由を述べている: 「generation 空間を共有しない viewer context と一緒に
所有する」)。App global にすると、detached と main で同じ世代番号が別の item 列を指した
瞬間に**別ファイルの寸法**を返す。

> ⚠ **detached リワークの凍結ルール**: bundle に触るので、着手前に
> [docs/detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) を読むこと。
> これは症状パッチではなく、per-context な idx キー状態を既存の所有境界へ足す構造的修正
> である、という認識で合っているかを**報告に明記**すること (ClaudeCode 側も同じ認識)。
> 触れた範囲は同 plan に 1 節記録すること。

### 2.3 clear は既存の 1 箇所に足す

`invalidate_idx_state_and_queues` ([src/app.rs:23963](../src/app.rs)) が
「idx 空間が変わった」ときの正規フックで、idx キー cache の clear が既に並んでいる。
**ここに 1 行足す**。新しい掃除経路を作らない。generation 刻印はこのフックを通らない
経路のための保険であって、代わりではない。

### 2.4 記録するのは 2 箇所だけ

**(a) サムネイルがロードされた時点** — `poll_thumbnails` の中で、`auto_aspect.samples` に
ratio を入れているところ ([src/app.rs:29710](../src/app.rs) 付近) と同じ場所。
そこには既に寸法が手元にある。

**(b) フルスクリーンのフレーム冒頭で `fs_cache` を 1 回なめる** — `Static` /
`Animated` / `Video` の各アームから、`is_landscape` が使うのと**同じ値**を取り出して
記録する `&mut self` の helper を書き、フルスクリーン update の入口 (spread が組まれるより
前) で 1 回だけ呼ぶ。`fs_cache` は先読み窓ぶんしかないので O(数十)。

- **`thumbnails` を毎フレーム全走査しないこと** (10 万件のフォルダで無駄)。
- (b) を「フレーム冒頭 1 回」でよい理由: ストアが必要なのは**生きた cache から既に
  消えたページ**だけで、生きているうちは `is_landscape` が直接読める。同じフレームで
  作られた直後のエントリは次フレームの harvest で拾われ、それまでは live 側が答える。
- 新しい寸法ソース (PDF / AI / 将来の何か) が増えても `fs_cache` に入る限り自動で拾える。
  producer ごとに記録を書き足す設計にしないこと (書き漏らしが起きる)。

### 2.5 読む側

`is_landscape` の**第 3 のソース**としてストアを引く (`fs_cache` → `thumbnails` →
ストア → `false`)。呼び出しは `build_spread_display_units` 経由の 6 箇所
(`src/ui_fullscreen.rs` の 9372 / 13152 / 13214 / 13244 / 13316 / 20135)。

`false` フォールバック自体は残してよい (一度も見ていないページに対しては他に答えが無く、
対にできないページを勝手に単独扱いすると別の崩れ方をする)。ただし **doc コメントを
更新**して「不明は縦長とみなすが、一度判明した寸法は忘れないので既知 → 未知へは
後退しない」ことを書くこと。今の「テクスチャサイズが不明な場合は false」という説明は、
それが**構造的な穴**だと読めないのが今回の原因の一部である。

---

## 3. やらないこと

- 詳細表示だけの特別扱い、`keep_range` の変更、先読み枚数の変更
- 回転 (`rotation_cache`) を寸法判定に反映すること。今は反映していないので**そのまま**
  (変えると回転ページの組み方が変わる = 別の仕様変更になる)
- カタログ DB から寸法を先読みすること (UI スレッド同期 I/O 禁止。将来の改善余地として
  バックログに書くだけにする)
- `build_spread_display_units_with_predicates` のアルゴリズム変更

---

## 4. テスト (すべて必須)

1. **`PageDimsCache` の単体テスト**: 記録 → 取得、generation 不一致で `None`
   (fail-closed)、generation 変更で中身が捨てられる。
2. **純関数レベル**: `build_spread_display_units_with_landscape` で、
   「先頭が横長のとき `1 / 3,2 / 5,4 …`」「先頭の横長フラグが false へ落ちると
   `1,2 / 3,4 …` にずれる」ことを固定し、**後者が退行の形**であることをテスト名で示す。
3. **App レベルの状態遷移テスト (今回の報告そのもの)**:
   idx 0 が横長・1..10 が縦長の items を用意 → 0 の寸法が `fs_cache` か
   `thumbnails[0]` から判明する状態にする → harvest → **`fs_cache` から 0 を消し、
   `thumbnails[0]` を `Evicted` にする** → 見開きユニットの区切りが**変わらない**ことを
   assert する。修正前のコードでは落ちること (= 退行ガードとして機能すること) を
   報告に書くこと。
4. `cargo test -p mimageviewer --lib` が全件 (現在 4985 件)、
   `cargo test -p mimageviewer --test ui_snapshot` が更新なしで通ること。

## 5. 完了条件

- `cargo fmt` (引数なし)
- 上記テスト + `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと (`cfg(windows)` 漏れ)
- **[docs/next-release-backlog.md](next-release-backlog.md) §1.61 に実装記録を追記**
  (原因・採った構造・fail-closed の理由)。§1.61 の「⚠ 要確認」は本ブリーフ §1.1 の
  確定結果に置き換えること
- **[docs/detached-rework-plan.md](detached-rework-plan.md) に bundle へ足した旨を記録**

## 6. 制約

- **アプリを起動しないこと。** 検証ビルドと実機依頼は ClaudeCode が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す
- 症状を消すガード / 追加 reset / silent fallback を足さないこと

---

完了したら次を報告すること:

1. 実装した構造 (ストアの置き場所、clear の位置、記録の 2 箇所)
2. §2.2 の detached 認識に同意するか (症状パッチではなく構造的修正である、で合っているか)
3. テスト結果と、3 のテストが修正前に落ちることの確認
4. **実機で確認してほしいこと** (再現データ `C:\tmp\miv-spread-test\` を使う手順)
