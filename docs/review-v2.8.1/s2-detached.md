## 1. サマリ

- **不一致: 19件**
  - 文書が古い／進捗未反映: 14件
  - 文書内・文書間矛盾: 2件
  - コードが正本の仕様に違反: 3件
- **リファクタ候補: 7件**
  - P1: 1件
  - P2: 3件
  - P3: 3件
- **バグ: 3件**
  - BA-7: 2件
  - BA-1: 1件

静的なソース監査のみ実施しました。read-only 制約に従いテスト・ビルド・アプリ起動は行っていません。ファイル変更はなく、最終 `git status --short` は clean でした。

### ステージ監査結果

| ステージ | 実装判定 |
|---|---|
| R0 | 完了 |
| R1/R1b/R1c | HWND registry/差分採用は完了。ただし BA-1 の rect 同定がキー入力経路に残存 |
| R2a | 完了 |
| R2b | **部分完了**。Runtime は使われているが、純粋 reducer・遷移合法性・散在フラグ集約は未完 |
| R2c | 完了 |
| R2d | 完了。ただし ParkedLive は文書どおり BA-5 非免疫 |
| R3 | **実質完了**。ViewportId は active session の `window_id` を優先 |
| R4 | 未完。keepalive/backstop はあるが single render entry 化は未実施 |

### BA 解消状況

| BA | 判定 | 根拠 |
|---|---|---|
| BA-1 | **部分解消** | host HWND は before/after 差分と registry に移行。ただしキー入力 subclass は rect 探索を使用 |
| BA-2 | 解消済み | 生存 HWND と初期 placement seed が runtime に分離 |
| BA-3 | 実動作は解消済み | host lost で旧 HWND を無効化。旧 recreate フィールドは残るが production で true にされない |
| BA-4 | 解消済み | active session の `window_id` から安定 ViewportId を生成 |
| BA-5 | **未解消・緩和のみ** | immediate/deferred/holdover/backstop の複数描画入口が残る。R4 対象 |
| BA-6 | 解消済み | placement は runtime 所有、settings は seed/persist 用 |
| BA-7 | **部分解消** | manager state は導入済みだが reducer が無制約。terminal close と pending producer が分離した実害あり |

根拠: [正本 §2・ステージ定義](../detached-rework-plan.md:28)、[現在の state enum](../../src/app/detached_window_manager.rs:18)、[state 遷移実装](../../src/app/detached_window_manager.rs:620)。

## 2. 不一致リスト

1. **正本の進捗表が自己矛盾**
   - 文書: [§9 R2b～R2d と「R2 全完了」](../detached-rework-plan.md:181)、[§10 の「R2/R3 未着手」](../detached-rework-plan.md:263)
   - コード: [Runtime state](../../src/app/detached_window_manager.rs:18)、[active session](../../src/app.rs:32575)、[ViewportId 決定](../../src/ui_fullscreen.rs:4190)
   - 判定: **文書内矛盾**。R2 は大部分実装済み、R3 も実質実装済み。
   - 修正案: §10 の孤立した3行を削除し、§9を唯一の進捗表にする。

2. **R2b の「完了」が設計上の完了条件を満たしていない**
   - 文書: [R2b 進捗](../detached-rework-plan.md:188)
   - コード: [`transition_state` は任意の遷移先をそのまま代入](../../src/app/detached_window_manager.rs:620)、[遷移中判定は複数 pending の OR](../../src/app.rs:32639)
   - 判定: **文書の完了表記が過大**。Runtime 導入は済んだが、純粋 reducer・合法遷移・typed state 集約は未完。
   - 修正案: R2b を「Runtime routing 完了／reducer 集約未完」に分割する。

3. **7月24～26日の重要な lifecycle 変更が正本の進捗にない**
   - 文書: [§9末尾](../detached-rework-plan.md:209)
   - コード: [typed transition-outstanding](../../src/app.rs:32639)、[bundle 内 poll/nav lock](../../src/app.rs:35350)、[terminal close](../../src/app.rs:33022)
   - 判定: **文書が古い**。
   - 修正案: physical open、terminal close、nav lock、holdover ownership を独立した完了項目として追記する。

4. **CUT 後も pin が現行設計として残っている**
   - 文書: [CUT でピン撤去](../detached-rework-plan.md:192)に対し、[architecture の「独立／ピン／Book」](../architecture-overview.md:154)などが残る。
   - コード: [現在の Runtime](../../src/app/detached_window_manager.rs:423)に `pinned` はない。
   - 判定: **CUT が正しく、他文書が古い**。
   - 修正案: 現行説明から pin を削除し、再導入案は将来項目として分離する。

5. **lifecycle redesign proposal の進捗が全面的に古い**
   - 文書: [「§5 構造リワーク未着手」](../detached-viewer-lifecycle-redesign-proposal.md:21)、[「6つの前提」だが BA-1～BA-7](../detached-viewer-lifecycle-redesign-proposal.md:58)
   - コード: [manager/runtime](../../src/app/detached_window_manager.rs:455)、[ViewerSession](../../src/app/viewer_session.rs:9)
   - 判定: **文書が古い**。数も7件が正しい。
   - 修正案: historical diagnosis と明記し、各 BA に「解消／部分解消／未解消」を付ける。

6. **implementation plan 冒頭の単一連動窓仕様が現行モードと矛盾**
   - 文書: [常時 selection sync／複数独立窓は対象外](../detached-viewer-implementation-plan.md:3)に対し、同文書後段 §3.0 は複数窓を記述。
   - コード: [active context 作成](../../src/app.rs:34702)、[park/reactivate](../../src/app.rs:34994)
   - 判定: **後段・正本・コードが現行、冒頭が古い**。
   - 修正案: 冒頭を historical v1 design として隔離する。

7. **独立モードの Ctrl フォルダ移動を「無効」とする記述**
   - 文書: [ship checklist](../detached-rework-ship-checklist.md:35)
   - コード: [物理ナビ可否](../../src/app.rs:36724)、[Ctrl ナビ handler](../../src/ui_fullscreen.rs:13763)
   - 判定: **文書が古い**。v2.8.0 stage-folder-nav で有効化済み。
   - 修正案: 「F12 は無効、Ctrl 物理フォルダ移動は有効」に更新する。

8. **右クリックを常に close とする旧仕様**
   - 文書: [implementation plan](../detached-viewer-implementation-plan.md:29)、[ship checklist](../detached-rework-ship-checklist.md:35)
   - コード: [短い右クリックの設定別 action](../../src/ui_fullscreen.rs:13339)
   - 判定: **コードと keymap が現行、文書が古い**。
   - 修正案: configurable action であることを記載し、close 固定の試験手順を修正する。

9. **open pending を main-only とする context separation 文書**
   - 文書: [`pending_auto_fs_open` / `fs_nav_after_pdf_enumerate` は main-only](../archive/detached/detached-viewer-context-separation-plan.md:151)
   - コード: [bundle fields](../../src/app.rs:2120)、[bundle swap](../../src/app.rs:12594)
   - 判定: **文書が古い**。現在は context-owned。
   - 修正案: ownership 表を現行 `ViewerContextBundle` に合わせる。

10. **focus 到達＋時間窓で active 化する旧記述**
    - 文書: [context separation activation](../archive/detached/detached-viewer-context-separation-plan.md:366)
    - コード: [manager/watcher](../../src/app/detached_window_manager.rs:455)、[正本の click-only 完了記録](../detached-rework-plan.md:185)
    - 判定: **コード・正本が現行、旧文書が古い**。
    - 修正案: 明示クリックだけを activation producer とする記述へ更新する。

11. **通常画像 detached physical context を未対応とする記述**
    - 文書: [PDF/ZIP のみとする旧 target](../archive/detached/detached-viewer-context-separation-plan.md:380)
    - コード: [通常画像 context 昇格](../../src/app.rs:36363)、[物理ナビ](../../src/app.rs:36724)
    - 判定: **文書が古い**。通常画像フォルダも対象。
    - 修正案: image/PDF/ZIP を列挙し、変換ダイアログだけ別 ownership として説明する。

12. **architecture の state enum が実装と異なる**
    - 文書: [`Active/Passive/Parked/ParkedLive/Resuming/Closing`](../architecture-overview.md:157)
    - コード: [`Opening/Active/Parked/ParkedLive/Resuming/Closing`](../../src/app/detached_window_manager.rs:18)
    - 判定: **コードが正しい**。Passive は window collection 上の役割で、runtime state ではない。
    - 修正案: Passive を Opening に修正し、passive window と runtime state を区別する。

13. **keepalive design が target と実装済み状態を区別していない**
    - 文書: [`closing` を session 内に持つ設計](../detached-viewer-keepalive-design.md:124)、[single render entry](../detached-viewer-keepalive-design.md:172)
    - コード: [session は window_id/source のみ](../../src/app.rs:32581)、[keepalive と通常 render が併存](../../src/app.rs:35433)、[backstop](../../src/ui_fullscreen.rs:6993)
    - 判定: **未実装 target を現行構造と読める状態**。
    - 修正案: K0～K3ごとの進捗を付ける。現状は K0完了、K1未完、K2/K3部分完了。

14. **soak 期間が文書間で矛盾**
    - 文書: 正本 §7 は2週間、[ship checklist はユーザー判断で1日](../detached-rework-ship-checklist.md:152)
    - コード: 該当なし。
    - 判定: **後日の明示決定である1日が当時の正しい出荷判断**。
    - 修正案: 正本へ override を追記するか、期間を release-specific decision として分離する。

15. **stage-audio が「候補」のまま**
    - 文書: [§10候補](../detached-rework-plan.md:263)に対し、[§9では完了](../detached-rework-plan.md:202)
    - コード: [動画 presentation 接続](../../src/app/native_video.rs:2670)と音声共用経路が実装済み。
    - 判定: **正本内矛盾**。
    - 修正案: §10から削除し、完了した拡張ステージへ移す。

16. **smoke checklist の成功ログ名が存在しない**
    - 文書: [`captured host` を成功条件とする手順](../detached-viewer-smoke-checklist.md:25)
    - コード: 現在は [before/after registry 登録](../../src/ui_fullscreen.rs:6871)と `hwnd_adopted_*` 系ログ。
    - 判定: **文書が古く、現状では判定不能な手順**。
    - 修正案: 現行ログ名と window_id/host 対応の確認方法へ更新する。

17. **「rect 一致捕捉の全廃」に反するキー入力経路**
    - 文書: [architecture BA-1 根治](../architecture-overview.md:158)
    - コード: [viewport rect から subclass 対象を探索](../../src/ui_fullscreen.rs:6280)、[全非 embedded viewport で実行](../../src/ui_fullscreen.rs:7499)
    - 判定: **コードが仕様違反**。バグ3参照。
    - 修正案: 文書を後退させず、R1 hardening で registry/window_id 所有へ統合する。

18. **terminal close が全 pending producer を終了させる不変条件に違反**
    - 文書: [Esc/× は terminal teardown](../detached-viewer-keepalive-design.md:153)
    - コード: [holdover cancel は一部 intent と lock だけを破棄](../../src/ui_fullscreen.rs:6884)、[残った producer は次フレーム poll](../../src/app.rs:35350)
    - 判定: **コードが仕様違反**。バグ1参照。
    - 修正案: BA-7/R2b の terminal transition ownership として扱う。

19. **動画 F12 OFF で manager runtime の terminal teardown が完了しない**
    - 文書: [F12 OFF は closing→None](../detached-viewer-keepalive-design.md:161)
    - コード: [session finish のみ](../../src/app/native_video.rs:2670)、[`finish` は session を take するだけ](../../src/app.rs:32622)
    - 判定: **コードが仕様違反**。バグ2参照。
    - 修正案: BA-7/R2b の session/runtime 一体遷移として報告する。

## 3. リファクタ候補リスト

### P1 — terminal transition と全 pending producer の typed owner 化

- **なぜ問題か**: terminal close と「遷移中」の判定が別々に管理され、実際に late completion が close 後の context を再度動かせる。現在の OR リストは producer 追加時の更新漏れを既に起こしている。
- **所属ステージ**: R2b 再開、K2 terminal lifecycle。R4より前。
- **影響範囲**: `app.rs`、`ui_fullscreen.rs`、open/enumerate handlers、tests の4～7ファイル、約400～900行。
- **回帰リスク**: PDF/ZIP、通常画像 scan、password、bookmark、Ctrl folder-nav の open/cancel/error。Windows multi-window 実機確認が必要。
- **テストで担保できるか**: reducer と cancel propagation は新規 unit/handler test で担保可能。実 worker completion と OS × は統合テスト／実機 smoke が必要。
- **規模**: Medium
- **優先度**: **P1**

### P2 — キー入力 subclass の HWND 所有を registry に統合

- **なぜ問題か**: 同一位置・同一サイズ窓では rect score が同点になり、別窓を選択できる。BA-1 の所有境界が入力経路だけ例外になっている。
- **所属ステージ**: R1 hardening。
- **影響範囲**: `ui_fullscreen.rs`、`dwm_transitions.rs`、manager/key-input、tests の3～4ファイル、約100～250行。
- **回帰リスク**: numpad/JISキーなど物理キー edge、IME、複数窓の shortcut routing。
- **テストで担保できるか**: window_id→HWND 解決は unit test 可。同rect窓と実 subclass はWindows実機確認が必要。
- **規模**: Small
- **優先度**: **P2**

### P2 — park 時の thumbnail pipeline pause/resume protocol

- **なぜ問題か**: [pause](../../src/app.rs:2445)は fullscreen/AI を止めるが thumbnail worker・queue・rx を止めない。park 中も decode が完了し、未 poll の `ColorImage` が context の rx に蓄積しうる。
- **所属ステージ**: R4 resource-lifecycle substage／backlog 1.9。
- **影響範囲**: `app.rs`、thumbnail loader/queue、bundle Drop、tests の3～5ファイル、約300～700行。
- **回帰リスク**: resume 後にサムネが再要求されない、Requested が孤児化、worker cancel が sibling context に波及する危険。
- **テストで担保できるか**: queued/in-flight/completed の状態遷移は新規テスト可。長時間parkのメモリ挙動は実機計測推奨。
- **規模**: Medium
- **優先度**: **P2**

### P2 — 全 mounted/parked context を横断する VRAM budget

- **なぜ問題か**: [eviction](../../src/app.rs:28071)は現在 mount 中の context 単位。parked bundle がそれぞれ上限近く保持でき、窓数に比例して総VRAMが増える。動画サムネは eviction 対象外。
- **所属ステージ**: R4 後半の resource ownership／backlog 1.9。
- **影響範囲**: cache owner、bundle、texture eviction、tests、新規budget manager の4～7ファイル、約500～1,200行。
- **回帰リスク**: active窓のtextureを誤evict、復帰時の再decode増加、GPU upload churn。
- **テストで担保できるか**: accounting/LRU は unit test 可。実VRAM・D3D11挙動は実機確認必須。
- **規模**: Large
- **優先度**: **P2**

### P3 — detached lifecycle 全体の typed reducer 化

- **なぜ問題か**: session、runtime、active context、window id、independent/open-next/reuse/no-activate/focus/recreate などが複数フィールドに分散し、相互排他性が型で保証されない。mode change は現在も[多数フィールドを一括リセット](../../src/app.rs:33172)している。
- **所属ステージ**: R2b 完遂からR4。
- **影響範囲**: manager、App、ViewerSession、fullscreen、native video、tests の5～9ファイル、1,000～2,500行。
- **回帰リスク**: open/switch/park/reactivate/close全般、動画・音声・Book。高い。
- **テストで担保できるか**: pure transition table は強く担保可能。ただしHWND/focus/video presenterは実機gate必須。
- **規模**: Large
- **優先度**: **P3**

### P3 — ViewerContextBundle と mount/poll 責務の分割

- **なぜ問題か**: bundle は約250行のフィールド、swap は約460行に達し、open request、thumbnail、PDF/ZIP、AI、metadata、navigationが同じ手動swap境界に集まる。poll追加位置の漏れが実際にterminal/nav lock回帰へつながった。
- **所属ステージ**: R4後、context resource ownership の整理。
- **影響範囲**: `app.rs`、`viewer_session.rs`、各feature moduleの8～15ファイル、2,000行以上。
- **回帰リスク**: context間のitems/cache/channel/cancel token混線。非常に高い。
- **テストで担保できるか**: sub-bundleごとのswap/drop testは可能。PDF/ZIP/AI/動画を含む広い回帰gateが必要。
- **規模**: Large
- **優先度**: **P3**

### P3 — `display_px` を decode request/context 所有にする

- **なぜ問題か**: App-global の表示pxをactive contextが更新するため、parked/復帰contextのdecode目標サイズが現在の別窓に影響される。直ちに誤画像になる問題ではないが、過剰decode・低解像度再decodeの原因になる。
- **所属ステージ**: R4 resource-budget follow-up／backlog 1.9。
- **影響範囲**: App、decode request、bundle/tests の2～4ファイル、約150～400行。
- **回帰リスク**: 画質、decode CPU、復帰直後の表示解像度。
- **テストで担保できるか**: request生成はunit test可。画質・性能は実機比較推奨。
- **規模**: Small
- **優先度**: **P3**

## 4. バグリスト

### バグ1 — terminal close 後に in-flight open/navigation が適用される（BA-7）

- **症状・条件**: detached 窓で Ctrl folder-nav、folder scan、PDF/ZIP enumerate 等の遷移待ち中に Esc または ×。worker が後から完了すると、閉じたはずの contextで load/reopen が行われる、または閉じたbundleへ結果が適用される可能性がある。実機再現は未実施だが、コード経路は静的に成立する。
- **壊れている不変条件**: terminal close は、そのcontext所有の全producerをcancelし、late completionより常に優先される。
- **原因経路**:
  - [holdover cancel](../../src/ui_fullscreen.rs:6884)は `fs_nav_after_pdf_enumerate` とlockだけを消し、`folder_nav_pending` 等をcancelしない。
  - [`cancel_pending_folder_nav`](../../src/app.rs:29986)は存在するが、この経路から呼ばれない。
  - outstanding が残るため [context dropが抑止](../../src/app.rs:35449)され、次フレームに[poll/apply](../../src/app.rs:35350)される。
  - folder-nav結果は[close→load→reopen](../../src/app.rs:31159)を実行する。
- **同型経路**: embedded holdover も[intent/lockのみ破棄](../../src/ui_fullscreen.rs:7112)。folder scan、PDF/ZIP enumerate、password、bookmark pendingも同一監査対象。
- **分類**: **BA-7**。terminal/transition state とproducer ownershipの分離。

### バグ2 — 動画 F12 OFF 後に `Closing` runtime が残留する（BA-7）

- **症状・条件**: native videoをdetachedからMainWindow/Fullscreenへ戻す。sessionは消えるがmanager runtimeは`Closing`のまま残る。以後、別の経路が同IDを再利用または全窓closeするまで、font-atlas resyncのsettled判定が通らない条件を作る。
- **壊れている不変条件**: terminal transition は session、runtime、host/placement cleanupを一体で完了し、`Closing → Removed`に到達する。
- **原因経路**:
  - [presentation switch](../../src/app/native_video.rs:2670)は begin/finish だけで `remove_detached_window_runtime` を呼ばない。
  - [`finish_active_detached_session_close`](../../src/app.rs:32622)はsessionをtakeするだけ。
  - `Closing`件数は[resync safety](../../src/app.rs:32204)に入り、0でなければsettledにならない。
  - 既存テストは[session Noneのみを検証](../../src/app/tests.rs:24927)し、runtime消去を確認していない。
- **同型経路**: matched、stale-converged、pending無しの全 `PlacementSwitched` 分岐が同じ関数へ収束する。通常のactive close、mode change、non-detached image openはruntimeを削除しており、動画切替だけ非対称。
- **分類**: **BA-7**。

### バグ3 — detached key subclass が window identity ではなく rect で選ばれる（BA-1）

- **症状・条件**: 同じ位置・サイズで重なる複数viewportがある場合、active viewportではなく先に列挙された別窓へkey-input subclassをinstallできる。新規窓でこれが継続すると、物理キーedge依存のshortcutが欠落または別窓由来として処理されうる。実Windows環境での症状確認は未実施。
- **壊れている不変条件**: detached HWNDの識別は `window_id → registered HWND` の所有関係だけで行い、rect一致をidentityとして使わない。
- **原因経路**:
  - [全非embedded fullscreenでrect install](../../src/ui_fullscreen.rs:7499)
  - [rectからHWND探索](../../src/ui_fullscreen.rs:6280)
  - [selector](../../src/dwm_transitions.rs:123)はscore同点時に先の窓を維持する。
- **同型経路**: host ownershipの主要経路はbefore/after差分へ移行済み。別のrect探索として仮想デスクトップ同期等があるが、今回確認したdetached入力への直接影響はこのsubclass経路。
- **分類**: **BA-1**。

## 5. 総評

bundle/context 分離そのものはかなり実装に追いついています。`ViewerContextBundle` のswap、Drop teardown、通常画像のphysical context化、PDF/ZIP/password request ownershipは、古い計画より実装のほうが進んでいます。mainとdetachedのbundleをmountした状態でpollする構造も概ね守られています。

一方、文書の信頼性は領域内で大きくばらつきます。

- 最も信頼できるのは `detached-rework-plan.md` の**§2憲法と個別findingsの履歴**。
- 同文書の**§9/§10進捗表は信頼できません**。R2/R3の状態、audio、最新terminal/nav lock変更が矛盾・欠落しています。
- `detached-viewer-lifecycle-redesign-proposal.md` と `detached-viewer-implementation-plan.md` 冒頭は、現行設計ではなくhistorical diagnosisとして読む必要があります。
- `detached-viewer-keepalive-design.md` は目標設計として有用ですが、K0～K3の実装状況がなく、現行構造の説明としては不正確です。
- `detached-viewer-smoke-checklist.md` はログ名とpin項目が古く、そのままでは現行ビルドの検収に使えません。
- archive済みのcontext separation planはbundleの基本思想は有効ですが、pending ownership、activation、通常画像対応の進捗が古いです。

v2.8.1観点では、最優先はBA-7のterminal ownershipです。park中のthumbnail/VRAM問題は構造的背景まで確認できましたが、既知P2としてR4系ステージでまとめるのが妥当です。症状パッチでclose時のフィールドを個別clearする形は、凍結ルール上も設計上も避けるべきです。