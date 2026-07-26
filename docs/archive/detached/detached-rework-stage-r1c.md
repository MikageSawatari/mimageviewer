# Stage R1c 指示書: font atlas resync の discard パスから passive 窓を守る + PDF 見開きの frozen 化

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 位置付け: ゲート B 再実施 (2026-07-05 深夜) で見つかった 2 件。R1/R1b の成果
  (registry・クリック限定・即時回復) は**この実機ログで正しく動作している**ことを
  確認済み。今回の 2 件はどちらも R1 系の退行ではない。
- 実装: Codex / 検収: Fable / 実機 smoke: ユーザー (ゲート B 再々実施)

## 1. 修正 1: 未 gate の font atlas resync が passive 窓を殺す (動画 F12 で窓が出て消える)

### 実機ログで確定した因果 (2026-07-05 23:5x)

```
68.368 session_finish window_id=3 reason=video_presentation_switched_non_detached (F12 detached→main)
68.390 [ui-fonts] schedule main font atlas resync: native_video_backdrop_hide   ← 即時 resync (未 gate)
69.349 [ui-fonts] discard pass for font atlas resync (same-frame repass): generation=1
69.406 discard pass ... generation=2
69.570 passive_event id=2 focus_edge=true (勝手にフォーカス)
69.570 passive_placement_update_rejected_default id=2 (既定サイズで再生成された証拠)
69.731 detached_hwnd_dead window_id=2 → 69.816 registered (新 hwnd)   ← OS 窓が作り直された
70.227 detached_hwnd_dead window_id=2 → 70.333 registered (また別の新 hwnd) ← 2 回目
```

resync の **discard パス (same-frame repass)** が走ったフレームでは、そのパスで描いた
immediate viewport (passive 窓) が egui に「描かれなかった」扱いになり、**OS 窓が
破棄→次フレームで再生成**される。resync は複数世代繰り返すので、passive 窓が
波状に消えて生え直し、フォーカスも暴れる。これが「窓が色々出て消えたり」の正体。

これはピン再設計時 (2026-06-29) に **既知の P2 残課題**として記録されていたもの:
detached cleanup 系 3 経路の resync は `pending_detached_cleanup_font_atlas_resync` +
`detached_cleanup_font_atlas_resync_is_safe()` (= detached 完全 idle まで遅延) で
gate 済みだが、**それ以外の即時 resync (`native_video_backdrop_hide` /
`fullscreen_viewport_cleanup` / `fullscreen_viewport_recreate` 等) が未 gate** のまま
残っていた。裏の病根は BA-5 (immediate viewport は親が毎フレーム描かないと死ぬ)。

### 修正内容

1. `schedule main font atlas resync` の**全発火点を列挙**する
   (grep: `schedule main font atlas resync` / resync reason 文字列)。
2. 各 reason について「何を直すための resync か」を git log / コメントから確認し、
   **detached viewport (active または passive) が 1 つでも生きている間は既存の
   deferred 機構に合流させる** (= `detached_cleanup_font_atlas_resync_is_safe()` が
   真になるまで遅延)。専用フラグを増やさず、既存の pending/flush 機構へ
   reason 付きで一本化するのが望ましい (憲法 3 の精神)。
3. **遅延が許容できない reason が見つかった場合は手を止めて報告する** (憲法 6)。
   「メイン窓の表示が detached を閉じるまで壊れたままになる」ような reason は
   遅延ではなく別の設計が要るため、Fable の判断を仰ぐ。
4. 回帰テスト:
   - passive 窓が存在する間に `native_video_backdrop_hide` 系 resync を要求
     → 即時実行されず deferred になる
   - detached が完全 idle になったフレームで flush される
   - 既存の deferred 3 経路のテスト
     (`detached_cleanup_font_resync_waits_until_outer_detached_idle`) は緑を維持

### 記録事項 (実装ではなくプランへの反映)

- これは **BA-5 の 2 度目の実害** (1 度目 = Y-32 font atlas panic)。R4 (deferred
  viewport 化) の実施判断 (ゲート C) で本件を重み付けする。3 度目が出たら
  「passive 窓だけ先行して `show_viewport_deferred` 化」を独立ステージとして
  繰り上げる。
- 今回 `passive_placement_update_rejected_default` (Phase B の既定サイズ拒否
  ヒューリスティック) が再生成窓の placement 汚染を**正しく防いだ**。R2 で
  このヒューリスティックを削除する計画だったが、**窓の意図しない再生成 (BA-5) が
  根治するまで削除しない**方針に変更する (R2 指示書に反映予定)。

## 2. 修正 2: PDF 見開きが passive 化で単ページ表示になる

### 原因 (コード確認済み・R1 系の退行ではない)

`build_active_detached_image_window_snapshot` ([app.rs:24774](../src/app.rs)) は
表示テクスチャを `resolve_fs_display_tex(idx, true)` の **1 枚**で凍結する。
縦連続モードには `detached_continuous_frozen_pages_for_snapshot` (`frozen_continuous_pages`)
という複数ページ凍結機構があるが、**見開き (spread) モードは未対応**で、pause 時に
現在ページ 1 枚のテクスチャだけが snapshot に入る → passive 描画が単ページになる。
detached 実装当初からの積み残しで、passive 窓が正しく生き残るようになったことで
目に見えるようになった。

### 修正内容

- 見開きモードで pause するときは、**見開きを構成する 2 ページ分のテクスチャと
  レイアウト (左右順・間隔・サイズ)** を snapshot に凍結し、passive 描画で
  再現する。実装は既存の `frozen_continuous_pages` パターンの流用を第一候補と
  する (新しい凍結機構を発明しない)。
- 再デコード・再レンダリングはしない (pause 時点で GPU に載っている
  テクスチャを保持するだけ)。見開きの片側がまだデコード中などで揃わない場合は
  従来どおり 1 枚凍結にフォールバックし、その旨をログに残す。
- 回帰テスト: 見開き状態の pause で snapshot に 2 ページ分の凍結情報が入ること、
  単ページ状態では従来どおりであること。
- 参照ドキュメント: [docs/display-pipeline.md](../../display-pipeline.md) (見開きの描画合成)。
  snapshot の仕様に触れる記述があれば同時更新する。

## 3. 完了条件

- [ ] 未 gate resync が 0 件 (発火点列挙と gate 有無の対応表を完了報告に貼る)
- [ ] passive 窓存在中の resync 遅延テスト + 見開き凍結テストが存在して緑
- [ ] `cargo fmt --check` / `cargo test --bin mimageviewer-core` / `cargo test` (フル) 緑
- [ ] `.\scripts\build-release.ps1` で実機検証バイナリを用意
- [ ] 完了報告に: resync reason ごとの遅延可否の判断根拠、見開き凍結の
      フォールバック条件

## 4. ゲート B 再々実施 (実機 smoke、ユーザー)

前回の 5 操作 + 追加観点:

- **動画 F12 往復で passive 窓が消えたり生え直したりしない** (今回の直接目標)
- PDF 見開き状態の窓を passive 化 → **見開きのまま** frozen 表示される
- passive 化→クリック復帰の往復で表示が崩れない
- 左右振動・アクティブ暴れ・小窓・panic の再発なし
