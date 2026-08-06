# ブリーフ: ナビゲータの縮小画像がカラー化 / 補正の前の絵になっている (バックログ 1.53)

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/next-release-backlog.md](next-release-backlog.md) §1.53。**着手前に読むこと。**
表示パイプラインに触れるので [docs/display-pipeline.md](display-pipeline.md) も読むこと。

---

## 1. 症状

v2.12.0 出荷前の実機確認 (2026-08-06、利用者報告):
**「Alt のナビゲータが、カラー化がきく前の状態で表示されている」**。

カラー化に限らず、**補正・AI・注釈・隠蔽・消しゴムのいずれも反映されていない**。
ナビゲータだけが「元の絵」を出しているので、拡大位置の確認という用途では実害が小さいが、
表示中の絵と食い違うので誤解を招く。

## 2. 壊れている前提

`draw_fs_navigator` ([ui_fullscreen.rs:15286](../src/ui_fullscreen.rs)) が、キャンバス描画で
`self.thumbnails.get(page.main.page_idx)` の `ThumbnailState::Loaded { tex, .. }` =
**一覧用サムネイル**をそのまま貼っている (15422 付近)。

実際に画面へ出ている絵はこれとは**別物**で、`resolve_fs_processed_texture`
([ui_fullscreen.rs:3420](../src/ui_fullscreen.rs)) が返す加工済みテクスチャ。

## 3. 直し方の骨子

**新しいテクスチャ生成は要らない。配線だけ。**

現在ページの加工済みテクスチャは、描画側が**毎フレーム既に解決している**:

- 単ページ: `prepare_fullscreen_state` ([ui_fullscreen.rs:11099](../src/ui_fullscreen.rs)) が
  11103 行で解決し `FsFrameState.tex` に入れている
- 見開き: `draw_fs_spread` ([ui_fullscreen.rs:21203](../src/ui_fullscreen.rs)) が
  21264 / 21266 で左右ぶんを解決している

これを `draw_fs_navigator` ([ui_fullscreen.rs:10164](../src/ui_fullscreen.rs) の呼び出し) まで
**解決済みの handle として引き回す**のが本体。

### 使うテクスチャは `source_texture()`

`FullscreenPaintResource` ([gpu_lanczos.rs:142](../src/gpu_lanczos.rs)) には
`source_texture()` と `paint_texture_id()` があり、**この 2 つを取り違えると悪化する**:

| 取り方 | 中身 | ナビゲータでの可否 |
| --- | --- | --- |
| `source_texture()` | 拡大前の **full-image** テクスチャ | **これを使う** |
| `paint_texture_id()` (`Lanczos { output, .. }`) | §1.46 の GPU 拡大出力。**可視 source 領域だけ** | **不可**。ズーム中に切り取られた絵がナビゲータに出る |

## 4. 外すと悪化する点 (backlog の注意点。必ず守ること)

1. **`resolve_fs_processed_texture` をナビゲータから直接呼ばないこと。**
   これは `&mut self` で **worker を起こす producer**。ナビゲータから呼ぶと、
   見開きの相方ページの合成を余計に走らせる。**解決済みの handle を渡す**形にする
2. **見開きは 2 ページぶん必要。** `draw_fs_spread` が持っている handle を経由させる。
   ナビゲータの `layout.pages` は `page.main.page_idx` を持っているので、
   idx で引ける形にすること
3. **合成が間に合っていないフレームは、今までどおりサムネイルへフォールバックする。**
   黒くしない・描画を飛ばさない。ナビゲータが点滅するのが最悪の結果
4. パノラマ経路 (`draw_panorama_navigator`、`is_panorama_mode_active` で分岐) は
   equirect の全体図という別用途なので、**今回の対象外**。触らないこと

## 5. 補足: どこで受け渡すか

`draw_fs_navigator` の呼び出しは [ui_fullscreen.rs:10164](../src/ui_fullscreen.rs) の 1 箇所で、
同じスコープに `fs_nav_holdover_for_draw()` の `FsDisplayUnitHoldover` (各ページが
`texture: FullscreenPaintResource` を持つ、[app.rs:6029](../src/app.rs)) を扱うコードがある。

**同型の「idx → 解決済み `FullscreenPaintResource`」を、そのフレームの描画結果から
組み立てて渡す**のが素直なはず。ただし構造は Codex 側の判断でよい。条件は:

- ナビゲータが producer を呼ばない (§4-1)
- 単ページ / 見開きの両方で正しいページの絵が出る
- **holdover 表示中 (ページ遷移の途中)** にナビゲータが古い / 新しいで食い違わないこと。
  食い違いを避けられない構造なら、その旨を報告すること

## 6. 完了条件 / 回帰テスト

- カラー化中の画像で `Alt+N` (`FsNavigatorToggle`) → **ナビゲータもカラーになる**
- 補正レイヤー / AI アップスケール / 注釈 / 隠蔽 / 消しゴムでも同様に反映される
- 見開きで左右それぞれ正しく反映される
- ズーム中でも**全体図が出る** (拡大出力の切り取りが出ない = §3 の取り違えをしていない)
- 合成待ちのフレームでサムネイルにフォールバックし、点滅しない
- パノラマのナビゲータが従来どおり
- `cargo fmt` (引数なし) と `cargo test -p mimageviewer --lib` を通すこと
- UI スナップショットに差分が出るなら意図確認のうえ更新し、PNG を目視したことを報告する

規模 / 優先度: Small〜Medium / P3。新機能ではなく配線だが、表示パイプラインの
テクスチャ選択に触れるので、**確認は実機目視が中心**になる。

## 7. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が `build-dev.ps1` で用意する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **範囲を広げないこと。** ナビゲータの他の改善 (配置・サイズ・操作) は今回の対象外
- detached-rework 凍結ルールは有効

完了したら、変更内容・触れた範囲・テスト結果・**§5 の holdover 中の挙動をどう扱ったか**を
報告すること。
