# 検収所見 #9: stale hwnd によるクリック棄却 + resync pending がテクスチャ供給を止める

正本プラン: [detached-rework-plan.md](detached-rework-plan.md) /
前提: A1v4 (0f57c361) の棄却診断が機能し、残る取りこぼしの理由が記録された。
証拠ログ = bug-20260707-0838-a1v4.log、動画 = 2026-07-07 08-38-26.mkv (左上に時計)。

## B1: parked 窓の registry hwnd が silent に stale 化し、クリックが棄却される

### 証拠

```
4.128s  registered host hwnd=0xfa13d6 window_id=2 (以後 id=2 の登録更新なし)
67.361s deferred_activate_watcher_rejected reason=down_window_from_point_mismatch
        foreground=0x3291b28 cursor_root=0x3291b28 target_id=2 target_hwnd=0xfa13d6
98.260s (同上 — 30 秒後も stale のまま)
```

- ユーザーがクリックした実窓 (cursor_root=foreground=0x3291b28) と registry の
  記録 (0xfa13d6) が不一致 → 棄却。**id=2 の OS 窓は 4.1s〜67s の間に再生成された
  のに、registry が更新されていない**。動画 08:38:47.717 の「右窓が操作不能」の正体。
- deferred (Parked) 窓には hwnd の生存再チェックが無く、registry が非ゼロのため
  未請求採用も走らない = 恒久 stale。

### 修正要件

1. **生存監視**: passive 窓の registry hwnd を root frame で `IsWindow` チェック
   (安価)。死んでいたら clear + 既存の未請求採用経路で再取得 (deferred の
   FirstCallback / 次 callback 契機)。
2. **watcher の自己修復**: down 棄却時に cursor_root が「egui クラスかつ registry
   未請求」の窓なら、棄却ではなく採用候補として App へ通知してよい (消去法の
   範囲内。geometry 推定ではない)。
3. **再生成の発生源を特定して塞ぐ**: OS 窓が silent に再生成される契機を調査する。
   有力候補 = **active→Parked (immediate→deferred) の park 方向のクラス替え**で
   「どちらにも登録されないフレーム」が挟まるケース (R2d fix1 は deferred→immediate
   の復帰方向だけを塞いだ。**park 方向に同じ保証が無い**)。fix1 と対称の
   「park commit と deferred 登録を同一 root pass 内で連続させる」修正を入れ、
   HWND が park をまたいで不変であることをテスト/ログで固定する。

## B2: resync pending の長期滞留が全テクスチャアップロードを止める (サムネ不表示・全体の遅さ)

### 証拠

- `pass_probe main_pending=true` が **28.4s〜131.0s の全域**で継続 (safe-frame 待ちの
  まま解放されていない)。
- [app.rs:20035](../src/app.rs): `defer_texture_uploads = main_font_atlas_resync_pending`
  — pending の間、**サムネイル等のテクスチャアップロードが keep_range 内でも
  スキップされ続ける**。
- 帰結: PDF サムネイルが延々表示されない (動画 08:38:34)・各窓の画像表示が遅い・
  SLOW FRAME 1157 件。**PDF ワーカーの詰まりではない** (backlog は 1〜2 で健全、
  stale prune も動作)。

### 修正要件

1. **デカップリング (本命)**: `defer_texture_uploads` は「resync が実際に発火する
   フレーム〜repeat 完了まで」だけ true にする (v1.8.0 対策の本来の意図)。
   safe-frame 待ちで pending が保持されている間は通常アップロードを許可する
   (この期間は set_fonts をまだ呼んでおらず、atlas は無傷なので安全)。
2. **滞留自体の調査**: 100 秒 settled にならなかった理由を特定する。pass_probe に
   **unsafe 判定の内訳 (placement_pending / cloak / opening=N / closing=N)** を追加し、
   どの条件が立ちっぱなしかを可視化。スタック気味のフラグ (例: pending_detached_video_host_resync
   の残留) があれば是正。多窓churn で真に settled が稀なら、resync の必要条件を
   「main viewport が確実に描けるフレーム」に絞れないか再検討 (Opening/Closing の
   有無は本来 main の描画成功とは独立のはず — 過剰条件の可能性)。
3. 計装コスト: `active_input_probe` ×2/frame 等の常時ログを 60 フレーム間引きに
   (F10 と同様)。SLOW FRAME への寄与を減らす。

## 完了条件

- [ ] B1: park をまたいで hwnd 不変のテスト + 生存監視 + 棄却時自己修復。
      コミット `(detached-rework findings-9 B1)`
- [ ] B2: デカップリング + unsafe 内訳ログ + 滞留原因の報告。
      コミット `(detached-rework findings-9 B2)` (B1 と別コミット)
- [ ] 既存テスト + full test 緑、`.\scripts\build-release.ps1`

## 実機確認 (次回)

1. 未フォーカス窓への素早い初回クリック連打 → 全て 1 回で復帰 (B1)
2. PDF 多窓でサムネイルが出続ける・表示が待たされない (B2)
3. 棄却診断 `watcher_rejected` に down_window_from_point_mismatch が出ない
