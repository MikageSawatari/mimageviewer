# Claude エージェント独立再レビュー: 深夜バッチ diff (2026-07-09)

対象: 未コミット差分 (クリック照準 / ActionSurface / Image 復帰 / §1.7)。
Codex 2 周とは別角度 (借用・状態機械・abort 対称性・テスト妥当性)。

## 指摘と対応

- **[P2] Image 復帰フォールバック×★固定 (Snapshot Lock)**: ★固定中は load_folder が
  範囲外 return して窓が無言消失。★固定は items を差し替える機能なので stamp 失敗と
  相関が高い → **同夜修正**: descriptor activation の事前チェックに snapshot 範囲判定を
  追加 (範囲外なら parked のまま + トースト)。親フォルダの画像 0 枚コーナーは
  受容 (空 bundle は次フレームの should_drop で畳まれ、アプリは安定)。
- **[P3] Viewer 面トーストの repaint 停止** → **同夜修正**: draw の repaint 要求を
  面ゲートより前に移動 (期限切れ掃除がグリッド側 draw で回り続ける)。
- **[P3] doc コメント 2 箇所の実装乖離** → **同夜修正** (selection_target_indices /
  detached_activation_target_for_cursor_root)。
- **[P3] contains_video が音声も含む暗黙知** → **同夜修正** (park 分岐にコメント)。
- **[P3] 発火面トーストの不可視エッジ** (完了前に面のビューアを閉じる):
  トレードオフとして受容、バックログ §1.7 (BA 報告) に記録。
- **[P3] Viewer 面が main fullscreen と active detached を区別しない** (ビューア×
  ビューアの二重表示): 2 値 enum の粒度の限界としてバックログ §1.7 に記録。

## 確認済み (問題なし)

クリック照準の全経路対称性 / passive activation の全 abort 分岐の Parked+insert 対 /
Image arm のフラグ消費と window_id 再利用 / §1.7 述語棚卸し (直読み残存 9 箇所は
still/多窓/編集/VST 専用で正当) / 設定の後方互換・overwrite 非対象 / トースト
設定・クリア全経路 / タグ発火面の呼び出し元全数 / cfg 整合 / テスト 7 本の契約妥当性。

原文はセッション記録参照。
