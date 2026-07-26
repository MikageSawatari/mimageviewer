# Stage R2b 検収所見 #3: legacy live-park がメイン文脈を持ち逃げする + 復帰時に OS 窓が作り直される

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md) /
既往所見: [findings-1](detached-rework-stage-r2b-findings-1.md) /
[findings-2](detached-rework-stage-r2b-findings-2.md)

fix2 (bfc6f530) で F5 (クリック復帰) は解消を実機確認済み。新たに 2 件。
**F6 が重大** (メインウィンドウの状態破壊)。どちらも headless テストで再現・固定
可能な見込みなので、実機検証なしで修正を進める。

## F6 (重大): linked (legacy) メディア窓の live-park がメイン文脈を持ち逃げし、close で喪失する

### 実機ログの証拠 (2026-07-06 08:4x)

```
18.001 === load_folder: c:\home\youtube\movie\youtube ===          ← メインで動画フォルダを開いた
18.961 allocate_window_id id=4 (linked=true)                        ← 動画を detached で開く (legacy=メイン文脈共有)
22.014 state_transition id=4 →ParkedLive reason=park_legacy_live_media   ← 別窓クリックで live-park
24.444 ParkedLive→Resuming reason=parked_live_activate_commit            ← クリック復帰
26.863 →ParkedLive reason=park_active_context_live_media                 ← ★2 回目の park は reason が
                                                                            active_context (窓の素性が変わった)
30.940 id=4 Closing→Removed reason=handle_fullscreen_close_request       ← 動画窓を閉じた
(以後 load_folder なし。全窓 close 後、メインはフォルダ空欄・一覧なし)
```

### 根因の見立て

id=4 は **linked (legacy) = メインの ViewerContextBundle をそのまま使う**動画窓。
その live-park が、**メイン文脈のbundle (items / current_folder / fs_cache) ごと
paused_bundle として snapshot に退避**してしまい、メイン側は空の bundle が残る。
1 回目の復帰で窓は「active context 窓」に化け (2 回目の park reason が
`park_active_context_live_media` に変わっているのが証拠)、**メイン由来の bundle を
抱えたままその窓を close した時点で items / フォルダ状態が drop され、メインが
空欄になる**。

### 修正要件

- **linked (legacy) メディア窓を live-park するときは、メイン文脈を奪わない**。
  方式は Codex に委ねるが、第一候補はピン昇格 (`promote_active_still_to_independent`)
  と同じ「clone 可能フィールドの複製で独自 bundle 化 → メインは fullscreen_idx
  クリアのみ」のパターンを動画に適用すること (実績のある機構の流用)。
- live-park 後にその窓を close しても、メインの items / current_folder / grid が
  無傷であることをテストで固定する:
  - `linked 動画 → live-park → 窓 close → メイン items 非空 + current_folder 不変`
  - `linked 動画 → live-park → 復帰 → close → 同上`
- park 前から independent な (active context) メディア窓の live-park は現行どおり。

## F7: ParkedLive → 復帰のたびに OS 窓が破棄→再生成される (一瞬消えて再表示)

### 実機ログの証拠

復帰 (parked_live_activate_commit) のたびに同じ viewport "866D" の HWND が変わる:

```
18.984 registered host hwnd=0x4423fc  (gen 4)   ← 初回生成
24.566 registered host hwnd=0x13911ee (gen 5)   ← 復帰 1 回目で別 HWND = OS 窓再生成
27.801 registered host hwnd=0x17421f0 (gen 6)   ← 復帰 2 回目でまた別 HWND
```

### 根因の見立てと修正要件

fix2 の「復帰要求を queue に積み、bundle を snapshot に戻した後で処理」の過程で、
**passive 描画が止まってから active 描画が始まるまでに、その viewport を誰も
描かないフレームが挟まっている**疑いが濃い (immediate viewport は 1 フレーム
描かれないと egui が OS 窓を破棄する = BA-5)。builder 差分 (active/passive で
title / decorations 等が異なると egui が窓を作り直す) の可能性もあるので、両方
確認すること。

- 修正の不変条件: **復帰をまたいで HWND が変わらない** (registered host が復帰で
  増えない)。
- 遷移フレームの描画責務を明確にする: snapshot を外すフレームでは active 側が
  同一フレーム内で必ず描く (または描画主が切り替わるまで passive 側が描き続ける)。
- builder 差分が原因だった場合: active / passive で OS 窓属性 (decorations /
  transparent / taskbar) を完全に一致させる。
- テスト: 遷移シーケンスで「どのフレームも描画主が存在する」ことを単体で固定
  (headless で可能な範囲で)。最終確認は次回実機で「復帰時に窓が消えない」ことと
  ログの `registered host` が復帰で増えないことを見る。
- **報告事項**: 調査の結果が「gap フレームによる egui の窓破棄」だった場合、
  BA-5 の実害 3 件目に該当するため、その旨を完了報告に明記する (R4 繰り上げ判断の
  材料。プラン §4 R0 の記録事項参照)。

## 完了条件

- [ ] F6 の 2 テスト + F7 の遷移テストが存在して緑
- [ ] 既存 parked_live / still_window / detached テスト + full test 緑
- [ ] `cargo fmt --check` OK、コミットは `R2b fix3` を含める
- [ ] 完了報告に: F7 の根因確定 (gap フレーム or builder 差分 or 他)、
      BA-5 該当か否か
- [ ] `.\scripts\build-release.ps1` で実機バイナリ準備 (実機検証は後日まとめて)
