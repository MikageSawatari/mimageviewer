# Claude エージェント監査: detached 中の selection 系操作の誤爆 (2026-07-09 深夜)

依頼: findings-19 のタグ誤爆 (selection_target_indices の fullscreen_idx 無条件優先) と
同族の対象解決を持つメタ操作・ファイル操作の横断監査。

## 結論

- **P1 (削除等のデータ破壊系の誤爆) = 0 件**。削除・リネーム・コピー等は selection /
  クリック idx ベースで健全。
- P2×3 / P3×5。3 グループに集約:
  1. **グローバル入力のコンテキスト解決** (`current_ring_shortcut_context` /
     `apply_mouse_button`): ゲームパッド X リング・マウス割当ボタンが fullscreen_idx
     優先で、detached 中にグリッド操作のつもりの入力が detached アイテムへ (P2×2)
  2. **Viewer 面からの container 系操作が main のナビ状態を対象化**
     (`set_current_folder_rating` / `toggle_folder_pin_for_idx`): detached 窓内の
     Shift+F1-F6 / P が main の現在フォルダに書かれる (P2×1 + P3×1)
  3. **旧述語 `fullscreen_idx.is_some()` の残存**: handle_delete_key (グリッド Delete が
     detached 中に無効 = **修正済み**) / current_shortcut_help_context /
     poll_stack_script (P3×3) + リングガイド表示 (P3)

## 対応

- handle_delete_key は同夜修正 (viewer_session_blocks_main_window へ置換)。
- 残りは docs/next-release-backlog.md §1.7 (新設) に BA 報告として記録し、
  detached リワークの後続ステージで設計対応する (グローバル入力の面解決は
  ActionSurface と同じ発火面設計が必要なため、出荷直前のパッチにしない)。

## 詳細

原文はセッション記録参照。要点: X リング ItemRating → detached アイテムに★ /
中クリック=回転割当 → detached 画像が回転 (回転 DB 永続化) / detached 窓内
Shift+F3 → main 表示中フォルダに★3 / detached 中はパッドのグリッド操作不能 /
スタック集約が detached 窓存在中は永久保留 / FsPin は先祖フォルダ側に pin。
確認済みで問題なし: グリッド F1-F6 レーティング・削除 (キー gate 除く)・リネーム・
コピー/切り取り・D&D・お気に入り・本へ追加・回転・代表サムネ・比較ピン・
消しゴム/補正スロット・グリッド右ドラッグリング (開始時に context 焼き付け済み)。
