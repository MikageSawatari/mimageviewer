# Detached viewer 構造リワーク マスタープラン (正本)

作成: 2026-07-05 / ClaudeCode
体制: **実装 = Codex / 検収 = ClaudeCode / 実機検証 = ユーザー**

対象: F12 別ウィンドウ表示 (detached viewer) と複数ウィンドウ (passive /
always-new) のライフサイクル全体。CUT 後の現行仕様には pin はなく、再導入する場合は
「独立窓として複製する」将来機能として別途設計する。

技術的な診断の正本は
[detached-viewer-lifecycle-redesign-proposal.md](detached-viewer-lifecycle-redesign-proposal.md)
(以下「提案書」)。本書はそれを **実行計画** に落としたもので、提案書と矛盾する場合は
本書が優先する (本書の方が新しい)。

---

## 1. なぜリワークするのか (2 段落で)

detached viewer は約 3 週間で 88 コミットの修正を重ねても収束していない。提案書が
特定したとおり、原因は個別バグではなく **7 つの壊れた前提 (BA-1〜BA-7)** の上に
ヒューリスティックを積んでいる構造にある。直近の「動画ウィンドウ左右振動」も
BA-1 (rect 一致による HWND 誤同定) × BA-6 (placement 三重所有) の帰結で、症状パッチ
では同クラスのバグが再生産され続ける。

そのため **新規の症状パッチを凍結し、提案書 §5 の構造リワークをステージ実行**する。
各ステージは「1 セッションで完結する粒度 + 機械的に確認できる完了条件 + 実機 smoke」
を持ち、Codex が実装し ClaudeCode が検収する。

## 2. 憲法 (全ステージ共通の不変条件・禁止事項) ⚠️ 最重要

実装セッション (Codex) は、**着手前に本節を読み、作業中に矛盾する誘惑が生じたら
手を止めて報告する**こと。これらは過去 15 ラウンド以上の失敗から抽出したルールであり、
「目の前の症状を早く消す」ためにこれらを破ると必ず別経路が壊れる。

> **適用範囲 (2026-07-29 明確化)**: 本節はリワークのステージ作業を縛る。
> **リワーク外の作業が detached 述語 / viewport 経路に触れること自体は禁止していない。**
> 禁止しているのは症状パッチである。他機能の修正や構造的に正しい修正で触れる場合は、
> CLAUDE.md「Detached viewer リワーク中のルール」の条件 —— 本節を読む / ClaudeCode と Codex の
> 双方のレビューで「症状パッチではなく構造的修正である」ことに合意する / 触れた範囲と判断理由を
> 本書へ記録する —— を満たせば実施してよい。記録は §11 (リワーク外からの変更記録) に追記する。

1. **rect 一致捕捉に条件を足さない**: `find_visible_thread_window_matching_rect` /
   `_excluding` (src/dwm_transitions.rs) へ、新しい除外・閾値・スコアリング・
   リトライを追加してはならない。滑るケースを見つけたら、直さずに報告して止まる。
   この機構は R1 で detached 経路から**撤去される予定**のもの。
2. **geometry 由来の host_lost を recreate トリガにしない** (提案書 BA-3)。
   viewport の再生成はユーザー/明示イベント起点のみ。
3. **App に新しい detached 用 bool / Option フラグを足さない** (提案書 BA-7)。
   状態が必要になったら R2 で導入する `DetachedWindowRuntime` / 状態 enum に足す。
   R2 より前のステージでどうしても必要なら、追加前に ClaudeCode に相談する。
4. **placement (位置・サイズ) の新しい保存先・同期経路を作らない** (提案書 BA-6)。
   既定サイズ拒否のようなヒューリスティック防波堤も新設しない。
5. **時間窓 (debounce / grace / settle ms) で競合を吸収しない**。判定は、問うている事柄と
   **同じ時間・所有境界の事実**で行う (世代、イベント生成時の情報、必要な場合の現在 OS 状態)。
   `GetAsyncKeyState` が示す**処理時点の current level を、既に配送された離散 `KeyDown` の
   fresh / stale identity の代用にしてはならない**。native F12 では、旧 HWND 由来 event を
   per-window epoch / generation で、hold による repeat を `WM_KEYDOWN` の previous-key-state
   (lParam bit 30) で判定し、App 側に追加の時間窓・物理 level proxy・generation guard を置かない。
   時間窓は「頻度を下げるだけでループを消さない」。

   > ⚠ **旧版はこの規則の好例として「stale F12 再配送を `GetAsyncKeyState` の物理状態で
   > 棄却した実例」を挙げていたが、2026-08-26 に実機ログで反証されて撤回した**。
   > その probe は本物の連打を stale と誤分類していた (backlog §1.124)。一般原則は維持される。
6. **実機で新症状が出ても、その場でヒューリスティックを入れない**。症状を BA 番号
   (提案書 §2) に対応付けて報告し、どのステージで根治されるかを確認してから動く。
7. **ステージの指示書に書かれていないファイル・機構を「ついでに」直さない**。
   気付いた問題は報告に含めるだけにする (スコープ膨張がレビューを壊す)。
8. 既存の detached テスト (src/app/tests.rs、104 本) を**削除・弱体化しない**。
   仕様変更で赤くなるテストは、指示書に列挙されたもの以外は ClaudeCode に確認してから
   書き換える。

## 3. 体制とステージ実行プロトコル

各ステージは次のサイクルで回す:

| # | 誰が | 何を |
| --- | --- | --- |
| 1 | ClaudeCode | ステージ指示書 `docs/detached-rework-stage-<ID>.md` を作成 (完了条件・触ってよいファイル・テスト要件を明記) |
| 2 | ユーザー | Codex に投入。プロンプト例: 「`docs/detached-rework-plan.md` の §2 (憲法) と `docs/detached-rework-stage-<ID>.md` を読んでから、指示書どおりに実装して」 |
| 3 | Codex | 実装 + テスト。同一ステージ内の往復は同一セッション (resume) で継続 |
| 4 | Codex | 完了条件の機械チェック (grep / cargo test) 結果を含む完了報告を書く |
| 5 | ClaudeCode | 検収: diff を指示書の完了条件と照合。指摘があれば 3 に戻す |
| 6 | ユーザー | 実機 smoke (指示書が指定する [smoke-matrix](archive/detached/detached-viewer-smoke-matrix-20260630.md) のケース) |
| 7 | — | 緑になったら master へ統合、次ステージへ |

- 検収 (5) は ClaudeCode の利用量節約のため「指示書との照合」に絞る。設計判断が
  必要な逸脱が見つかった場合のみ ClaudeCode が構造判断をする。
- 実装ブランチ: `detached-rework` (main working tree 上でよい。ただし**リワーク中は
  detached 関連を他セッションで並行して触らない**。並行作業が必要になったら
  worktree 分離 + `scripts/safe-worktree-remove.ps1` 運用)。
- コミットメッセージには `(detached-rework R<N>)` を含める (履歴の紐付け)。

## 4. ステージ構成

順序は提案書 §6 から変更している: **HWND 同定の根治 (提案書 S2) を最初に**やる。
現在進行中の振動バグを含む「窓の誤同定」クラスを最初に絶滅させるのが、以降の
ステージの検証を安定させる最短路のため。

### R0: スパイク — child viewport の HWND を geometry 非依存で取得できるか

- 提案書 §5.3 / §7-2 の未確認事項を確定する調査タスク。本体コードは変更しない。
- 指示書: [archive/detached/detached-rework-stage-r0.md](archive/detached/detached-rework-stage-r0.md)
- 成果物: 調査レポート + 使い捨てプロトタイプ。結論次第で R1 の実装方式が決まる:
  - (a) eframe/egui-winit/raw-window-handle 経由で直接取得できる → それを使う
  - (b) できない → 生成直前後の `EnumThreadWindows` 差分法 (提案書 §5.3)

### R1: HWND は生成イベントで 1 回だけ確定 — rect 捕捉を detached から全廃 (提案書 S2 / BA-1)

- R0 で確定した方式で、detached 窓 (active / passive とも) の HWND を生成時に確定し
  `IsWindow` 生存確認のみで運用する。
- **撤去対象** (完了条件として grep で 0 件を確認):
  - detached の host 同定経路からの `find_visible_thread_window_matching_rect*` 呼び出し
    (`capture_detached_viewer_host_hwnd_from_logical_rect` 系)
  - 振動バグ対応で入れた passive HWND 除外リスト
    (`find_visible_thread_window_matching_rect_excluding` + `passive_detached_host_hwnds`)
  - `detached_capture_rect_looks_like_default_viewport` 等、誤同定を前提にした防波堤
- **対象外** (rect 捕捉の非 detached 用途、そのまま残す):
  `raise_visible_thread_window_matching_rect` (窓 raise)、仮想デスクトップ同期
  (app.rs の `fs_viewport_virtual_desktop_synced_hwnd` 経路)、キー入力 subclass 導入
  (ui_fullscreen.rs)。→ R1 完了後に「確定 HWND レジストリを使う形に移行できるか」を
  別途評価する (backlog 化、必須ではない)。
- 実機 smoke: smoke-matrix S1/S3 の A/B 系 + 動画 detached の F12 往復。

### R2: 状態の集約 — `DetachedWindowRuntime` + reducer + placement 一本化 (提案書 S1 / BA-6, BA-7)

- **R2a** (挙動不変): `DetachedWindowRuntime { window_id, state, hwnd, linked, ... }`
  を導入し、R1 の hwnd registry を吸収。状態 enum
  `{ Opening, Active, Parked, ParkedLive, Resuming, Closing }` を定義し、既存遷移点
  から `transition_detached_window_state()` 1 本で**記録** (診断ログ専用の shadow state)。
  指示書: [stage-r2a](archive/detached/detached-rework-stage-r2a.md)
- **R2b** (挙動変更): Runtime を経由する park/close/activate routing と
  **§6-3 のメディア live-park (`ParkedLive`) を実装**する。純粋 reducer、合法遷移の
  検証、相互排他的な pending/flag の typed state 集約までを本来の完了条件とする
  (park 時にメディアは凍結せず native presenter を生かしたまま非アクティブ化、
  クリックで復帰、新メディア再生開始で旧窓 close)。実機 smoke は R2b 完了後に実施。
- **R2c**: placement を runtime に一本化 (settings は seed のみ)。
  **既定サイズ拒否ヒューリスティックは削除しない** (BA-5 が根治するまで防波堤として
  残す。2026-07-05 の実機で有効性を実証済み)。テスト検証のみで実機は最終 matrix に
  委ねる。

### R3: viewport identity を window_id に一本化 (提案書 S3 / BA-4)

- active / passive / fullscreen の ViewportId を window_id 由来に統一。
  `fs_viewport_generation` は content 世代専用に格下げ。
- 現行実装では `fullscreen_viewport_id()` が active session の `window_id` を最優先する。
  passive window も window_id 由来であり、このステージは実質完了している。
- active↔passive 切替・folder-nav reopen で OS 窓が作り直されないことをログで確認。

### R4 (実施判断はゲート C で): deferred viewport 化 + state machine 完成 (提案書 S4)

- `show_viewport_deferred` へ移行し「毎フレーム描かないと死ぬ」制約から解放。
  keep-alive / holdover / backstop の特殊フレーム分岐を撤去。
- リスク最大。R3 まで到達した時点の安定度と残バグの性質を見て、実施可否を
  ユーザー + ClaudeCode で判断する。R3 までで smoke が安定していれば見送り (= 出荷) も可。
- 現行には keepalive / holdover / backstop が残り、通常 render と複数の描画入口を構成する。
  single render entry 化は未実施である。

## 5. ゲート (ステージ間の判断点)

- **ゲート A (R0 完了後)**: 取得方式の確定。ClaudeCode が R1 指示書を方式に合わせて作成。
- **ゲート B (R1 完了後)**: 「窓の誤同定」クラスのバグが実機で消えたかを確認。
  ここで振動・小窓フラッシュ・host_lost ループの再発が観測されたら、原因を BA に
  対応付けて計画を修正する (先に進まない)。
- **ゲート C (R3 完了後)**: R4 の実施可否 + 出荷判断。出荷基準は §7。

## 6. スコープ判断 (2026-07-06 確定)

1. **動画 detached は第 1 弾に含める** (ユーザーが実使用しており、§6.3 の新要件も
   動画前提のため)。
2. **paused_bundle による復帰モデルは維持** (Fable 判断): R1 系の修正後、クリック
   限定の復帰は安定動作している。bundle を捨てて再オープンにすると zoom / ページ
   状態が失われる UX 退行になるため、割り切り案は採用しない。
3. **新要件: メディア窓の live-park (ユーザー決定 2026-07-06)**。再生中の動画 / 音声の
   detached 窓は、別の窓をアクティブ化しても**閉じずに再生を継続**する
   (現行は park 不能 → `close_legacy_detached` で閉じてしまう)。仕様:
   - live-park は OFF / ON の両モードに適用。メディア窓は常に最大 1 本
     (再生エンジンが 1 本のため)。当初案にあった pin は後続 CUT で撤去済み。
   - 非アクティブ中は映像と音声のみ継続。**操作は最初のクリックで復帰のみ**
     (シークバー等は復帰後に有効化。passive のクリック限定ルールと一貫)。
   - 別のメディアを新しく再生開始したら、live-park 中の古いメディア窓は**閉じる**。
   - 実装は R2b (状態 `ParkedLive`)。仕様書 implementation-plan への反映も R2b。

## 7. 出荷ゲート (リリース基準)

- [smoke-matrix](archive/detached/detached-viewer-smoke-matrix-20260630.md) の 3 設定セット × 全ケースを
  **連続 2 回**グリーン。
- マスタープランの基準は、その後 **2 週間の実機常用で新規 P1 ゼロ**
  (P2 以下は backlog 化して出荷可)。
- ただし 2026-07-07 の当該リリース出荷判断では、ユーザー判断により
  [出荷前チェックリスト](detached-rework-ship-checklist.md) の **1 日程度**へ短縮した。
  これは release-specific override であり、一般基準の変更ではない。
- `panic.log` に detached 起因の新規 panic なし (Y-32 / OOM 含む)。
- 満たせない場合は「設定既定 OFF の実験的機能」としての出荷、または動画 detached の
  部分封印 (§6-1) へフォールバック。

## 8. 現在の未コミット作業の扱い

振動バグ対応 (rect 捕捉への passive HWND 除外リスト、2026-07-05 時点で Codex が
作業中・未コミット) は**応急処置として完成させてコミットしてよい**。ただし:

- コミットメッセージに「stopgap: detached-rework R1 で除外リストごと撤去予定」と明記。
- これが landed してから R0 に着手する (作業ツリー衝突を避ける)。

## 9. 進捗記録

### 9.1 現況 (唯一の進捗表、2026-07-26 コード確認、R2e 分を 2026-08-25 に反映)

この表だけを現行ステージ判定に使う。後続のコミット・検収記録は、各時点の実装履歴であり、
現在の完了判定ではない。コードから実装を確認できた範囲を記載し、7 月 24〜26 日分を含む
直近変更の手動検証状況は未確認とする。

| ステージ | 現況 | コード上の到達点 / 残件 |
| --- | --- | --- |
| R0 | **完了** | geometry 非依存の生成前後 HWND registry 方式を採用済み |
| R1 | **完了** | detached host HWND は registry 所有へ移行済み。キー入力 subclass の rect 探索は R1 の対象外として残り、BA-1 の後続課題 |
| R2a | **完了** | `DetachedWindowRuntime` と manager を導入済み |
| R2b | **部分完了 (R2e で 1 軸閉じた)** | Runtime routing と `ParkedLive` は実装済み。HWND 再生成・差分登録・watcher repair は `ParkedLive` を保持し、OS host 状態だけで live media state を降格させない。**所有の型化 (R2e、2026-08-25 完了) で、bundle の保管場所は registry に一本化され、mount / build / fork / retire / promote の transaction でしか遷移できない**。残るのは **純粋 reducer** と、**所有以外の散在 pending / flag の typed 集約** |
| R2c | **完了** | placement は runtime 所有、settings は seed |
| R2d | **完了** | live-park と active 復帰の直列化を実装済み |
| R3 | **実質完了** | active session の `window_id` を ViewportId 決定で最優先し、passive も window_id 由来 |
| R4 | **未完** | keepalive / holdover / backstop はあるが、deferred viewport 化と single render entry 化は未実施 |
| CUT | **完了** | 現行は OFF=連動 1 窓、ON=独立複数窓。pin は撤去済み |

完了済みの拡張と、後続 lifecycle 整理は次のとおり。

| 項目 | 現況 | コード上の到達点 / 残件 |
| --- | --- | --- |
| stage-audio | **完了** | 音声も動画と共通の単一メディア窓へ接続済み |
| stage-folder-nav | **完了** | 独立静止画窓の通常画像 / PDF / ZIP の物理フォルダ移動を context-owned pending で実装済み |
| physical open routing | **実装済み** | strict target と detached `window_id` を維持して bundle 内で poll / apply |
| terminal close routing | **完了** | 7b991af0 の `ViewerContextBundle::cancel_all_context_work` と `Drop` 集約により、terminal retire は context-owned の全 in-flight producer を停止する。有限 pending は各 pending 型の `Drop` を維持 |
| nav lock ownership | **実装済み** | lock generation と遅延 nav intent は `ViewerContextBundle` 所有 |
| holdover ownership | **実装済み** | holdover texture は `ViewerContextBundle` とともに mount / swap |
| manager / session 分離 | **実装済み** | `DetachedWindowManager` と `ViewerSession` を導入済み |

### 9.2 実装・検収履歴

以下は当時の状態を保存した履歴である。たとえば R2b の当時の検収合格は、現在の
R2b 完了条件 (純粋 reducer / 合法遷移 / typed state 集約) を満たしたことを意味しない。

| ステージ | 状態 | 指示書 | 完了日 / メモ |
| --- | --- | --- | --- |
| R0 | **完了** (da4c6b5d、実機ログ解析済み 2026-07-05) | [stage-r0](archive/detached/detached-rework-stage-r0.md) | 差分法合格 (8/8 生成が created_count=1、同期生成、混入ゼロ)。旧 rect 捕捉は同一 rect に 9 HWND 重畳で交互捕捉チャーンを実測 = 振動バグの直接証拠。所見は [r0-report](archive/detached/detached-rework-stage-r0-report.md) 末尾 |
| R1 | **実装済み・検収合格 (01727a4c)。ゲート B: 振動/rect 誤同定の解消は確認、新規露出 2 件 → R1b へ** | [stage-r1](archive/detached/detached-rework-stage-r1.md) | 正味 −271 行、registry 一本化。ゲート B 実機 (2026-07-05) で①フォーカス到達だけで passive がアクティブ化しピンポン (`via=focus`)、②show 中の窓再生成を registry が取り逃がす穴 (no_new_window リトライ 37f) を発見 |
| R1b | **実装済み・検収合格 (41cfdc08)。ゲート B 再実施: R1/R1b 部分は正常動作を確認、別の既知 P2 が露出 → R1c へ** | [stage-r1b](archive/detached/detached-rework-stage-r1b.md) | クリックのみ復帰・focus 時間窓削除・未請求窓消去法。再実施実機 (2026-07-05) でクリック限定/registry/即時回復は正常動作。露出 2 件 = ①未 gate の font atlas resync (`native_video_backdrop_hide`) の discard パスが passive 窓を破棄→再生成 (BA-5 の 2 度目の実害、ピン再設計時の既知 P2 残)、②PDF 見開きの pause snapshot が単ページ凍結 (当初からの積み残し) |
| R1c | **完了** (727507e8、検収合格 + 実機で見開き/動画 F12 の解消確認 = **ゲート B 通過**) | [stage-r1c](archive/detached/detached-rework-stage-r1c.md) | 全 resync を gated wrapper に一本化 (遅延不可 reason なし)、見開き 2 ページ凍結。方針変更: 既定サイズ拒否ヒューリスティックは BA-5 根治まで残す |
| R2a | **完了** (807dbbf7、Fable 検収合格 2026-07-06) | [stage-r2a](archive/detached/detached-rework-stage-r2a.md) | Runtime 導入 + registry 吸収 + shadow state。state の読み取りは診断ログ 1 箇所のみ = 挙動不変を確認。hwnd テスト 7 本緑 |
| R2b | **fix3 (c192a09a) まで適用済み・Fable 検収合格。実機 gate は帰宅後バッチで** | [stage-r2b](archive/detached/detached-rework-stage-r2b.md) | 所見履歴: [findings-1](archive/detached/detached-rework-stage-r2b-findings-1.md) F1〜F4 → fix1 (76f36d94) / [findings-2](archive/detached/detached-rework-stage-r2b-findings-2.md) F5 クリック復帰不能 → fix2 (bfc6f530、実機確認済) / [findings-3](archive/detached/detached-rework-stage-r2b-findings-3.md) F6 メイン文脈持ち逃げ + F7 復帰時窓再生成 → fix3 (preserve_main_context clone 戻し + 復帰 commit を passive render 後に遅延、テスト固定)。F7 は **BA-5 gap フレームと確定 (実害 3 件目) → エスカレーション条件成立、R2d 新設** |
| R2c | **完了** (5858e391、Fable 検収合格 2026-07-06) | [stage-r2c](archive/detached/detached-rework-stage-r2c.md) | placement 旧フィールド grep 0 件・既定サイズ拒否は runtime 付け替えで温存・入力経路表 (§3.0.1) 追加。⚠ 報告 2 件は既知/backlog: 当時は音声 F12 detached が意図的 no-op (音楽 Inc7 で先送り、後続 stage-audio で解除)、gamepad folder-nav の foreground 依存は rework 外 backlog |
| R2d | **完了** (本体 55643b7b + fix1 67996621、実機確認済み 2026-07-06 夜) | [stage-r2d](archive/detached/detached-rework-stage-r2d.md) | 実機バッチで live-park 通し・メイン文脈無傷・動画窓 HWND 安定を確認。fix1 = deferred→immediate 復帰の未登録フレームを「activation commit の次 root frame 遅延」で解消 (実機でちらつき消失確認)。**残リスク: ParkedLive は BA-5 非免疫** (ゲート C / R4 で再評価) |

| findings-4 | fix (5a130ec9) 適用も **F8-v2 実機 NG** → CUT で経路ごと削除する方針に転換 | [findings-4](archive/detached/detached-rework-findings-4.md) | ログ確定: 連動窓の park が `park_legacy_active_to_passive` で Parked→Closing に上書きされ、Closing ガードが復帰要求を無限に無視。F9 (session_begin ストーム) / F10 (ログ間引き) は 5a130ec9 で対応済み |
| **CUT** | **完了** (実装 d3b56c15/64cddcab + fix1 f966a68c + fix2 5a71c196 + fix3 500cbd1f。§6 smoke 全消化 2026-07-07: OFF live-park 通し / ON 独立 3 枚 / ピン不在 / 設定 ON⇔OFF 切替の全窓クローズ + トースト OK) | [stage-cut](archive/detached/detached-rework-stage-cut.md) | **連動時の任意ピンを撤去、2 モード制へ** (ON=全窓独立/OFF=連動 1 枚のみ・passive にならない)。連動⇔独立境界のバグクラス (F8 系 3 連続) を設計から削除。設定切替時は全窓自動クローズ + トースト。live-park は両モード維持。ピンは将来「独立窓として複製」意味論で再導入可 |

| findings-8 | A1v4 (0f57c361) / A2 (939825b7) 適用済み | [findings-8](archive/detached/detached-rework-findings-8.md) | クリック取りこぼしを watcher スレッド化 (A1v3→v4)、open フラッシュを初回描画まで非可視化 (A2) |
| findings-9 | B1 (cff57ab1) / B2 (0f264f91) 適用済み。**B1 が多窓 churn の回帰を導入 → findings-10 へ** | [findings-9](archive/detached/detached-rework-findings-9.md) | B1 = stale-hwnd クリック棄却の解消 (生存監視 + watcher 修復) は成功。ただし追加した clear-on-park が回帰源。B2 = texture defer デカップリングは成功 (実機で deferred_for_resync=0) |
| findings-10 | **完了** (4901ac4f、実機で切替/PDF サムネ/別窓 PDF の解消確認 2026-07-07) | [findings-10](archive/detached/detached-rework-findings-10.md) | **B1 の clear-on-park (`handoff_clear_host_for_deferred`) を撤去**。park のたびに登録 hwnd を捨てる → 未確定化 → R2d 直列化が `show_viewport_deferred` をスキップ → egui が OS 窓を破棄・再生成する多窓 churn (実機 4 症状の主因)。証拠 = id=2 の park→再採用が短間隔では同一 HWND (窓生存)・多窓競合の 4.4s 間隔でのみ HWND 変化。B1 の生存監視/watcher 修復・B2 は残す |
| findings-11 | **指示書作成済み・実装待ち** (Fable 診断 2026-07-07) | [findings-11](archive/detached/detached-rework-findings-11.md) | C1 = **fa09cc5a (サムネ stale 掃除) の revert**: requested+Evicted は enqueue 後の正常 in-flight 状態で、stale と誤判定して毎フレーム remove→重複 requeue のループ (同一 idx に 2842 回/68s、55 idx)。ログ spam 16MB/68s が close 問題の証拠をローテートで破壊。C2 = 窓 close 時の別窓 消失/突然表示/再表示 (churn とは別機構)。C1/C3 適用後に MIV_DETACHED_WINDOW_DEBUG=1 で再現→機構確定 (修正は Fable 承認制)。C3 = logger デバッグ保持モード (デバッグ env フラグ時のみ 256MB × .bak1-4 の 4 世代、通常 16MB×1 は不変) = 手動ログ退避の廃止 (証拠喪失 2 回の再発防止、ユーザー承認 2026-07-07)。**C1 (c12d721b) / C3 (0a795d64) 完了・検収合格**。C2 の再現ログは C3 初回稼働で完全取得 → findings-12 で機構確定 |
| findings-12 | **完了** (D1=322c37a8 / D2=2a9f9a4e / D3=e362f3d7、検収合格。実機確認 4 点すべてクリア 2026-07-07: 複数窓切替/close OK・book open 時のグリッド静止 OK・読み込み中 book open のサムネ継続 OK) | [findings-12](archive/detached/detached-rework-findings-12.md) | D1 = close 時フラッシュ: font resync 発火の early-return (update_early, app.rs:44851) が render_detached_image_windows (45667) より前で return し deferred 登録ゼロの pass が発生 → egui が parked 窓を破棄 (BA-5 実害 4 件目)。再生成は initial_placement_applied=true のため既定 533x400 小窓で出現 = フラッシュ。修正 = early-return 前に deferred 登録 + hwnd clear 時に placement 再適用リセット。D2 = watcher repair が close 直後の dying hwnd (未請求化直後・生存中) を隣の窓に geometry 根拠で養子縁組 + 同一クリック二重解釈 (× 押下で隣窓 activate)。修正 = repair は対象窓の登録 hwnd が 0/死亡のときのみ。D3 = book open が main context 経由で load するため bundle 外グローバル (auto_aspect / queue+worker) が汚染 → ①メイングリッドのアスペクトリフロー = 「スクロールする」の正体 ②bundle 復元で requested が孤児化 = 元の「サムネ停止」の真の leak point (Codex の観測は実在、fa09cc5a は検出方法だけが誤り)。修正 = auto_aspect を bundle に追加 + swap-in で requested/pending_finalize クリア |
| findings-13 | **Phase 1 完了・クローズ** (計装 3bc9ce77、Fable 解析 2026-07-07: 全 2681 フレームで paint 欠落なし + 録画の合成結果クリーン = 消去法で scanout/DWM flip 遷移レベル [アプリ外] と確定。Phase 2 実施せず P3 環境依存としてクローズ、出荷ブロッカーにしない) |
| stage-settings | **完了** (e4e8535e、検収合格 2026-07-07。実機は ship-checklist と合流) | [stage-settings](archive/detached/detached-rework-stage-settings.md) | 設定 UI を「ビューワモード」(フル機能ウィンドウ / 複数ウィンドウ) の 1 構造に再構成。**複数ウィンドウモードは ZIP/PDF/本を常に直開きに固定** (ON×ページ一覧の組み合わせを設計から削除 = 検証パターン削減)。保存キー 3 つは不変 (auto_fullscreen_zip_pdf は v2.2.0 リリース済みのため実効値ヘルパーで override、マイグレーション不要)。findings-7 の ON×直開きOFF 象限テストのみ書き換え許可 |
| stage-audio | **完了** (Phase I=b2063ef3 / fix1=d9bb93c4 / fix2=27704d25 / fix2b=0925d749、検収合格 2026-07-07。実機 = 症状 A/B とも解消確認は fix2b 後の checklist と合流) | [stage-audio](archive/detached/detached-rework-stage-audio.md) | 音声 detached の no-op 解除 (F12 含む)。メディア窓は動画と共用 1 本・live-park 同規則・music_* 非 bundle 化 (メディア窓 1 本規則で混線を構造排除)。fix1 = ParkedLive 音声窓の最小ライブ描画 (方式 a)。fix2 = F11 を toggle_egui_viewer_window_mode_for_input に集約 (detached 中は borderless 切替)。fix2b = 音声モード中の host resync / presenter rect sync を no-op 化 (video_audio_mode_hides_native_presenter_for 所有境界、VST 中は除外) = F11 settle が hidden presenter に SwitchPlacement を投げて再表示される問題の根治 |
| findings-14 | **完了** (計装 6727e713 / 修正 5f6cd105、検収合格 2026-07-07) | [findings-14](archive/detached/detached-rework-findings-14.md) | 動画 detached 窓のドラッグ後振動 = 動画中毎フレーム seed (BA-5 防波堤) の builder position が 1 フレーム遅れの保存値を live 追従 → egui patch との周期 2 遅延フィードバック (最後の 2 報告値を永久往復)。修正 = builder_placement_latch (真の seed 契機のみ更新・生存中は定数) = echo 構造的消滅・フラッシュ防波堤は維持 |
| findings-15 | **完了** (27201f0c) | [findings-15](archive/detached/detached-rework-findings-15.md) | parked 窓復帰の速いクリックが up_dragged で棄却され 2 クリック必要 (G1) + × ボタンの間欠不発 (G2)。watcher / deferred 入力の findings-8 世代由来 (リワーク新規退行ではない)。ドラッグ判定を押下位置で層別 |
| findings-16 | **完了** (e3acbc76) | [findings-16](archive/detached/detached-rework-findings-16.md) | メイングリッドのフォルダ移動 (epoch bump) が detached PDF 窓の先読みページレンダを stale prune → `FsLoadResult::Failed` が fs_cache に焼き付き「デコード失敗」表示。フルスクリーン PDF レンダをグリッド epoch の対象外に |
| findings-17 | **完了** (cc488bd9) | [findings-17](archive/detached/detached-rework-findings-17.md) | 動画 live-park でメイングリッドの自動比率が 1:1 にリセット (findings-12 D3 の取り漏らし = `clone_current_viewer_context_grid_fields_into` に auto_aspect が無く default で復元)。clone 対象に追加 |
| findings-19 | **fix15 まで適用・検収済み** (eac47547〜01910684、実機 OK 記録あり: fix10〜12) | [findings-19](archive/detached/detached-rework-findings-19.md) | ON モードのアクティブ切替が close+reopen 往復で窓を再構築し高速切替で窓が消える (A/P1) + アクティブ化時に画像がわずかに移動 (B/P2)。fix10a=in-place park / fix10b=snapshot rect 一致 / fix11=passive の OS × ボタン / fix12=VST UI を main fullscreen 限定 / fix13 系=凍結見開きの trim/clip パリティ / fix14=trim bbox の runtime 焼き込み / fix15=trim 状態の viewer context 保持 |

**当時の判断: ゲート C (2026-07-06 到達)、スコープ決定 = CUT
(2026-07-07)。現在のステージ判定は §9.1 を正とする。**
**2026-07-09 追記: 出荷前品質レビュー (docs/archive/review-v2.3.0/final-report.md) で BA-7 系の構造修正を実施 (コミット 1bb26360): ロード複合体 (thumb channel / cancel_token / worker queue / keep atomic) の ViewerContextBundle 化 + bundle Drop teardown + legacy ParkedLive park の複合体引き継ぎ + unclaimed HWND fallback の可視性フィルタ (BA-1/BA-4)。凍結例外はユーザー指示。実機確認は archive/review-v2.3.0/fix-verification-checklist.md。**

**2026-07-09 夜 追記 (実機検証で出た findings-19 続報 + stage-media-window 前倒し、凍結例外はユーザー指示):**

- **クリック照準バグ** (findings-19 実機 3 件目の真因): parked 窓が重なっていると、activation watcher の down-edge 照準解決が「targets リスト先頭からの矩形あたり判定」で背面窓を選び、実クリック先 (cursor_root_hwnd) と不一致 → クリック丸ごと棄却。修正 = hwnd 一致 target を最優先で解決 (`detached_activation_target_for_cursor_root`)。リスト順 ≠ z-order。
- **stale window_id 再利用** (d6bf04f0) / **legacy park のゴースト連動セッション** (5c9be17d): 同夜の実機 2 件、個別修正済み。
- **連動 park 画像窓の復帰フォールバック**: 連動セッション由来の parked still (descriptor=none) は main の一覧変更で stamp_not_resolved になり復帰不能だった。`ViewerContextDescriptor::Image` を parked snapshot 専用フォールバックとして追加 (open ルーティングは不変)。activation は stamp 優先・解決不能時のみ親フォルダを窓内コンテキストとして開き直す。
- **stage-media-window (旧バックログ §1.7) 前倒し実装**: フル機能モードのサブオプション「動画・音声は別ウィンドウで再生」(`Settings::fullfeature_media_window` + 派生述語 `effective_media_in_media_window()`)。メディア窓インフラはモード非依存のまま、入口 (`requested_viewer_presentation_for_open` / F12 / カーソル同期 skip / grid open 前の unbundled live-park) だけ実効述語化。

### 9.3 DetachedWindowManager 構造分離 (2026-07-15)

`App` が直接所有していた window_id ごとの runtime map、activation watcher、HWND 生存判定用の
テスト状態を `src/app/detached_window_manager.rs` の `DetachedWindowManager` へ移した。
`DetachedWindowRuntime` と watcher の型・worker 実装も同モジュールへ移し、`App` は設定への
placement 永続化、メディア presenter、ログなど複数領域をまたぐオーケストレーションだけを担う。

- runtime の生成、state / linked / placement / trim bbox / builder latch / HWND / deferred activation
  の変更は Manager の意味付き API を経由する。runtime map への直接変更は残さない。
- gamepad の固定入力は Manager が保持する最後の入力面を fallback に使う。通常は OS 前面 HWND
  を優先し、メイン HWND ならグリッド、それ以外の自プロセス HWND なら viewer を対象にする。
- マウス割り当てボタンは grid / viewer の呼び出し元が面を明示し、同じ context resolver へ渡す。
- gamepad picker overlay と中ボタン短クリック状態も発火面を所有する。別 viewport の描画・
  空入力 pass が、所有面の picker を二重描画したり短クリック開始状態を消したりしない。
- `ViewerContextBundle` の swap 廃止、`ViewerSession` / `MediaWindowSession` 導入、native presenter
  再設計はこの段階の対象外。Manager 抽出後も既存の表示・park / resume 意味論は維持する。

### 9.4 ViewerSession 準備分離 (2026-07-16、実機確認済み)

退避中の `ViewerContextBundle` が個別フィールドとして持っていた次の session 意味状態を
`src/app/viewer_session.rs` の `ViewerSession` に集約した。

- `ViewerPresentation`
- 最後に同期した `ViewerSyncStamp`
- independent detached の active 状態
- 次の静止画を detached で開く one-shot 状態
- session が使う detached window ID

現在表示中の session は、互換性を保つため引き続き `App` の既存フィールドへマウントする。
bundle との交換は `ViewerSession::swap_with_mounted` に集約し、5項目の追加・交換漏れを純粋な
round-trip test で固定した。退避 bundle を independent detached として再開するときも、4項目の
identity tuple を `activate_independent_detached` で同時設定する。

この段階では挙動変更を行わず、次は対象外とする。

- `ViewerContextBundle` 全体の swap 廃止、または表示中 `App` 状態の全面的な session 化
- `pending_detached_video_host_switch`、`video_audio_*`、`music_*` など media 固有状態
- HWND / placement / activation watcher (`DetachedWindowManager` 所有)
- native presenter / `MediaWindowSession` の再設計

自動テストに加え、active / passive 切替、ParkedLive 復帰、複数 detached 窓の切替・close を
Windows 実機で確認済み。次は表示中 `App` の session 化か `MediaWindowSession` のどちらへ
進むかを判断する。

### 9.5 R2 残件の再定義と、依存する 2 件の先行実施 (2026-08-21)

R2b の残件 (純粋 reducer / 合法遷移制約 / 散在 pending・flag の typed 集約) と、それに依存すると
記録されていた backlog §1.99 / §1.100 に着手した。**設計を 2 周レビューした結果、作業順を
§1.99 → §1.100 → R2e (所有の型化) に変更した** (利用者判断 2026-08-21)。

**所有の型化 (R2e) の設計は 2 版とも落とせず、第 3 版が要る。**
正本は [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)。
第 1 版 (単一スカラーで「今マウントされている所有者」を表す) は、次を表現できずに破棄した。

- **Vacant**: `take_current_viewer_context_bundle` は App に**空 bundle** を残す (`src/app.rs:16067`)
- **Building**: 新しい detached context は App のフィールド上で組み立てられる
  (`src/app.rs:39778` → `39885`)。identity は**予約済みだが未 commit** (serial は `39793` で払い出し済み)
- **押しのけられた bundle はスタックローカルにあり `self` から到達できない** (`src/app.rs:16103`)
- **owner に window_id は使えない**。フォルダ再オープンで意図的に再利用される (`src/app.rs:37568`)

第 2 版 (phase + slot map + 復元スタック) にも BLOCKER が 4 件残っている (同書 §6)。
所有の transaction が無い / `window_id → ViewerContextId` の対応表が無い /
ステージ分割がコンパイルできない切り方になっている / 「組み立て中は identity が無い」が誤り。

**第 3 版を書いた (2026-08-23、branch `r2e-ownership`、worktree `C:\home\mimageviewer-r2e`)。
Codex レビュー 6 巡で BLOCKER 0 に到達。**正本は同じファイル
[briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md)。要点:

- 診断は 1 つ ——「1 個の保管場所の値の有無で、identity 以外の軸 (所在 / 駆動者 / 時制) を
  答えている」。**修理は 3 箇所に分かれる**: 所在 = R2e、右ドラッグの producer = R2f、
  placement の現在/復元ジオメトリ = レーン A-0 (§1.4)。R2e に 3 件全部を持ち込まない。
- 保管を 1 箇所に統合する。`active_detached_viewer_context` と `paused_bundle` の往復を
  registry の slot 1 種類にし、active か parked かは `DetachedWindowRuntime.state` だけが持つ。
- `ViewerContextId` に `Main` 変種を置かない (`Main` は identity ではなく binding)。
- 所有を動かす操作は mount / build / fork / retire / promote の 5 transaction だけ。
  生 bundle を返す API を 1 つも置かない。
- binding は registry 所有で `bind` / `unbind` / `transfer` の 3 本。build 中の binding は
  予約として積み、commit と同時にだけ公開する。
- 完了確認の第一の門は **Rust の可視性** (型を registry モジュールへ移してフィールドを private に
  すると、生成 / destructure / 生 swap がモジュール外で書けなくなる)。syn 監査 (A1〜A7) は
  可視性で表せない残りだけを見る。塞げない穴 (マクロ展開 / ローカル関数エイリアス /
  別モジュール再エクスポート / 意味的到達可能性) は明記した。
- ステージは ① 抽象データ構造 → ②-pre 手書き mount の helper 化 → ②-a 終端 digest →
  ②-b 型の移設 → ②-c accessor と監査 → **②-d 保管・binding・transaction の一括切替 (1 コミット)**
  → ②-e 完全 private と allowlist → ③ 巡回 → ④ 非同期 identity。各段の終了時に
  コンパイルが通る状態を明記した。

レビューで訂正した事実 (実装時に踏まないこと):

- **folder-nav reopen (Ctrl+↑↓) は「同じ窓・同じ context・新しいフォルダ」**。
  `close_fullscreen_for_folder_nav_reopen` ([app.rs:52211](../src/app.rs:52211)) は
  マウント中の context を保つ。window_id 再利用は ViewportId を安定させるためで、
  context の差し替えではない。
- **生きた context 同士で窓が移る経路は実在する**: live-media fork
  ([app.rs:41868](../src/app.rs:41868))。`transfer_window_binding` が要る。
- **build transaction は 2 本ある**: `start_active_detached_book_context_with_start` と
  `open_bookmark_media_in_detached_context` ([startup_ops.rs:608](../src/app/startup_ops.rs:608))。
  後者には正常な失敗経路があるので `BuildOutcome::Abort` が要る。
- **App の投影は空にできない** (225 個の実フィールドで `Option` ではない)。
  「取り出す」と「空を据える」は 1 動作。
- abort の drop は **main を戻した後** ([startup_ops.rs:652](../src/app/startup_ops.rs:652)〜654)。
- `ViewerContextBundle` は **225 フィールド**。tests.rs 側の参照は 294 行。

ステージ①の実装指示書は
[detached-rework-stage-r2e-1.md](detached-rework-stage-r2e-1.md) (2026-08-23)。
`ContextTable<P>` (payload ジェネリックの状態機械) を 1 モジュール追加するだけで、
production は `mod` 宣言 1 行しか変わらない。**モジュール外への公開は 0 件**
(②-e で入れる監査 A4 の allowlist が空から始まる)。実機 smoke 不要。

**ステージ① 完了 (2026-08-23、実装 = Codex / 検収 = ClaudeCode)。**
`src/app/viewer_context_registry.rs` (1,400 行弱) + `src/app.rs` の `mod` 宣言 1 行のみ。
挙動不変 (誰からも呼ばれない)。registry テスト 22 本、ライブラリ全体 6224 本が緑、
`cargo fmt --check` clean、公開項目 0、非 test cfg 0、既存テストファイルの差分ゼロ。

検収で見つかった **BLOCKER 2 件** (いずれも修正済み。②以降で同じ罠を踏まないこと):

1. **`main` が指す context を retire できてしまい I4 が回復不能に壊れた。**
   fork → 別 context を mount → 旧 main が `AtRest` → `begin_retire(main)` が通る。
   以後 `main()` は Retired な id を指し、mount も promote もできない。
   → `RetireError::IsMain` で拒否する。**main は `promote` で置き換えるもので、破棄しない。**
   「`main` が非 Option だから I4 は守られる」は**誤り**だった (名前が常にあることと、
   指す先が存在することは別)。
2. **op ベクタのテストが fork の policy を固定していなかった。**
   `plan_fork(LiveMediaPark)` が `ForkProjectionIntoTransient(MaterializedStillOpen)` を
   返す実装でも全テストが通った。②-d では policy が 225 フィールドの move / clone 分けを
   決めるので、間違えると静かに壊れる。→ 両 policy の完全一致 assert + 派生 payload の検証。

**②-d への申し送り**: ①の failpoint sweep は **op と op の境目**にしか panic を注入しない。
op の内部で binding を早く公開して正常終了までに戻す実装は検出できない。production の
`ReplaceProjectionWithFreshEmpty` / `RestoreProjectionAndDropDisplacedEmpty` は
`swap_viewer_context_bundle` を通り、その中で rating 同期と visible index 再構築が走る
= **op の内部で panic し得る**。②-d では swap の内側にも failpoint を刺すこと。

**ステージ②-pre 完了 (2026-08-24、実装 = Codex / 検収 = ClaudeCode)。実機 smoke 待ち。**
指示書 [detached-rework-stage-r2e-2pre.md](detached-rework-stage-r2e-2pre.md)、
憲法 §2 の合意は §11 の 2026-08-23 の項。手書き mount 18 箇所 (active 9 / parked 9) を
`with_active_detached_viewer_context` と新設 `with_paused_detached_context` の 2 本へ集約。
`src/app.rs` は正味 19 行減、`src/app/tests.rs` は 592 行追加・**削除ゼロ**、
ライブラリ全体 6235 本が緑。mount inventory は active take 4 件 / parked take 5 件
(= helper 2 本 + 変換しない 7 箇所) で手書きは 0。

検収で見つかった **P1 1 件** (修正済み):

- **parked-live の入力 marker が panic でリークした。** VST3 の deferred consume は
  marker を立てて実行し**後で**戻していたので、consume が panic すると helper が投影と
  bundle を復元する一方 marker は `Some(id)` のまま残り、**「main がマウント中なのに
  marker は parked 窓を指す」**という旧コードには作れない状態になった
  (旧コードは panic 時に parked 投影をマウントしたまま落ちるので両者は一致していた)。
  → 呼び出し元側で `catch_unwind` → marker 復元 → `resume_unwind`。
  **helper は marker 非関与のまま** (§11 の合意どおり)。

**「落ちないテスト」を 4 件強化した**(いずれもこのステージ特有の誤実装を通してしまう形だった):

- 窓が closure 内で閉じられるテストに**後続の兄弟窓**を足した (無いと、id ではなく
  保存した index で戻す誤実装が通る)
- 累積値の不変テストを **true 始まり**にした (false 始まりだと「代入で累積を潰す」誤りを検出できない)
- 「既にマウント中の context も処理される」を、**どの context に何回走ったかの列を完全一致で
  検証**する形にした (「1 回以上」だと parked ループを丸ごと飛ばす実装も二重実行も通る)
- panic 経路そのもののテストが無かったので追加した

**②-d への申し送り** (設計 §7 ②-d に記録):
現行の全 context 操作は「マウント中を先に処理 → その後 parked を巡回」で、マウント中のものは
巡回に含まれない。②-d の統一 mount は「既にマウント済みなら swap せず `f` を実行」するので、
**巡回側にマウント中の id が含まれると `f` が 2 回走る**。上の exactly-once テストが
それを捕まえるので、②-d で弱めないこと。ほかに、②-pre が holder 側に残した前置きフィルタ
2 箇所 (vst3 の pending 判定 / rename 述語) を `ContextRef` 経由へ移すこと。

**実機 smoke (2026-08-24) で挙がった 2 件** — どちらも②-pre の退行ではない
(`release_viewer_surfaces_for_removed_paths` も `stale_before_apply` も
`git diff 488e00f0..HEAD -- src/app.rs` に 1 行も出てこない):

1. **ファイル名変更で開いていた PDF ウィンドウが閉じる → 仕様として確認済み (利用者判断 2026-08-24)。**
   `poll_rename_pending` ([ui_dialogs/rename_item.rs:139](../src/ui_dialogs/rename_item.rs:139)) が
   `release_viewer_surfaces_for_removed_paths(.., "rename_old_path")` を呼び、旧パスを表示している
   viewer を active / parked とも閉じる ([app.rs:27709](../src/app.rs:27709))。
   **新パスで開き直す形には変えない。** 動画では開き直しても再生が一度止まるので、
   「移動されたら閉じる」方が動きとして自然、という判断。
   ⚠ **将来これを「症状」と誤認して開き直しを実装しないこと。**
2. **PDF のレーティングが import 後に右パネルへ反映されない** → **表示側の既存問題だった。**
   import も refresh も正常で、**グリッドはコンテナキー、右パネルは現在ページのキー**を読んでいて
   どちらも正しい値を出していた。パネルにコンテナ★を出す行は存在したが、対象を
   **タグ行のラベル文字列 `"フォルダ"` を探して**決めていたため、PDF ページ (ラベル `None`) では
   行が見つからず出なかった。→ `current_container_rating_target()` を正にする形へ修正 (fa731724)。
   ついでに ZIP 内ディレクトリの合成キー誤読と、コンテナ★書き込みの種別 (`Folder` 固定) と
   Undo 欠落も直った。
3. **インポートで別ウィンドウの動画が停止し、窓が真っ黒になる** → **既存の重大な不具合だった**
   (導入は 2026-07-23 の `c169a081`)。代表サムネの更新が **`load_folder()` = ナビゲーションの
   プリミティブ**を呼んでおり、`start_loading_items_inner` → `close_fullscreen` まで走っていた。
   影響 context ごとに 1 回走るので、parked-live の動画はプレイヤーがキャッシュから落ちて
   presenter が停止し、アクティブな静止画 context は静止 snapshot 化されて次フレームで
   bundle が drop された (detached 窓が main 由来の `items_gen` を持っていたのはこれ)。
   → キャッシュ更新をナビゲーションで書くのをやめ、影響サムネの evict / 不要 `#pin:` 行の削除 /
   video pin の再 seed / context 所有の worker channel 交換に置き換えた (b3ea74bb)。
4. **★の Ctrl+Z がパネルから効かない** → **同じ書き込みなのに操作元で挙動が違っていた。**
   フルスクリーンのキーと動画 HUD は自分の呼び出し側で Undo を記録していたが、パネルと
   音楽パネルは記録していなかった。→ 記録を書き込みと同じ場所 (`set_rating`) へ移し、
   呼び出し側のコピー 2 つを削除 (6316102c)。グリッド一括は「操作 1 回 = 1 エントリ」を維持。

**実機 smoke 全項目グリーン (利用者確認 2026-08-24)。②-pre 完了。**

⚠ 上の 2〜4 はいずれも**②-pre の退行ではない**が、**レーンの外**の修正である。
2 と 4 は表示・Undo の一貫性、3 は「ナビゲーションのプリミティブをキャッシュ更新に再利用していた」
という R2e と同じ形の構造問題。3 の教訓は②-d に効く: **所有権を動かす経路を、別の目的で
呼び直さない。**

**ステージ②-a も完了** (2026-08-24、`3c435070`、指示書
[detached-rework-stage-r2e-2a.md](detached-rework-stage-r2e-2a.md))。
終端 2 本 — ブックマーク照合と メディア teardown — が生 bundle を外へ出さなくなった。
`ClosedBookmarkSummary` が reconcile の読む 4 値 + PDF パスワードの bool を運び、
teardown は窓ごとに plan → normalize clear → drop してから App-global の後始末をする。
**保管も挙動も変えていない。** lib テスト 6251 件緑。

指示書で「推測せず確かめろ」と書いた 2 点の結論:

- **PDF パスワードの畳みが `detached_viewport_finalized` 側に無いのは意図的。**
  両分岐とも `should_drop` の内側にあり、`should_drop` は
  `active_detached_transition_outstanding()` が偽であることを要求する。この述語は
  **detached bundle をマウントした状態で評価される**うえ `pdf_password_request.is_some()` を
  含むので、そこへ来る context は request を持ち得ない。畳みが効くのは gate の無い
  明示 close 経路 (`close_current_active_detached_viewer_context`) の方である。
- **teardown plan の入力は context を跨がない。** よって 2 窓目の plan を 1 窓目の drop 後に
  作っても値は同じ。加えて、App-global の後始末より前に走るようになった
  `normalize_ui_states` / `normalize_auto_scan_suppressed` の clear は
  **`Copy` の素データで `Drop` を持たない**ため観測不能で、後始末側は plans と main 自身の
  state しか読まない (`cancel_viewer_context_teardown_normalize` /
  `clear_music_state_after_viewer_context_teardown` を確認済み)。

⚠ **検収で 1 件差し戻した。指示書の要求が間違っていた。**
指示書に「追加テストは現行コードで必ず落ちること」と書いたが、**この段は挙動不変なので
挙動テストは原理的に落ちない**。実装側はそれを `include_str!("../app.rs").contains(...)` の
**ソース文字列 assertion 3 本**で満たしていた。これは (1) 完了判定を grep でやらないという
設計 §6 に反し、(2) 目的の文字列が消えた瞬間から永久に真になって何も守らなくなり、
(3) ②-b / ②-d で当の関数が移動すると腐る。3 本とも削除し、うち 1 本が守っていた
「窓が bundle を持たないとき」のテストは**混在ケース** (bundle 無しの窓を先頭に置く) へ
書き直した。

**挙動不変の段では「旧コードで落ちること」ではなく「間違った新コードで落ちること」を
要求する。** 今回はそれを mutation で実証した:

| 変異 | 落ちたテスト |
| --- | --- |
| bundle が無い窓で `continue` せず `break` する | `media_teardown_skips_a_bundleless_window_without_abandoning_the_rest` (新規) |
| tile-companion / mode-switch の判定を take の後に動かす | `parked_close_clears_bundle_owned_tile_companion_without_pending` (既存) |

2 つ目は、§3.3 の「判定をループより前から動かすな」を**既存テストが既に守っていた**ことの
確認でもある。

**ステージ②-b も完了** (2026-08-24、`fd121be3`、指示書
[detached-rework-stage-r2e-2b.md](detached-rework-stage-r2e-2b.md))。
`ViewerContextBundle` (225 フィールド) / `Drop` / `impl` 3 メソッド /
`swap_viewer_context_bundle` / `split_current_context_preserving_main_grid` の 5 ブロック
1602 行が `src/app/viewer_context_registry.rs` へ移った。**純粋な移設**で、5 ブロックの
SHA-256 が可視性接頭辞を除いて一致することを実装側が確認している。
lib テスト 6251 件緑 (件数不変)、`src/app/tests.rs` は無変更。

- **実効可視性は変わっていない。** 移設前は app.rs 内の private = `app` とその子孫から見える。
  移設後は `pub(in crate::app)` = 同じ範囲。**広げていない**ことを検収で確認済み
  (移設領域に `pub` / `pub(crate)` が 1 つも無い)。
- 素直に移すと壊れる 2 点を指示書で先に潰した:
  **①`impl App` のメソッドの private は「`impl` を書いたモジュール」の private** なので、
  移設した 2 プリミティブには `pub(in crate::app)` が要る。
  **②`#![allow(dead_code)]` はファイル全体に効く**ので、そのままだと production 型にもかかり
  未使用フィールドの検出が止まる。stage ① の非テスト 13 項目への個別付与に置き換えた
  (②-d で registry が本番に繋がったら全部外す)。
- ②-e で弾けるようになるのはこの移設の結果である。今はまだフィールドが
  `pub(in crate::app)` なので**何も弾けていない**。

**設計の②-c は 3 つに割った。** 「accessor 移行」と「監査ツール導入」は共有するものが無く、
accessor 側も**読みと書きで性質が違う**ため。②-c1 (読み) / ②-c1b (書き) / ②-c2 (監査ツール)。

**ステージ②-c1 完了** (2026-08-25、`2918e639`、指示書
[detached-rework-stage-r2e-2c1.md](detached-rework-stage-r2e-2c1.md))。
lib テスト 6251 件緑 (件数不変)。

### 対象を数えた — 見積もりを使わなかった

設計 B-3 の「~26 種」を使わず、**225 フィールドを一時的に private にして
`cargo check -p mimageviewer --bin mimageviewer-core` を通した**。設計 §6.1 の完了判定は
この性質そのものなので、ここで一度使って確かめた意味もある。

| | 箇所 |
| --- | --- |
| 外部アクセス 合計 | 164 |
| うち `split_materialized_physical_context_for_independent_still_open` の中 | 31 |
| うち **書き** | 27 |
| **読み (②-c1 の対象)** | **106** (21 フィールド) |

⚠ **見積もりとの差より重要だったのは、27 箇所が書きだったこと。** 書きに場当たりで setter を
生やすのは設計 §6.2 が名指しする「公開面が静かに育つ」に当たるので、②-c1b へ回した。

### ②-c1 で分かったこと

- **`ViewerSession` には mounted 側の実体が無い。** App は `last_viewer_sync_stamp` /
  `detached_viewer_window_id` / `detached_viewer_independent_active` を**別々のミラー
  フィールド**として持ち、`ViewerSession::swap_with_mounted` が対応付けている。よって
  `ContextRef` からは `&ViewerSession` を返せず、**成分 3 つの accessor に分解**した。
  「mounted 側は bundle の逐語ミラー」という素朴な前提が成り立たない実例。
- `contains_video` の 2 本は **`self.` と `bundle.` の違いだけで一字一句同じ**だった。
  3 経路が 1 本になった。`is_music_consumer` の片方は元から delegate だけの wrapper。

### 検収で 1 件差し戻した — `impl Into<ContextRef>`

helper 3 本が `impl Into<ContextRef<'a>>` を取り、`From<&ViewerContextBundle> for ContextRef`
経由で `&bundle` をそのまま渡せる形になっていた。**`src/app/tests.rs` を無変更にする**という
指示書の条件を満たすためだったが、
① 生の `&ViewerContextBundle` が helper 境界を**暗黙に**越え続ける、
② 設計 §6.3 の A4 が「ジェネリック境界と trait 実装は可視性だけ見る監査を素通りする」と
名指ししている形そのもの、の 2 点で採らなかった。helper は `ContextRef<'_>` を直接取り、
テスト 6 箇所が `ContextRef::at_rest(...)` と明示するようにした。
**「テストを触らない」は「API の変化をテストで糊塗しない」ための代理条件**であって、
読み口を明示するための 6 行の書き換えはそれに当たらない。

### ⚠ 数が合っても中身は変わり得る

完了判定を「private 化したとき残る E0616 がちょうど 27」としたが、**27 のまま中身が入れ替わって
いた**。私の分類器が `bundle.viewer_session.activate_independent_detached(..)` のような
「フィールド経由の mutating メソッド呼び出し」を読みと誤判定していたためで、実装側は
それらに狭い名前付き操作を足し (`activate_independent_detached_session` /
`pause_animations_for_remote_session` / `adopt_bookmark_media_open_pending`)、
代わりに catalogue 済みだった `details_order.clear()` を `clear_details_order()` へ変えていた。
差し引き 27 のまま。**リストを突き合わせるまで気づけなかった。数だけの門は弱い。**

### ②-c1b の材料 (②-c1 完了後の実測、`2918e639`)

残る 27 箇所。**この 4 本は既に②-c1 で名前付き操作になっている**ので、②-c1b は同じ形に揃える:
`clear_details_order` / `activate_independent_detached_session` /
`pause_animations_for_remote_session` / `adopt_bookmark_media_open_pending`。

| # | 束 | 箇所 (`2918e639`) |
| --- | --- | --- |
| W1 | tag prewarm 起動 | app.rs:27289 / 27296 (同じ代入が 2 箇所) |
| W2 | normalize state の破棄 | app.rs:38495 / 38496 (設計 §4.4 が `clear_normalize_state()` と命名済み) |
| W3 | auto-open intent の解除 | app.rs:39210〜39213 (4 フラグ) / 40105〜40106 (**同じ 4 つのうち 2 つだけ**) |
| W4 | detached physical への読み替え | app.rs:40983〜41000 (9 箇所。`details_order` は②-c1 で分離済みなので**畳み直すこと**) |
| W5 | index を指し直す | app.rs:41210〜41217 (7 箇所。`viewer_session` への直接代入を含む) |
| W6 | bookmark_view_state の設定 | startup_ops.rs:591 |

⚠ **W3 の非対称は②-c1b で結論を出す。** 意図的な部分解除か、同型経路の直し残しか。

**ステージ②-c1b 完了** (2026-08-25、`2894926c`、指示書
[detached-rework-stage-r2e-2c1b.md](detached-rework-stage-r2e-2c1b.md))。
書き 27 箇所が registry モジュール内の名前付き操作になった。lib テスト 6251 件緑 (件数不変)。

### 到達点: 非テストのビルドは 225 フィールド private で通る

`ViewerContextBundle` の全 225 フィールドを private にして
`cargo check -p mimageviewer --bin mimageviewer-core` が **E0616 ゼロ・exit 0**。
設計 §6.1 が「モジュールの外では struct literal も destructure も `empty()` も
言語仕様として書けなくなる」と言っている状態に、**非テスト面では既に到達している**。
残るのは `src/app/tests.rs` からの直接アクセスだけで、それが②-e (`test_access`) の仕事。

主な操作: `ensure_tag_prewarm_started` / `clear_normalize_state` (設計 §4.4 の命名) /
`activate_parked_live_as_independent_detached` / `activate_passive_as_independent_detached` /
`become_independent_detached_viewer` / `retarget_bookmark_media_open` /
`split_materialized_physical_context_for_detached_scope` (fork + 絞り込みを 1 本に)。

**ステージ②-d 実装完了・実機 smoke 待ち (2026-08-25、`bf391e6a`。指示書
[detached-rework-stage-r2e-2d.md](detached-rework-stage-r2e-2d.md))。**
`App::active_detached_viewer_context` と `DetachedImageWindowSnapshot::paused_bundle` を削除し、
production の唯一の保管先を `ViewerContextRegistry` の slot に切り替えた。registry は
`ViewerContextId` / `ContextResidence`、window↔context の双方向 binding、mount / build / fork /
retire / promote の 5 transaction を所有する。build の binding は commit まで公開せず、
live-media fork は binding を旧 main から新 context へ transfer する。

- §6.5 の 3 stopgap は `locate_window_context()` と `ContextResidence` の問い合わせへ置換した。
  active / parked は保管場所ではなく `DetachedWindowRuntime.state` が引き続き表す。
- 全 context 操作は mounted id を先に 1 回処理し、同じ id を registry 巡回から除外する。
  `all_context_clear_processes_a_paused_context_that_is_already_mounted` の完全一致列は維持した。
- VST3 と rename の前置きフィルタは `ContextRef` へ移した。VST3 consume の marker 復元は
  呼び出し元の `catch_unwind` 境界に残した。
- swap 内部 failpoint を production payload の build abort / panic unwind に対して追加し、
  I8 (commit 前の binding 非公開) を確認した。`RetireError::IsMain` と
  `highest_reserved_serial` による Retired / Unknown の区別も維持している。
- 4 one-shot の解除は registry 内の activate 操作へ集約した。ただし mounted 側に
  `ViewerSession` の逐語実体があるとは扱わず、App の 3 ミラーは従来どおり個別に復元する。
- 監査 A1 / A5 を有効化した。A1 は bundle 型の registry 外流出、A5 は旧 2 識別子の復活を
  fixture で検出する。production 監査は既知の `activate_snapshot` 1 件だけを維持している。
- library は 6285 tests / exit 0、audit は 25 tests / exit 0。実機では F12 still の
  always-new / reuse、PDF/ZIP の park・resume と folder-nav、video/audio の live park・resume と
  main context change promote、複数窓の順不同 close、右ドラッグ / keep-alive、tray / remote、
  native presenter の HWND / focus / z-order を確認する。

### ②-d 回帰修正と dead API 清算 (2026-08-25)

detached 動画の host 準備前に作った `NativeVideoOpenPending` が再開しない回帰を修正した。
pending は enqueue 時の `native_video_parked_live_input_window_id` を owner として持ち、poll は
その値を現在の marker と比較していた。②-d で marker の set / restore が registry mount の
closure 内へ移ったため、**同じ viewer context のままでも marker の寿命だけが先に終わる**。
この場合 owner gate は deadline 判定と repaint 要求より前で return し、host が登録された後も
pending が永久に残った。marker は入力 routing の一時 policy であって context identity ではない。

修正後の regular-open pending は enqueue 時の `ViewerContextId` を持つ。App の投影が Building
中でも予約済み ID を取得でき、AtRest / Mounted の移動や window binding の変化では owner を
書き換えない。live-media fork で payload が新 context へ分岐する transaction だけが pending
owner も同時に transfer する。poll は現在 App に投影された context ID と照合する。host 準備判定
自体は従来どおり、登録済み HWND に対する `GetClientRect` の正サイズで成立する。

回帰テストは marker がある状態で defer し、marker を復元した次 frame で host 未準備の待機を
確認した後、同じ context を再 mount して host 準備済みにすると resume callback が実行され、
pending が消えるところまで固定する。pending の生成だけを確認するテストではない。

②-d が残した dead item 16 個 (warning group 11 件) も清算した。旧 bundle-side の bookmark retarget / tag prewarm /
remote animation pause / detached activation / promote helper は、実際に
`with_viewer_context` から呼ばれる mounted-side 実装へ置換済みだったため削除した。
window binding の公開 transfer wrapper 2 本は、`finish_fork` が同じ `transfer_core` を ownership
transaction 内で直接呼ぶため削除した。未使用の汎用巡回 helper、read accessor 2 本、
`ContextMut` の未使用 ID も削除した。

`ViewerContextBundle::pause_background_work_keep_current_frame` については、失われた挙動ではなかった。
②-d の同じ差分が同内容の `App::pause_mounted_background_work_keep_current_frame` を追加し、
`pause_current_active_viewer_context` の mount closure 内で snapshot 作成直後・AtRest へ戻す前に
呼んでいる。final-AI cancellation を含む既存の実 park テストもこの経路を通る。したがって未使用の
bundle 版は重複として削除し、その同等性テストは mounted-side 実装へ移した。

### 検収 (ClaudeCode、2026-08-25)

**テストが減っていないことを最優先で確認した** (この段は `src/app/tests.rs` が
956 追加 / 1238 削除 = 差し引き 282 行減っているため)。

- `src/app/tests.rs` の `#[test]` 数は **1399 → 1399 で不変**。消えた 6 つの識別子は
  helper 3 つ (`paused_test_window` / `parked_bundle_snapshot` /
  `passive_right_drag_image_bundle`) と、**改名された test 3 つ**だった:
  `paused_detached_mount_skips_missing_bundle_and_window` →
  `window_mount_rejects_unbound_snapshot_and_unknown_window`、
  `paused_detached_mount_drops_bundle_when_target_closes_inside_closure` →
  `window_mount_round_trips_context_when_snapshot_vector_changes_inside_closure`、
  `exit_resume_harvest_reads_active_and_paused_bundles_without_dropping_them` →
  `..._active_and_parked_contexts_without_dropping_them`。
  行数減はテスト本体ではなく**セットアップの共通化** (直書き 4 行 →
  `push_window_context_for_test` 1 行) による。
  ⚠ 2 番目の改名は**意味も変わっている**: 窓の snapshot が消えても context は registry に
  残るので drop されない。これは②-d の目的そのものであって退行ではない。
- 保護指定した 2 本は健在。`all_context_clear_processes_a_paused_context_that_is_already_mounted`
  の**完全一致列の assert は diff に現れない = 無変更**。差分は setup の移行のみ。
- test helper と failpoint は**すべて `#[cfg(all(test, windows))]`** で、production に costs が無い。
  failpoint は**本物の `swap_viewer_context_bundle` の内側** (rating 同期と
  visible index 再構築の直後) に刺さっている。
- 独立実行: `cargo fmt --check` 無出力 / `cargo run -p viewer_context_audit` exit 0 (既知の指摘
  1 件のまま) / `cargo test -p viewer_context_audit` 25 件 / **`rg "active_detached_viewer_context|paused_bundle"
  src/ -g "!viewer_context_registry.rs"` が 0 件** / `cargo test -p mimageviewer --lib` **6258 passed**。
- registry モジュールの `#[test]` は 22 → 29 (+7)。lib 件数 6251 → 6258 と一致する。

### 実機 smoke 結果 (2026-08-25)

| シナリオ | 結果 |
| --- | --- |
| 1 複数ウィンドウ (always-new) の静止画・順不同 close | **OK** |
| 2 フル機能ウィンドウの窓再利用・フォルダ移動 | **OK** |
| 3 PDF を別ウィンドウで | **OK** |
| 4 detached 動画の再生 | **OK** (下の退行を直した後) |
| 7 複数ウィンドウで PDF / 動画を開閉 | **OK** |
| 5 VST3 | **OK** |
| 6 parked 右ドラッグ (非アクティブ窓へのリング) | **OK** |
| 8 マルチモニター (窓移動 / 最小化・復元) | **OK** |

**②-d の実機 smoke は完了 (2026-08-25)。**

- 8 の「VST GUI の開閉」は**確認対象外だった**。in-window モードは設計上 VST を対象外にしており、
  フルスクリーン以外へ切り替える時点で VST GUI を自動的に隠す
  ([native_video.rs](../src/app/native_video.rs) の placement 切替、
  「in-window モードは VST を対象外にするため」のコメント)。利用者の観測どおりで正常。
- smoke 中に F12 の placement 往復が遅いという指摘が出たが、**利用者が v3.2.0 と v3.0.0
  ポータブルでも同じだと確認した既存の問題**。R2e は presenter 本体を 1 行も触っていない。
  backlog [next-release-backlog.md](next-release-backlog.md) **§1.116** に、
  共有出力プール枯渇のログ証拠付きで積んだ。

### ステージ②-e 完了: bundle の言語境界と audit A4 / A6 (2026-08-25)

`ViewerContextBundle` の 225 フィールドをすべて module private にし、`empty()` /
`set_items_generation()` / `clear_normalize_state()` も registry module private にした。
registry 外では struct literal、destructure、field access、`empty()` 呼び出しを Rust の可視性規則が
拒否する。production / detached の挙動は変えていない。

- `src/app/tests.rs` の bundle 構築 103 箇所と field write 約 230 箇所は、context を一時 mount して
  `&mut App` を受け取る closure setup へ移した。読み取り assertion も mounted App 上で行う。
  assertion マクロ数・期待値・message は変更していない。
- bundle を引数／戻り値にする test helper は削除した。`build_window_context_for_test` は
  `install_window_context_for_test` の closure 版後継であり、snapshot を追加しない at-rest window
  context の構築に必要。test 専用 registry 入口は 14 → 9 に減った。
- A3 の `tests.rs` 特例除外を削除した。A4 は production registry の正規化 API 指紋 62 件を
  完全一致 allowlist とし、正確な可視性、generic / bound / where、public field / variant、
  re-export、公開型への trait impl と関連型まで固定する。
- A6 は設計が想定した未実装の `viewer_context_registry::test_access::` ではなく、実在する
  `#[cfg(all(test, windows))] impl App` の `*_for_test` 定義と呼び出しを対象に再定義した。
  `_for_test` 定義は `cfg(test)` を含む必要があり、呼び出しも test cfg 外では禁止する。
  この差は audit allowlist コメントにも明記した。
- 検証は library **6261 passed / 0 failed / 27 ignored** (検出総数 6288)、audit **31 passed**。
  audit 実行は A1 / A2a / A2b / A3 / A4 / A5 / A6 / A7 が有効で、既知の
  `activate_snapshot` 1 件のみ。library の dead-code warning は従来どおり **9 件**。

### ステージ④完了: metadata import refresh の context identity (2026-08-25)

`metadata_import_refresh::ContextSlot` を廃止し、request / result が
`ViewerContextId` を所有するようにした。main は場所を表す特別値ではなく、request 構築時の
`registry.main()` を焼き込む。window context も Vec index / window id から適用時に引き直さず、
要求時の同じ identity を worker 往復後まで保持する。`items_generation` は従来どおり別フィールドに
残し、「どの context か」と「その items snapshot がまだ current か」を独立に照合する。

- apply 前後は registry の `residence(ViewerContextId)` を正本にする。`Mounted` / `AtRest` だけが
  適用可能、`Retired` は正常な遅延結果として debug 記録だけで破棄、`Unknown` は identity
  不変条件違反を常時ログして破棄する。UI thread の同期 transaction 外から観測不能な
  `Building` / `Retiring` も不変条件違反として診断し、guard / retry / delay は追加しない。
- 変更前の HEAD で、先行 window close により後続 window の refresh が失われるテストと、
  request / apply 間の main promote で別 context に結果が入るテストがともに失敗することを確認した。
  修正後は両テストに retired / unknown の分岐テストを加えた 3 本が通る。
- `ContextSlot` は `src/` で **0 件**。A4 は non-Windows の単一 context identity を構築する
  `ViewerContextId::single_context()` の正確な指紋を同じ変更で allowlist へ追加した。
- 検証は library **6313 passed / 0 failed / 27 ignored** (検出総数 6340)、audit **31 passed**。
  library の dead-code warning は従来どおり **9 件**。実機 smoke はこの behavior change の
  verification build で利用者確認待ち。

## ②-d の退行 1 件と、その調査でかかった 3 往復

**症状**: detached で動画を開くと黒いウィンドウのまま再生が始まらない (両モード)。

**真因** (`549be614` で修正): `should_poll_main_video_context` の第 1 項を
②-d が `active_detached_viewer_context_contains_video()` から
`active_viewer_context_contains_video()` へ**改名と同時に意味を広げていた**。

- 旧: holder を読む = context が **at rest のときだけ** true
- 新: `locate_window_context` の結果から **residence を捨てる** = **mounted でも** true

detached で動画を開く瞬間、その context は App にマウントされていて `fullscreen_idx` が
動画を指す。よって門が閉じ、**main が `poll_video` を飛ばす**。飛ばされた poll こそが
プレイヤーを作る当人で、detached 側の poll 経路は「生きたプレイヤーがある」ことが前提。
**動画を開く poll が、「動画を開こうとしているから」閉じる門の後ろにいた。**
期限判定も poll の中なのでタイムアウトも出ない。

述語は `other_active_viewer_context_contains_video` へ改名し、投影中の context を除外した。
**11 箇所の呼び出し全部が旧い意味を必要としていた** (広い意味が要る箇所はゼロ)。

### 調査から得たもの

- **計装が答えを出した。** 無言だった出口 3 つ (`bc723138`) と破棄 4 箇所 (`0bb5dd81`) に
  理由を書かせた結果、**7 つとも無言のまま**だった。その沈黙自体が
  「誰も pending を見ていない = poll が呼ばれていない」を意味し、門にたどり着けた。
- ⚠ **最初に掴んだ手がかりを捨てた。** この述語の変化は、最初にログを読んだ直後に
  ②-d の diff で目に留めていた。にもかかわらず dead-code 警告 11 件という
  **機械的で分かりやすい手がかり**へ乗り換え、そこから 2 往復を空振りした。
  警告は確かに何かの印だったが「掃除し忘れ」であって退行ではなかった。
- ⚠ **dead-code 警告から「呼び出しが落ちた」と推論しない。** 16 件を 1 件ずつ
  ②-d の diff と突き合わせたところ、**全部が置き換え後の残骸**だった。
  「後継が実際に呼ばれているか」を見るまでは、どちらとも言えない。
- 回帰テストは**門の戻り値ではなく poll への到達**を見る。前者は門を書き換えれば通る。
  述語を元の広い形へ戻す変異でこのテストだけが落ちることを確認済み。

### 実機 smoke シナリオ (利用者へ)

リポジトリルートから。**起動前にインストール版 / トレイ常駐版を終了すること**
(single-instance mutex を共有する)。**引数なしでは実利用中の `%APPDATA%\mimageviewer` を
使う**ので、設定・キャッシュ・ログを更新し得る。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

1. **always-new の静止画**: 「常に別ウィンドウで開く」ON。静止画 A を F12 → main へ戻る →
   B を開く → A を再アクティブ化 → **B を A より先に閉じる**。
   各窓が画像・選択・ズーム/パン・配置・フォーカス・右ドラッグの identity を保つか。
2. **reuse の静止画**: always-new OFF。F12 で開き、Ctrl+↑↓ でフォルダ移動、閉じて開き直す。
   **同じ OS ウィンドウ / サイズが再利用され、小窓のチラつきが出ない**か。
3. **PDF / ZIP の本**: 別ウィンドウで開き、**非同期の列挙中とパスワード入力中**に park / resume。
   ページ / フォルダ移動。空の viewport・別の本・パスワード状態の喪失が無いか。
4. **動画 / 音声 + promote**: detached で再生 → main 側のフォルダ / お気に入り /
   スマートフォルダを変更。**再生が promote をまたいで続き**、main の一覧は独立し、
   再アクティブ化で同じセッションに戻るか。
5. **VST3**: 4 を VST3 有効で。deferred media open コマンドも含める。
   再生 / marker の所有 / VST GUI / pending コマンドが**正しい窓に付いたまま**か。
6. **右ドラッグ (park 中 / keep-alive の隙間)**: parked な窓、または PDF/ZIP の keep-alive 中に
   passive 右ドラッグ。**その窓だけに届き**、viewport が消えないか。
7. **順不同 close + tray / remote**: 静止画 / PDF / 動画の窓を複数作り、**順不同で閉じる**。
   トレイへの hide / 復帰、リモートセッションの pause / resume。兄弟窓が壊れないか。
8. **マルチモニター / 混在 DPI**: detached 動画窓をモニター間で移動し、アクティブ化・
   最小化 / 復元・VST GUI・close。native child HWND の配置・フォーカス・z-order・
   presenter の連続性。

## 「独立 detached viewer にする」3 経路のフラグ集合が違う — 調査済み・不具合なし

**②-c1b では意図的に揃えなかった** (揃えると 2 経路の挙動が変わるため)。
名前だけ分けて集合はそのまま保ってある。

| 経路 | `fs_open_intent_from_grid` | `pending_auto_fs_open` | `pending_return_to_parent` | `pdf_prefetch_grace_until` |
| --- | --- | --- | --- | --- |
| ParkedLive → active | ✓ | ✓ | ✓ | ✓ |
| passive → active | ✓ | — | — | ✓ |
| promote | ✓ | ✓ | ✓ | — |

履歴 (実際に `git show` で確認済み):

- `7ee84fdb` (2026-06-28) passive resume 新設 — `pdf_prefetch_grace_until` のみ
- `5ba4d537` (2026-07-01) promote 新設 — 3 フラグ。`pdf_prefetch_grace_until` は当時既に
  存在していたので、「フィールドが無かったから」ではない
- `76f36d94` (2026-07-06) ParkedLive 経路 新設 — 4 フラグまとめて
- `d3b56c15` (2026-07-06、`(detached-rework CUT) remove linked detached pin`) —
  passive resume に `fs_open_intent_from_grid` **だけ**追加。**同じ diff の中に 4 フラグ版が
  見えている状態で、残り 2 つは足していない**

**非対称を意図的だと述べたコミットメッセージもコメントも無い。** 3 経路が別々の時期に
別々のリストとして育ち、`d3b56c15` は目の前の症状に対応する 1 つだけを足した形に見える。

### 決着 (2026-08-25、Codex Sol に read-only で調査依頼 → 主要 3 点を自分で照合)

**不具合は無い。差は「どこで消すか」の違いであって「消さない」ではない。**

| 経路 | park 時 | activate 時 | 結果 |
| --- | --- | --- | --- |
| passive → active | `pause_background_work_keep_current_frame` が `pending_auto_fs_open` / `pending_return_to_parent` を消す ([viewer_context_registry.rs:1489](../src/app/viewer_context_registry.rs:1489)、呼び出しは [app.rs:37447](../src/app.rs:37447)) | 残り 2 つ | **4 つとも false** |
| ParkedLive → active | **消さない** (メディアを生かしたまま park するので `pause_background_work_...` を通らない) | 4 つとも | **4 つとも false** |
| promote | — (mounted context をそのまま取る) | 3 つ | `pdf_prefetch_grace_until` だけ残る |

promote が残す 1 つは**観測できない**。理由 3 つとも確認済み:
① grace が `Some` になるのは **PDF 仮想フォルダを開いたときだけ** ([app.rs:22495](../src/app.rs:22495) / [:22546](../src/app.rs:22546))、
② promote は現在のフルスクリーン項目が **video / audio のときしか走らない** ([app.rs:41156](../src/app.rs:41156))、
③ 消費側 `update_keep_range_and_requests` を呼ぶ `update_thumbnail_frame_bookkeeping` の
production 呼び出しは [app.rs:65555](../src/app.rs:65555) **1 箇所だけ**で、detached bundle を
マウントしている `update_active_detached_viewer_context` ([app.rs:65862](../src/app.rs:65862)) より
**前**に走る。マウント中の値は誰も読まない。
そもそも grace は絶対時刻 100ms の期限で、読まれれば期限切れとして clear される。

**したがって挙動は変えない。** 3 経路の集合は現状のまま正しい。

### ここから出た②-d への申し送り 3 件

1. **「active detached になる context は 4 つの one-shot がすべて false」という不変条件が、
   経路によって別の場所で維持されている** (park 時 / activate 時)。これは R2e が畳もうとしている
   「1 つの不変条件を 2 箇所が分担する」形そのもの。②-d の mount / activate transaction が
   1 箇所で持てば、**挙動を変えずに** 3 経路が同じ集合になる。今やると挙動変更になるのでやらない。
2. **active detached 側の `pending_return_to_parent` 消費が、フィールドの documented semantics と
   違う。** 定義 ([app.rs:16034](../src/app.rs:16034)) は「親のファイル一覧へ戻るナビを解決して流す」
   だが、active detached の消費は `mem::take` して `close_fullscreen()` するだけ
   ([app.rs:40478](../src/app.rs:40478))。R2e 由来ではないが、別途見る価値がある。
3. **caller 側の所有権**: `pending_auto_fs_open` は container dispatcher が走る前に立つことがあり
   ([app.rs:17962](../src/app.rs:17962))、ZIP dispatch は消費 ([app.rs:20917](../src/app.rs:20917)) より
   前に promote を試みる ([app.rs:20871](../src/app.rs:20871))。通常の grid 経路は新しい intent を
   立てる前に旧 context を park する ([ui_main.rs:13082](../src/ui_main.rs:13082)) が、
   起動引数 / 外部 / 変換アーカイブ経路までは確認できていない。**promote が現に消しているので
   今のリスクは無い**が、これは 3 つの activate メソッドでは決着しない caller 側の問題。

## その他

- W1 (tag prewarm 起動) は **mounted 側に 3 コピー目がある** ([app.rs:27280](../src/app.rs:27280))。
  `&mut` が要るので②-d の `ContextMut` 待ち。触っていない。
- ②-c1 で `From<&ViewerContextBundle> for ContextRef` + `impl Into<ContextRef>` を
  差し戻したのと同じ理由で、②-c1b でも引数でフラグ集合が変わる汎用操作は作っていない。

**ステージ②-c2 完了** (2026-08-25、指示書
[detached-rework-stage-r2e-2c2.md](detached-rework-stage-r2e-2c2.md))。
`tools/viewer_context_audit` (workspace member、`syn` の source-only 解析、本体に非依存) が
**A2 / A3 / A7** を実装。自身のテスト 23 件、CI に 3 つ目の軽量 job を追加
(apt も FFmpeg ヘッダも要らない)。**`src/` は 1 行も変えていない。**

### 規則を実測してから 2 点ずらした

| 規則 | 設計のまま | 実測 | 採用した形 |
| --- | --- | --- | --- |
| A2 | swap/replace/take の実引数に 225 フィールド名 → 行単位 allowlist | **62 箇所・全部 false positive**。`mem::swap` は **0 件** | **A2a** = `mem::swap` に bundle フィールド (0 件) + **A2b** = 1 関数内の**異なる**フィールドが **3 つ以上** (5 件) |
| A3 | `src/**/*.rs` 全部 | **110 箇所中 109 が `tests.rs` の `empty()`** | `#[cfg(test)]` と `tests.rs` を除外。②-e で `test_access` が入れば**コンパイラが弾く**ので監査は不要 |

A2b の分離は 21 / 11 / 10 / 5 と **2 以下**で、閾値 3 に十分なマージンがある。
allowlist の鍵は**行番号ではなくファイル + 関数名 + 理由** (`src/app.rs` は 4 万行あり
行キーは即腐る)。**A7 は対象 (`App::viewer_contexts`) が②-d まで存在しない**ので、
(a)〜(f) の 6 形すべてを fixture テストで覆ってある。「本番 0 件」を根拠にしない。

## 監査が初回実行で見つけた本物: `activate_snapshot` ✅ 解決済み (2026-08-26)

**当初は allowlist に入れず「既知の指摘」として記録した (R2e の範囲外だったため)。
後述の「R2e snapshot ownership follow-up」で解決し、KNOWN_FINDINGS は空になった。**

`activate_snapshot` ([src/app/snapshot_ops.rs:270](../src/app/snapshot_ops.rs:270)) は
**per-context の 5 フィールド** (`items` / `thumbnails` / `visible_indices` /
`scroll_offset_y` / `selected`) を `App::snapshot` へ `mem::replace` で退避する。

**`App::snapshot` は `ViewerContextBundle` のフィールドではなく、
`swap_viewer_context_bundle` で交換されない** (`swap_field!(snapshot)` は存在しない)。
つまり **per-context の状態を App-global の枠が持っている** — R2e が畳もうとしている
BA-7 系の形そのもの。

書き戻す経路は 1 つだけ ([snapshot_ops.rs:539](../src/app/snapshot_ops.rs:539)) で、
`current_folder == snap.origin` で守られている。`current_folder` 自体も per-context なので、
context を切り替えた後は通常「snapshot を捨てて再読み込み」に倒れる。
**ただし 2 つの context が同じフォルダを指していれば一致する** — main と同じフォルダで
detached viewer を開いた状態がそれ。

正しい修正は `snapshot` を per-context にすることだが、**挙動が変わる**ので別の段の仕事。
②-d で registry が context を所有するようになっても、`snapshot` を bundle へ移す判断は
別に要る。

→ **その段を 2026-08-26 に実施した。下の「R2e snapshot ownership follow-up」を参照。**

### 「既知の指摘」というカテゴリを足した理由

allowlist に入れれば見えなくなり、違反のままなら CI が永久に赤い。どちらも良くない。
既知の指摘は **毎回印字され / 実行は失敗させず / 検出されなくなったら失敗する**。
最後の性質が要点で、誰かが黙って消したり規則が退行したりしたときに気づける
(stale allowlist entry も同じ扱いで失敗する)。

ついでに `ENABLE_ALLOWLIST` という const スイッチを `--no-allowlist` フラグに置き換えた。
フリップして忘れられるうえ、false 側が誰も通らない死んだ枝になっていたため。

**ステージ②-d-pre 完了** (2026-08-25、`7282fd01`、指示書
[detached-rework-stage-r2e-2d-pre.md](detached-rework-stage-r2e-2d-pre.md))。
lib テスト 6251 件緑 (件数不変)、監査 exit 0 (既知の指摘 1 件のまま)。

### 設計に無い段を足した (保管は分けていない)

設計 §7 は②-d を「ここだけは分けられない」とする。**保管についてはそのとおり**なので
**保管は 1 コミットで切り替える**。分けたのは**読み方**である。実測 (非テスト):

| | 箇所 |
| --- | --- |
| `.active_detached_viewer_context` のフィールドアクセス | 84 (うち存在検査 47 / 所有操作 23) |
| `.paused_bundle` のフィールドアクセス | 42 (うち存在検査 4 / 所有操作 33) |

**存在検査 51 箇所がこの rework の病因そのもの。** 設計 §1 の診断は
「`None` は『ここに無い』であって決して『別の場所にある』ではない」で、`.is_some()` は
その 2 つを混ぜたまま書ける形 = **どの問いなのかがコードに書かれていない**。
保管を差し替える段で同時に問いを決めると、**命名の誤りが保管のバグに見える**。

### 判断サイト 23 箇所の読み解き

| 付けた名前 | 箇所 | 意味 |
| --- | --- | --- |
| `active_detached_context_is_at_rest()` | 13 | holder に context が在る (2 箇所は否定で使用。否定形の名前は作らない) |
| `active_detached_context_exists()` | **5** | context が在る **または** session が detached。**同じ問いが 5 回書かれていた** |
| `viewer_session_is_detached_or_switching()` | 3 | **生の OR / AND が冗長だった** (下記) |
| `detached_viewer_host_owns_surface()` | 1 | presentation が detached、または holder に context |
| `mounted_projection_owns_active_detached_session()` | 2 | §6.5 の暫定回避策。**撤去せず名前で言う** |

**判断不能だったサイトは無し。**

- **5 コピー**: `had_active_detached` の 3 行 (直前の `base_placement` 行を含む) が
  4 箇所でバイト単位一致、5 つ目は改行だけ違う同じ問いだった。1 本に畳んだ。
- **冗長だった 3 箇所**: `viewer_session_is_detached_or_switching()` は
  **冒頭で holder に context があれば true を返す**ので、同じ検査を `||` で足すのも、
  その否定に `&& is_none()` を足すのも**元から意味が無かった** (防御ではなく重複)。
  非 Windows 側も元から `false` 固定で、挙動は変わらない。
- **§6.5 stopgap 2 箇所**: `!is_at_rest()` に
  `mounted_projection_owns_active_detached_session()` という名前を付け、
  **「holder が空であることは mounted session の所有を証明しない」**とコメントに書いた。
  撤去は②-d。
- ログ引数 28 箇所は 1 つの診断入口 (`active_detached_context_debug_state()`) に通した。
  **出力文字列は変えていない。**
- ②-pre が holder 側に残した前置きフィルタ 2 つ (vst3 pending / rename 述語) は
  `.as_deref().is_some_and(..)` の形で、この段の存在検査には含まれない。②-d で扱う。

### ②-d に残っているもの

- 所有操作 **56 箇所** (`as_ref` / `as_mut` / `take` / 代入、23 + 33)
- 保管 2 つの削除と `App::viewer_contexts` への統合
- 生プリミティブ 4 種 → 5 transaction (設計 §4)、window binding table
- §6.5 の暫定回避策 3 種の撤去 (stopgap 2 箇所 + keep-alive backstop)
- 監査 **A1 / A5** の有効化 (保管が消えて初めて通る)
- swap の内側への failpoint (設計 §7 ②-d の I8 確認)

### ステージ④の実機 smoke — 想定を 1 度書き直した (2026-08-25)

最初に出した手順は**実機で成立しなかった**。利用者の指摘で 3 点とも誤りと分かった:

1. **「import 実行中に窓を閉じる」は時間的に無理。** 通常サイズのフォルダでは import が
   速すぎて、要求〜適用の窓に手が入らない。
2. **「import 中に F12」も同じ理由で無理。**
3. **複数ウィンドウモードに F12 は無い。** `ToggleDetachedViewerMode` は
   **フル機能ウィンドウの main ⇄ 別ウィンドウ切替**であって、常に別ウィンドウで開くモードには
   切り替える対象が無い。**F12 を使う手順はフル機能ウィンドウモードでしか書けない。**

**そもそもレースを実機で狙う必要が無かった。** ④ が直した 2 つのレースは
**現行コードで落ちる単体テスト**で捕まえてある
(`metadata_refresh_context_identity_survives_closing_an_earlier_window` /
`..._survives_main_promotion`、どちらも旧コードで結果が別 context へ届くことを示して失敗した)。
相関ロジックは決定的なので、実機で再現させる価値は低い。

**実機で見るべきは日常の経路**である。④ は要求の作り方 (registry の id を列挙) と
適用の引き方 (residence で分岐) を両方変えたので、そこが壊れていないことを見る:

| # | モード | 手順 | 見るところ |
| --- | --- | --- | --- |
| A | 複数ウィンドウ | 別ウィンドウ 3 つにそれぞれ違う値の項目 → import | **各窓が自分の値**を受け取るか (混ざらないか) |
| B | **フル機能ウィンドウ** | main と別ウィンドウ (F12) で別項目 → import | 両方が自分の値を受け取るか (`Main` の焼き付け経路) |
| C (任意) | どちらでも | **数千枚**のフォルダで import 中に窓を 1 つ閉じる | 窓を広げたいときだけ。DB は 500 件ずつ chunk 読みなので大きいフォルダなら秒単位の窓ができる |

**教訓**: smoke 手順を書くときは「**その操作が実機で成立する時間があるか**」と
「**そのモードにその操作が存在するか**」を先に確かめる。決定的なロジックの検証は
単体テストの仕事で、実機に回すのは**実機でしか出ないもの** (HWND / focus / 実 viewport /
タイミング) に限る。

### R2e snapshot ownership follow-up (2026-08-26 完了)

R2e が保留した監査の既知の指摘 1 件を閉じた。**リワーク外の変更 (§11) ではなく、
R2e の続きの独立ステージ**としてここに記録する (Codex の手続き判断)。

**守るべき不変条件**: 表示中の `items` と、それを元へ戻す `SnapshotState` は
**同じ `ViewerContextId` が所有する**。

`activate_snapshot` は `items` / `thumbnails` / `image_metas` / `visible_indices` /
`scroll_offset_y` / `selected` / `zip_nav` の **7 つとも bundle field** を `App::snapshot` へ
退避する。ところが
`App::snapshot` だけが App-global で `swap_viewer_context_bundle` で交換されなかった。
**表面の `top_level_grid_view` (どの top-level surface を表示中か) は既に per-context** なので、
「表示は context ごと・取り消しは App 共有」という所有境界の食い違いになっていた。

**入れたもの**: `snapshot` を `ViewerContextBundle` の field にした。bundle に field を
足すと R2e の完了ゲートが働き、**3 箱所でコンパイルが止まって分類を迫られた**:
`empty()` / swap の destructure / `split_current_context_preserving_main_grid` の destructure。
park 時は `duplicate_for_parked!` (= `items` / `top_level_grid_view` と同クラス)、
物理フォルダ scope の detached fork は `detached.snapshot = None` (隔壁の
`top_level_grid_view` リセットと対)。

**動かさなかった隣接フィールドと、その理由**:

- `snapshot_internal_nav` — set → call → clear を同一同期スコープで行う再入フラグ。
  ⚠ Codex の指摘で訂正: 「call tree に swap が絶対に無い」は強すぎる。`load_folder` の下流には
  `start_loading_items_inner` → `promote_active_detached_video_for_main_context_change` という
  transaction 経路がある。ただし現在の 2 入口は promotion の `fullscreen_idx` 条件を
  満たさないので、正常実行で `true` のまま swap する経路は見つからない。
- `snapshot_next_generation_id` — global のまま。
  ⚠ **当初書いた根拠は誤りだった**。field の doc コメントは「stale な pending folder nav を
  無視するため」と言うが、**`SnapshotState::generation_id` を読む production の consumer は
  存在しない** (採番とテストだけ)。将来の ID allocator として global に残すのは妥当だが、
  「global でなければならない」までは言えない。**doc コメントが実装より古い**。

**監査の handshake (Codex の指摘で修正)**: `snapshot` を bundle へ移しても
`activate_snapshot` は 6 field の `mem::replace` + `zip_nav.take()` のままなので、
**A2b は検出され続ける**。
KNOWN_FINDINGS から削除するだけだと untracked violation になるので、
**ALLOWLIST_ENTRIES へ意味を移した** (理由 = 退避先が context 所有になったこと)。
KNOWN_FINDINGS は空になり、「既知の指摘が存在する前提」の監査テスト 1 本も直した
(合成データ側の 3 本が描画・失敗条件を担っているので、実リストが空でも回帰は検知できる)。

**挙動の変化** (Codex の列挙を採用):

- A / B がそれぞれ独立した snapshot を持てる (旧: 2 つ目が 1 つ目の退避を上書き)
- 同じフォルダでも他 context の退避を復元しなくなる
- A だけ snapshot 中で B は通常、が可能になる (旧は badge / ナビ制限 / `is_snapshot_active()` に漏れた)
- `build_viewer_context` の fresh context は snapshot inactive から始まる
- `promote` では旧 context と一緒に stash され、fresh main へ漏れない
- `retire` では所有 context と一緒に破棄される
- 他 context の snapshot があるだけで materialized-still fork が阻止される現象が消える

**回帰テスト** (どちらも `swap_field!(snapshot)` を取り除く変異で落ちることを確認済み):

- `a_second_context_taking_a_snapshot_does_not_clobber_the_first_stash`
- `deactivating_does_not_restore_another_context_stash_for_the_same_folder`
  — 変異下で main の解除が他 context の `other.jpg` を書き戻し、**実バグをそのまま再現した**

**残した確認事項の解決 (2026-08-26)**:
`rating_filter_suppressed_at` と `favsearch_subfolder_restore` /
`global_search_subfolder_restore` も `ViewerContextBundle` 所有へ移した。検索 fallback の
先行 `take()`、`image_metas` と Details index state の交換漏れも同時に、互いを混ぜない
ownership / index-space 修正として閉じた。判断と変更境界は §11 の同日記録を参照。

### R2e の現況 (2026-08-25 時点、新しいセッションはここを最初に見る)

**R2e は全段完了。** master へは②-e まで (`189803b5`) をファストフォワードで投入済み。
その後 ③ / ④ が `r2e-ownership` に載っており、**master より 8 コミット先**。

| 段 | 内容 | 状態 |
| --- | --- | --- |
| ① 〜 ②-e | 状態機械 / mount helper / digest / 型移設 / accessor / 監査 / 保管切替 / private 化 | ✅ master 投入済み |
| ③ | `other_ids()` — 巡回から「投影中の context」を正しく除く | ✅ 未 merge |
| ④ | `ContextSlot` → `ViewerContextId`。**挙動が変わる** | ✅ 未 merge・実機 smoke 実施済み |

**④ の実機 smoke (2026-08-25)**: 別ウィンドウ 3 つ (**A / A / C** — 利用者が A を 2 つ開いた)
でメタ情報 import → **各窓に正しく反映された**。C は A と別の値なので
**「別 context の結果が混ざらない」は確認できている**。A 同士の取り違えは同値のため区別できない。
厳密な A / B / C は未実施だが、実用上は通過と見なす。

**残件 (R2e の外)**:

- **backlog §1.123** — スレッド終了中の heap 例外を、例外ハンドラ自身が二次クラッシュで
  握り潰す。**通常操作 (タグ付け → フォルダ移動) で落ちた実機クラッシュ。**
  利用者は**次のリリースまでに直したい**意向。(B) 二次 AV は解決済みで一次例外は観測待ち。
  **未解決のまま残っている唯一の項目。**
- **backlog §1.122** — F12 の placement 往復が遅い。✅ **主因は 2026-08-25 に解決**
  (NIS シェーダの実行時コンパイル 2.1 秒。当初書いた「共有出力プール枯渇」は**誤診**で、
  引用ログが別セッションのものだった)。残りは `publish` 289-424ms と `egui_overlay` 116-139ms。
- **backlog §1.124** — F12 連打で本物の押下が捨てられる。✅ **2026-08-26 に解決** (廃れた
  `GetAsyncKeyState` proxy の撤去)。憲法 §2 規則 5 の好例記述もこれで撤回した。
- ~~**監査の既知の指摘 1 件**~~ — ✅ **2026-08-26 に解決**。`snapshot` を
  `ViewerContextBundle` の field にした。上の「R2e snapshot ownership follow-up」を参照。
  KNOWN_FINDINGS は空になった。

### R2e の作業環境 (新しいセッションが最初に読むもの)

- **worktree**: `C:\home\mimageviewer-r2e` / branch `r2e-ownership`。
  master の作業ツリー (`C:\home\mimageviewer`) ではない。
- **master へのマージは全ステージ完了後にまとめて行う** (利用者判断 2026-08-24)。
  途中でマージしない。**master 側では別のバグ修正が進行中なので、時期は利用者が決める**
  (こちらから merge を提案しない)。差分は docs + `src/` のみで、他レーンとは衝突していない。
- **vendor は実体コピー済み** (junction 禁止。`ffmpeg` / `pdfium` / `ort` / `susie-worker` /
  `vst3-host` / `models` / `twemoji`)。`eframe` / `egui-wgpu` は git 追跡下なので触らない。
  worktree を作り直す場合は
  [briefs/session-lane-b-video-strip.md](briefs/session-lane-b-video-strip.md) §5 の robocopy 手順。
- **`cargo test` は PowerShell から実行する。** bash 経由の `PATH` 追加では
  FFmpeg DLL が解決されず `STATUS_DLL_NOT_FOUND` になる。DLL は
  `target\debug` と `target\debug\deps` へコピー済み (worktree を作り直したら再度必要)。
- **Git for Windows の `grep.exe` は Codex の制限付きトークン下で
  `CreateFileMapping ... Win32 error 5` で落ちる。** Codex には `rg` / `Select-String` を使わせる。
- **実機確認バイナリ**: `.\scripts\build-dev.ps1` → `target\dev-runtime\mimageviewer-core.exe`。
  引数なしでは実利用中の `%APPDATA%\mimageviewer` を使うので、**エージェントは起動しない**。
- **Codex は 1 worktree に 1 本だけ**走らせる (同時実行すると変更が混ざる)。

**§1.99 / §1.100 は R2e の完成に依存しない** (ClaudeCode / Codex 双方で確認)。

- §1.99 が必要とするのは既存 context の所有者ではなく「この要求のために新しい detached context を
  作る」型付きの宛先意図で、ブックマーク経路 (`src/app.rs:39993`) が同型で先行実装されている。
- §1.100 のコマンドは**アクティブ化 commit 後の既存マウント境界の中**で実行するので、
  マウントされていない context へ適用する必要が無い。
  ⚠ ただし `activate_detached_image_window_snapshot` は **Main をマウントしたまま return する**
  (`src/app.rs:40130`〜`40142`)。detached owner が実際にマウントされるのは同じ pass の後段
  `update_active_detached_viewer_context` (`src/app.rs:40380`、root の呼び出し順は
  `src/app.rs:66256` → `66258`)。**アクティブ化から戻った直後に実行すると Main に当たる。**

| 項目 | 状態 | 指示書 | メモ |
| --- | --- | --- | --- |
| §1.99 複数ウィンドウでの RAR / 7z / LZH open | **実装済み・検収合格 (21c3dc0d + fix1 d7e139d0)。実機確認待ち** | [stage-archive-open](detached-rework-stage-archive-open.md) | typed open plan に `ConvertibleArchiveCandidate` を追加し、直読み完了 / 変換完了 / 変換キャッシュ命中の 3 経路すべてを detached へ着地。着地結果は `Opened` / `Cancelled(reason)` / `Failed` の typed outcome で、stale と設定 OFF はトーストを出さずログのみ。App に `detached_grid_archive_open_request_seq: u64` を 1 つ追加した (既存 `bookmark_open_request_seq` と同型の単調 sequence。**憲法 3 が禁じる detached 用 bool / Option フラグではない**と双方で判断)。visibility 述語 / `show_viewport_*` builder / viewport ID / HWND registry / activation watcher / placement には触れていない |
| §1.100 非アクティブ窓の右ドラッグ | **実装済み・検収合格 (8db282e3 + fix1 5b83df3f)。実機確認済み 2026-08-21** | [stage-passive-gesture](detached-rework-stage-passive-gesture.md) | `MouseGestureState` / `MouseFlickState` は `RightDragOwner` を保持。deferred 静止画と `ParkedLive` は sequence 付きの同一 reducer を通り、成立コマンドは `DetachedWindowRuntime.activation_intent` の `Recognized → Activating → PendingExecution` を経て、`update_active_detached_viewer_context` の owner mount 中だけ実行する。通常クリックは同じ intent の `ActivateOnly` として従来動作を維持。実行の可否は active window ID 一致 / mounted window ID 一致 / `fullscreen_idx` の readiness / **認識時に控えた viewer identity** (paused bundle の `items_generation`、無ければ `reopen_sync_stamp`) の一致で決め、時間窓は使わない。fix1 = 種別 (`RightDragContext`) 比較だけでは、列挙が確定しないまま待ち続けたコマンドが同じ窓の別画像に当たり得た退行を閉じたもの。⚠ 実装コミットが backlog §1.100 の項目を丸ごと削除していたため 8f1c9b17 で復元した (憲法 7。実装コミットで backlog 項目を消さない) |

### 9.6 非アクティブ窓のジェスチャガイド (2026-08-21)

§1.100 の前段を実機確認した利用者から「非アクティブでも UI のガイド表示はそのまま出したい」
という要望が出たため、続きとして実施した。指示書は
[stage-passive-gesture-guide](detached-rework-stage-passive-gesture-guide.md)。

- **原因**: ガイドを描く 2 関数が `owner != RightDragOwner::Root` で早期 return していたことと、
  非アクティブな静止画窓の deferred コールバックが `self` を借りられず
  `settings` / `mouse_gesture` / `mouse_ring_flick` を読めないこと。
- **変更**: 各描画関数を「所有者を受け取って内容を組み立てる `right_drag_guide_for_owner`」と
  「データだけを受け取る `draw_right_drag_guide`」に分割。**描画の実装は 1 本**で、
  呼び出し元は 3 つ (アクティブ overlay / deferred 静止画窓 / `ParkedLive` 窓)。
  抑止条件 (モード / `*_help_visible` / `ring_picker` / context 一致 / `guide_visible()`) は
  すべて組み立て側へ集約した。
- ⚠ **`owner != RightDragOwner::Root` の判定は全部で 6 箇所ある。変えたのは描画側の 2 箇所だけ。**
  残り 4 箇所は Root 限定が正しいので温存した: 右クリックメニュー抑止
  (`src/app/gamepad_input.rs:1379` / `:1399`) と native video HUD (`:3980` / `:4166`)。
  ここを一律に変えると別機能が壊れる。
- 子 viewport の repaint は、その窓のジェスチャが生きている間だけ
  `request_detached_right_drag_guide_repaint` が要求し、終了 / cancel のフレームで
  消去用に 1 回出す。**判定は所有者と状態で行い、時間窓では行っていない** (憲法 5)。
  ガイド出現遅延 (`mouse_flick_menu_delay`) は既存の UX 仕様で、その時刻に描き直すだけ。
- コミット: d2c19796。**実機確認済み 2026-08-21**。

### 9.7 再生中の動画ウィンドウの右ドラッグ (2026-08-21)

§9.6 を実機確認した利用者から「動画は右クリックが反応しない」と報告があった。
**これは退行ではなく §6-3 の live-park 仕様そのもの**で、実機ログに明示的な記録がある:

```
[native-video] parked-live passive event ignored: idx=59 window_id=9 event=Window(MouseMove(...))
```

`handle_native_video_output_event` (`src/app/native_video.rs:3583` 付近) は
`native_video_parked_live_input_window_id` が `Some` のとき、左ボタンを「クリックで復帰」へ、
HUD コマンドをアクティブ化へ変換し、**それ以外の利用者入力をすべて捨てる**。

**利用者決定 (2026-08-21): 静止画と揃える。ガイドも出す。**
指示書は [stage-passive-gesture-video](detached-rework-stage-passive-gesture-video.md)。

- **入力**: allow-list に足したのは **右ボタンの非ダブルクリック down / up** (右ドラッグ有効時のみ) と、
  **その窓を所有者とするジェスチャ / リングが進行中の間の `MouseMove`** だけ
  (`src/app/native_video.rs:3496` 付近)。キー / ホイール / Touch / IME / seek / 音量 / 速度 /
  中ボタンは従来どおり遮断。左ボタンのクリック復帰と HUD 変換も不変。
  `RightDragMode::Disabled` では右ボタンも通さない。
  **進行中判定は状態 (owner + context 一致) だけで行い、時間窓を使っていない。**
- **ガイド**: 動画は native presenter (別 HWND) が映像を描き、egui のビューポートはその裏にいるため
  §9.6 の egui ガイドは見えない。**native presenter のオーバーレイへ流す。**
  `set_native_video_*_overlay` は `fullscreen_idx` / `fs_cache` から player を引くので、
  **その窓の bundle がマウントされている区間でしか正しい presenter を指せない**。
  組み立てと push は `poll_parked_live_detached_windows` のマウント区間内で行う
  (`src/app.rs:39547`)。`None` も必ず push して残留を消す。
- **排他**: egui 側は music view (presenter hidden) のときだけガイドを組み立て
  (`src/ui_fullscreen.rs:11918`)、native 側は music view のとき両オーバーレイを `None` にする
  (`src/app/gamepad_input.rs:4245`)。構造として二重に出ない。
- ⚠ §9.6 の指示書は native オーバーレイ組み立ての 2 箇所を「Root 限定のまま」としていたが、
  **本ステージで所有者引数を取る形へ変更した**。右クリックメニュー抑止の 2 箇所
  (`src/app/gamepad_input.rs:1399` / `:1419`) は**引き続き Root 限定**。
- コミット: c71d8c08 + fix1 4c5260a2 + fix2 12f60c97。**実機確認済み 2026-08-21** (リング / ジェスチャとも動作)。
- **fix1 (実機ログで確定した退行)**: 再生中の動画ウィンドウで右ドラッグ状態の **producer が 2 つ**になっていた。
  native presenter がドラッグを開始する一方、presenter の裏にいる egui の ParkedLive 経路は
  右ボタンを一度も見ないため `right_drag_live && !secondary_down` を満たし、
  `Cancel { ButtonStateLost }` を発行して native が始めたドラッグを殺していた。
  `right_drag_pointer_pos` (`src/app/gamepad_input.rs:1161`) は**所有者でしか絞っておらず、
  どちらの producer が持っているかを区別しない**のが原因。
  - 実機ログの証拠 (`MIV_DETACHED_WINDOW_DEBUG=1`、2026-08-21): 記録された right_drag イベント
    10 行が**すべて** `reason=button_state_lost` のキャンセル (5 試行 sequence 0〜4)。
    一方 `parked-live passive event ignored ... MouseButton` は **0 件**で、
    allow-list は正しく右ボタンを通していた。
  - 修正: `egui_owns_right_drag` (`src/ui_fullscreen.rs:11911`) を**唯一の所有判定**として
    1 回だけ評価し、ガイドと入力の両方に使う。所有していない側はキャンセルも入力も発行しない。
    イベント種別の決定は自由関数 `parked_live_egui_right_drag_event_kind`
    (`src/ui_fullscreen.rs:3144`) へ切り出してテストで固定した。
    **ガイドと入力で所有者判定を分けないこと。分けるとこのクラスのバグが再発する。**
- **fix2 (実機ログで確定した 2 件目)**: 成立したコマンドが `phase=recognized
  reason=viewer_identity_unavailable` で捨てられていた (5 試行すべて)。
  `right_drag_viewer_identity` は `snapshot.paused_bundle` から `items_generation` を読むが、
  `poll_parked_live_detached_windows` はポーリングの間 **bundle を snapshot から take して
  App にマウントする**。native のジェスチャが成立するのはその区間の中なので、
  読みに行った瞬間だけ `paused_bundle` が `None` だった (メディア snapshot には
  `reopen_sync_stamp` も無くフォールバックも効かない)。
  - 修正: その窓の bundle がマウント中かどうかを事実で判定し、マウント中なら
    マウント中の `items_generation` を identity にする (`src/app.rs:1249`)。
    判定材料の `native_video_parked_live_input_window_id` は**マウント区間の内側でしか立たない**
    (`src/app.rs:39556`〜`39564`)。identity 検査の強度は変えていない。
  - ⚠ **これは parked-live 経路にしか無い facts に依存した暫定解であり、一般解ではない。**
    R2e 第 3 版で「この window の bundle は今どこにあるか」を一級の問い合わせにすること
    (材料は [briefs/detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §6.5)。

## 10. 将来候補 (現行仕様では未採用)

- **pin の再導入**: CUT 前の linked/pin 意味論は戻さない。必要なら、既存窓を
  「独立窓として複製する」明示操作として新規設計する。
- **R4 の実施**: ステージ状態は §9.1 だけで管理する。deferred viewport 化と
  single render entry 化を行う場合は、BA-5 と BA-7 を同時に閉じる設計として扱う。

---

## 11. リワーク外からの変更記録

リワークのステージ外から detached 述語 / viewport 経路へ触れた変更をここに残す。
§2 の適用範囲どおり、ClaudeCode と Codex の双方が「症状パッチではなく構造的修正である」
ことに合意したものだけが対象。リワーク側は次のステージ設計時にここを読み、整合を取る。

**2026-08-27 detached grid の Descriptor open から main filter suppression を分離
(backlog §1.131 P2-2 follow-up、利用者提示の ClaudeCode re-review と Codex の callee
inspection が一致):**

**触った範囲**: [src/app.rs](../src/app.rs) の
`open_grid_item_in_detached_book_context_with_auto_fullscreen` にある
`DetachedGridItemOpenPlan::Descriptor` arm、[src/app/tests.rs](../src/app/tests.rs) の
実 detached open producer 回帰テスト。detached predicate、viewport / HWND / placement /
focus / epoch、keep-alive、overlay GPU、grid selection / paste / new-folder は変更しない。

**不変条件と判断理由**: Descriptor は park が完了しない場合、または context start が
`false` を返す場合に通常 main navigation へ戻り得るが、責務を持つ static site は4つ
(通常到達する Enter / double-click / gamepad accept の3入口と、multi-window 時には副作用なしで
早期 return する明示 container mode) であり、採用した main 分岐で
既に rating / facet suppression を行う。成功時は fresh bundle が `DetachedPhysical` を所有し、物理 scope の全ページを
App-global display filter より前に確定するため suppression は誰にも不要である。したがって
detached helper は mounted main projection を変更せず、抑制責務は実際に main navigation を
採用した境界にだけ残す。FolderCandidate も分類後の main fallback だけで抑制し、
ConvertibleArchiveCandidate の detached completion は抑制しないため、sibling route と一致する。

**回帰証明と別件記録**: 新テストは★5 filter と ZIP/PDF kind facet が有効な main から、
★5 の ZIP / PDF を実 helper で detached open し、main の suppression / filter / stack /
items / visible set / search filter が不変で、detached は3ページすべてを表示することを検査する。
Descriptor arm へ rating producer だけを戻す mutation は main anchor の `is_none()` で失敗し、
facet producer だけを戻す mutation は `{Pdf}` filter の保持比較で失敗した。新しい App-level
state、時間窓、guard / retry / repaint は追加していない。同じ caller 前段の
`note_reading_history_open(idx)` が context-owned `reading_history_return_from` を main 上で
変更してから detached build に入る別件は、§2 規則7に従い修正せず backlog §1.131 に記録した。

**2026-08-27 detached container open から main の閲覧履歴戻り先予約更新を分離
(backlog §1.131 reading_history_return_from、利用者提示の ClaudeCode re-review と
Codex の4入口 inspection が一致):**

**触った範囲**: [src/app.rs](../src/app.rs) の Enter / 明示 container mode /
main-owned Folder candidate 完了境界、[src/ui_main.rs](../src/ui_main.rs) の
double-click、[src/app/gamepad_input.rs](../src/app/gamepad_input.rs) の gamepad accept、
[src/app/tests.rs](../src/app/tests.rs) の実 gamepad producer / 非同期 Folder fallback
回帰テスト。detached predicate、viewport / HWND / placement / focus / epoch、
keep-alive、overlay GPU、grid selection / paste / new-folder は変更しない。

**不変条件と判断理由**: context-owned reading_history_return_from は main が
閲覧履歴 view から本へ遷移したときの Backspace 戻り先であり、fresh
DetachedPhysical bundle は予約を None から開始する。別窓 open は main navigation
ではないため、履歴 view からの set も既存予約の clear も行わない。3つの通常入口では
detached arbitration と必要な active-context park が main fallback を許可した後、
各 container arm でだけ更新する。明示 mode は always-new 設定中に入口で無効化されるため
現行の detached helper は到達不能だが、ZIP/PDF の park 失敗も main navigation を
採用しないので同じ境界へ揃えた。Folder candidate は分類中の scan を従来どおり main が
所有し、mixed folder と確定して main fallback を採用した完了境界でだけ path 予約を更新する。
image-book の detached completion と scan error は更新しない。

**回帰証明と mutation**: main に既存予約を持たせ、別 PDF を実
handle_gamepad_grid_accept 経路で detached open しても予約と
reading_history_back_nav() == ReadingHistory が残ることを検査する。通常 main open は
履歴 view / 非履歴 view の双方で従来どおり set / clear し、mixed Folder candidate は
分類開始時には不変、main fallback 採用時にだけ同じ set / clear を行う。
gamepad の note_reading_history_open(idx) を guard 直後の detached attempt 前へ戻す
mutation では、新 detached テストが予約 Some(history-book.zip) に対する実値 None で
失敗した。新しい App-level state、時間窓、guard / retry / repaint は追加していない。
前回の「4入口の arbitration 前に同形の別 mutation は見つからなかった」という記述は不正確だった。
反例は arbitration 後の `GridItem::ConvertibleArchive` arm で、実 navigation の成否が決まる前に
reading-history 予約、smart-folder position、rating / facet suppression を更新していた。Ignore と
convert dialog cancel は main を遷移させないため、App-global request owner であることだけではこの
mutation を正当化できない。次項で request-owned intent と成功時 commit に修正した。

**2026-08-27 convertible archive の main transition を実 load 成功時 commit へ変更
(backlog §1.131 P2、利用者提示の ClaudeCode re-review と Codex の lifecycle inspection が一致):**

**触った範囲**: [src/app.rs](../src/app.rs) の4 static entry site / open request owner / common load
boundary、[src/ui_main.rs](../src/ui_main.rs) の double-click、
[src/app/gamepad_input.rs](../src/app/gamepad_input.rs) の gamepad accept、
[src/ui_dialogs/archive_convert.rs](../src/ui_dialogs/archive_convert.rs) の cache / direct / convert
completion、[src/app/tests.rs](../src/app/tests.rs) の実 request lifecycle 回帰テスト。detached predicate、
viewport / HWND / placement / focus / epoch、grid selection / paste / new-folder は変更していない。

**不変条件と成功境界**: source grid 上で reading-history の次値、rating / facet suppression の適用可否、
smart-folder drill の有無を `MainGridArchiveTransitionIntent` に capture し、既存
`OpenRequestOwner::MainGridArchive` と `ArchiveConvertState::completion` が所有する。新しい App field は
追加しない。cache は `open_archive_via_cache_owned` が cache ZIP を `current_folder` として採用し source
override を設定した後、RAR direct は `load_zip_as_folder_with_input_seq` 後に source が
`current_folder` と一致した後、非同期変換は `pending_nav` が cache ZIP load と source override を
完了した後だけ commit する。Ignore、request start failure、dialog cancel、load block は commit しない。
smart-folder の one-shot load 認可だけは resident session を共通 load 境界で保存するため直前に行うが、
position は進めない。cache ZIP は source scope 外の実装 alias なので typed owner の common load では
その実パスによる reconciliation を遅延し、成功 commit で source archive path へ position を進める。

**回帰証明と mutation**: Ignore、Escape による dialog cancel、`ConvertDone` から実 cache ZIP を開く
成功完了を gamepad の実 producer / request / dialog path で駆動し、reading-history 予約、rating
suppression、facet filter と suppression stack、smart-folder position、main items の不変または更新を
比較した。`commit_before_ignore_guard`、`commit_on_request_adoption`、
`omit_success_completion_commit` の各 mutation で対応テストが失敗する。d7645901 が新設した regression
ではなく、同 commit 前から入口先頭にあった reading-history mutation を arm 内の同じ誤った側へ移し、
両 suppression も従来から Ignore / cancel より前にあった。Folder / ZIP / PDF も caller 側 mutation
後に `AddressBarNav::Direct` の load block が起こり得る同形を持つが、Ignore / convert cancel はなく、
§2 規則7に従い本修正には含めない。

**2026-08-27 ★固定中の converted cache alias を typed source scope で判定し、拒否を
common load の effect より前へ移動
(backlog §1.131 final P2、利用者提示の ClaudeCode 検証と Codex の funnel inspection が一致):**

**触った範囲**: [src/app.rs](../src/app.rs) の `load_folder_with_scan_claimed` にある既存
snapshot scope guard、[src/app/tests.rs](../src/app/tests.rs) の main-grid convertible cache hit /
`ConvertDone` / out-of-scope common-load 回帰テスト、backlog §1.131 の記録。detached predicate、
viewport / HWND / placement / focus / epoch、context registry / mount、grid selection / paste /
new-folder は変更していない。

**不変条件と判断理由**: cache ZIP は source archive の実装 alias であり、snapshot membership は
利用者が固定した source identity を所有する。`OpenRequestOwner::MainGridArchive` が既に持つ
`MainGridArchiveTransitionIntent::source_path` だけを `snapshot_owner_entry` の完全一致 / prefix
owner 解決へ渡し、cache path との OR や App-level flag は追加しない。したがって snapshot 内 source の
cache hit / async completion は許可され、source 自体が範囲外なら cache の配置にかかわらず拒否される。
また snapshot guard は navigation scope、snapshot state、既存 forced internal-nav flag、typed source
identity だけを読み、smart session consume / surface clear、folder-pane cancel、synthetic restore、
smart scope / reading-history / bookmark reconciliation の結果には依存しない。precondition をこれらの
effect より前へ移すことで、拒否された load は stale auto-fullscreen reservation の破棄と toast 以外の
main ownership を変更しない。新しい bool / Option、時間窓、retry / repaint / reset は追加していない。

**回帰証明とスコープ外**: 実 gamepad producer の同期 cache hit、同 producer request の
`ConvertDone` completion、smart-folder surface と one-shot source 認可を保持した out-of-scope load を
駆動する。`scope_cache_zip_instead_of_owned_source` mutation は前2本、smart session consume / clear
または reading-history reconcile を guard より上へ戻す mutation は拒否テストで失敗する。拒否後の
認可は `preserve_smart_folder_session_for_load` で実際に consume して非空性検査の vacuity を除いた。
Folder / ZIP / PDF の通常到達3入口と明示 container mode static site が common guard より前に行う
caller-side effect、および guard を持たない内部 `load_zip_as_folder_with_input_seq` /
`load_pdf_as_folder` continuation は §2 規則7に従い変更せず、backlog §1.131 に残した。

**2026-08-27 snapshot scope preflight を open lifecycle claim 前と RAR direct 完了へ共有
(backlog §1.131 final P2 re-review、利用者提示の ClaudeCode re-review と Codex の
lifecycle / generation inspection が一致):**

**触った範囲**: [src/app.rs](../src/app.rs) の2つの visible-load claim 入口、純粋な
`snapshot_scope_allows_open`、common load guard、
[src/ui_dialogs/archive_convert.rs](../src/ui_dialogs/archive_convert.rs) の RAR direct-read 完了、
[src/app/tests.rs](../src/app/tests.rs) の request ownership / generation swap 回帰テスト、backlog
§1.131 の記録。detached predicate、viewport / HWND / placement / focus / epoch、context registry / mount、
grid selection / paste / new-folder は変更していない。

**不変条件と判断理由**: snapshot scope 拒否は visible-open lifecycle の採用より前でなければならない。
`claim_open_request_owner` は archive conversion、未解決 startup open、競合 bookmark open を終了するため、
後段 common guard の拒否では「stale auto-fullscreen clear と toast 以外を変えない」という前項の契約を
守れない。navigation scope、snapshot active、既存 `snapshot_internal_nav`、typed owner identity だけを読む
純粋述語へ判定を一元化し、container dispatcher と pre-scan folder load の双方で claim より前に呼ぶ。
common guard も同じ述語を防衛的に再利用する。`MainGridArchiveTransitionIntent::source_path` は load path を
置き換え、OR による scope 拡張はしない。ユーザー操作の拒否 effect は共通 helper 1箇所に置き、各入口は
拒否後に内側へ進まないため toast は1操作につき1回である。新しい App-level state、時間窓、retry、
repaint、reset は追加していない。

**非同期完了とスコープ外**: dialog を隠して走る RAR direct completion は、generation N の解除後に
filter を変えて generation N+1 を固定できるため到達可能だった。実 load / auto-fullscreen / smart 認可より
前に同じ述語を再実行し、現在 snapshot 外なら toast なしで旧完了を捨てる。state Drop の cancel token、
deferred fullscreen の `release_fs_nav_lock`、bookmark owner cleanup は拒否時も完了する。PDF の guard 非経由
retry は password dialog が common modal input blocker で背面 snapshot 操作を止め、もう一方は
`DetachedPhysical` descriptor open なので、現行 UI から同じ generation swap は到達不能と確認し、§2 規則7に
従って変更していない。

**回帰証明と mutation**: 両 claim 入口で進行中 archive request と cancel token、別テストで未解決 startup
owner と競合 bookmark owner が拒否後も生存する。RAR は current snapshot 外の generation swap 完了が
current folder / enumerate を変えず state Drop と nav lock cleanup を終える一方、scope 内完了は通常どおり
開く。`move_scope_preflight_below_claim`、`omit_rar_direct_completion_scope_preflight`、
`reject_all_rar_direct_completions_in_snapshot` の各 mutation で対応 assertion が失敗する。

**2026-08-27 RAR direct scope refusal で folder navigation history を request-owned snapshot へ復元
(backlog §1.131 follow-up、3aaa4659 が追加した refusal branch の gap):**

**触った範囲**: [src/ui_dialogs/archive_convert.rs](../src/ui_dialogs/archive_convert.rs) の
`pending_direct_nav` consume と scope-refusal cleanup、[src/app/tests.rs](../src/app/tests.rs) の
Navigation completion 回帰テスト。detached predicate、viewport / HWND / placement / focus / epoch、
context registry / mount、grid selection / paste / new-folder は変更していない。

**不変条件と判断理由**: Back / Forward と検索・タグ・レーティング内の container open は、実 open より
先に変更した history / drill stack の `FolderNavHistorySnapshot` を `ArchiveConvertState` に所有させる。
direct-read 完了が現在 snapshot scope から拒否された場合は open が成立していないため、cache load refusal、
dialog close、worker cancel と同じ `restore_folder_nav_history` で pre-click state に戻す。3aaa4659 は
`completion` と `deferred_fullscreen` を state drop 前に退避したが、この snapshot を退避しなかったため、
拒否だけ正しく行われて history が成功時の形に残った。新しい App-level state、時間窓、retry / repaint /
reset は追加していない。

**回帰証明と mutation**: `Navigation` + `Some(rollback)` の direct RAR completion を generation N から
N+1 へ差し替え、scope 外では current folder / enumerate を変えず back / forward stack が pre-click へ
戻ること、scope 内では通常どおり RAR を開き post-click stack を維持することを検査する。
`omit_rar_direct_scope_refusal_history_restore` は前者、`restore_rar_direct_history_unconditionally` は後者で
失敗する。ファイル内の state 終端を再監査し、復元すべき user close / worker cancel / cache load refusal は
既に同じ helper を呼び、navigation / activation supersede と bookmark / detached / sibling 専用終了は
rollback ownership を持たないため、ほかに同型の欠落はない。

**2026-08-27 terminal retire 時の context-owned producer 停止を bundle Drop へ集約
(利用者提示の ClaudeCode sweep と Codex の source inspection が一致。ClaudeCode review /
mutation check 完了。cancel 4 件を個別に抑す mutation で各テストが落ちることを確認済み):**

**触った範囲**: [src/app/viewer_context_registry.rs](../src/app/viewer_context_registry.rs) の
`ViewerContextBundle::cancel_all_context_work` と `Drop for ViewerContextBundle`、
[src/app/tests.rs](../src/app/tests.rs) の pending 別 / bulk retire / sibling 非干渉テスト。
detached predicate、viewport / HWND / placement / focus / epoch、keep-alive backstop、overlay GPU、
grid selection / paste / new-folder は変更しない。

**不変条件**: context の terminal retire は caller が pause や `close_fullscreen` を先に実行したかに
依存せず、bundle が所有する thumbnail pool の cancel + 両 queue notify、tag prewarm、legacy seed、
metadata、converted archive cache paths、folder nav、folder pane open、全 final effect に加えて、
全 `fs_pending`、`details_meta_pending`、全 `comic_bake_pending`、全
`erase_inpaint_pending` の cancel token を立てる。`final_ai_pending`、
`local_adjust_pending`、`zip_enumerate_pending` は各 pending 型自身の Drop による停止を維持し、
bundle 側へ二重化しない。bulk ParkedLive retire と sibling context の停止境界も同じ規則にする。

**重大度と判断理由 (なぜ症状パッチではないか)**: P1 の thumbnail pool は condvar 待ちのまま
窓の開閉ごとに蓄積する thread leak だった。今回追加した4種は有限 one-shot worker なので、
receiver が無くても計算を終えて退出し、hang や蓄積は起こさない。ただし破棄済み context のために
CPU / GPU / AI を使い続け、最悪は orphaned MI-GAN が画像全体の tile 推論を完走する。
そこで caller 別の guard や retry を足さず、すべての retire 経路が必ず通る既存 owner の Drop に
停止責務を集約した。新しい App-level bool / Option、時間窓、debounce / grace / retry、repaint、
detached heuristic は追加していない。4 cancel 行をそれぞれ個別に検出するテスト、pause /
`close_fullscreen` を経ない bulk retire テスト、sibling の4 token が未変更であるテストを持つため、
§2 の context ownership 境界修正であり leak 対策用の過剰な lifecycle machinery ではない。

**2026-08-26 ★固定に残っていた context ownership / index-space 同期の補完
(利用者指定の構造修正、Codex 実装。ClaudeCode review / mutation check 完了):**

**触った範囲**: [src/app/viewer_context_registry.rs](../src/app/viewer_context_registry.rs) の
`ViewerContextBundle` mount / fork policy、[src/app/snapshot_ops.rs](../src/app/snapshot_ops.rs) と
[src/snapshot.rs](../src/snapshot.rs) の snapshot 交換境界、対応する回帰テストと
viewer-context audit allowlist。detached predicate、viewport / HWND / placement / focus / epoch、
keep-alive、overlay GPU、grid selection / paste / new-folder は変更しない。

**不変条件**: rating filter の一時解除 anchor と Ctrl+S / Ctrl+G の synthetic subfolder
restore payload は、それを読み書き・consume する snapshot と同じ `ViewerContextBundle` が所有する。
live-media fork は snapshot / top-level state と一緒に複製し、fresh / materialized physical context は
`None` から始める。`dismiss_snapshot_without_restore` は canonical `return_to` がある限り unused
fallback を `take()` しない。items の直接交換では、位置対応する `image_metas` も capture / swap /
restore し、generation bump 後に旧 Details metadata worker を cancel、旧 prewarm indices を破棄して、
最終 `visible_indices` から `details_order` を再構築する。activate と at-origin deactivate の両方向、
および snapshot list 復帰を同じ規則にする。

**判断理由 (なぜ症状パッチではないか)**: Group A は per-context operation が consume する state の
owner を既存 bundle の mount / retire 境界へ揃え、Group B は `items` と平行な state を既存の
capture → swap → generation bump → invalidate → restore lifecycle に参加させた。新しい App-level
bool / Option、時間窓、delay / debounce / grace / retry / repaint、detached heuristic は追加していない。
各 mutation (`rating_filter_suppressed_at` を App-global に戻す、search restore の先行 `take()` を戻す、
`image_metas` subset を外す、Details rebuild を外す) を個別に検出するテストを持つため、§2 の
所有境界修正であり症状 guard ではない。

**2026-08-26 `ViewerContextBundle` の production Drop 復元
(ClaudeCode / Codex 双方が既存 ownership 契約の構造的復元と合意):**

**触った範囲**: [src/app/viewer_context_registry.rs](../src/app/viewer_context_registry.rs) の既存
`Drop for ViewerContextBundle` から誤って追加された `#[cfg(all(test, windows))]` を除去し、
Windows production compile 時の明示 `Drop` trait bound 証明を追加した。
[tools/viewer_context_audit/src/lib.rs](../tools/viewer_context_audit/src/lib.rs) の A4 exact-surface
にも `#[cfg(windows)] impl Drop` を正規形として固定し、同じ test cfg 事故の fixture test を追加した。

**不変条件**: viewer context の破棄は production でも context-owned `cancel_token` を立て、通常 / heavy
worker queue の condvar を両方起こし、tag prewarm / legacy seed / metadata / converted archive cache paths /
folder nav / folder pane open / 全 final effect の pending work をその context だけ cancel する。

**判断理由 (なぜ症状パッチではないか)**: `2918e639` は context read view の導入 commit であり、
既存 teardown の意味変更を伴わず Drop の直前にだけ test cfg を追加していた。新しい状態、detached 述語、
viewport 経路、待機・retry・時間窓は追加せず、出荷済みの context ownership / teardown 契約を production
へ戻す修正である。本番 compile proof は明示 Drop が消えれば型検査で失敗し、A4 は正規形から cfg を
変更しても失敗するため、テストだけが teardown を持つ状態を再発させない。

**2026-08-26 native video の固定16ms repaint pump 撤去 = backlog §1.122
(ClaudeCode / Codex 双方が完全な Direction A を構造的修正と合意):**

**触った範囲**: [src/app.rs](../src/app.rs) `poll_video` の native presenter 常時 pump、
[src/video/mod.rs](../src/video/mod.rs) `VideoPlayer::tick` の native 16ms return、native event /
worker completion wake、CH/BM 境界 deadline、detached host HWND registration / adoption / watcher
repair 後の ROOT one-shot wake。`find_visible_thread_window_matching_rect`、keep-alive backstop、
native window pump の focus / placement / epoch reducer は変更しない。

**不変条件**: native event は唯一の `NativeOutputEventSender::send` から
`request_repaint_of(ViewportId::ROOT)` で起こす。時間・位置で決まる仕事は、それぞれの既存 owner が
stuck seek / EOF quiet / placement timeout / resume save / preparing HUD / CH-BM境界の正確な残り時間を
one-shot として返す。host 再親付けは実際の HWND generation 変更だけが起こし、別の
ParkedLive / detached context を mount・変更しない。準備済み paused native video は予約ゼロ。

**判断理由 (なぜ症状パッチではないか)**: 16msを延長・detached時だけ間引き・viewport paintを
skipする修正ではなく、入力イベントと期限の所有者を明示して固定 cadence 自体を除去した。
新規 App bool / Option、時間窓、grace、debounce、retry は無い。EOF の旧「3 tick」は cadence に
依存しない実時間48msへ意味を固定し、CH/BMは既知境界へ直接起こすので最大16msのovershootも減る。
host change wake の sibling 非干渉を含む回帰テストで context ownership を固定する。

**2026-08-26 native video egui overlay の process-lifetime wgpu device epoch 共有 =
backlog §1.122 残り優先 2 (ClaudeCode / Codex 双方が構造的修正と合意):**

**触った範囲**: [src/video/native_presenter/overlay_gpu.rs](../src/video/native_presenter/overlay_gpu.rs)
を新設し、[src/video/native_presenter/render_core.rs](../src/video/native_presenter/render_core.rs) の
`NativeEguiOverlay` が process-owned `OverlayGpuService` の compatible / healthy `DeviceEpoch` を共有する。
[src/video/mod.rs](../src/video/mod.rs) は F12 placement switch の旧 core drop 計測を overlay / rest に
分割しただけ。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、
focus / placement / epoch reducer、keep-alive backstop、font atlas resync は変更しない。

**不変条件**: `OnceLock` は device 直置きでなく service cache manager を保持する。各 overlay は共有
Instance から Surface を先に作り、loss 未確定かつ Surface-compatible な epoch を選ぶ。通常 epoch は
1 個、multi-GPU の incompatible Surface または device loss 後だけ追加する。健康な epoch は最後の
presenter が閉じても process lifetime の強参照を維持し、Surface / Renderer / Context / DComp lease は
従来どおり presenter ごと。epoch の RwLock は configure を write、texture update から acquire / submit /
present までを read とする (理由は同時再生ではなく、`Drop for NativeVideoOutput` が join を
待たないために新旧 render thread が重なり得ること)。`Context::run` と tessellation は外に置く。device-lost callback は自 generation
の一方向 latch だけを立て、次の overlay construction が latched epoch を skip する。dead epoch を既に
持つ overlay は次 draw で terminal error。Surface Lost / Outdated / Timeout は epoch を invalidate しない。
D3D11 present device / immediate context は presenter ごとのまま共有しない。

**判断理由 (なぜ症状パッチではないか)**: F12 の待ちを非同期化・pre-warm・hidden presenter 維持・
delay / retry で隠す変更ではなく、Surface / Renderer / UI context の window lifetime と、Instance /
Adapter / Device / Queue の process lifetime を所有型で分離した。新規 App bool / Option、detached 分岐、
時間窓、recreate trigger は無い。lost epoch 非再利用、旧 generation callback の successor 非干渉、
configure / submission exclusion を純ロジック test で固定したため §2 に適合する。

**2026-08-26 ★固定による items index-space 交換時の viewer/session 所有権調停 =
backlog §1.125 (ClaudeCode / Codex 双方が構造的修正と合意):**

**触った範囲**: [src/app/snapshot_ops.rs](../src/app/snapshot_ops.rs) の
`activate_snapshot` と `deactivate_snapshot` の at-origin 直接復元だけ。現在 mount 中の
`ViewerContextBundle` が所有する fullscreen / media session / folder-nav / 派生 index と、
既存 owner stamp を持つ App-global native pending を、同じ items 交換境界で調停する。
detached predicate、viewport / registry / recreate、runtime / host ownership、geometry / focus、
`find_visible_thread_window_matching_rect` は変更しない。

**不変条件**: 交換前の `GridItem` から `snapshot_key_from_grid_item` の完全一致 key を取り、
同一 item が交換先にあれば live `FsCacheEntry` を generation bump 前に owner から退避し、
bump + invalidate 後に新 idx へ戻す。音声モード、VST shell、normalize、EOF / loop、marker、
native open/source/fast/tile pending も同じ owner のものだけを exact old→new 対応へ移す。
source-swap の `native_output` は pending 内で生存させ、`target_idx` を移す。miss は items 交換前に
正規 `close_fullscreen()` を通す。解除側は activation 時の idx ではなく、解除直前に実際に開いて
いる item を解決する。旧 generation を答える media-nav / marker worker は remap せず cancel し、
folder-nav / holdover は任意の一覧交換を越えさせない。別の active / ParkedLive context の pending
と App-global media index は owner fact が一致しない限り触らない。

**判断理由 (なぜ症状パッチではないか)**: `selected` が既に使う snapshot の正規 identity と、
`remove_items_batch` / pending completion が既に使う context owner stamp・idx/path validation を
items の wholesale swap に適用した欠落 lifecycle の補完である。新規 detached bool / Option、
時間窓、debounce、grace、retry、repaint、rect / HWND heuristic を追加しない。完全一致と prefix
owner lookup を混ぜず、mounted main / mounted detached / ParkedLive / promoted active / sibling parked
の ownership 行列を回帰テストで直接固定するため、§2 に適合する構造的修正である。

**2026-08-26 廃れた proxy の削除 = backlog §1.124
(ClaudeCode / Codex 双方が構造的修正と合意):**

**触った範囲**: [src/app/native_video.rs](../src/app/native_video.rs) の F12 arm と
`native_video_key_physically_down` / `NativeVideoKeyBlockReason::StaleDetachedToggle`、
[src/video/mod.rs](../src/video/mod.rs) の window event drain の述語切り出し。

**症状**: 別ウィンドウから F12 で main へ戻るとき、連打すると最初の何回かが無反応。
実機ログに `ignore stale native F12 toggle: os_down=false` が 1 セッション 7 件、
**切替完了の 390ms 後から 240〜300ms 間隔で 3 連続**。rebuild 直後に 1 回来る stale
再配送ではなく、人間の連打そのものだった。

**判断理由 (なぜ症状パッチではないか)**: 新しい状態・時間窓・分岐を**一つも足さない**。
2026-07-01 に入った物理 probe は、当時存在しなかった識別子の**代用**だった。
その識別子は **2026-07-30 の pump 分離** (`0cf4b6a9`) で入っている ——
`NativeVideoWindowEventSink` は host window ごとに epoch を焼き付け、render 側が
`window_event_belongs_to_generation` で落とす。つまり **古い代用判定を撤去して
既存の所有境界に戻す修正**であり、追加ではなく削除である。

F12 arm に届く `repeat=false` は、**現 HWND・現 epoch が生成した first-key-down** である:

| 軸 | 何が落とすか |
| --- | --- |
| 旧 presenter 由来 | `window_event_belongs_to_generation` (host ごとの epoch) |
| hold による auto-repeat | `WM_KEYDOWN` の previous-key-state (lParam bit 30) → 既存の `!key.repeat` |
| 切替中の押下 | `switch_native_video_viewer_presentation` の pending guard |

**Codex の提起で 1 点修正した**: 「旧世代 event は絶対に filter を通らない」は強すぎる。
旧世代がまだ current の間に通過済みの event はあり得る。ただしそれは `PlacementSwitched`
より前の sequence を持ち、bus は sequence 順に drain されるので、App は pending 中に処理して
既存の early-return で落とす。

**回帰テスト**: `window_event_is_accepted_only_for_the_current_presenter_generation`
(述語を表で 3 通り、述語を `true` に変異させて落ちることを確認済み) と
`native_video_f12_does_not_toggle_while_a_placement_switch_is_pending` (キー経路を通して
pending guard を固定)。既存の `native_video_f12_toggles_detached_viewer_mode` は
**probe 削除後に初めて意味を持つ** —— 旧コードでは `#[cfg(test)] { true }` で
probe を迷回しており、**この退行を 2 ヶ月近く隠していた**。

→ 憲法 §2 規則 5 の好例記述をこの実機ログで撤回した (一般原則は維持)。

**2026-08-23 手書き mount 18 箇所の helper 化 = R2e ステージ②-pre
(ClaudeCode / Codex 双方が構造的修正と合意):**
「bundle を holder から取り出す → App へ swap → 何かする → swap で戻す → holder へ戻す」
という**所有権移動の定型が 17〜18 箇所に手書きコピーされている**。正しい版は既にあり
([app.rs:16703](../src/app.rs:16703) `with_active_detached_viewer_context`、`catch_unwind` +
`resume_unwind` で panic 安全)。②-pre は parked 用の対 `with_paused_detached_context(window_id, f)`
を足し、**18 箇所** (active 9 / parked 9) を 2 本の helper へ寄せる。

- **変換する 18 箇所**: active = [19808](../src/app.rs:19808) / [27885](../src/app.rs:27885) /
  [27932](../src/app.rs:27932) / [28151](../src/app.rs:28151) / [28224](../src/app.rs:28224) /
  [28374](../src/app.rs:28374) / [28994](../src/app.rs:28994) / [30422](../src/app.rs:30422) /
  [54322](../src/app.rs:54322)、parked = [19844](../src/app.rs:19844) / [27892](../src/app.rs:27892) /
  [27951](../src/app.rs:27951) / [28170](../src/app.rs:28170) / [28232](../src/app.rs:28232) /
  [28398](../src/app.rs:28398) / [29017](../src/app.rs:29017) / [30430](../src/app.rs:30430) /
  [54329](../src/app.rs:54329)。
- **変換しない** (mount-and-restore ではない): park ([38344](../src/app.rs:38344) /
  [42556](../src/app.rs:42556))、終端 close / teardown ([38583](../src/app.rs:38583) /
  [39457](../src/app.rs:39457))、activation の恒久移動 ([40155](../src/app.rs:40155) /
  [41048](../src/app.rs:41048))。parked-live poll ([39830](../src/app.rs:39830)) は
  mount-and-restore だが close-after-poll と結合しているので**②-d へ送る**。
- **挙動の差は 1 つだけ**: mount 中の closure が panic したとき、押しのけられた bundle が
  drop されずに戻る。panic 自体は `resume_unwind` でそのまま伝播するので**何も握り潰さない**。
  今日は drop → `Drop for ViewerContextBundle` ([app.rs:2743](../src/app.rs:2743)) で
  その context の worker pool が cancel される。**既に致命的な事象の巻き添えが減るだけ**である。
- **なぜ症状パッチではないか**: guard / delay / retry / 追加 repaint / 一括 reset /
  silent fallback を 1 つも足さない。R2e が集約しようとしている primitive の複製を 18 個消す。
  panic 安全は目的ではなく、**既に正しい形を使った結果**である。②-d では
  「取り出せなかったので飛ばす」分岐も 1 箇所へ集まり、そこが `residence()` の match になる。
- **Codex が付けた等価性の制約 2 件** (指示書に反映済み):
  1. [28398](../src/app.rs:28398) の `window_index` + `window_id` の検証を維持すること。
     window_id だけで引くと、index がずれた context に対して**今は捨てられている結果が
     適用されてしまう**。identity の是正はステージ④の担当。
  2. [19844](../src/app.rs:19844) の `native_video_parked_live_input_window_id` は
     **mount 中の closure の内側**で立てること。
- **helper に `native_video_parked_live_input_window_id` を入れない** (双方合意)。あれは
  parked-live の入力 / メディア方針であって汎用の residence ではない。汎用化すると
  metadata import や cache 保守の mount 中に入力フィルタ・HUD・activation・メディア所有権が
  変わる。汎用化は②-d で `residence()` として行う。
- 補足: active 側 helper は `detached_viewer_main_history_suppression_depth` を一時的に上げるが、
  変換対象 9 本の本体からその読み手へ到達しないことを Codex が確認済み (追加の差は出ない)。
- **憲法チェック**: 1 rect 捕捉に触れない / 2 recreate トリガを作らない / 3 App に新しい
  bool・Option を足さない / 4 placement の保存先を作らない / 5 時間窓を使わない /
  7 範囲は上の列挙どおり / 8 既存テストを削除・弱体化しない。
- 指示書: [detached-rework-stage-r2e-2pre.md](detached-rework-stage-r2e-2pre.md)。

**2026-08-20 keep-alive backstop の所有権 (ClaudeCode / Codex 双方が構造的修正と合意):**
`render_active_detached_viewport_backstop` ([ui_fullscreen.rs](../src/ui_fullscreen.rs) 12740 付近) が
**mount の外**で走り、**App にマウントされている別 context の状態から detached window の内容を
組み立てていた**。題の index は `self.fullscreen_idx.unwrap_or(0)`、題そのものは `self.items[..]`、
live texture も `self.fullscreen_idx` と App 上の cache から解決していた。

- **実機ログの証拠** (利用者、2026-08-20)。動画ウィンドウ (session 6, hwnd 0x3c1a2c) の 1 フレーム:

  ```
  frame=25216 source=keepalive_backstop session=Some(6) hwnd=0x3c1a2c
    fs_idx=0 items_gen=4 items_len=785 main_fs_idx=None
    computed="（成）だれにもいえないコト…pdf - mimageviewer"   ← PDF の名前
    os="【東方Full Flavor】Petaverse….mp4 - mimageviewer"
  frame=25217 source=active_render session=Some(6) hwnd=0x3c1a2c
    fs_idx=59 items_gen=8 items_len=167
    computed="【東方Full Flavor】Super Rabbit….mp4 - mimageviewer"   ← 正しい
  ```

  利用者の再現条件とも一致する: **他のウィンドウがアクティブな間は backstop だけが描くので
  題が別 context のものになり、動画ウィンドウを再びアクティブにすると `active_render` に戻って直る。**
- **変更**: backstop も所有 bundle を通してから組み立てる。**3 分岐**にした:
  bundle が `Some` なら mount して実行 / `None` かつ active session が alive なら
  **App が既に所有者なのでそのまま実行** / owner の bundle が本当に消失しているなら sibling を描かない。
  **`None` に「mount 中で take 済み」と「別 bundle を持たない直マウント経路」の 2 つの正規な意味が
  ある**ことは Codex のフェーズ 1 指摘で判明した。無条件に mount helper で包む当初案は誤りだった。
- **採らなかった案**: 題と `resolve_fs_display_tex` だけを bundle 直接参照へ変える案。holdover の
  一方向ラッチ、processed / edit / conceal / local-adjust / animation cache、thumbnail 判定、
  generation 付き描画資源を取りこぼし、同種の漏れを再生産する (双方合意)。
- **費用**: `alive_wanted` / frame marker / first-host の判定は mount の**前**に評価するので、
  **backstop が実際に必要なフレームだけ** mount/unmount が 1 往復増える。
- **憲法チェック**: 3 抵触なし (既存の `Option` と mount helper だけで振り分け、新しい state を
  足さない) / 5 抵触なし (alive session・frame marker・HWND registry・既存 content state で判定し、
  時間窓を使わない) / 7 範囲内 / 8 既存 104 本を維持。
- **今回あえて直していない**: `fullscreen_idx=None` のとき題が item 0 へ fallback する
  ([ui_fullscreen.rs](../src/ui_fullscreen.rs) 12791)。今回の cross-context 問題とは別件であり、
  同時に触ると範囲が混ざる (憲法 7、Codex の指摘)。
- **リワーク側への申し送り**: 分類は BA-5 / K1 の「複数描画入口・複数 context」seam。今回は
  **現行 K0 の入口を owner-correct にする限定修正**であって K1 (単一描画入口) を完成させるものではない。
  K1 を設計するとき、backstop がこの 3 分岐で何を必要としていたかを出発点にできる。

**2026-08-20 サムネイル結果の所有権 (ClaudeCode / Codex 双方が構造的修正と合意):**
mount 中の active detached context がサムネイル worker の結果を
`drain_thumb_results_discard` で読み捨てていた ([app.rs](../src/app.rs) 40392 付近)。
根拠は「detached はグリッドを描かないので表示する場が無い」だったが、**その後に入った
pass-through rendition (ページ送り連打時の代役表示) が、グリッドではなくフルスクリーン側で
サムネイルを消費する**ようになったため、前提が失効していた。

- **実測** (利用者実機、2026-08-20): `passthrough_unavailable` が **12,773 回** (理由はすべて
  `thumbnail_not_loaded`)、代役を要求した sequence が **2,424 個**、実際に代役で描いた回数は
  **0 回**。`page_turn_decision` の paint mode は 100% `materialized`。
  **別ウィンドウでは連打の軽量化が一度も成立していなかった。**
- **変更**: 捨てるのをやめ、**所有 context 自身の `poll_thumbnails` に処理させる**。
  producer と consumer の間に guard / retry / 時間窓を足すのではなく、結果を持ち主の
  既存 reducer へ戻す変更なので、所有境界での構造的修正である。
- **同時に決めたこと**: グリッドを描かない context では **keep 外 Video を強制テクスチャ化しない**、
  **auto-aspect を回さない**。`poll_thumbnails` は Video を keep 範囲外でも常にテクスチャ化し、
  末尾で `maybe_apply_auto_aspect` を呼ぶが、どちらもグリッド用の政策であり、代役のためだけに
  poll する context には消費者が居ない (代役が要求する `thumb_pixels` は Image / ZipImage /
  PdfPage のみ保持で、Video は対象外)。区別は**呼び出し時の型付き consumption policy** で行い、
  **App に新しい bool / Option を足していない** (憲法 3)。
- **憲法チェック**: 3 抵触なし (App state を増やさない) / 5 抵触なし (判断材料は channel 受信・
  generation・index・keep_set であって時間ではない) / 7 範囲内 (panorama、一般の auto-aspect、
  他の detached lifecycle 不具合は触れていない) / 8 既存 104 本を維持。
- **リワーク側への申し送り**: 「グリッドを描く context」と「代役のためだけに poll する context」の
  区別は、R2 で `DetachedWindowRuntime` に状態を集約するときに **consumption policy として
  型に載せ直せる**。現在は呼び出し時引数なので、状態化する場合はここを起点にすること。

**2026-08-02 利用者仕様変更により撤回:** remote video は fullscreen player / native presenter を
使わず、streaming UI state が headless player を所有する構造へ変更した。このため増分 13 の
`FullscreenEguiMediaSurface::RemoteStreaming`、owner-scoped presenter hide、専用 snapshot を削除し、
`src/ui_fullscreen.rs` / `src/app/native_video.rs` の述語は増分 13 より前の music-only 条件へ戻した。
remote start は detached / viewport / presentation を選択も変更もせず、既存の remote-session modal
だけを表示する。新しい detached bool / Option、viewport ID / runtime / host lifecycle、geometry /
focus、delay / retry は追加していない。これは表示上の症状 guard ではなく、remote decoder の owner
を fullscreen context から remote session context へ移したことで detached seam 自体を不要にした変更。

**未修正の実機観測 (2026-08-09):** 静止画 fullscreen で mIV 以外のプロセスの窓が
foreground のままになる環境では、タッチの primary start ごとに既存 focus reclaim が
`SetForegroundWindow` を試みても foreground が移らず、同じ試行が継続する。利用者には別窓の
ちらつきとして見える。これは focus / foreground machinery と detached-rework 凍結範囲の問題なので、
touch Phase 3 Step 3g は相関済み touch canvas gesture へ primary 抑止を適用しない修正だけを行い、
focus claim、debounce、viewport / detached predicate には変更を入れていない。後続リワークで
foreground ownership を扱う際の観測として残す。

| 日付 | 変更 | 触れた範囲 | 合意の根拠 |
| --- | --- | --- | --- |
| 2026-08-27 | ParkedLive の HUD クリック分類器 `native_video_output_event_is_parked_live_hud_click_activation` から catch-all `_ => true` を撤去し、`NativeVideoOutputEvent` 77 variant すべてを網羅 match で分類した = backlog §1.131 (ClaudeCode / Codex 双方が構造的修正と合意) | [src/app/native_video.rs](../src/app/native_video.rs) の当該述語のみ。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、placement / focus、window lifecycle、activation 要求の生成・消費経路は変更なし。App state の追加なし、時間窓・guard・retry・repaint の追加なし | 実機ログ (`MIV_DETACHED_WINDOW_DEBUG=1`) で、描画経路が layout 変化のたびに出す `RequestSeekStripWindow` が「利用者の HUD クリック」と分類され、利用者が活性化した別窓を 13ms で降ろしていたことを確定。原因は個別イベントの登録漏れではなく **「未分類のイベント = 利用者のクリック」という open-world の既定**であり、シークストリップが 10 個足して 3 個しか分類されなかったのはその帰結。網羅 match 化は新 variant の分類をコンパイラに強制し、誤分類の発生境界そのものを閉じるため症状パッチではない。Codex は方針に同意したうえで一次分類案に 5 件の反例を出し (`TouchChromeLearned` は利用者タップ由来 / `CloseSeekStrip` は cause 依存で `HudHidden` は描画由来 / `SetVst3PanelPos` は自動 clamp でも発火 / `SetVst3PanelVisible` は producer 不在 / `TileColumnsDelta` は入力源が 2 つ)、ClaudeCode が全件を emit 元で裏取りして反映した。`CloseSeekStrip` は既存の `SeekStripCloseCause::is_user_dismissal()` を使うだけで payload 変更を伴わない。`TileColumnsDelta` の provenance 分離と、活性化要求の**寿命・順序**問題 (消費側が「まだ望まれている要求か」を問うていない) は §2 規則 7 に従いスコープ外とし、backlog §1.131 に残した |
| 2026-08-25 | R2e-2d で active_viewer_context_contains_video が registry 化と同時に mounted context まで含む意味へ広がり、detached video open の main-update poll を自己抑止した回帰を修正。active ID が現在 projected ではない場合だけ動画を検出する other_active_viewer_context_contains_video へ改名し、旧述語から継承した十一 caller を全数監査して同じ「別 context」意味へ移行 | src/app.rs の context predicate、main video poll 入口、pending index / tile / mode-switch / park / close / media-navigation ownership、src/app/native_video.rs の source-swap / mounted clear ownership、src/app/startup_ops.rs の bookmark handoff、src/app/tests.rs の main-update poll 到達テスト。viewport ID / registry transaction / runtime / host ownership、geometry / placement / focus、window lifecycle、pending field と既存計装は変更なし | 利用者のログ・source 分析と Codex の十一 caller / mounted・at-rest lifecycle 監査が、旧 holder 述語の意味は「App に投影されていない active context」であり、全 caller が current/mounted を別の条件または現在処理中の owner として扱う点で一致。guard / retry / delay / re-entrancy flag を足さず、projected identity との比較で residence を保つ ownership 修正である。修正前に production main-update 入口から pending poll に未到達する red を確認し、修正後は poll_native_video_open_pending 固有の host_not_ready 記録まで到達する回帰テストで固定したため、症状パッチではなく §2 に適合すると双方合意 |
| 2026-08-22 | backlog §1.106 の右ドラッグ中左ボタン取消を passive detached viewer にも通し、既存 `DetachedRightDragEventKind::Cancel` に typed reason `PrimaryButtonPressed` を追加。deferred / ParkedLive の window callback は右ドラッグ中の primary down をこの cancel として同じ sequence channel へ送り、owner reducer が既存 cancel を実行する。左 release による通常の passive window activation は維持し、選択 / open / viewer action へは再利用しない | `src/app.rs` の `DetachedRightDragCancelReason` と deferred callback capture、`src/ui_fullscreen.rs` の ParkedLive callback / 既存 passive right-drag reducer、callback・owner・activation tests。`Input` variant の field、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`ViewerContextBundle` field は変更なし | 利用者の再調査と Codex の全 producer 列挙が、backlog 作成後の §1.100 で passive start point が第 4 surface として増え、typed cancel channel だけが primary press を表現できない点で一致。利用者が §2 を確認し、既存 `Cancel { reason }` への reason 追加は新規 App-level detached bool / Option、時間窓、heuristic、rect 条件を作らず、入力を既存 owner cancel 境界へ流す構造修正であると明示的に同意。Codex も同じ理由で症状パッチではなく §2 に適合すると合意した。activation reducerへ cancel suppression を足さず、cancel と Windows 標準の activation が並立することをテストで固定 |
| 2026-08-22 | backlog §1.99 の ConvertibleArchive grid open を、probe / 変換前は main 所有の typed candidate、実体確定後は新しい detached context を作る destination intent として完結させた。直読み RAR、変換 RAR、RAR cache fallback、非 RAR 同期 cache hit、ZIP / PDF control の page-list 完了後にも main context 不変を回帰テストで固定 | src/app.rs の DetachedGridItemOpenPlan::ConvertibleArchiveCandidate、OpenRequestOwner::DetachedGridArchive、request sequence / stale arbitration、open_converted_grid_archive_in_detached_context と同期 cache-hit arm、src/ui_dialogs/archive_convert.rs の ArchiveConvertCompletionPolicy::DetachedGridArchive および direct / converted completion consumer、src/app/tests.rs。visibility 述語、show_viewport_* builder、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、R2e は変更なし | backlog §1.99 と docs/detached-rework-stage-archive-open.md の ClaudeCode 分析、Codex の source inspection が、誤りは完了先を Navigation に固定した request ownership にあり、RAR を ZIP descriptor に分類する問題ではない点で一致。要求 identity と completion policy が元アーカイブを保持し、確定した backing archive から ViewerContextDescriptor::Zip { archive_source_override: Some(source), .. } を作るため、別 context の main grid を後段で戻す guard ではなく request 作成境界の構造修正である。新規 detached bool / Option、delay / retry / repaint / reset / fallback を追加せず、フル機能ウィンドウの Navigation と ZIP / PDF descriptor を維持するので §2 に適合すると ClaudeCode / Codex 双方が合意 |
| 2026-08-21 | content identity A2 の検出 owner を main の通常物理フォルダ一覧に限定し、detached physical context では worker / 候補 state を作らない | `src/app/content_identity_detection.rs::is_physical_folder_listing` と、`src/app.rs::start_loading_items_inner` の既存 `detached_physical` projection。detached predicate は read-only で再利用し、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`ViewerContextBundle` field は変更なし | 正本 `docs/briefs/edit-identity-a2-detection.md` が smart folder / 検索 / archive 等を除く「その物理フォルダ一覧」だけを owner として確定し、Codex の source inspection でも A2 の size index / pending / A3 候補は App-global で detached bundle の context-owned resource ではないことを確認。detached 側へ App-global worker を発生させると別 context の folder change が cancel / apply するため、既存 typed `ViewerNavigationScope::DetachedPhysical` を開始境界の除外にだけ使う。新規 detached bool / Option、guard / delay / retry、viewport heuristic を追加せず、main Folder / detached / smart / search / ZIP / PDF の predicate test と generation + folder key の stale test を持つ ownership 境界の追加なので、症状パッチではなく §2 に適合する |
| 2026-08-20 | 修飾キー所有権の再設計の S0 (契約を型とテストで固定、挙動変更なし) と L1 (アプリのキーとホイールの所有権) に着手する合意。正本 `docs/briefs/modifier-ownership-design.md` 第 7 版 | S0 は型とテストのみ。L1 で触れる予定の範囲は入力キュー / attach / acquisition の owner (`src/key_input.rs`)、UI と presenter の dequeue / drain-complete producer (`src/lib.rs` の `WH_GETMESSAGE`、`vendor/eframe` の `about_to_wait`、`src/video/native_window.rs` の pump)、keymap の chord 照合と command fallback 3 箇所、固定キー経路とホイール消費側。ボタン / クリック / ドラッグ (L2) は含めない | Codex は、第 7 版の S0 および L1 が、入力キュー・attach・acquisition・drain-complete reseed を単一の typed owner に集約し、キー / ホイールの producer と consumer を同時移行して application command の egui modifier fallback を残さない根本修正であり、detached 専用分岐、時間窓、retry、reset、silent fallback を追加しないため、§2 の症状パッチには当たらず着手可能と合意した。ボタン / クリック / ドラッグの L2 はこの合意に含めない。7 周のレビューで却下された案 (プロセス全体 timeline の `GetMessageTime` 順序付け、`Unavailable` 時の前 epoch 保持、`Unknown` を not held、RAlt による LCtrl 抑制、marker fence、ポインタを `Current` 扱い、acquisition 時 seed、`about_to_wait` を空の証明とする) は同ドキュメントに理由付きで記録した |
| 2026-08-20 | 修飾キー所有権の再設計が実機の事実 2 件で止まっているため、StickyKeys latch 時の左右別 async / sync サンプルと StickyKeys flags を既存 probe の snapshot へ追加し、`claim_foreground` の attach span 内 4 点 (attach 直前 / attach 直後 / focus API 直後 / detach 直後) を記録 | `src/modifier_probe.rs` の snapshot (発火条件は不変)、`src/video/native_window.rs` の `claim_foreground` の attach span と、これまで捨てていた detach 戻り値の受け取り。detached predicate、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、foreground claim の分岐と API 呼び出し順、presenter lifecycle、input consume / dispatch / guard / keymap は変更なし。detach は `&&` の short-circuit で従来どおり attach 成功時のみ呼ばれる | 正本 `docs/briefs/modifier-observation-probe.md`。設計 (`docs/briefs/modifier-ownership-design.md`) が epoch 境界の seed 規則を決めるのに、StickyKeys の latch が物理サンプルへどう映るかと、transient attach が実際にキー状態を reset するかの 2 点を必要とし、どちらもコードからは決まらない。推測で決めると設計全体の下に推測が 入るため、観測を先に置く。detach 戻り値は記録のみで分岐を足さず (失敗時の扱いは設計側の決定)、repaint / guard / delay / retry / fallback も追加しないため、症状パッチではなく §2 に適合する診断追加 |
| 2026-08-20 | 修飾キー stale の再発を原因修正せず判別するため、raw input plugin で key / wheel / 既存 frame 相乗り heartbeat と、成立した対象 action の modifier snapshot を perf event へ追加 | `src/modifier_probe.rs` の全 viewport 共通 input hook、`src/app.rs` の `GridParentFolder`、`src/ui_fullscreen.rs` の通常 / embedded / detached 共通 `FsClearAdjust` / `FsBackToList` / `FsClose` / Esc close / wheel zoom action 地点、`src/lib.rs` の plugin install。detached predicate、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、input consume / dispatch / guard / keymap / rendering は変更なし | 正本 `docs/briefs/modifier-staleness-probe.md` の確定仕様と Codex の input producer / action consumer 確認が、現行 perf event には egui / OS modifier state の比較材料が無いという同じ観測欠落で一致。Event A の emit 条件を modifier 値から分離し、key event / wheel event / 既存 frame 上の 2 秒経過だけで決める。heartbeat 用の repaint、状態修正、guard / delay / retry / reset / fallback を追加しないため、症状パッチではなく §2 に適合する診断追加 |
| 2026-08-19 | backlog §1.96 のページ移動で、page renderer が描かない動画 / 音声を含む unit には `Display` navigation sequence を作らず直接着地 | `src/ui_fullscreen.rs::begin_fs_page_navigation_sequence` の通常 / embedded / detached 共通 target 構築境界と handler / state tests。detached predicate、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、native presenter lifecycle、input permit、`blocks_new_target()`、時間窓は変更なし | 正本 `docs/briefs/codex-video-target-wedge-brief.md` の ClaudeCode 確定分析と Codex の red test / producer・consumer 確認が、`Display` の唯一の retire producer は page renderer で、native presenter 所有項目を target にした sequence は閉じられないという同じ根因で一致。target page set の全項目を既存 `GridItem::has_page_data` で検証し、閉じられない request を ownership 境界で作らない。guard / delay / retry / timeout / reset / fallback、detached 専用 state を追加せず、画像→動画→画像の handler flow と画像→画像の atomic sequence を回帰テストするため症状パッチではなく §2 に適合する構造修正である |
| 2026-08-19 | backlog §2.6 の未再現ダブルクリック調査で、明示 archive open の自動 fullscreen 要求から最初の実描画 display unit までを同じ perf 相関 id で記録 | `src/ui_fullscreen.rs::emit_fs_paint_for_display_unit` の通常 / embedded / detached 共通 paint 完了地点と、`src/app.rs` の明示 grid open 由来 RAR/ZIP 列挙 lifecycle。既存の解決済み `trace_pages` を read-only で観測するだけで、detached predicate、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、presenter lifecycle、`ViewerContextBundle` field、描画 source 選択は変更なし | 正本 `docs/briefs/codex-doubleclick-diagnostics-brief.md` が未再現のため症状修正を禁じ、accepted 後の archive request / 自動 fullscreen paint を同じ id で判別する計装だけを指定。Codex の producer / consumer 確認でも共通 display-unit paint が実描画を保証する既存 ownership 境界であり、別 viewport 推測や detached 分岐は不要と確認した。新しい guard / delay / retry / repaint / fallback を足さず、性能ログ ON 時の有界イベントを追加するだけなので症状パッチではなく §2 に適合する診断追加 |
| 2026-08-19 | `VideoAdjustSlot1..10` の順序付き action 一覧を単一 owner に集約し、native / egui の両動画 key 経路から既存 slot load へ dispatch | `src/keymap.rs` の `VIDEO_ADJUST_SLOT_ACTIONS`、`src/app/native_video.rs` の既存 native key mapping と typed source 診断、`src/ui_fullscreen.rs::handle_video_input` の egui mapping、handler-level tests。既存の `video_audio_mode_hides_native_presenter_for` を読込可否に再利用しただけで、detached predicate、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、presenter lifecycle、時間窓は変更なし | brief の ClaudeCode 分析と Codex の pre-fix handler test が、可視 presenter へ届く同じ `FsVideo` action の egui mapping だけが欠け、slot 一覧も native 内へ閉じていたことを同じ根因として確認。新しい state / predicate / guard / delay / retry / fallback を足さず、一覧を keymap owner へ移して両 consumer を同じ配列に合流した。通常動画 / hidden 音声モード / VST-visible 音声モード / modal 未消費 / 空 slot と一覧完全性を回帰テストで固定した構造修正であり、症状パッチではなく §2 に適合する |
| 2026-08-18 | `VideoToggleAudioMode` の意味を単一 helper に集約し、native / egui の動画・音楽入力から既存の VST exit / 音声モード exit / enter 遷移へ dispatch | `src/app/native_video.rs` の既存動画→音声遷移と native key / HUD event、`src/ui_fullscreen.rs::handle_video_input` と既存音楽ビュー key 分岐、handler-level tests。detached predicate、viewport ID / registry / recreate、runtime / host ownership、geometry / placement / focus、presenter生成・破棄、時間窓は変更なし | brief の ClaudeCode / Codex 事前合意と実測どおり、欠けていた action→既存遷移 mapping を補い、経路ごとに分裂していた意味を一つの owner へ移した。VST 表示中は `fs_music_view_active == false` になる反証を 3 分岐の最優先条件として取り込み、`AlreadyActive` を排他に使わない。新しい状態・述語・guard・delay / retry / fallbackを追加せず、通常動画 egui、通常動画 native、音声モード egui、VST native、egui repeat と既存画像 ownership を handler level で固定したため、症状パッチではなく §2 に適合する |
| 2026-08-17 | アニメーション展開の進行表示を先読み後の昇格専用から、現ページの初回 `Display` / `AnimationPromotion` 共通の in-flight projection へ統一 | `src/app.rs` の既存 typed `FsLoadPurpose` と context-owned `fs_pending` / upload backlog、`src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通右上 status 描画。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`ViewerContextBundle` field、worker cancel / stale 判定は変更なし | 利用者提示の確認済み原因と Codex source inspection が、初回 `Display` も `FullFrames` なのに開始時刻 projection だけが昇格 variant に限定されていた不一致で一致。新規 bool / Option や別 progress owner を作らず、同じ typed request variant が開始時刻を所有する。150ms は従来どおり提示だけの gate で競合判定には使わず、静止画専用形式を除く current item と既存 in-flight owner の一致をテストする表示修正なので、症状パッチではなく §2 に適合する |
| 2026-08-17 | §1.91 の `original_preview` blocker を原因修正せず判別するため、frame-local memo の評価順・cache hit・評価時 / blocker 時 sequence 値を1秒集計の perf eventへ追加 | `src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 original-preview memo / page-turn blocker と read-only perf probe。診断 rate state は viewport key で分離。detached predicate、navigation sequence、holdover、pass-through、先読み、描画 / 入力分岐、`ViewerContextBundle` fieldは変更なし | 利用者が候補 (a)/(b) を修正前にログで判別するよう確定し、Codex source inspection でも active 値だけの memo では評価順を復元できないことを確認。frame / pass / call order と既存 typed sequence projectionを観測するだけで、guard / delay / retry / reset / fallback を追加しない。1秒 heartbeat は調査信号が0件でも出し、抑制条件を調査対象へ依存させないため、症状パッチではなく §2 に適合する診断追加 |
| 2026-08-17 | §4.2 の音声モード Z 計装を、実ログで `Z:down` 観測済みの fullscreen event summary 直後へ追加し、`handle_fs_key_input` 前の全 guard と viewport / pass を無制限記録 | `src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 viewport closure の既存 `[fs-key] source=fullscreen` 地点。既存 rate-limited `exit key diagnostic` は維持。key consume / KeyAction / dispatch、audio-mode enter / exit、detached predicate / viewport ID / runtime / host ownership / focus lifecycleは変更なし | 利用者提示の実ログが下流診断では Z frame を捕捉できない事実を確定し、Codex source inspection でも event summary と handler 呼出しの間が唯一の確実な read-only 観測点と確認。source-side `Z:down` ごとの snapshot だけを追加し、入力 eligibility や処理順を変えず、rate limit / 時間窓 / fallback も導入しないため、症状パッチではなく §2 に適合する診断追加 |
| 2026-08-17 | navigation sequence が新 target を block している間は元画像ホールドを適用せず、sequence 終了後の静止表示では従来どおり effective `FsOriginalPreviewHold` を適用 | `src/ui_fullscreen.rs::original_preview_active` の通常 fullscreen / embedded / detached 共通述語と unit tests。既存 `fs_navigation_sequence_blocks_new_target()` の read-only projection だけを再利用し、detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`blocks_new_target()` caller、先読み窓、pass-through 条件は変更なし | 利用者提示の確定済み ClaudeCode 実測・機構分析と Codex の source inspection が一致。sequence 進行中に画面を所有するのは現ページでなく補正済みの previous holdover なので、「現ページの元画像を見せる」契約はその時点では成立しない。静止確認用途という利用者判断を、再割り当て可能な chord でなく既存 typed sequence の事実から導出する。新規 bool / Option、時間窓、delay / retry / fallback、detached 専用 guard を追加せず、再割り当てした hold の in-flight false / idle true を直接テストする共有表示 ownership 修正であるため、症状パッチではなく §2 に適合する |
| 2026-08-17 | fullscreen canonical decode のアニメ方針を typed `FullFrames` / `FirstFrameOnly` へ統一。先読みは全形式の第1フレームだけを context-owned `fs_cache` に保持し、現ページ化で同じ idx の全フレームへ非同期昇格する。archive entry の GIF / APNG も WebP と同じく再生対象にした。昇格中は in-flight 状態から右上3段目の進捗を描く | `src/canonical_image_loader.rs`、`src/fs_animation.rs`、`src/app.rs` の既存 fullscreen load / `ViewerContextBundle` 内 cache・pending・upload backlog、`src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 toast 描画チョークポイント、remote AI canonical caller と回帰テスト。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、新規 `ViewerContextBundle` field、サムネイル経路は変更なし | 正本 `codex-animation-prefetch-policy-brief.md` の確定済み利用者方針と source inspection が、先読みでも WebP 全フレームを展開する一方で archive GIF / APNG を静止扱いする非対称を根因として確定。load purpose と static animation state を bool でなく typed enum にし、worker / backlog の items generation + target idx で stale を拒否する。描画元は第1フレームのまま維持し、全フレーム完成時だけ同画素の第1フレームを持つ `Animated` entry へ差し替える。150ms は提示判断だけで競合解決には使わず、detached 専用分岐、delay / retry / fallback、別 cache owner を追加しないため、共通 context owner の構造修正として §2 に適合する |
| 2026-08-17 | fullscreen navigation target の readiness source を、その frame の描画 source と一致させた。`fs_display_bypasses_final_pipeline` が true の元画像表示 / 分析モードは target 全ページの raw `resolve_original_preview_tex`、false の通常表示は従来の加工済み source を要求する。OS キー状態は frame-local sample を producer gate / readiness / draw で共有 | `src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 navigation sequence resolver と unit tests、`docs/display-pipeline.md` の atomic display-unit 契約。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`ViewerContextBundle` field、`blocks_new_target()`、通過表示 blocker、AI producer は変更なし | 正本 `codex-original-preview-readiness-brief.md` の ClaudeCode 分析と Codex の source inspection が、raw を描く frame で readiness だけが AI + カラー化済み source を待つ ownership 不一致を同じ根因として確認。機能固有の block 迂回や時間窓を足さず、既存の共通 bypass 述語から描画・待機対象を導出し、raw 不在は `Awaiting`、見開きは両ページ `all` を維持する。bypass / 通常 / raw 不在 / analysis / spread atomic の回帰テストと、§1.88 の4本・既存 atomic test を無修正で固定した構造修正なので §2 に適合 |
| 2026-08-17 | fullscreen navigation sequence の描画所有元を texture id 包含から推測せず、ページごとの typed `Live` / `Holdover` provenance を選択元から trace / retire 判定へ伝播。見開き 1 ページずらしで previous / target が共有するページも、実際の live 描画なら target 提示として扱う。previous capture は target anchor 更新後の mutable pairing ではなく直前の実描画 layout を優先 | `src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 display-unit source、trace、sequence observer、spread-shift capture と unit tests。`src/app.rs` は不要になった texture 包含 helper の削除だけ。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`ViewerContextBundle` field は変更なし | 正本 `codex-spread-shift-sequence-release-brief.md` が texture identity と描画 provenance の混同を根因として確定し、Codex の source inspection でも source 選択地点が live / overlay の事実を保持したまま `FsNavigatorTextureSources` で捨てていたことを確認。新規 detached bool / Option、時間窓、guard / retry / repaint / reset、`blocks_new_target()` 迂回を追加せず、target 全ページ `Live` の肯定条件でのみ retire する。共有 texture live / 実 holdover / 不完全 unit / index 再利用、LTR / RTL・前後・単発 / repeat を state test で固定した所有境界の構造修正であり、正本の ClaudeCode 分析と Codex 実装確認が症状パッチではない点で一致 |
| 2026-08-16 | §1.31-A として message-dispatch 位相と render 位相を分離。通常 RedrawRequested、bootstrap、AccessKit、hidden direct work は per-window dirty として新設した about_to_wait の外側 drain だけで描画し、非ゼロ Resized 由来の InteractiveResize frame とその immediate viewport render subtree だけを inline 例外とした | vendor/eframe の run.rs、winit_integration.rs、wgpu_integration.rs。dirty は WindowId / ViewportId / surface generation / 最新 client size を持つ typed reducer が所有し、scheduled repaint 時刻とは分離。既存 detached 述語、HWND registry、viewport ID / recreate policy、geometry / focus / placement、src 配下、vendor/egui-wgpu、winit / wgpu は変更なし | 正本 codex-render-phase-separation-brief.md §3 と §7 の ClaudeCode / Codex 事前合意どおり、全 viewport 共通の event-loop 位相 ownership を修正した。判定は event provenance の非ゼロ Resized のみで、時刻・geometry・focus・VST・detached 述語を使わない。global Painting bool、新規 App state、delay / retry / repaint guard を追加せず、immediate recursion は親 render subtree のまま。reducer 11契約、既存 hidden throttle 3件、別thread SendMessageTimeoutW + RedrawWindow の Windows process test、detached フィルタ 199件で境界を固定したため症状パッチではなく §2 に適合する。§1.31-B の acquire / Present 上限は未着手 |
| 2026-08-16 | `Painter::paint_and_update_textures` の一回限りの texture delta 配送を、`begin_delivery` / typed `PaintOutcome` / `finish_delivery` の二段階 transaction owner へ集約。surface acquire の `RecreateSurface` / `SkipFrame` も単一 finalizer へ合流 | `vendor/egui-wgpu/src/winit.rs` の共有 renderer への set/free 配送と in-crate headless outcome tests だけ。set は surface lookup 前、free は成功時 `queue.submit` 後、非 submit 時は inner block の encoder / command buffer drop 後に適用する。`RenderState` 全体の clone、`src/` / `vendor/eframe/`、§1.31 の wndproc / scheduler / frame-drop、detached predicate / viewport runtime / surface lifecycle は変更なし | §1.86 指示書 §2.1 の ClaudeCode / Codex 事前合意どおり、exit 3/4 へ free loop を複製せず、5 exit に分散していた配送責任を単一 typed outcome と finalizer へ移した所有構造の修正。既存 §1.85-A no-surface test を無変更で維持し、`SurfaceRecreated` / `Skipped` は observable な `Renderer::texture()` 消失で検査する。guard / delay / retry / repaint / reset / fallback や新規 detached state を追加しないため症状パッチではなく §2 に適合する |
| 2026-08-16 | surface 無し viewport でも `Painter::paint_and_update_textures` が `textures_delta.set` / `free` を共有 renderer へ配送する契約を、DX12 headless の in-crate unit test で固定 | `vendor/egui-wgpu/src/winit.rs` の `#[cfg(test)]` module、同 crate manifest の明示的 lib test target / dev-dependency、`scripts/test-full.ps1` の専用 manifest test 段だけ。`paint_and_update_textures` の production 制御、`src/` 配下の resync 回避策、exit 3/4、detached predicate / viewport runtime / surface lifecycle は変更なし | §1.85-A 指示書に記録された ClaudeCode / Codex 双方の事前合意どおり、過去に欠落した配送境界を `Renderer::texture_size` と `texture()` の観測可能な状態で検査する回帰テストである。guard / delay / retry / repaint / reset / fallback や新規 production state を追加せず、既に成立している不変条件を gate に固定するだけなので症状パッチではなく §2 に適合する |
| 2026-08-16 | keyboard ownership を、同一 viewport へ配送済みの離散 `KeyEdge` と、処理時点の focused current-level 観測へ分離。focus 喪失後も stamped edge は dispatch し、`GetAsyncKeyState` / modifier / held は型付き focus permit がある場合だけ読む。fullscreen の Z transient 取消は handler 全体の focus gate から KeyHold state transition へ分離 | `src/keyboard_input.rs` の pass-local permit、`src/keymap.rs` の共通 edge / level consumer、`src/ui_fullscreen.rs` と `src/app.rs` の通常 fullscreen / embedded / detached / ROOT 共通 keyboard handler、current-level を読む編集 UI caller、`src/app/tests.rs` の viewport ownership 回帰テスト。detached predicate、rect 一致 HWND 発見、registry、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus lifecycle、native video key 経路、`ViewerContextBundle` field は変更なし | 利用者の承認取得済み案A′と Codex の先行レビューが、配送済み edge を frame-final focus で再判定して破棄する ownership 境界の混同を根因として一致した。permit は App に保存せず各 viewport pass の focus と既存 edge routing から導出し、別 viewport / modal / TextInput / IME を fail-closed のまま維持する。keyboard 以外の pending / pointer は focus 必須、focus 喪失時の Z は edge を drain して transient reset 後に再生成しない。新規 detached bool / Option、grace / debounce / delay / retry、focus heuristic、recreate 分岐を追加せず、全 viewport 共通の event-time ownership と current-level ownership を型で分離するため、症状パッチではなく §2 に適合する構造的修正である |
| 2026-08-14 | fullscreen ページ送りの consume を、viewport-stamped `KeyEdge` の順序を保つ typed result へ変更。first keydown / auto-repeat、matched chord、送信元 viewport、処理時点の held / 同一 frame key-up を入力 owner で判定し、release 済み auto-repeat だけを navigation へ昇格させず消費 | `src/key_input.rs` の既存 viewport-routed frame queue と `src/keymap.rs` の共有 chord consumer、`src/keyboard_input.rs` の既存 raw-key permit、`src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 page-turn handler。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`ViewerContextBundle` field は変更なし | 利用者提示の本日合意済み設計レビューと Codex の source inspection が、generic consume が `repeat` / chord / viewport / 後続 key-up の provenance を bool へ潰し、release 後に処理された repeat を navigation として受理することを根因として一致した。既存 `KeyEdge` の source stamp と frame 内順序を ownership fact として保持し、first keydown は同一 frame tap でも必ず発火する。受理済み `FsNavigationSequence` target の rollback、新規 detached bool / Option、UI 側 held OR による release 推測、guard / delay / retry / fallback は追加しない。first press / held repeat / released repeat / same-frame tap を handler/keymap 境界で固定する共有入力 ownership 修正であり、症状パッチではなく §2 に適合する |
| 2026-08-13 | main HWND が隠れたときも UI heartbeat watchdog を生かす条件を、detached lifecycle・active mounted media session・tray resident playback projection から導く単一述語へ統一 | `src/app.rs` の既存 detached / media owner の read-only projection と外部 hide 検出、`src/tray_integration.rs` の tray hide / resident wake heartbeat 同期。detached predicate 自体、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、render state は変更なし | ClaudeCode の backlog §1.31 分析が、外部 hide 経路と tray hide 経路で異なる述語を使い native fullscreen 中に watchdog を suspend する観測欠落を特定。Codex の source inspection でも tray resident wake は active playback の部分集合で、detached と mounted video / audio session を単一の read-only policy に合成すれば新規 bool / Option、guard / delay / retry、描画経路変更なしで両 caller を統一できると確認した。idle hidden → suspend、detached → keep alive、native video fullscreen → keep alive の状態遷移を unit test で固定する観測境界の構造修正であり、症状パッチではなく §2 に適合すると双方の分析が一致 |
| 2026-08-12 | physical held level 所有の page-turn pass-through を見開きから単一ページへ広げ、burst 中は resident final の有無に関係なく全ページ一様に rendition を選択。`fs/paint` は単一の既存 producer に加え、見開きでは holdover 解決後の atomic display unit から発火 | `src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 page-turn blocker、single / spread texture resolver、perf producer。既存 context-local rendition cache と viewport-local decision をそのまま使用し、detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle、`ViewerContextBundle` field は変更なし | 利用者実測で見開き修正が 267 / 267、decode 0、release settle 29〜56ms、I1〜I5 violations 0 と確認された一方、単一の 4.1MP ZIP は materialized 経路で 29 / 174 だった。Codex の source inspection で `single_page_materialized` blocker が単一だけ pass-through から外す所有境界と確定。blocker を除いて同じ current physical level / context-owned rendition 経路へ統一し、detached 専用 bool / Option、cache readiness 分岐、delay / retry / repaint を追加しないため、共通 viewport consumer の構造修正として §2 に適合すると利用者判断と Codex 分析が一致 |
| 2026-08-12 | 見開きページ送りを physical key level 所有の display-unit 原子 pass-through にし、色忠実な低解像度 rendition、full-resolution producer の defer/cancel、release 時の current-unit materialization、`fs/page_turn_*` 計装を接続 | `src/app.rs` の既存 `ViewerContextBundle` に context-local rendition cache を載せ、create / swap / independent still split / invalidation に追従。`src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 paint resolver と既存 viewport-local keymap level を使う。`src/key_input.rs` は test-script の当該 RawInput frame attribution だけを計装へ渡す。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle は変更なし | ClaudeCode の §2.5 実測正本が、frame/input は正常で旧 display unit が final 完成まで上描きされる R1 違反と、eager decode + cache eviction による再デコードを分離して指定。Codex の source inspection も `ColorizeDisplayUnitHoldover` overlay と `open_fullscreen` の eager load/prefetch 経路を根因として確認した。新しい cache は sibling と共有せず既存 bundle と同じ ownership で移動し、通過判断は current physical level と spread mode から導出するため、detached 専用 bool / Option、page readiness gate、delay / retry / repaint / predicate 分岐を追加しない。共通 viewport consumer の ownership 境界を直す構造修正として §2 に適合すると双方の分析が一致 |
| 2026-08-11 | アプリ内蔵テストスクリプト基盤 S1 として、`key_input` の既存 HWND registry で foreground HWND を一度だけ viewport target へ解決し、typed synthetic timeline から Win32 edge と viewport-local egui event を同時生成 | `src/key_input.rs` の既存 registry / pending queue と全 egui viewport 共通 input plugin。detached は既存 subclass 登録済み HWND → 安定 `ViewportId` 対応を read-only routing source として使うだけで、detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus claim、window lifecycle は変更なし。`src/app/detached_window_manager.rs` は keyboard seam 対象外の mouse low-level read である理由をコメントしただけ | ClaudeCode / Codex 合意済みの正本 `test-script-runner-plan.md` §§1〜6・§12 が、App 側で fullscreen target を再構成せず既存 HWND registry を単一 routing owner にする構造を指定。Codex も未登録 HWND を ROOT へ fallback せず typed result で待機 / 失敗可能にし、Down 時に確定した target を repeat / Up まで保持する実装であることを確認した。detached 専用 bool / Option、guard / retry / delay / repaint、geometry heuristic を追加せず全 viewport に同じ input ownership を適用するため、症状パッチではなく §2 に適合する |
| 2026-08-11 | v2.13.0 のページ送り通過表示を全撤去し、通常の完成画像解決・final-effect 回収・fullscreen upload ペーシングだけへ戻した | `src/app.rs` の `ViewerContextBundle` から rendition cache と create / swap / split / invalidation / drop を削除。`src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 page-turn decision、single / spread のサムネイル代役分岐、専用 perf probe を削除。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle は変更なし | ClaudeCode 正本 `brief-v213-remove-page-turn-passthrough.md` が実機で 5 回修正して 5 回失敗した経緯と、無効化で残さず手で全撤去する方針を指定。Codex も context-scoped cache と共通 consumer を所有境界ごと除去して §1.58 以前の単一経路へ戻すもので、新規状態・detached 分岐・guard / delay / retry / repaint を足さないため、症状パッチではなく §2 に適合する構造的な撤去と確認した |
| 2026-08-11 | ページ送りの catalog thumbnail 代役を、色調補正・カラー化・Creative LUT まで適用した `FinalCompositeKey` keyed の小容量 LRU rendition へ置換し、display unit の全ページが生成可能なときだけ共通の `Thumbnail` paint を選択 | `src/app.rs` の既存 `ViewerContextBundle` に rendition cache を追加して mount / swap / independent still split の ownership に追従。`src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 page-turn decision と single / spread paint consumer を同じ生成入口へ接続。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle は変更なし | ClaudeCode 正本 `brief-v213-passthrough-rendition.md` が判定条件の追加ではなく代役画素を忠実にする方針、既存 effect key、display-unit 原子性を指定。Codex も cache が `items` / thumbnail pixels / edit generation / effective params に従属する context-scoped resource であり、App-global や detached 専用 bool / Option にすると sibling context を混同すると確認した。既存 bundle の create / swap / split / invalidation / drop に載せ、同じ rendition producer を全 viewport が使う構造修正で、delay / retry / repaint / detached 分岐を追加しないため §2 に適合する |
| 2026-08-11 | ページ送りの `ThumbnailPassThrough` を、現在 display unit の final composite が未完成の間だけ許可し、全ページの完成表示が cache 在住になった unit は同じ idx の入力 pending frame でも `Materialize` から降格させない | `src/ui_fullscreen.rs` の通常 fullscreen / embedded / detached 共通 `fs_page_turn_materialization_for_frame` と display-unit readiness。既存 frame / items generation / idx cache を維持し、`src/app.rs` の final-effect / raw upload 抑制 consumer も同じ決定を読む。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle は変更なし | ClaudeCode 正本 `brief-v213-page-turn-strobe.md` が、入力 pending とサムネイル準備だけで品質を決め、既に出せる完成画像まで隔フレームで降格させたことを根因として確定。Codex も `current_final_composite_texture` が既存 cache の read-only lookup で producer / worker / GPU upload を起動しないこと、見開きと連結読みの spread unit が同じ `build_spread_display_units` を使うこと、縦 / 横連結は通過分岐外で常に materialize することを確認した。新規 bool / Option / delay / retry / repaint / detached 分岐を足さず、品質決定の所有境界で不変条件と状態遷移を直接テストするため、症状パッチではなく §2 に適合する構造的修正であると双方の分析が一致 |
| 2026-08-09 | fullscreen の idx-keyed `fs_cache` / `fs_early_dims` / `fs_pending` / `fs_upload_backlog` を generation-stamped collection へ集約し、一覧世代更新・全 lookup / iteration・非同期完了の各着地点で fail-closed に照合。不一致を通常ログへ記録し、pending は owner と同時に cancel | `src/items_generation_cache.rs`、`src/app.rs` の既存 `ViewerContextBundle` field / swap / items-generation 更新 / `poll_prefetch`、`src/ui_fullscreen.rs` の main / detached 共通 cache consumer。4 状態は `items` / `items_generation` と同じ bundle 所有のまま。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle は変更なし | ClaudeCode 正本 `brief-v213-fs-cache-generation.md` と backlog §3.3 が、個別 clear ではなく欠けていた一覧世代 identity を entry と参照・完了適用へ追加する構造修正を指定。Codex も既存 `PageDimsCache` と bundle swap を照合し、App-global 世代、新規 detached bool / Option、delay / retry / repaint / silent fallback を追加せず同一 context 内だけで完結すると確認した。main の `install_new_items` 差し替え、旧 load 完了拒否、generation accessor、既存 detached lifecycle を状態遷移テストで固定するため §2 に適合する |
| 2026-08-09 | ページ送り pending 不成立の診断を、Win32 の viewport-routed frame queue、egui-winit 翻訳直後の read-only `RawInput`、egui 処理後イベントの3段で同じ chord cardinality として記録 | `src/key_input.rs` / `src/keymap.rs` の既存送信元付き edge queue の read-only count、`eframe::App::raw_input_hook`、`src/ui_fullscreen.rs` の通常 / embedded / detached 共通ページ送り判定と入力 probe。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle は変更なし | 正本 `brief-v213-page-turn-backward.md` が、原因未確定時は条件を緩めず Win32 / winit / egui のどこで repeat cardinality が変わるか計測するよう ClaudeCode 側の仕様として指定。Codex も方向別 predicate が存在せず既存ログからは gate を確定不能と確認した。新規状態・detached 分岐・入力消費・時間窓を追加せず、既存 viewport identity を観測ラベルとして使うだけの read-only 計測なので、症状パッチではなく §2 に適合すると判断した |
| 2026-08-09 | 一度判明したページ寸法を GPU texture の寿命から分離する generation 付き `PageDimsCache` を、導出元の `items` / `items_generation` と同じ `ViewerContextBundle` 所有へ追加。main / detached の同値 generation を混同せず、bundle swap / parked split / 正規 idx invalidation に追従させた | `src/page_dims.rs`、`src/app.rs` の既存 bundle / swap / split / `invalidate_idx_state_and_queues`、`src/ui_fullscreen.rs` の main / detached 共通見開き判定と frame 冒頭 harvest。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus、window lifecycle は変更なし | 正本 `brief-spread-page-dims.md` が、cache 在住に依存して既知寸法が未知へ後退することを根因と特定し、generation 空間を共有しない context ごとの idx 状態として bundle 所有する構造を ClaudeCode 側の確認済み判断として指定。Codex も source inspection と修正前に落ちる状態遷移テストで一致を確認した。新規 detached bool / Option、guard / retry / delay / repaint / reset 経路を追加せず、producer / consumer / invalidation / drop を既存 owner に揃えるため、症状パッチではなく §2 に適合する構造的修正と判断した |
| 2026-08-08 | 静止画 / 本の中央タップ初回ヘルプを、既存 still touch surface と同じ viewport-local temporary state で所有し、学習後は既存 chrome latch を表示状態へ遷移 | `src/ui_fullscreen.rs` の通常 / fullscreen / detached 共通 still viewport 入力・描画経路と `src/touch_input.rs` の中央矩形 producer。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus model、window lifecycle は変更なし | 正本 `touch-support-plan.md` §5.1 / §5.5 が viewport + surface 単位の typed state と共通 touch command ownership を指定し、Codex の source inspection でも App-global な viewer 状態や detached 分岐を追加せず実装できることを確認した。既存 `StillTouchChromeLatch` と同じ scope を使い、delay / retry / repaint loop / geometry heuristic を追加せず、中央矩形を分類器と描画で共有して複数 viewport を混同しないため、症状パッチではなく §2 に適合する構造的な入力 ownership 追加と判断した |
| 2026-08-05 | Enter / NumpadEnter の KeyHold は、対象 viewport の送信元付き物理ラッチを確認できる場合だけ押下成立とし、subclass 登録前は未押下に固定 | `src/key_input.rs` の既存 frame-active viewport / main・numpad Enter ラッチ参照 API と `src/keymap.rs::key_held_via_os`。viewport ID / 登録 / recreate、detached predicate、runtime / host ownership、rect matching、geometry / placement / focus、window lifecycle は変更なし | 正本 §1.43 の確定済み原因分析が、process-global `GetAsyncKeyState(VK_RETURN)` へのフォールバックで送信元 HWND と extended bit を失うことを根因として特定し、Codex も source inspection で一致を確認した。既存 per-HWND ラッチの有無を `Option` で表し、送信元不明だけを false にする ownership 修正で、新規状態、登録 retry、時間窓、FS 状態初期化、detached 分岐を追加しない。未登録 / 登録済み viewport、本体 / テンキー分離、既定 Z を owner-level test で固定するため症状パッチではなく §2 に適合する |
| 2026-08-04 | remote ownership を Local / AcquiringRemote / RemoteActive / DrainingRemote の純状態機械へ統一し、local input block を三つの remote phase 全体へ投影。acquire 時は main / active detached の local AI pending を同じ barrier で静止し、drain final release まで既存 modal を維持 | src/ui_fullscreen.rs の passive detached activation gate、src/app/native_video.rs / src/app/gamepad_input.rs / App::common_modal_dialog_open の既存 remote owner predicate、ViewerContextBundle swap を使う final AI / erase pending の列挙。detached viewport ID、runtime/host owner、placement、geometry、focus、window lifecycleには変更なし | remote の見た目だけを塞ぐ guard ではなく、App-global session lifecycle を全 input surface と全 mounted AI producer へ同じ typed phase から投影する ownership 修正。新規 detached bool / Option、geometry heuristic、delay / retry / repaint workaround を追加せず、main と active detached の既存 bundle ownerを同じ cancel / terminal-count 規則で扱う。純 phase test、main modal lifecycle test、既存 active / parked media pause testで境界を固定したため、Codex は §2 に適合する構造的修正と判断。**ClaudeCode 同意** — detached 側の差分は述語 1 行の改名 (`remote_session_active` → `remote_session_blocks_local_control`、`src/ui_fullscreen.rs` の passive activation gate) で、`src/app/native_video.rs` / `src/app/gamepad_input.rs` も同じ改名 1 行ずつ。新しい分岐を足していない。唯一の新機構である `LocalAiActivityLease` は AI job の生存期間に束ねた RAII counter で、viewer context ごとの `pending` map を liveness の代理にしていた点を置き換えるもの。bundle swap で map から外れても worker は生きているという **所有権の取り違えが根本原因**であり、その原因側を直しているので症状 guard ではないと判断した。detached の bool / Option / geometry heuristic / delay / retry / repaint 追加は無く、viewport ID・runtime/host owner・placement・focus・window lifecycle も未変更であることを diff で確認した。 |
| 2026-08-05 | ファイル名 facet の正規化 cache・generation stamp・pending receiver・failed generation を、導出元の `items` / `items_generation` と同じ `ViewerContextBundle` 所有へ移動。query / tokens / debounce は全 context で同じ条件を使う App-global 状態として維持。poll を所有ごとに 2 分し、cache lifecycle は mount 中の context から、入力 debounce は main を mount した本流からだけ進める | `src/app.rs` の既存 bundle / swap / main-grid split と active detached の mounted-owner poll 境界、`src/app/facet_name_filter.rs` の cache lifecycle、`src/app/tests.rs` の active detached bundle 回帰テスト。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、placement / geometry / focus、window lifecycle は変更なし | ClaudeCode が、bundle ごとに独立した generation と同件数が偶然一致すると別 context の basename cache を silent に採用できる所有境界不整合を特定し、Codex も source inspection で producer / consumer / pending / failure stamp がすべて items snapshot に従属すると確認した。swap 時破棄や generation guard を足さず、cache lifecycle 全体を owning context の create / mutate / drain / invalidation / drop に揃え、同 generation・同件数・異名の 2 bundle を直接検証するため、症状パッチではなく §2 に適合する構造的修正であると双方合意 |
| 2026-08-05 | 見開きの高さ合わせを解決後の物理倍率ではなく実効フィット方式だけで決め、`Original` は固有寸法、それ以外は高さ合わせに固定 | `src/ui_fullscreen.rs` の共有 spread geometry 選択と、通常 / Z ズーム共通見開き、連結読み、既存 detached frozen spread snapshot producer。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、placement / focus、window lifecycle は変更なし | ClaudeCode の実装 brief が、倍率依存の geometry 再選択をレイアウト跳ねと Z ズーム 2 段解決の根因として特定し、3 箇所を fit-mode-only helper へ集約する構造修正を指定した。Codex も source inspection で同型 producer と Z の feedback loop を確認した。新規状態・detached 分岐・guard / retry / repaint を追加せず、通常と frozen snapshot が同じ fit mode 契約を共有するため、症状パッチではなく §2 に適合する |
| 2026-08-05 | 標準拡大の Lanczos3 出力を画像全体ではなく表示 trim ∩ viewport の可視 source 領域だけから生成し、その source UV を共有 `FullscreenPaintResource` の出力 identity と描画位置へ保持 | `src/displayed_image_transform.rs` の共有 screen/source transform、`src/gpu_lanczos.rs` の既存 typed paint resource / viewer-context cache、`src/ui_fullscreen.rs` と `src/app.rs` の single / spread / continuous frozen snapshot producer・consumer・keep-alive。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、placement / focus、window lifecycle は変更なし | 実機 perf log と今回 brief が、full source × zoom による上限 fallback を根因として確定し、可視 source 領域を既存 transform から毎回導出して typed resource に焼き込む構造修正を指定した。Codex も crop texture を full image として貼ると配置が壊れるため producer と consumer が同じ source UV identity を共有する必要を確認した。detached 専用 bool / pending / retry / heuristic を追加せず、通常・frozen とも同じ transform / resource contract を使い、縮小 entry と detached lifecycle を分岐させないため §2 に適合する |
| 2026-08-04 | フルスクリーンの透過背景・原画プレビュー・スライドショー進捗・比較ピンの4インジケータを、固定バーが確保した共通 `fullscreen_media_rect` 基準へ統一 | `src/ui_fullscreen.rs` の main fullscreen / detached 共通 overlay 描画。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、media geometry、placement / focus、window lifecycle は変更なし | 正本 `brief-v2.11.0-topbar-indicator.md` が、バーごとの定数補正ではなく既存 content rect へアンカーを統一する構造修正を ClaudeCode の実装仕様として固定。Codex の source inspection でも、4 consumer だけが window 全体 rect を使ってバーの共通予約境界を迂回していたと確認した。新規状態・detached 分岐・高さ定数・guard / retry / repaint を追加せず、同じ viewport で既に解決済みの content rect を全 consumer が共有するため、症状パッチではなく §2 に適合する |
| 2026-08-04 | 「縮小時のなめらかさ」を Lanczos3 の支持幅スカラーとして追加し、正規化済み percent を共有 `FullscreenPaintResource::Lanczos` と viewer-context cache key の出力 identity に含めた。設定変更時は context 内の Lanczos 出力だけを clear し、旧 percent の snapshot resource を同一出力として再利用しない | `src/gpu_lanczos.rs` の既存 typed paint resource / `GpuLanczosCache`、`src/ui_fullscreen.rs` の共通 prepare 入口、`ViewerContextBundle` が既に所有する cache lifecycle。detached snapshot producer / geometry / predicate、viewport ID / 登録 / recreate、runtime / host ownership、placement / focus、window lifecycle は変更なし | 正本 `dot-by-dot-and-downscale-plan.md` §4.3.3 が、別表示経路や detached 専用フラグではなく同じ Lanczos3 経路の scalar と cache identity で扱う決定を固定。Codex の producer / consumer 確認でも、既存 typed resource に処理結果の identity を焼き込み context owner で無効化すれば足り、guard / retry / repaint / geometry heuristic は不要だった。保持中 resource の旧出力を percent 不一致で拒否するため、症状パッチではなく §2 に適合する ownership 修正である |
| 2026-08-04 | GPU Lanczos3 の C-1 製品統合で、元 `TextureHandle` を論理寸法 owner として維持し、表示時の `TextureId` だけを差し替える `FullscreenPaintResource` を live / holdover / detached snapshot で共有。Lanczos 出力と native ID の lifetime を snapshot と viewer-context cache の owner に結び付けた | `src/app.rs` の既存 `DetachedImageWindowSnapshot` / frozen page / deferred view payload と `ViewerContextBundle`、`src/ui_fullscreen.rs` の single / spread / continuous frozen snapshot producer と keep-alive backstop、`src/app/vram_accounting.rs` の detached bundle / snapshot 会計。snapshot geometry、detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、placement / focus、window lifecycle は変更なし | 正本 `dot-by-dot-and-downscale-plan.md` §4.3.2 と段階4 brief が、C-1 と「全経路を同じ typed paint resource へ通し、別 bool / Option を増やさない」方針を ClaudeCode / Codex の合意事項として固定。Codex の producer / consumer inventory でも detached は通常表示と同じ最終 paint resource を保持すれば足り、専用 guard、retry、recreate、geometry heuristic は不要と確認した。元 handle を geometry owner のままにして snapshot の既存正規化 rect / UV / clip を一切再解決せず、cache を bundle の swap / park / drop lifecycle に含める ownership 修正なので §2 に適合する |
| 2026-08-04 | 物理整数倍率の見開きでは左右ページの高さ合わせを外して固有寸法・縦中央配置を維持し、高さ合わせが残る非整数倍率ではページ別実効倍率で原点スナップを判定 | `src/ui_fullscreen.rs` の共有 spread geometry と、通常見開き・連結読み・既存 detached frozen spread snapshot layout。frozen snapshot はその viewport の `ctx.pixels_per_point()` を引き続き使い、選択後 geometry から正規化矩形と clip を導出。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、placement / focus、window lifecycle は変更なし | 正本 `dot-by-dot-and-downscale-plan.md` §4.2.1 と ClaudeCode の実機原因確認が、旧 `combined_h / page_h` による片ページだけの非整数拡大を根因として確定。Codex も通常 / frozen / 連結読みの同型 producer を確認し、detached 専用 guard や新規 bool / Option を足さず共有 geometry owner で修正した。既存 snapshot consumer と lifecycle を分岐させず、viewport 固有 ppp、物理 1:1、ページ別スナップを同じ不変条件でテストするため §2 に適合する構造修正である |
| 2026-08-03 | 100% 原寸 / 拡大しない / 縮小しないを viewport 固有の物理 px 基準へ変更し、物理整数倍率で描画原点を pixel boundary へ整列。見開き gap と連結読みの累積位置も物理 px へ量子化 | `src/displayed_image_transform.rs` の共通 transform producer、`src/ui_fullscreen.rs` の単ページ・見開き・連結読みと detached single / frozen spread snapshot layout、`src/app.rs` の既存 detached snapshot ppp 受け渡し。render 中は対象 `ctx.pixels_per_point()`、ctx を持たない park snapshot は既存 `detached_viewer_last_pixels_per_point` を使用。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、rect matching、geometry / placement / focus model、window lifecycle は変更なし | 正本 `dot-by-dot-and-downscale-plan.md` §4.1–4.2 と実装 brief が、論理倍率 1.0 と非整数原点を根因とし、detached / frozen は各 viewport の ppp を使う構造修正として ClaudeCode / Codex 双方の判断を固定している。新規 detached bool / Option、時間窓、retry、heuristic、recreate を追加せず、既存 context と既存 detached render probe の所有値だけを transform producer へ渡した。描画と `DisplayedImageTransform` を同じ snapped rect に統一し、consumer、predicate、host lifecycle を分岐させないため §2 に適合する |
| 2026-08-02 | 利用者仕様変更に従い remote video player を remote session 所有の headless player へ移し、増分 13 の remote replacement surface と normal folder/fullscreen open を撤回 | `src/remote_ipc/ui.rs` / `src/app.rs` の remote player owner。`src/ui_fullscreen.rs` / `src/app/native_video.rs` は増分 13 前の music-only predicate へ復元。`src/app/startup_ops.rs` の remote fullscreen entry を削除。detached runtime / host / placement / geometry / focus / lifecycle に新規変更なし | remote start が viewer presentation を一切要求しないため、detached 分岐を追加・補修するのではなく freeze 範囲への依存を除去する。player tick と stream tap は remote state 内で継続し、本体は既存 modal のままという利用者確定仕様を owner 境界で表現する。増分 13 で合意済みだった一般化を残す理由が消えたため、同変更を丸ごと撤回  **ClaudeCode 同意** — 増分 13 で触れた述語が music-only 条件へ戻り、detached seam への依存自体が無くなったことを確認した。remote player を fullscreen context から remote session context へ移した結果であり、症状 guard ではない。認証面も別途確認済み: identity 解決は `auth != Unauthorized` で gate され、`/stream/` は fail-closed guard の下のまま。 |
| 2026-08-02 | remote video start の address を既存 folder/fullscreen open 経路へ通し、ローカル設定が detached media window を選ぶ場合も既存の presentation one-shot で mounted core player を選択 | `src/app/startup_ops.rs` の既存 folder load / loaded-file selection / `open_fullscreen` seam と `fs_media_open_forced_presentation`。detached predicate、viewport/runtime ownership、host registry、geometry/focus、window lifecycle は変更なし | ユーザー提示の正本は remote session が本体 player を占有し encoder tap を接続する設計であり、Codex の source inspection でも detached player では App 所有 `fs_cache` から streaming session を開始できないことを確認した。新規 detached bool / Option、placement state、geometry heuristic、delay/retry を追加せず、既存の一回限りの presentation request で通常 open を mounted context に具体化するため、症状パッチではなく既存 owner 境界の再利用として §2 に適合。**ClaudeCode 同意** — `fs_media_open_forced_presentation` が detached-rework stage-audio (`b2063ef3`) 由来の既存機構であり、今回の差分がその利用箇所を 2 つ増やすだけで新規の述語 / bool / Option / delay / retry / reset を導入していないことを確認した |
| 2026-08-02 | remote session が操作権を取得した時だけ、mounted / active detached / ParkedLive の current media transport を停止し、停止位置を保存。tray hide は transport を変更せず再生を継続する現行方針を維持 | `src/app.rs` の既存 `fullscreen_idx` + `fs_cache`、`ActiveDetachedViewerContext.bundle`、passive window の `paused_bundle` という media-owner projection と remote-session acquire handler。`src/app/tests.rs` の remote acquire / tray residency 回帰テスト。detached predicate、viewport 登録、runtime ownership / state transition、host registry、placement、geometry、focus、window lifecycle は変更なし | ユーザー提示の ClaudeCode 分析と Codex の source inspection が一致し、コンパイル不能の原因は remote acquire が tray 専用だった削除済み helper を呼んでいる意味的衝突と確認。新規 detached bool / Option / pending、delay / retry / repaint / reset を追加せず、既存 bundle の current `VideoPlayer` に `set_playing(false)` を送り、既存 resume 保存 API を使う remote-session 固有の ownership projection である。3 owner の停止・位置保存・state 保持と、tray では play intent を維持する既存テストで境界を固定するため、症状パッチではなく §2 に適合すると双方合意 |
| 2026-08-02 | native video の cursor input ownership を幾何 `cursor_within_client` から、presenter / HUD の source-stamped mouse edge を drain 単位で集約する pump-owned typed router (`Unknown / Presenter / Hud / CapturedPresenter / CapturedHud`) と auto-hide reducer へ移動。ownership 喪失後は cursor を書かず外部 window の `WM_SETCURSOR` に委ねる | `src/video/native_cursor.rs`、`src/video/native_window.rs`、`src/video/native_window_host.rs` / `hud_window.rs`、`src/video/native_window_pump.rs`、`src/video/native_presenter/render_core.rs` と event forwarding / health projection。`DetachedViewerChild` も同じ presenter/pump 経路を通るが、detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus model、VST owner handoff / anchor / bridge C++ は変更なし | 2026-08-02 の Codex Sol 設計レビュー確定事項と現行 source inspection が、別 top-level VST editor が presenter client rect 内に重なると幾何判定を input ownership と誤用することを根因として一致した。新規 App bool / Option、時間 grace、retry、`WindowFromPoint` / `SendMessage`、WM_SETCURSOR 復帰、VST owner workaround を追加せず、HWND owner pump の既存境界へ source identity・capture・transition を集約した。presenter↔HUD 双方向 handoffの通知順、hidden/activity clock、範囲外 capture / 自己 release / 外部喪失、startup Unknown / placement / hide / close / stale epoch、別 top-level window show/hide を owner-level test で固定するため、症状 guard ではなく §2 に適合する構造的 ownership 修正である |
| 2026-08-02 | Win32 `KeyEdge` に送信元 HWND / `ViewportId` を焼き付け、全 consume / pressed / frame-state / Enter-held API を対象 viewport 必須に変更。subclass 済み HWND と viewport の対応、Enter held、未登録診断を `key_input` の単一 owner に集約し、`WM_NCDESTROY` で対応と同じ HWND 由来の未処理 edge を除去 | `src/key_input.rs` の既存 subclass / edge queue、`src/keymap.rs` と操作カスタマイズ key capture の consumer、`src/ui_fullscreen.rs` の既存 rect 捕捉後 subclass install 呼び出しへ既知の `fullscreen_viewport_id()` を明示的に渡す箇所。detached predicate、viewport ID の生成 / recreate、runtime / host ownership、rect matching、geometry / placement / focus model、window lifecycle は変更なし | ユーザー提示の案D正本（Codex Sol 設計レビュー + ClaudeCode）と Codex の source inspection が、全 HWND の edge から送信元 identity を捨てる単一共有 queue を根因として一致した。edge 作成時の OS HWND と既存の安定 `ViewportId` を事実として保存し、消費時の前面窓推測、App-global bool / Option、TextEdit 個別 guard、delay / retry / fallback broadcast を追加しない。`find_visible_thread_window_matching_rect` の条件も変更しない。sibling 非消費、登録解除 / HWND 再利用時の stale 対応除去、未登録 HWND の記録付き `ROOT` 配送を owner-level test で固定する routing ownership 修正であり、症状パッチではなく §2 に適合する |
| 2026-08-01 | gilrs の global gamepad gate を、App root / 対象 fullscreen・detached viewport の IME state と native presenter overlay の `NativeOverlayInputRouting::wants_keyboard_input` を union する単一述語へ訂正 | `src/app/gamepad_input.rs` の既存 gamepad dispatch predicate、`src/video/mod.rs` の既存 native output event bus と output-local latest snapshot、`src/video/native_presenter/render_core.rs` の既存 public routing snapshot。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus routing、window lifecycle は変更なし | ユーザー提示の ClaudeCode 実機ログ (`overlay=ROOT FFFF`, `detached=39E5`) と Codex の source inspection が一致し、独立 egui Context の overlay state は App の viewport data から原理的に見えず、presenter を迂回する gilrs だけが keyboard routing の保護を素通りすると確認した。既存の一方向 output event bus で typed latest snapshot を publish し、App から presenter を直接参照する逆向き経路、新規 detached bool / Option、delay / retry / viewport heuristic を追加しない。root / viewer / presenter / 全 inactive の predicate test と snapshot consumption test で ownership 境界を固定するため、症状パッチではなく §2 に適合すると双方の分析が一致 |
| 2026-08-01 | App-global IME composition state を撤去し、App shortcut gate と TextEdit helper が同じ `ViewportId` 単位の temporary state を参照するよう統一。静止画 FS の固定 Esc / 矢印も pass owner が発行する専用 raw-key permit 経由へ変更 | `src/ime_focus.rs` の既存 viewport-keyed IME / focus contract、`src/app.rs` の ownership snapshot と IME facade、`src/ui_fullscreen.rs` の既存 main / fullscreen / detached 共通 handler と各 `ctx` 参照。detached predicate、viewport ID / 登録 / recreate、runtime / host ownership、geometry / placement / focus routing、window lifecycle は変更なし | ClaudeCode 正本 `keyboard-input-ownership-plan.md` と今回の実装 brief が、timeout clear や editor 個別 guard ではなく viewport 粒度への ownership 移動を構造修正として指定し、Codex の source inspection でも stuck App bool が sibling viewport を停止する根因と確認した。新規 App bool / Option、delay / retry / reset、detached heuristic は追加せず、A の未完了 IME が B を止めない lifecycle test、実描画順の Pending / FocusRecovery、FocusedUi slider 退行 test で境界を固定したため §2 に適合 |
| 2026-07-31 | remote session active 中のローカル入力を App-global owner で遮断し、active/passive detached viewport に同一の切断 modal を描画。解放時の再読み込みでは item identity を保持して既存 fullscreen open 経路へ復元 | `App::common_modal_dialog_open`、`src/ui_fullscreen.rs` の active/passive viewport modal と passive activation gate、native presenter/gamepad の既存入力入口。detached runtime ownership、host registry、placement/geometry、bundle swap は変更なし | 利用者が「操作者は常に1人」「remote が無確認で取得、local は切断で奪還」と確定した session ownership 境界を全入力 surface に同じ predicate で投影する機能増分。viewport ごとの session bool、geometry heuristic、delay/retryは追加せず、App-global typed session owner と既存 modal/input入口だけを使う。復帰も新規 detached invalidation を作らず既存 reload/open を再利用するため、detached の症状パッチではない |
| 2026-07-31 | hidden tray residency では通常の winit redraw が `WM_PAINT` を生成できず `App::update` が 0 Hz になる契約を明示し、既存 tray thread の 50ms pump から active media / continuous EOF handoff 中だけ hidden main HWND へ UI tick を配送。mounted / active detached / ParkedLive の current player と App-global owner-stamped EOF resolver / typed source-swap を同じ projection で扱い、paused / 処理済み terminal EOF / still は完全 sleep に戻す | `src/app.rs` の既存 mounted / `ActiveDetachedViewerContext.bundle` / passive `paused_bundle` media owner の read-only 走査と `poll_video` 前の projection publish、`src/tray_integration.rs` の tray hide / restore bridge、`src/tray.rs` の既存 tray pump。detached / switching predicate、viewport 登録 / ID、runtime state transition、host registry、placement、geometry / focus、transport、close semantics は変更なし | ユーザーの実機報告と Codex の source inspection から、`request_repaint` は hidden HWND への `RedrawWindow(..., RDW_INTERNALPAINT)` まで進むが不可視 window には `WM_PAINT` が届かず、decoder / audio / native render だけが継続することを確認。次 item 選択は items / display order / file-existence / owner stamp / source-swap を App が所有するため、別 thread への移設はその全 ownership の再設計になる。新規 App bool / Option、detached 述語、viewport recreate、delay / debounce / grace / retry は追加せず、wake は既存 `VideoPlayer` play intent + EOF / error fact、typed pending action、`IsWindowVisible` から毎 tick 導出する。50ms は既存 tray message pump の cadence であり競合を時間窓判定しない。resident EOF が次の音声へ進む App-level test、paused / non-media と hidden / visible gate test を追加したため、症状 guard ではなく UI / media ownership 間の欠落 scheduler bridge として §2 に適合 |
| 2026-07-31 | close-to-tray を presenter/viewport の破棄または transport pause ではなく、同一 session/window identity の hidden residency として統一。close を横取りした update frame でも active fullscreen/F12 と passive/ParkedLive viewport を登録し、egui host とその WS_CHILD presenter を維持する。mounted / active detached / ParkedLive / source-swap pending の native output は既存 typed `WindowHostState::Hidden` と consume-and-hold へ遷移し、復帰で同じ host/frame owner を Visible に戻す。2026-07-30 の pause mitigation を置換 | `src/tray_integration.rs` の tray hide/restore と viewport visibility、`src/app.rs` の update frame 継続・既存 bundle owner 走査・native config 初期 visibility、`src/ui_fullscreen.rs` の既存 viewport builder/初回 visibility release、`src/video/mod.rs` / `src/app/native_video.rs` の output-local initial/queued visibility、`src/video/window_host_contract.rs` の typed transition test、`src/video/native_window_pump.rs` の test config、`src/video/gpu_renderer/d3d11_device.rs` の idle-pool契約。detached / switching predicate、viewport/runtime ownership、host registry、placement storage/sync、geometry/focus model、close/error terminal semantics は変更なし | ユーザー提示の ClaudeCode 調査と Codex の source inspection が一致。`native window visibility cancelled` は資源節約の producer ではなく、tray close を横取りした直後の `App::update` early return が immediate/deferred viewport 登録を省略し、egui host HWND teardown → child presenter `WM_DESTROY` → output cancel を起こした後の正確な terminal 結果だった。main root は persistent なので戻るが detached host は ephemeral なので失われる、という §1.28/§1.29 共通原因を ownership 境界で修正した。同じ viewport/session/native host identity を明示 Hidden/Visible に遷移させ、新規 App bool/Option、recreate、retry、time window、geometry heuristic、placement path、focus guard を追加しない。hidden 中も full-rate decode と最新 frame hold を継続し、active GPU lease/DComp/swapchain を保持する資源コストを仕様化しつつ idle pool だけ解放する。host identity の hide→show、running transport/session の mounted 3 presentation・音声モード・単体音楽・active/ParkedLive、restore non-close を unit test で固定するため、症状パッチではなく §2 に適合すると双方の分析が一致 |
| 2026-07-31 | フォルダ移動 holdover の payload を単一 texture から画面順の単ページ / 見開き `FsDisplayUnitHoldover` へ統一し、通常 fullscreen・PDF/ZIP 列挙待ち・in-window・detached keep-alive/backstop をカラー化側と同じ unit 描画へ接続。各 page は texture と capture 時点の rotation / source size / trim bbox を自己所有 | `src/app.rs` の既存 `FsHoldover::FolderNavigation` payload、`src/ui_fullscreen.rs` の holdover capture / draw consumer、`src/app/native_video.rs` の静止画 F11 表示切替時 capture。detached predicate、viewport/runtime ownership、host registry、geometry/focus model、nav lock 解放条件は変更なし | ユーザー提示の ClaudeCode 分析と Codex の source inspection が一致し、producer が anchor 1 枚だけを所有して全 consumer が全面 contain していたことを確認。さらに review で items 差し替え後に旧 idx から新 item の geometry を再解決する欠陥を確認し、payload を自己記述にして除去。新規 detached bool / Option、geometry heuristic、delay / retry / repaint loop を追加せず、既存の typed display unit と spread resolver に ownership 粒度を統一する根本修正である。LTR/RTL の物理画面順、単ページ、index 再利用後の rotation/source size、nav readiness の一方向ラッチを unit test で固定し、カラー化とフォルダ移動の解放条件は別 variant のまま維持するため、症状パッチではなく §2 に適合すると双方の分析が一致 |
| 2026-07-30 | タスクトレイ格納前に mounted / active detached / ParkedLive の全 media transport を明示 pause し、復帰後は同じ位置から手動再開できる paused session として保持 | `src/tray_integration.rs` の tray lifecycle、`src/app.rs` の既存 `ViewerContextBundle` media owner 走査と通常 fullscreen surface の focus 復元。detached / switching predicate、viewport/runtime ownership、host registry、geometry model、typed placement request は変更なし | ユーザー承認済みの ClaudeCode 調査で、格納時に presenter が drop され音声だけ別系統で継続し、復帰後の terminal close が表面化していたことを確認。Codex の source inspection でも tray hide が owner を close/drop する一方、再生継続を前提にした detached 例外が残る非対称と一致した。新規 bool / Option / pending、detached geometry heuristic、delay / retry は追加せず、既存 `VideoPlayer::set_playing(false)`、resume 保存、bundle ownership、focus grace を再利用する transport owner 境界の修正であり、動画 / 音声 / 3 presentation / active+ParkedLive / restore-frame non-close の回帰テストで不変条件を直接検証するため §2 に適合すると双方合意 |
| 2026-07-30 | 動画→音声モードの hidden native presenter が visible 用 pacing queue を迂回し、到着済み映像 frame を全件 consume して最新 1 枚だけを typed `Hidden` state に保持 | `src/video/mod.rs` の native render/source frame drain と既存 `FramePresentationState`。`DetachedViewerChild` も同じ native output を通るが、detached predicate、viewport/runtime ownership、host registry、geometry/focus model、window lifecycle は変更なし | ユーザー提示の ClaudeCode 実機ログ（`video_tx_len=8` / `pkt_rx_len=32`）と Codex の source inspection が一致。現行 hidden も clock-paced selection は行っていたが、visible と同じ `source.queue=8` cap のため channel を継続 drain できず video decoder が demux EOF 前で停止することを確認した。新規 bool / Option、timer、強制 EOF、decode 停止を追加せず、既存 typed `Hidden` owner を最新 frame で置換し続ける ownership 修正であり、visible の bounded back-pressure と detached/window lifecycle を変更しないため §2 に適合すると双方合意 |
| 2026-07-30 | PDF 初回レンダを、各 viewer の実 viewport・DPI・fit mode を所有する typed display target から導出 | `ViewerContextBundle` に `PdfDisplayTarget` を保持し、`src/ui_fullscreen.rs` の実 `CentralPanel` media rect と `pixels_per_point` から更新する。`start_fs_load` は target を PDF worker へ渡す。detached predicate、viewport runtime / host、geometry / focus model は変更なし | ユーザー提示の ClaudeCode 調査（初回 4096px 固定）と Codex の source inspection が一致し、専用 fullscreen / detached / in-window の実描画 context でのみ正しい物理 viewport が得られることを確認した。新規 detached bool、geometry heuristic、delay / retry を追加せず、表示要求を mounted bundle が所有して worker がページ寸法・content type と同時解決する構造修正であるため、症状パッチではなく §2 に適合すると双方合意 |
| 2026-07-30 | hidden presenter 音声モード中に毎フレーム出ていた detached host resync の skip ログを削除 | `src/app/native_video.rs::try_resync_detached_video_host` の既存 early-return 内にあった診断出力だけ。`video_audio_mode_hides_native_presenter_for` の predicate、戻り値、host resync / viewport / geometry / focus の制御フローは変更なし | ClaudeCode の実機ログ解析で同一メッセージが 1 session 3214 回出力され、状態変化時だけにするか削除する方針が提示済み。Codex もこの分岐が hidden 中は常に「解決済み」を返す既存制御で、反復ログに状態遷移・所有権上の意味がないことをコード確認した。新規 bool / Option / rate-limit timer を足さず診断副作用だけを除去するため、detached の症状パッチや runtime 変更ではなく §2 の凍結範囲を維持する |
| 2026-07-30 | トレイ復帰フレームの main-focus close が、格納で維持した host 待ち / placement switch 中の detached 動画を閉じる非対称を修正 | `src/app.rs` の current viewer lifecycle predicate と main-focus consumer。App-global predicate は active detached sibling も含む一方、current predicate は現在 mount 中の session だけを判定し、複数 context を混同しない。`src/ui_fullscreen.rs` の viewport cleanup、host registry、geometry / focus model は変更なし | 修正後実機ログの `App state synced` → thumbnail queue → conceal/text reset という順序から、前回の同期的な外部再読込ではなく update 後段の close と判定。Codex の production-path テストで `sync_after_restore` 後の main focus consumer が修正前に `fullscreen_idx` / `VideoPlayer` を落とすことを再現し、ClaudeCode の「別経路が残る」分析と一致した。格納側と同じ current detached / switching fact から main-blocking を導き、新規 bool / Option、delay / retry、外部変更抑止、cleanup guard を追加しない ownership 修正なので §2 に適合。`cleanup_visible_false` の `host=0` は同関数内で runtime 除去後に出すログ値であり、producer ではないため変更しない |
| 2026-07-30 | トレイ格納で維持した detached メディア session を、復帰時の外部フォルダ更新でも維持 | `src/app.rs` の main-context change promotion が、格納側と同じ `viewer_session_is_detached_or_switching()` から確定済み / 切替中 session を判定。viewport runtime、host registry、geometry / focus modelは変更なし | ClaudeCode のログ解析どおり `sync_after_restore` の外部変更反映が `start_loading_items` から `close_fullscreen` へ落ち、`VideoPlayer` を drop していた。Codex の source inspection では原因は外部変更機能そのものではなく、格納述語が detached 切替中を含む一方、context promotion が確定済み detached だけを含む非対称と確認した。新規 bool / Option、delay / retry、外部変更機能の抑止を追加せず、既存 context ownership 境界で同じ述語を共有し、外部追加 items と元 `VideoPlayer` の同時維持を回帰テストする構造修正なので双方の分析が一致し §2 に適合 |
| 2026-07-30 | ページ単位のカラー化 holdover を単ページ texture から `SpreadDisplayUnit` と同じ単ページ / 見開き typed state へ変更し、見開き左右の準備完了を原子的に判定 | `App` / `ViewerContextBundle` の既存 `fs_holdover_tex` を `FsHoldover::FolderNavigation` / `ColorizeDisplayUnit` の単一 enum へ置換し、`src/ui_fullscreen.rs` の通常 fullscreen overlay で旧 display unit を再描画。detached predicate、viewport runtime / ownership、host registry、geometry / focus modelは変更なし | ユーザー提示の ClaudeCode 原因分析（1 texture slot と `fullscreen_idx == idx` のページ別 fallback が見開き相方を黒にする）と Codex のコード確認が一致。左右用 field、detached bool / Option、delay / retry を追加せず、既存 folder-nav 一方向ラッチは `FolderNavigation` variant 専用 consumer として維持した構造修正であり、paged spread / single / colorize OFF / continuous / cover・shift・landscape・RTL と folder-nav latch の回帰テストで ownership invariant を直接検証するため §2 に適合 |
| 2026-07-30 | native video §1.25/§1.26: native output context が最新 frame を `Empty` / `Hidden { frame }` / `Visible { frame, fence }` の単一 typed state で所有し、paused grade refresh と placement prime を同じ state から行う。GPU frame は render copy 後に reader key 1 へ直接 release し、置換・解放後の writer key 回復は producer 側へ集約 | `src/video/mod.rs` の native output frame lifecycle と既存 `SwitchPlacement` render orchestration、`src/video/native_presenter/render_core.rs` の keyed-mutex read handoff。`NativeVideoPlacement::DetachedViewerChild` も通常 fullscreen と同じ output-local state を消費するが、detached predicate、viewport/runtime ownership、host registry、geometry/focus model は変更なし | ユーザー提示の ClaudeCode 原因分析と Codex の現行 Stage 4 コード確認が一致。`present_retire` / hidden frame / source queue に分裂していた最近 frame の所有を output-local enum へ統合する根本修正で、App-global detached bool/Option、placement store、geometry heuristic、delay/retry/repaint loop を追加しない。prime 失敗時も旧 typed state/host/core を保持し、pump thread に GPU 呼び出しを持ち込まず、re-arm 用 `AcquireSync` も追加しないため症状パッチではなく §2 に適合 |
| 2026-07-29 | native video Stage 4: 全 placement/HUD の HWND owner を専用 `native-video-window-pump` へ移し、GPU/DComp/present 専用の `native-video-render` と production で分離。request/ack reducer による hidden staging と atomic publish、typed shutdown、render-fault quarantine を接続 | `src/video/mod.rs`、`src/video/native_window_pump.rs`、`src/video/native_window.rs`、`src/video/native_window_host.rs`、`src/video/native_window_host/hud_window.rs`、`src/video/native_presenter/render_core.rs`、`src/video/window_host_contract.rs`、`src/app/native_video.rs`、`src/app.rs`、`src/video/dsp/mod.rs`。既存 `NativeVideoPlacement::DetachedViewerChild` と host registry/owner HWND を typed request の read-only 入力として使い、detached predicate、viewport runtime/ownership、rect/focus model は変更なし。placement switch 失敗時は旧 host/core を保持 | ClaudeCode が Stage 1〜3 と着手前に「症状パッチではなく構造的修正」と判断済み。Codex も §2 を再確認し、根本原因の「HWND owner thread が unbounded GPU work を実行」を child/popup/HUD の全 topology から型/thread 境界で除去した。新規 detached bool/Option/pending、viewport heuristic、delay/retry、機能制限を追加せず、production render-stall 中の parent destroy と pump join が各 2 秒未満で進む watchdog test により ownership invariant を直接検証したため §2 に適合 |
| 2026-07-29 | native video Stage 2: HWND owner (`NativeWindowHost`) と GPU/DComp core (`NativeRenderCore`) を型/module で分離し、detached placement も同じ host 境界へ通した | `src/video/mod.rs` の既存 `NativeVideoPlacement::DetachedViewerChild` create/switch/reflow 呼び出しを host API へ移動。detached predicate、viewport/runtime、rect/focus 意味論は変更なし | ClaudeCode が事前に「症状パッチではなく構造的修正」と判断済み。全 placement/HUD の owner-thread invariant を同じ型境界で固定する責務分離であり、新規 detached bool/Option/pending、delay/retry、viewport heuristic を追加しないため §2 に適合 |
| 2026-07-29 | 複数ウィンドウ / independent detached session ではメイン一覧の見開き相方カーソルを描画しない | `App::main_grid_spread_pair_cursor_idx` から既存の `detached_independent_session_blocks_folder_nav` を再利用 | 破線カーソルはメイン一覧と同期する F12 linked session 専用の表示であり、独立 session の ownership と矛盾していた。事前分析と Codex のコード裏取りが一致し、新規 bool / Option / detached 述語、viewport / lifecycle 変更を伴わない構造的な表示責務の修正と判断 |
| 2026-07-29 | nav lock 中の Ctrl+↑↓ / sibling 入力を detached physical bundle 自身の folder-nav request へ累積 | `handle_fullscreen_ctrl_nav_context` / `handle_fullscreen_sibling_nav_context` の lock 分岐から既存 `detached_physical_folder_nav_available` を読み取り専用 resolver で再利用 | nav lock を入力拒否ではなく context-owned holdover の表示確定世代へ限定する修正。App / detached runtime に新規状態を足さず、mounted bundle の `folder_nav_pending` / `FolderNavResult` だけが request と累積を所有する。ユーザー提示の ClaudeCode 分析と Codex のコード裏取り・追加 ownership 分析が一致し、症状 guard ではなく入力 router と request owner の構造修正として双方合意 |
| 2026-07-29 | 同一フレームの Ctrl+↑↓ / sibling 物理押下数を既存 folder-nav request へ渡す | `key_input` / `keymap` の edge cardinality と、`handle_fs_key_input` → `handle_fs_navigation` の既存 generic dispatch。detached predicate、viewport/runtime、request owner は変更なし | ユーザー提示の ClaudeCode 根本原因分析と Codex のコード裏取りが一致。未消費 edge の寿命は 1 フレームのまま、新規 bool / Option / pending、delay/retry を追加せず、mounted bundle が既に所有する `folder_nav_pending` と上限 5 の accumulator へ物理押下回数を渡す構造修正として双方合意 |
| 2026-07-29 | キーボード入力所有権 S3: root / fullscreen の各 viewport pass で型付き `KeyboardOwner` を一度だけ決定・共有 | `App::update`、`handle_fullscreen_root_key_input`、`handle_fs_key_input` の既存入力入口で共通 snapshot / pass cache を収集。detached predicate、入力送信元 routing、viewport / window lifecycle は変更なし | ClaudeCode レビューを前提に設計確定した `keyboard-input-ownership-plan.md` の S3 をそのまま実装し、Codex も純粋決定関数と既存判定への互換投影を確認した。新規 detached bool / Option、geometry / focus heuristic、delay / retry は追加せず、pending claim も bookmark TextEdit の 1 pass focus 要求だけを型付きで所有するため、症状パッチではなく全 viewport 共通の入力 ownership 境界である |
