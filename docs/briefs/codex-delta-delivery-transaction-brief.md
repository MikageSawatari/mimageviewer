# §1.86 — texture delta 配送を単一の transaction owner へ寄せる

対象: [next-release-backlog.md](../next-release-backlog.md) §1.86。
前提 = [codex-texture-delivery-test-brief.md](codex-texture-delivery-test-brief.md) (§1.85-A) が
**完了・マージ済みであること**。順序の根拠はそちらの §0 に書いてある。

このブリーフは §1.31 (wndproc 内 GPU 待ちの構造修正) の第 2 段である。
**§1.31 本体に手を出さないこと。**

## 0. 位置付けの訂正 — これは P3 のリーク修正ではない

backlog は本件を「小 / P3、影響はテクスチャリークであってクラッシュではない」と書いている。
**現時点の障害影響としてはそれで正しい。** だが §1.31 の依存関係上は、**P1 作業の必須前提**
として扱う。

理由: §1.31 は「acquire が間に合わなければフレームを捨てる」を通常経路にする。
捨てる経路で `free` が落ちる構造のまま §1.31 を入れると、frame drop のたびに GPU
residency が増え続ける。さらに本件を「単一 exit の transaction」として直しておけば、
§1.31 の frame drop は**新しい 6 個目の early return ではなく、既に検査済みの契約に
outcome を 1 つ足すだけ**になる。

つまり本件は「リークを塞ぐ」のではなく、**「一回限りの delta の配送責任が各 return に
分散している」という所有構造を直す**作業である。

## 1. 現状 — 5 つの exit と、それぞれの配送状況

`vendor/egui-wgpu/src/winit.rs` の `Painter::paint_and_update_textures`:

| # | exit | `set` | `free` |
| --- | --- | --- | --- |
| 1 | `render_state` 不在 | 適用不能 | 適用不能 |
| 2 | `surfaces.get(&viewport_id)` 不在 | 適用済 (lookup 前) | 適用済 |
| 3 | `SurfaceErrorAction::RecreateSurface` | 適用済 | **落ちる** |
| 4 | `SurfaceErrorAction::SkipFrame` | 適用済 | **落ちる** |
| 5 | 成功 | 適用済 | 適用済 (`queue.submit` の**後**) |

exit 1 は renderer 自体が無いので配送不能。これは**明示的な例外 outcome** として型に
残すこと (黙って同じ扱いにしない)。

## 2. 「1 つのヘルパ」は二段階でなければ誤り ⚠️

backlog の「`set` / `free` の適用を 1 つのヘルパに寄せる」を、
**`apply_set_and_free()` のような単一関数と読まないこと。両者は同じ時点に置けない。**

- `set` は surface lookup の**前**でなければならない (`ce6616ef` の修正内容)。
- `free` は成功経路では `queue.submit` の**後**でなければならない。
  未 submit の command buffer が参照している texture を `destroy` すると
  その command buffer が無効になる (現行コメントの制約は正しい)。

守るべき構造は次のとおり:

- **`begin_delivery`**: renderer が存在すれば `set` を**ちょうど一度**適用する。
- **inner paint / acquire**: typed な `PaintOutcome` を返す。
  **未 submit の encoder / command buffer を持ち越さない**こと。
- **`finish_delivery`**: outcome を受け取り `free` を適用する。
  - submit しなかった outcome では、command buffer が破棄された**後**に `free`。
  - 成功 outcome では、`queue.submit` の**後**に `free`。
- `RenderStateAbsent` は「配送不能」という**明示的な例外 outcome**。

すなわち、**1 つの delta transaction owner が begin / finalize を所有する**構造にする。

### 2.1 これが「構造的修正」である理由 (合意事項)

Codex Sol の判断 (2026-08-16): **上記の形なら症状パッチではなく構造的修正である。**
ClaudeCode も同意している。

ただし条件付きである。**exit 3/4 へ同じ `free` ループをコピーするだけの修正は、
「構造的修正」の合意対象外**。それは症状 (exit 3/4 で leak する) を消すだけで、
所有構造 (配送責任が各 return に分散している) を直さない。

したがって**コピーで済ませないこと**。単一 exit にできない理由が実装中に出てきたら、
その場で分岐を足さずに手を止めて報告すること。

## 3. 触ってよいファイル

- `vendor/egui-wgpu/src/winit.rs`
- `vendor/egui-wgpu/Cargo.toml` (test target 追加が要る場合)
- `scripts/test-full.ps1` (§1.85-A で追加した段に追記する場合)
- `docs/next-release-backlog.md` (§1.86 の状態更新)
- `docs/detached-rework-plan.md` (§11 への記録。§6 参照)

`src/` 配下と `vendor/eframe/` に触れないこと。

## 4. 借用の注意

`self.render_state.as_mut()` が `self` を可変借用する一方、`self.surfaces` /
`self.configuration` / `self.screen_capture_state` / `self.msaa_texture_view` /
`self.depth_texture_view` も同じ関数内で読む。inner を単純にクロージャへ切り出すと
借用検査で詰まる。

先に借用の割り付けを決めてから書くこと。分割が借用検査の都合で歪むなら、
**歪んだ形を無理に通さず報告する**こと (歪んだ分割は次の §1.31 で作り直しになる)。

## 5. テスト要件

§1.85-A で作った in-crate headless の足場を使う。同じ足場に足すこと。

### 5.1 outcome ごとの配送テスト

実 driver の `get_current_texture` を故意に失敗させない。
window destruction / minimize / device loss は不安定で、Recreate 時の surface 再 configure
も絡む。代わりに §2 で作る seam を検査する:

- acquire の結果と `on_surface_error` コールバックを typed outcome へ分類する**小さい関数**を
  切り出す。
- `Err + RecreateSurface → SurfaceRecreated`
- `Err + SkipFrame → Skipped`
- 各 outcome を outer の finalizer に与え、seed 済み texture が実際に free されることを
  `renderer.texture(&id).is_none()` で確認する。

これは「実 wgpu エラーが起きること」ではなく「起きたときの mIV / egui-wgpu の反応」を
検査するテストである。実エラーの生成は wgpu の責務なので、この切り分けで十分に honest。

### 5.2 §1.85-A のテストが**そのまま**通ること

exit 2 の `set` / `free` 配送テストを書き換えないこと。再構成が既存契約を壊していない
ことの guard がそれである。**赤くなったら再構成が間違っている**と読むこと。
テストを再構成に合わせて直さない。どうしても仕様上直す必要があるなら、
直す前に ClaudeCode に確認する。

### 5.3 判定は observable な renderer 状態

§1.85-A の §4.1 と同じ。`texture_size` / `texture(id).is_none()` が主判定。
「validation error が出ない」は overflow guard が範囲外 partial を skip するため
**判定として成立しない**。

## 6. 凍結ルール対応 (必須)

本件は `paint` 経路に触れるため、CLAUDE.md「Detached viewer リワーク中のルール」と
[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。

- 着手前に §2 を読むこと。
- 「これは症状パッチではなく構造的修正である」ことの ClaudeCode / Codex 双方の合意は
  **§2.1 の条件付きで取得済み** (2026-08-16)。条件を外れた実装 (exit 3/4 への
  `free` ループのコピー) は合意対象外なので、その形になりそうなら手を止めて報告する。
- 完了時に `docs/detached-rework-plan.md` §11 (リワーク外からの変更記録) へ追記する。
  触れた範囲・判断理由・症状パッチでない理由を書く。

## 7. やらないこと

- exit 3/4 へ `free` ループをコピーして終わりにしない (§2.1)。
- §1.31 の frame drop / render scheduler / wndproc 即 return に手を出さない。
  本件は**契約を作るところまで**。契約を使うのは §1.31。
- `src/` 側の既存回避策を撤去しない (§1.85-A の §7 と同じ)。
- 時間窓 (debounce / grace / settle ms) で競合を吸収しない (憲法 5)。

## 8. 完了条件

1. `set` / `free` の適用が単一の transaction owner (begin / finalize) に集約されている。
   `free` の適用箇所が exit ごとに散っていない。
2. `PaintOutcome` 相当の typed outcome があり、`RenderStateAbsent` が明示的な例外
   variant として存在する。
3. 成功経路で `free` が `queue.submit` の後に来ていること (§2 の制約)。
   submit しない経路では command buffer 破棄後であること。
4. §5.1 の outcome ごとの配送テストが通る。
5. §1.85-A のテストが**無修正で**通る (§5.2)。
6. `scripts/test-full.ps1` から全テストが実行され、出力にテスト名が出る。
7. `cargo fmt --check` が通る。
8. `docs/detached-rework-plan.md` §11 に記録がある (§6)。
9. `docs/next-release-backlog.md` §1.86 を完了に更新し、§1.31 の前提が満たされた旨を
   §1.31 側にも 1 行追記する。
