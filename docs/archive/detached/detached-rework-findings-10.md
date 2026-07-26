# 検収所見 #10: B1 の clear-on-park が多窓 churn を生む (窓が消える/現れる/PDF が遅い)

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
前提: findings-9 の B1 (cff57ab1) / B2 (0f264f91) を投入済み。実機再検証 (2026-07-07 09:32、
動画 = C:/Users/mikag/Videos/2026-07-07 09-32-02.mkv、ログ = 当該セッションの
mimageviewer.log / .bak、Fable が scratchpad に凍結して解析) で残存 4 症状:

1. PDF のサムネイル一覧の表示が進まないことがある
2. 別ウィンドウの PDF 閲覧がおそらく遅い
3. 複数ウィンドウでアクティブを切り替えると、窓が非表示になることがある
4. 複数ウィンドウを閉じるとき、別の窓が突然表示される

## 0. B1/B2 の効いた部分 (回帰させないこと)

- **B1 の stale-hwnd クリック棄却は解消**: `down_window_from_point_mismatch` = 0、
  `host_lost` = 0、`no_new_window` = 0。B1 の①生存監視
  (`refresh_parked_detached_window_hwnd_liveness`) と②watcher 自己修復
  (`repair_detached_window_hwnd_from_watcher`) は**正しく機能しており、残す**。
- **B2 のテクスチャアップロード デカップリングは有効**: `deferred_for_resync` の
  非ゼロ計上 = 0。`main_font_atlas_resync_pending` の滞留も 78.6〜87.5s の約 9 秒に
  短縮 (findings-9 時点の 28〜131s から改善) し、その間も `detached_safe=true`
  (placement_pending / cloak / opening / closing すべて 0)。**サムネ不表示の主因は
  もうテクスチャ defer ではない** (下記のとおり churn が UI スレッドを食う方が主因)。

## 1. 根本原因: B1 の clear-on-park が「park のたびに登録を捨てる」→ 直列化スキップで OS 窓が破棄される

### 1.1 機構

B1 は `handoff_active_detached_viewport_to_passive`
([app.rs:25021-25028](../src/app.rs)) に、**park のたびにその窓の登録 hwnd を無条件で
clear する**処理を追加した (`active_viewport_handoff_clear_host_for_deferred`)。
findings-9 B1 の要件③「HWND が park をまたいで不変であることを固定する」を、
**「clear して deferred 側で再確認させる」**方向に実装したもの。

しかしこの関数の**すぐ上のコメント (25010-25013)** が既存の不変条件を明記している:

```
// Active -> Passive の切替では OS viewport 自体は閉じない。... 次フレームから
// 同じ ViewportId を passive renderer が描く。
```

= park では OS 窓は閉じず、同一 ViewportId のまま deferred renderer が描き続ける。
**B1 の clear-on-park はこの不変条件と矛盾している** (窓は生きているのに登録を捨てる)。

登録を捨てると、その窓は「未確定 (hwnd 非生存)」扱いになり、R2d の直列化ゲート
`deferred_detached_window_registration_allowed` ([app.rs:25717](../src/app.rs)) の対象に
落ちる。このゲートは**1 フレームにつき未確定窓を 1 つしか
`show_viewport_deferred` させず、残りは `continue` でスキップする**
([ui_fullscreen.rs:4078-4091](../src/ui_fullscreen.rs))。

**egui は「そのフレームで show されなかった deferred viewport を破棄する」**。park 直後の
窓は immediate 描画も止まっている (active でなくなった) ので、deferred 側でスキップ
されると **immediate でも deferred でも描かれない = 孤児フレーム**になり、OS 窓が破棄
される。次フレームで再度 show されると**別の HWND で作り直される** → 再び clear-on-park の
対象 → 直列化スキップ → 破棄 …と churn する。

複数窓が同時に未確定になる = ON モードでユーザーが 2 枚以上を切り替えるたびに発生する
(各切替で旧 active が park され未確定化)。

### 1.2 証拠 (id=2 の park→再採用サイクル、同一セッション)

| park | 再採用 | 間隔 | HWND (before→after) |
| --- | --- | --- | --- |
| 27.32s | 28.47s | 1.15s | 0x17c2846 → **同一** |
| 31.91s | 32.11s | 0.20s | 0x17c2846 → **同一** |
| 61.01s | 61.50s | 0.48s | 0x17c2846 → **同一** |
| 63.83s | 64.33s | 0.50s | 0x17c2846 → **同一** |
| 76.42s | **80.82s** | **4.41s** | 0x17c2846 → **0x1d0a6e (変化)** |

- ViewportId は全区間 "4B7E" で不変。
- **短い間隔 (競合窓が少ない) では同一 HWND を再採用** = OS 窓は park をまたいで生存して
  いた = **clear-on-park は無駄な往復** (有効な登録を捨てて同じ窓を採り直しただけ)。
- **長い間隔 (76-80s の多窓 churn 中) だけ HWND が変化** = その間 `show_viewport_deferred`
  が直列化で連続スキップされ、egui が OS 窓を破棄→再生成した。

id=5 も同型: 75.06s park (0x250882 clear) → 79.46s まで
`deferred_registration_delayed id=5` が**連続 508 回** (`passive_windows=4`) →
80.64s に別 HWND 0x1e168e で再採用 = **約 5.5 秒間、窓が登録を失って limbo**。

`deferred_registration_delayed` は 56-58s (265 回) と 75-80s (863 回) の 2 バーストで
計 1129 回、対象は id=2/4/5。id=4 (0x991aac→0x9a1aac)・id=5 (0x250882→0x1e168e) も
長間隔 park で HWND が変化している。

### 1.3 4 症状との対応

- **症状 3 (切替で窓が非表示)** = park された窓が直列化スキップで数秒破棄される直接の帰結。
- **症状 4 (閉じると別窓が突然表示)** = 別窓が churn 中 (破棄→再生成→可視化の途中) に
  active を閉じてフォーカスが移り、再生成された窓が「突然現れる」ように見える。
- **症状 1/2 (PDF サムネ/閲覧が遅い)** = SLOW FRAME 665 件はほぼ `pre_grid` 100-135ms
  (detached 窓の render/snapshot 処理) 支配で、`keep` (テクスチャ upload) 支配は 1 件のみ。
  PDF pool は健全 (starvation/backlog スパイクなし)。**churn による UI スレッド飽和が
  メインのサムネ進行を止めている** (テクスチャ defer や PDF pool ではない)。

## 2. 修正要件

### 2.1 主修正: clear-on-park を撤去 (B1 回帰の除去)

- [app.rs:25021-25028](../src/app.rs) の
  `clear_detached_window_hwnd_for_window_id` 呼び出しと
  `active_viewport_handoff_clear_host_for_deferred` ログを**削除**する。
  active→passive の handoff は窓の登録 hwnd を**触らない** (同一 ViewportId / 同一 OS 窓が
  そのまま生存する = 関数コメント 25010-25013 の不変条件どおり)。
- これで park 済み窓は「確定 (hwnd 生存)」のままになり、
  `deferred_detached_window_registration_allowed` が常に true を返す → 直列化スキップの
  対象にならない → 破棄されない → churn 消滅。
- **findings-9 B1 要件③「HWND が park をまたいで不変」は、登録を捨てないことで達成する**
  (捨てて採り直すのは逆方向で、競合時に破壊的だった)。

### 2.2 残す (正しく効いている B1 部分)

- `refresh_parked_detached_window_hwnd_liveness` ([app.rs:1327 付近](../src/app.rs)):
  park 済み窓の hwnd を root frame で `IsWindow` チェックし、**本当に死んでいる場合だけ**
  clear→再採用。これが唯一の staleness ネット。撤去しない。
- watcher 自己修復・A1v4 の棄却診断・B2 の texture decouple はそのまま。

### 2.3 直列化スキップが安全であることの確認 (念のため)

- clear-on-park 撤去後、`deferred_detached_window_registration_allowed` が false を返す
  (= show をスキップする) のは **hwnd 非生存の窓だけ** (新規初回 open か、liveness で
  clear された真の死窓)。生きている OS 窓を持つ窓は絶対にスキップされない (孤児化しない)。
- この不変条件を守るため、**「生存 hwnd を持つ window を直列化で delay してはならない」**
  ことをテストで固定する。憲法どおり時間窓・ヒューリスティックは足さない。

## 3. テスト要件

1. **多窓 park 不変テスト**: detached 窓を 2 つ以上 registry に持たせ、active を 1 つ
   handoff-to-passive したとき、その窓の登録 hwnd が **clear されない / 変化しない**こと。
   再 activate 時に同じ hwnd を引くこと (findings-9 B1 要件③の正しい形)。
2. **直列化の安全性テスト**: `deferred_detached_window_registration_allowed` は生存 hwnd を
   持つ window に対して常に true (delay しない) を返すこと。未確定窓が 2 つのときだけ
   1 つ delay する既存挙動は維持。
3. 既存の detached テスト (src/app/tests.rs) を弱体化しない。仕様変更で赤くなるものが
   あれば列挙して報告。

## 4. 完了条件

- [ ] `handoff_clear_host_for_deferred` が emit されない (該当行削除)。grep 0 件。
      コミット `(detached-rework findings-10)`。
- [ ] 上記テスト追加 + 既存テスト + full test 緑。
- [ ] `cargo fmt --check` / `python scripts/check_ui_glyphs.py` / `.\scripts\build-release.ps1`。

## 5. 実機確認 (次回)

1. ON モードで detached 窓を 3〜5 枚開き、アクティブを速く切り替える →
   **窓が消えたり点滅したりしない**。`deferred_registration_delayed` がログに出ない
   (出るのは真の初回 open のみ)。
2. detached 窓を開いたまま、メイン窓の **PDF サムネ一覧が止まらず進む**。
3. 窓を 1 枚ずつ閉じても、**別の窓が突然現れない**。
4. 別ウィンドウの PDF 閲覧が引っかからない。
