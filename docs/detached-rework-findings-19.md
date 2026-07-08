# findings-19: ON モードのアクティブ切替が close+reopen 往復で窓を再構築しており、高速切替で窓が消える (2026-07-08)

報告: ship-checklist v2 の W1/W2 実施中 (ユーザー実機)。
ログ: scratchpad/w1-cur.log (MIV_DETACHED_WINDOW_DEBUG=1、3780〜3812s 付近)。

## 症状

- **A (P1)**: ON モードで窓を複数開き順にアクティブ化を繰り返すと、別の窓が**消える**ことがある。
- **B (P2)**: アクティブにした瞬間、その窓の**画像がすこしだけ移動**する (毎回)。

## A の機構 (ログで確定)

### 正常時でも切替のたびに close+reopen している

アクティブ切替 1 回ごとに、旧 active 窓が以下を辿る (このセッションで
`park_close_legacy_detached` が **74 回** = ほぼ全切替):

```
22: Active → Parked   reason=park_active_detached_image_window
22: Parked → Closing  reason=main_context_change          ← park 直後に close 判定
22: session_finish    reason=main_context_change
22: Closing → Removed reason=park_close_legacy_detached   ← runtime 削除 (OS 窓は孤児で生存)
  (0.7〜1.4 秒後)
22: Opening → Opening reason=deferred (hwnd=0x0)          ← 同じ window_id で再作成 (descriptor)
22: Opening → Parked  reason=hwnd_adopted_deferred hwnd=0x3c27e6  ← 孤児 HWND を養子縁組
                                                            (host_generation++)
```

つまり「旧 active → passive 降格」が **in-place の handoff ではなく、runtime を
close → descriptor から再作成 → 孤児 HWND 再採用**という往復で実装されている
(メディア窓の live-park は `handoff_active_detached_viewport_to_passive` で in-place
降格しており、静止画切替だけがこの churn 経路)。

- `main_context_change` による close は本来 OFF (linked) の規則のはずだが、
  **linked=false の独立窓**に対して毎回発火している (直後の descriptor 再作成が
  補償するため、ゆっくり操作すると見た目は保たれる)。

### 高速切替での破綻 (窓消失の直接原因)

再作成完了 (hwnd_adopted_deferred) 前に次の切替が起きると、**未請求の孤児 HWND が
2 つ以上並ぶ**:

```
3782.917 deferred_activate_watcher_dropped id=23 reason=repair_failed
         observed_hwnd=0x21817ae ... claimed_by=Some(21)
3811.587 hwnd_deferred_retry window_id=23 reason=ambiguous
         candidates=[2808d8, 21b17ae] claimed_count=0 host=hwnd=0   ← 候補 2 つ = 採用不能
3803.8xx deferred_registration_delayed id=21 / id=22
         reason=unconfirmed_hwnd_serialized  ← 毎フレーム交互に飢餓 (R2d の 1frame1窓
                                               直列化が、未確定窓 2 つで永久に回る)
```

- R1b の消去法採用は「未請求がちょうど 1 件」が条件のため、孤児 2 つで **ambiguous →
  hwnd=0 のまま stuck**。
- 未確定窓が 2 つになると R2d の直列化 (1 フレーム 1 窓) が交互に delay し続け、
  `show_viewport_deferred` されないパスが続く → **egui が未登録 viewport を破棄 = 窓が消える**
  (findings-10 と同じ最終形。今回の発生源は clear-on-park ではなく close+reopen の孤児併存)。

## B の観察 (未確定、調査指示あり)

`runtime_placement` は切替前後で **完全に不変** (x/y/w/h に 1px の揺れもなし) →
窓移動ではない。疑い = **passive の凍結スナップショット描画と active の live 描画で
画像コンテンツの配置矩形が数 px ずれる** (fit 計算・バー領域の扱い・ppp=1.5 の丸めの
いずれか)。アクティブ化の瞬間に snapshot → live に切り替わるときズレが見える。

## 修正指示 (fix10) — Phase 1 調査 → Fable 承認 → 実装

1. **Phase 1 (A)**: 静止画独立窓のアクティブ切替経路を特定して報告する:
   - 旧 active 窓が `main_context_change` close に落ちる call site (park_current_active_detached
     / close_legacy_detached / deferred_activate_commit 周辺) と、直後に同 id で再作成している
     機構 (reopen_descriptor)。
   - この close+reopen が**意図された設計か、OFF (linked) 規則の誤発火を descriptor が
     補償しているだけか**を判断する (git 履歴の確認は任意、コード構造からで可)。
2. **実装方針 (A、承認前提の推奨案)**: 切替時の close+reopen をやめ、**旧 active 窓を
   in-place で Parked (deferred) に降格**する — メディア窓の
   `handoff_active_detached_viewport_to_passive` と同じ「OS 窓・viewport id・HWND 登録を
   維持したまま状態だけ落とす」形に静止画も揃える。これで孤児 HWND が存在しなくなり、
   ambiguous / 直列化飢餓 / host_generation 増加が構造的に消える。
   - 憲法 §1/§2: 採用ヒューリスティックの強化 (候補 2 つの解決ロジック追加等) で
     対処**しない**こと。孤児を作らないのが根治。
3. **Phase 1 (B)**: アクティブ化前後で「画像コンテンツの描画矩形」を比較する
   (snapshot 描画の rect と live fit 計算の rect を同条件でログ or テストで突き合わせ)。
   数 px 差の出所 (バー領域 / fit / 丸め) を特定して報告 → 修正。
4. テスト: (A) 切替 2 連続 (再作成完了前に次の切替) のシーケンスで、旧 active 窓の
   HWND 登録が切替をまたいで不変 = 孤児が発生しないことを固定。(B) snapshot/live の
   rect 一致を純関数で固定。
5. コミット: `(detached-rework findings-19 fix10)` (A と B は別コミット可)。

## 参考: ログ抽出

```powershell
Select-String -Path $log -Pattern 'park_close_legacy_detached|hwnd_adopted_deferred|ambiguous|unconfirmed_hwnd_serialized|repair_failed'
```
