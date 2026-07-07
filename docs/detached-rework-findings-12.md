# 検収所見 #12: close 時フラッシュの機構確定 (3 因子) + book open のメイングリッド反動

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
前提: findings-10 (4901ac4f) / findings-11 C1 (c12d721b) / C3 (0a795d64) 適用済み。
実機 (2026-07-07 12:0x、`MIV_DETACHED_WINDOW_DEBUG=1`、C3 のおかげでローテートなしの
完全ログ取得済み。Fable が凍結・解析済み)。症状: ①PDF を別窓で開く瞬間にメイン窓の
サムネがスクロールすることがある ②窓を閉じるとき多数の窓がフラッシュする。

C3 が初回から効いた (0〜155s 全量保持・証拠完全) ことを明記しておく。

## D1: close 時フラッシュ = font resync の early-return が deferred 登録を飢餓させ、parked 窓が既定サイズで再生成される

### 実機シーケンス (77.700〜78.594s、窓 9=active close、窓 2/3=Parked、窓 7=Parked)

```
77.700 窓 9 close_requested → active_close_finalize → state=Removed (Closing 非経由)
77.704 font resync 予約 (reason=detached_viewer_cleanup)。safety gate は
       opening=0/closing=0 で settled 判定 → 即発火経路へ
77.70-78.51 gen26-29 の resync 発火が数フレームに渡り繰り返される。各発火は
       discard repass (update_early, pass0) または no-budget defer (pre_main_ui)
78.506 frame7760 pass0: gen29 が update_early で発火 → discard + early return
       ⚠ update_early の early-return (app.rs:44851) は render_detached_image_windows
       (app.rs:45667) より前 = この pass は deferred 登録ゼロ
78.516 (egui が pass 境界で未登録 viewport を破棄 — 窓 2/3 の OS 窓死亡)
78.553 parked_hwnd_liveness_clear id=2 (0x16173e) / id=3 (0x271cc4)  ← B1 生存監視が正しく検出
78.594 再生成。しかし builder は apply_initial_placement=false (適用済み扱い) →
       egui 既定 (304,304)/533x400 の小窓で出現。placement guard は
       passive_placement_update_rejected_default ×43 で「保存値の破壊」は防いだが、
       OS 窓自体は既定サイズのまま表示 = 「多数の窓がフラッシュ」の見た目
78.596-79.9 直列化 (unconfirmed 2 窓) で id=3 の再登録が 1.3 秒以上 delay = フラッシュ長引く
```

BA-5 (「毎フレーム描かないと死ぬ」) の実害 **4 件目**。ただし今回は show スキップでは
なく **`App::update` 自体の early-return** が原因なので、findings-10 の修正 (直列化の
対象を未確定窓に限定) では防げない。

### 修正要件 (D1)

1. **early-return パスでも deferred 登録を飢餓させない**: `maybe_defer_for_main_font_atlas_resync`
   が true を返して `return` する 2 箇所 ([app.rs:44851](../src/app.rs) update_early /
   [app.rs:45880](../src/app.rs) pre_main_ui) で、**return の前に
   `render_detached_image_windows(ctx)` を呼ぶ** (pre_main_ui 側は既に 45667 を通過して
   いるため実質 update_early 側が本命だが、二重呼び出しの安全性を確認のうえ両方に
   置いてよい。同一 pass 内で 2 回呼ばれないことはコード順で保証される)。
   - render_detached_image_windows はイベント drain + batch 適用の副作用を持つ。
     同一 pass 内での多重適用が起きない配置にすること (discard repass は別 pass なので
     再実行されて問題ない)。
2. **再生成窓の placement 再適用**: registry hwnd を clear した時 (parked_hwnd_liveness_clear
   / 各 clear 経路) に、その窓の `initial_placement_applied` を **false に戻す**。
   これで万一将来の再生成が起きても、保存済み placement で出現する (既定サイズの
   小窓フラッシュを構造的に消す)。runtime placement 側の値は正しく保持されている
   ことをログで確認済み (from= 側は正値)。
3. テスト: (a) hwnd clear → initial_placement_applied リセットの単体テスト。
   (b) 可能なら「update の early-return パスが deferred 登録をスキップしない」ことの
   構造テスト (難しければ検証ログマーカー + 実機確認で代替可、報告に明記)。

### 観察メモ (修正対象外、報告のみ)

- `active_close_finalize` は Active→Removed を直接行い **Closing を経由しない**
  (`state_transition_unexpected` が close のたびに出る)。resync safety gate の
  closing_count が close 中を検知できない一因。今回は D1-1/2 で症状を塞ぐ。
  状態機械の整理はゲート C の R3/R4 判断材料として記録のみ。

## D2: B1 watcher 自己修復が「死につつある閉窓の HWND」を隣の窓に養子縁組する

### 実機シーケンス (同じ close の 48ms 後)

```
77.700 窓 9 close (hwnd 0xbe286a は registry から clear されたが OS 窓はまだ生存、
       runtime からは Removed = 「未請求」化)
77.752 watcher が同じクリック (窓 9 の × 押下) を down=窓 7 の rect 内と解釈
       (窓が重なっている)。cursor_root=0xbe286a は「egui クラス + 未請求 + 生存」の
       条件を満たすため repair 採用 → hwnd_adopted_watcher 窓 7: 0x1023d0→0xbe286a
       (⚠ 窓 7 の登録済み 0x1023d0 は生きているのに上書き)
77.752 同クリックで窓 7 が activation commit = 「× を押しただけなのに別の窓が
       アクティブ化」
78.554 0xbe286a が実際に死亡 → host_lost_before_render → clear → 消去法は
       candidates=[0x1023d0(窓7の旧窓・孤児), 再生成された2窓] の 3 択で ambiguous →
       リトライ (hwnd_deferred_retry ×172 の主因)
```

1 回の物理クリックが「egui 経路の close」と「watcher 経路の activation」に**二重解釈**
され、さらに repair が rect 包含 (geometry) を根拠に、直前に close された窓の dying hwnd を
採用してしまった。R1 の原則 (geometry 推定禁止) に対する事実上の違反経路。

### 修正要件 (D2)

1. **repair の許可条件を「対象窓の登録 hwnd が 0 または死んでいる場合のみ」に絞る**
   (`repair_detached_window_hwnd_from_watcher` 冒頭で known_hwnd 生存なら reject)。
   B1 の本来の目的 (silent stale = 登録が死んでいるときの自己修復) はこの条件でも
   完全に機能する。今回のように**生きた登録を geometry 根拠で上書きすることを禁止**する。
2. reject 時は既存の `repair_failed` 診断 (findings-11 で拡充済み、known_hwnd_state /
   claimed_by 付き) がそのまま出るので追加ログ不要。
3. テスト: known_hwnd 生存時に repair が拒否され activation も commit されないこと /
   known_hwnd 死亡時は従来どおり repair + commit されること。

## D3: book open がメイン context を経由するため、bundle 外グローバル状態が汚染される (症状①の根因 + 元の「サムネ停止」の真因)

### 機構 (コード + ログで確定)

detached book open (`start_active_detached_book_context_from_descriptor`,
[app.rs:26134](../src/app.rs)) は:

1. main bundle を take → 2. **`load_pdf_as_folder` を生きた App フィールドで実行**
   (worker 全停止→再 spawn・cancel_token・auto_aspect 再シード・catalog 再ロード) →
3. できた PDF context を bundle 化して detached へ → 4. main bundle を swap で復元

`ViewerContextBundle` ([app.rs:1584](../src/app.rs)) は items / scroll_offset_y /
requested 等を含むが、**`auto_aspect` は含まない** (グローバルのまま)。また
**worker pool と queue はグローバル**なので、手順 2 の全停止で main フォルダの
in-flight ロードが黙って死ぬ。

- **症状① (メイングリッドがスクロールして見える)**: 手順 2 で `auto_aspect` が PDF の
  値に再シードされる (ログ: open ごとに `auto_aspect cache: restored` / 20.220s には
  `switched 1:1 -> 2:3` の実切替も発生)。復元された main グリッドはセルのアスペクト比が
  変わって**リフロー**し、同じ scroll_offset_y でも行の中身がずれる = スクロールに見える。
  「することがある」= aspect が実際に変わったときだけ、と整合。
- **元の「PDF サムネが止まる」(findings-11 で fa09cc5a を revert した件) の真因**:
  bundle 復元で `requested` マップは戻るが、その要求が乗っていた queue/worker は
  手順 2 で殺されて応答が来ない = **requested に残骸が孤児化**。keep_range ロジックは
  「requested にあるから要求済み」とみなして再要求しない → 恒久停止。
  **Codex の元の観測 (requested+Evicted の矛盾) は実在した**。fa09cc5a が誤っていたのは
  検出方法 (正常 in-flight と区別不能な毎フレーム掃除) であって、leak 自体は本物。
  leak point はここ。

### 修正要件 (D3)

1. **auto_aspect を bundle に含める**: `ViewerContextBundle` に `auto_aspect`
   (AutoAspectState 一式) を追加し、take/swap で退避・復元する。これで main の
   セルアスペクトが book open で変わらなくなる = 症状①解消。
2. **bundle 復元 (swap-in) 時に `requested` と `pending_finalize` をクリアする**:
   これらはグローバルな queue/worker の状態を指す簿記であり、bundle をまたいで
   有効性を保証できない。クリアしても keep_range ロジックが次フレームで
   Pending/Evicted を再要求するだけで安全 (重複ジョブは発生しない)。
   これが findings-11 C1 で保留した「真の leak point での修正」。
3. テスト: (a) bundle round-trip で auto_aspect が復元される。(b) swap-in 後に
   requested が空で、keep 範囲の Evicted が再要求される (元の停止シナリオの回帰テスト)。

## 完了条件

- [ ] D1: early-return 飢餓の解消 + placement 再適用リセット + テスト。
      コミット `(detached-rework findings-12 D1)`
- [ ] D2: repair 条件の絞り込み + テスト。コミット `(detached-rework findings-12 D2)`
- [ ] D3: bundle への auto_aspect 追加 + swap-in requested クリア + テスト。
      コミット `(detached-rework findings-12 D3)` (D1/D2 と別コミット)
- [ ] 既存テスト + full test 緑、`cargo fmt --check`、`.\scripts\build-release.ps1`

## 実機確認 (次回、`MIV_DETACHED_WINDOW_DEBUG=1` のまま)

1. ON モードで窓 3〜5 枚 → 1 枚ずつ閉じる → 他の窓がフラッシュ/消失/再表示しない。
   ログに `parked_hwnd_liveness_clear` / `passive_placement_update_rejected_default` /
   `hwnd_adopted_watcher` (生存登録持ち窓への) が出ない
2. 窓の × を押しても隣の窓がアクティブ化しない
3. PDF を別窓で開く瞬間にメイングリッドが動かない (aspect 由来のリフロー消滅)
4. メインのサムネ読み込み中に book open してもサムネが止まらない (requested 孤児の解消)
