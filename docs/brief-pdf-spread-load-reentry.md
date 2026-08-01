# ブリーフ: 見開き PDF の片ページが「読み込み中」で完走しない再入ループ

実装 = Codex Sol / レビュー = ClaudeCode。v2.9.1 出荷前。**優先度 P1。**

正本は [next-release-backlog.md](next-release-backlog.md) の **§1.29**。着手前にそちらを読むこと。
本ブリーフは実装時の制約と進め方を足したもの。

## 1. 症状と証跡

利用者報告 (2026-08-01 実機): 見開き表示でファイルを開いたところ、**片ページが 10 秒以上
「読み込み中」のまま**。補正スライダーを動かしたら表示された。

ログ (`%APPDATA%\mimageviewer\logs\mimageviewer.log`、`open_fullscreen: idx=19` 直後から
約 50 秒で 23,000 行弱):

- idx=19 — 15,240 回 `Retained page final-ai available idx=19 reason=start_fs_load_skip_pdf_render`
- idx=20 — 7,576 回 `Retained page miss idx=20 ... target_px=2376 entries=10
  reason=start_fs_load/resolution_mismatch` + 毎回の `fs pdf render cancelled/interrupted Page 21`
- 同区間で `[SLOW FRAME]` 39 回

idx=19 と 20 は同じ見開きの左右ページ。**リリース済み v2.9.0 から存在する** (導入は `7c3a9363`)。

スライダーで解けた理由: パラメータが変わると retained の final-AI と raster が現在パラメータに
一致しなくなり、早期 return と解像度判定の両方が外れて通常のレンダ経路に入るため。

## 2. 分かっていること / 確定させること

### 2-1. 確定済み (idx=19 側)

呼び出し側の再入ガードは [ui_fullscreen.rs](../src/ui_fullscreen.rs) の

```rust
if !self.fs_cache.contains_key(&idx) && !self.fs_pending.contains_key(&idx) {
    self.start_fs_load(idx);
}
```

[app.rs](../src/app.rs) の `start_fs_load` にある
`has_retained_pdf_final_ai_for_current_params(idx)` の早期 return は、**`fs_pending` へ登録せずに
return する**。よって cache にも pending にも入らず、ガードが毎フレーム通る。

「retained にあるので描画不要」と「ロードが完了した」が同じ状態で表現できていないのが誤り。

### 2-2. 着手時に確定させること (idx=20 側)

通常経路へ落ちるはずの idx=20 が、なぜ毎フレーム `fs_pending` から消えて再入するのか。
**先にこれを特定してから修正に入ること。** 候補:

1. `pdf_target_changed` → `update_prefetch_window` 経路が pending を落としている。
2. retained store が `entries=10 / max_entries=10` で満杯で、見開きの 2 ページが相互に evict し
   合っている (同セッションに `Retained page evict` 21 行)。見開きは 2 ページが同じ store を
   同時に奪い合うので、単ページでは再現しない可能性がある。

特定できたら、その根拠 (ログ / テスト / source inspection のどれか) を
[next-release-backlog.md](next-release-backlog.md) §1.29 の「未確定」節を置き換える形で記録する。
**推測のまま修正に入らないこと。**

## 3. 制約

**3-1. 症状パッチにしない。** 次はいずれも根本原因に対応しないので採用しない:

- 解像度一致判定 (`0.9..=1.1`) の閾値を緩める
- レンダのキャンセルをやめる / 遅延させる
- 再入を時間 (デバウンス / グレース) で抑制する
- 早期 return の直前に「ログを出さない」等の抑制を足す

**3-2. 集約する方向で直す。** 現在「このページは表示可能か」に対して `fs_cache` /
`fs_pending` / retained store の **3 つが別々に答えている**。producer (retained store、render 完了)
と consumer (表示、再入ガード) が同じ 1 つの状態を見る形へ寄せる。CLAUDE.md
「バグ修正の一般原則」の「相互排他的な状態を複数の bool / Option で表現している場合は単一の
typed state owner へ集約」に該当する。

正しい構造修正が現在の範囲を超えると判断した場合は、**症状パッチを入れずに報告すること**。

**3-3. 見開き固有にしない。** 単ページでも同じ早期 return は通る。見開きは 2 ページが同じ
store を奪い合うことで露見しやすいだけなので、「見開きのときだけ」の分岐を足さない。

**3-4. UI スレッドの同期 I/O を増やさない。** [ui-responsiveness.md](ui-responsiveness.md) §4 の
チェックリストを通すこと。

## 4. テストで縛ること

- 見開き表示で、**操作なしで両ページとも表示に到達する** (待つだけで完走する)
- `start_fs_load` が同一 idx / 同一パラメータで毎フレーム再入しない (状態遷移テスト)
- retained store が満杯のとき、見開きの 2 ページが相互に evict し合わない
- 早期 return 経路 (retained final-AI あり) でも、呼び出し側の再入ガードが
  「もう呼ばなくてよい」と判定できる
- 単ページでも早期 return 後に再入しない (3-3 の担保)

## 5. 検証

```
cargo fmt --all
cargo test -p mimageviewer --lib
cargo test -p mimageviewer --test ui_snapshot
python scripts/check_ui_glyphs.py
```

実機確認はレビュー側が依頼する。確認シナリオは「AI 補正を有効にした PDF を見開きで開き、
何も操作せずに両ページが表示されること」と、そのときログに
`Retained page final-ai available` / `resolution_mismatch` が数千行出ないこと。

## 6. スコープ外

- [final-composite-budget-thrash-plan.md](final-composite-budget-thrash-plan.md) 本体
  (連結読みのカラー化 texel 予算)。同じ系統なので考え方は参照してよいが、今回の対象は
  PDF retained page ストア
- backlog §1.28 のカーソル auto-hide (Stage 5)
- native video window の Stage 5 / 6
