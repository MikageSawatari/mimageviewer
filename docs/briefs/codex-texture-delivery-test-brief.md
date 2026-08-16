# §1.85-A — vendored egui-wgpu のテクスチャ配送に回帰テストを入れる

対象: [next-release-backlog.md](../next-release-backlog.md) §1.85 の **前半のみ**。
後続 = [codex-delta-delivery-transaction-brief.md](codex-delta-delivery-transaction-brief.md) (§1.86)。

このブリーフは §1.31 (wndproc 内 GPU 待ちの構造修正) の第 1 段である。3 段の順序と
その根拠は本書 §0 に記す。**§1.86 / §1.31 に手を出さないこと。**

## 0. なぜこれを最初にやるか (順序の根拠)

順序は `§1.85-A → §1.86 → §1.31`。ClaudeCode と Codex Sol で合意済み (2026-08-16)。

§1.31 の設計は「acquire が間に合わなければそのフレームを捨てる」を**通常経路にする**。
ところが §1.86 のとおり、まさにその経路 (`get_current_texture` 失敗 → `RecreateSurface` /
`SkipFrame`) では `textures_delta.free` が実行されない。捨てるのが日常になれば本物の
リークになる。だから §1.31 の前に配送を堅くする。

その §1.86 は `set` を surface lookup の前へ移した**まさにその境界**を作り直す。
先に exit 2 (no-surface) の guard が無いと、再構成の途中で過去の `set` 消失 —— 黒サムネイル
(v1.8.0)、font atlas パニック 2 回 —— を再導入し得る。よって **guard が先、再構成が後**。

逆順にしたときの具体的な失敗:

- `§1.86 → §1.85`: 再構成中に exit 2 の `set` 配送を壊し、黒サムネ / font atlas crash を再導入する。
- `§1.31 → §1.86`: 通常化した frame drop ごとに `free` が落ち、GPU residency が増え続ける。

**本ブリーフのテストは exit 3/4 (`RecreateSurface` / `SkipFrame`) に到達しない。**
`surfaces` が空なら exit 2 で終わり、`get_current_texture` に届かない。`on_surface_error`
は発生したエラーを分類するだけで、エラーを注入できない。exit 3/4 の coverage は §1.86 で
typed outcome の seam を作ってから入れる。**本ブリーフでそこへ手を伸ばさないこと。**

## 1. 何を直すか

直すのではなく、**既に直っている境界に回帰テストを付ける**。

v3.0.0 出荷前の `ce6616ef` で、`Painter::paint_and_update_textures` が surface 無しの
viewport で早期 return し `textures_delta.set` を丸ごと捨てていた問題を直した。
テストは入れていない。同じ境界は過去 3 回、別の症状で表面化している:

1. サムネイルが純黒で固着 (v1.8.0 回帰)
2. font atlas `Y 29..44` パニック → `set_fonts` 最大 5 世代リトライで回避
3. font atlas `Y 45..126` パニック → **リトライ 5 世代を回りきって落ちた**

3 は「リトライ回数では防げない」ことの実証である。配送そのものを検査するテストが要る。

## 2. 触ってよいファイル

- `vendor/egui-wgpu/Cargo.toml` (dev-dependency と test target の宣言)
- `vendor/egui-wgpu/src/winit.rs` (`#[cfg(test)] mod tests` の追加のみ)
- `vendor/egui-wgpu/src/renderer.rs` (テストから読むための可視性調整が要る場合のみ。
  **挙動は変えない**)
- `scripts/test-full.ps1` (§5 のゲート組み込み)
- `docs/next-release-backlog.md` (§1.85 の状態更新)
- `docs/detached-rework-plan.md` (§11 への記録。§6 参照)

上記以外に触れないこと。特に `src/` 配下の回避策 (§7) は今回撤去しない。

## 3. テストの足場 — egui_kittest ではなく in-crate headless

`egui_kittest` (tests/ui_snapshot.rs) を足場にしない。理由:

- `ui_snapshot` テスト実行体には**既知の間欠 AV** がある (GL 隠しウィンドウ由来、
  ダンプが残らない)。配送の guard をその上に建てると、guard 自身が信用できなくなる。
- window / アプリ全体 / font fixture / snapshot rendering のいずれも本件には不要。

代わりに `vendor/egui-wgpu` の in-crate unit test にする。成立する根拠 (確認済み):

- `RenderState::create(config, instance, compatible_surface, options)` の
  `compatible_surface` は **`Option<&Surface>`**。`None` で surface 無しの device /
  renderer を作れる。
- `RenderState` の全フィールドが `pub`。
- `winit.rs` 内の `#[cfg(test)] mod tests` なら private な `Painter.render_state` /
  `Painter.surfaces` を直接扱える。
- Windows では backend を DX12 に限定して adapter を要求すること (`Backends::DX12`)。
  GL 経路を踏まないため、上記 AV を避けられる見込みが高い。

adapter が取れない環境ではテストを **skip ではなく成功扱いで早期 return** し、
その旨を `eprintln!` する。CI (ubuntu の `cargo check`) はこのテストを走らせない。

## 4. 入れるテスト

`Painter` を「`render_state` あり / `surfaces` 空」にして駆動する。

1. `Managed(0)` を 32px 高で seed する (full delta)。
2. **`surfaces` が空のまま** `paint_and_update_textures` に 128px 以上の full 置換を渡す。
3. 呼出し直後に `renderer.texture_size(&Managed(0))` が **128 以上**であること。
4. 同じ no-surface 状態で `pos=Some([_, 45])` / 高さ 81 の partial (= `y=45..126`) を渡す。
5. size が 128 のままで、`device.push_error_scope` / `pop_error_scope` が空であること。

さらに `free` 側:

6. 別の texture id を seed し、`surfaces` 空のまま `textures_delta.free` に載せて渡す。
   呼出し後に `renderer.texture(&id).is_none()` であること (現状の exit 2 は free を
   適用済みなので、これは今のコードで通る。§1.86 の再構成でも守られることの guard になる)。

### 4.1 判定は `texture_size` が主、validation error は従 ⚠️

**「validation error が出ない」だけを判定にしないこと。**
`Renderer::update_texture` には mIV の overflow guard がある
([renderer.rs](../../vendor/egui-wgpu/src/renderer.rs) の `report_overflow` の直後)。
範囲外の partial は wgpu へ渡す前に **skip されて `return`** する。したがって
full 置換が落ちていても partial は静かに捨てられ、validation error は出ない。
**テストが通ってしまう。**

主判定は必ず `renderer.texture_size(&id)` の observable な値にする。
validation error scope は補助にとどめる。

### 4.2 内部呼出し箇所を assert しない

「`apply_delta_set` が何回呼ばれたか」のような内部構造ではなく、**呼出し後の renderer の
観測可能な状態**だけを assert すること。§1.86 がこの関数を再構成するので、内部構造に
依存したテストは書き直しになる。テストは再構成を跨いで生き残らなければならない。

## 5. ゲートへの組み込み (これを忘れるとテストは存在するだけで走らない) ⚠️

`vendor/egui-wgpu` は **workspace から除外**されている
(`Cargo.toml` の `exclude = [..., "vendor/eframe", "vendor/egui-wgpu"]`)。
さらに vendor 側 manifest は `autotests = false`、`winit` は optional feature。

したがって `cargo test --workspace` では**絶対に走らない**。次を両方やること:

1. vendor 側 manifest に test target を明示宣言する (`autotests = false` のため
   `[[test]]` か `#[cfg(test)] mod tests` + lib target の扱いを確認して選ぶ)。
2. `scripts/test-full.ps1` に専用の `--manifest-path vendor/egui-wgpu/Cargo.toml`
   実行を 1 段追加し、`$LASTEXITCODE` を検査する。必要な feature を明示する。

完了報告には **`test-full.ps1` の出力に当該テスト名が現れている行**を貼ること。
「テストを追加した」だけでは完了条件を満たさない。

## 6. 凍結ルール対応 (必須)

本件は `paint` 経路に触れるため、CLAUDE.md「Detached viewer リワーク中のルール」と
[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。

- 着手前に §2 を読むこと。
- **「これは症状パッチではなく構造的修正である」ことの ClaudeCode / Codex 双方の合意は
  取得済み** (2026-08-16、Codex Sol の独立レビュー)。本ブリーフはテスト追加のみで
  production の挙動を変えないため、症状パッチの定義 (guard / delay / retry / 追加 repaint /
  一括 reset / silent fallback を根本原因の代わりに入れる) のいずれにも当たらない。
- 完了時に `docs/detached-rework-plan.md` §11 (リワーク外からの変更記録) へ追記すること。
  触れた範囲・判断理由・症状パッチでない理由を 1 行ずつ書く。

## 7. やらないこと

- `src/` 側の既存回避策 (`poll_thumbnails` の resync 窓中 upload 先送り、`set_fonts` の
  5 世代リトライ) を**撤去しない**。境界が直った今は保険であって正しさの担保ではないが、
  撤去は別レビューで行うと決めてある。今回同時に触らない。
- exit 3/4 (`RecreateSurface` / `SkipFrame`) の coverage に手を伸ばさない (§1.86 の仕事)。
- 実 driver の `get_current_texture` を故意に失敗させるテストを書かない。
  window destruction / minimize / device loss は不安定で、Recreate 時の surface 再 configure
  も絡む。§1.86 で typed outcome の seam を作ってから、そこを検査する。
- `paint_and_update_textures` の production 挙動を変えない。

## 8. 完了条件

1. 上記 §4 の 6 項目が in-crate unit test として存在し、通る。
2. `texture_size` が主判定になっている (§4.1)。内部呼出し回数の assert が無い (§4.2)。
3. `scripts/test-full.ps1` から当該テストが実際に実行され、出力にテスト名が出る (§5)。
4. `paint_and_update_textures` の production 挙動に差分が無い
   (`git diff` で `#[cfg(test)]` 外の挙動変更が無いこと)。
5. `cargo fmt --check` が通る。
6. `docs/detached-rework-plan.md` §11 に記録がある (§6)。
7. `docs/next-release-backlog.md` §1.85 に、前半完了と残り (exit 3/4 は §1.86) を追記。
