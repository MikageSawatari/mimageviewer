# 検収所見 #17: 動画 live-park でメイングリッドの自動比率が 1:1 にリセットされる

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
実機 (2026-07-08、smoke 中)。「動画を別窓で開いているとメインの比率 (自動) が
キャンセルされ 1:1 になる。別ディレクトリへ移動して戻ると直る」。

## 機構 (コードで確定、findings-12 D3 の取り漏らし)

- findings-12 D3 で `auto_aspect` を ViewerContextBundle に追加し swap 対象にした。
- しかし `clone_current_viewer_context_grid_fields_into` ([app.rs:27548](../src/app.rs))
  は items / scroll 等をクローンするが **auto_aspect を含まない** (ベース bundle は
  default = 1:1 のまま)。
- 動画 live-park の `preserve_main_context` 経路
  ([app.rs:27511-27515](../src/app.rs)) は:
  1. main 復元用 bundle を上記 clone で作る (**auto_aspect = default 1:1**)
  2. live context (正しい自動比率入り) を parked_bundle として動画窓へ take
  3. main 復元の swap で **live の auto_aspect が default 1:1 になる**
- `load_folder` は auto_aspect cache から再導出するので「別ディレクトリへ移動して
  戻ると直る」も一致。

## 修正要件

1. `clone_current_viewer_context_grid_fields_into` に
   `bundle.auto_aspect = self.auto_aspect.clone()` を追加する (grid の見た目を
   構成する状態なので clone 対象が正しい)。
2. 同関数を使う他の経路 (pin 系クローンの名残り等) も同時に恩恵を受けることを確認。
3. 回帰テスト: auto_aspect を非 default にした状態で動画 live-park
   (preserve_main_context=true) → main 復元後に auto_aspect が維持されている。
4. コミット `(detached-rework findings-17)`。

## 実機確認

1. 比率=自動のフォルダ (2:3 に定まるもの) で動画を別窓再生 → メイングリッドの
   セル比率が変わらない
2. park/復帰を数往復しても変わらない

## 実装メモ (Codex 2026-07-08)

- `AutoAspectState` を `Clone` 化し、`clone_current_viewer_context_grid_fields_into`
  で `auto_aspect` を main 復元 bundle へコピーするようにした。
- 回帰テスト
  `live_media_park_preserves_main_auto_aspect_state` を追加し、動画 live-park
  (preserve_main_context=true) 後も main 側の `auto_aspect` の samples /
  cache gate / streak が維持されることを固定した。
