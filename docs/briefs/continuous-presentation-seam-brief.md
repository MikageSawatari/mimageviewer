# 連結表示を通過表示の提示継ぎ目へ接続する

## 直す不具合

フォルダ移動 (Ctrl+↓) 連打で、ビューアが「読込中」のまま**永久に固着**する。165fps で
空転し、**再起動以外に復帰手段がない**。利用者が 4 回再現。出荷阻止級。

## 確定した原因 (推測ではない。ログとコードで裏取り済み)

```
[22.4s] UI uploads deferred for 2048 consecutive frames: fs_idx=0
        passthrough_ready=true items_generation=60
        passthrough idx1:ok@2046f  idx0:ok@1f
```

`page_turn_decision` は idx 0 と 1 の両方を対象にし続ける (見開き表示ユニット)。
`fs/paint` / `fs/page_turn_ready` / `fs/texture_choice` はすべて停止。

**連結表示レンダラーが提示継ぎ目に繋がっていない。**

| 事実 | 場所 |
| --- | --- |
| 連結表示は `draw_fs_spread` を迂回する | `src/ui_fullscreen.rs:12902` |
| `draw_fs_continuous_reading` は戻り値 `()` — 何を描いたか報告できない | `src/ui_fullscreen.rs:24273` |
| 見開き時の `navigator_texture_sources` は空で初期化され、連結描画は埋めない | `src/ui_fullscreen.rs:12886` |
| `fs_display_unit_trace_pages` は source が欠けると `None` → emit が即 return | `src/ui_fullscreen.rs:14599` |
| `observe_fs_navigation_sequence_presented` は描画 idx 集合が `target.pages` と**完全一致**したときだけ retire | `src/ui_fullscreen.rs:14838` |
| `draw_fs_spread` は PassThrough 時に左右両方の rendition を明示取得する (連結側は未実施) | `src/ui_fullscreen.rs:26505` |

循環: sequence が `Ready(Rendition)` → `defer_ui_uploads=true` → 連結描画が提示を報告しない
→ sequence が retire しない → 遅延が解けない → 永久。key release では解けない
(`page_turn_burst_active` は probe にしか渡っていない、`src/ui_fullscreen.rs:7948`)。

**フル解像度ロードは提示に不要**。`rendition_ready` は target 全ページの
`ensure_passthrough_rendition(...).is_some()` (`src/ui_fullscreen.rs:5343`) なので、
`passthrough_ready=true` は左右の rendition が既にある証拠。

## 回復すべき不変条件

> `Ready(presentation)` にした target unit は、現在選ばれている renderer が必ずその
> presentation source を消費し、**実際に描いた完全な page set** を sequence owner へ返せること。

## 実装 (4 ステップ)

1. `draw_fs_continuous_reading` に、そのフレームで確定済みの `FsPageTurnDecision` を渡す。
2. `PassThrough` かつ navigation target に属するページでは、**target 全ページの rendition を
   all-or-none で解決**して描画 source にする。ページごとの raw/final fallback と混ぜない
   (混ぜると「片方だけ本番画質」になり通過表示の意味が壊れる)。
3. `draw_fs_continuous_reading` が、**実際に draw command を作れた** `(usize, FullscreenPaintResource)`
   を返す (`FsNavigatorTextureSources` を再利用してよい)。
4. その戻り値を、現在の空の `navigator_texture_sources` の代わりに
   `emit_fs_page_turn_ready_for_display_unit` へ渡す。

**decode / GPU upload / AI / final effect の defer は一切変更しない。** rendition は resolver が
作成済みなので描画側は原則 cache hit。

## やってはいけないこと (このリポジトリの規約 = CLAUDE.md「バグ修正の一般原則」)

- タイムアウト / watchdog / retry / 「詰まったら強制解除」guard — すべて症状パッチ
- `defer_ui_uploads` の対象から full-resolution load を外す — 別 gate (upload / final effect)
  でも止まるので不完全、かつ「軽い rendition で通過表示する」設計を迂回して
  ページ送りを重くし直す
- 連結表示で `accept_rendition=false` にして full materialization を待つ — AI / final effect
  完了まで Ctrl+↓ をブロックし得るので、通過表示の目的に反する

## 注意 (レビューで挙がったリスク)

- 連結表示は**複数 unit が可視になり得る**。提示と数えるのは
  **target pages が実際に draw された場合だけ**。cache にある / layout に含まれる /
  近傍が描かれた、では retire させないこと。
- `docs/display-pipeline.md:1923` は「縦・横連結は通過表示の対象外」としている。
  cross-folder navigation sequence だけ例外にするなら、**同じコミットで文言を更新**すること。
- `FsNavigatorTextureSources` は navigator 用と提示トレース用を兼ねている。最小変更では
  再利用してよいが、兼務している旨をコメントに残すこと。

## 必須の回帰テスト

```
continuous_navigation_sequence_retires_after_complete_target_rendition_is_drawn
```

状態遷移テスト (統合テストではなく)。横連結 + 見開き、target pages `[0, 1]`、
sequence は `Ready(Rendition)`、左右の thumbnail pixels / rendition は ready、
`fs_cache` / full upload / final composite は**未完成**の状態で:

- 1 枚だけ描けた時点では sequence 継続 (block したまま)
- 2 枚の実描画 source が揃った時点で sequence が消える
- 次の decision で `defer_ui_uploads=false`

**現行コードでは連結 renderer が source set を返さないので、このテストは落ちること**を
実装前に確認すること (落ちないなら、テストが継ぎ目を通っていない)。

既存の `navigation_sequence_requires_the_complete_atomic_target_unit`
(`src/ui_fullscreen.rs:33270`) は「2 枚必要」を証明済みだが、連結 renderer と sequence の
**接続を通らない**ので今回の回帰テストにはならない。

## 完了条件

- `cargo fmt` 済み
- `cargo test -p mimageviewer --lib` が全緑 (現在 5664 件)
- 上記の回帰テストが追加され、修正前は落ち、修正後は通ること
- ドキュメント (`docs/display-pipeline.md`) の整合
