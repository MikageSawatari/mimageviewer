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
5. **時間窓 (debounce / grace / settle ms) で競合を吸収しない**。stale F12 再配送を
   `GetAsyncKeyState` の物理状態で棄却した実例のように、**事実 (OS 状態・世代・
   イベント) で判定**する。時間窓は「頻度を下げるだけでループを消さない」。
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

### 9.1 現況 (唯一の進捗表、2026-07-26 コード確認)

この表だけを現行ステージ判定に使う。後続のコミット・検収記録は、各時点の実装履歴であり、
現在の完了判定ではない。コードから実装を確認できた範囲を記載し、7 月 24〜26 日分を含む
直近変更の手動検証状況は未確認とする。

| ステージ | 現況 | コード上の到達点 / 残件 |
| --- | --- | --- |
| R0 | **完了** | geometry 非依存の生成前後 HWND registry 方式を採用済み |
| R1 | **完了** | detached host HWND は registry 所有へ移行済み。キー入力 subclass の rect 探索は R1 の対象外として残り、BA-1 の後続課題 |
| R2a | **完了** | `DetachedWindowRuntime` と manager を導入済み |
| R2b | **部分完了** | Runtime routing と `ParkedLive` は実装済み。HWND 再生成・差分登録・watcher repair は `ParkedLive` を保持し、OS host 状態だけで live media state を降格させない。純粋 reducer、合法遷移制約、散在 pending/flag の typed state 集約は未完 |
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
| terminal close routing | **基盤実装済み・不変条件は未完** | session/runtime の terminal 経路はあるが、close 後の全 in-flight producer 停止は未達。BA-7 / R2b の残件 |
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

| 日付 | 変更 | 触れた範囲 | 合意の根拠 |
| --- | --- | --- | --- |
| 2026-07-29 | 複数ウィンドウ / independent detached session ではメイン一覧の見開き相方カーソルを描画しない | `App::main_grid_spread_pair_cursor_idx` から既存の `detached_independent_session_blocks_folder_nav` を再利用 | 破線カーソルはメイン一覧と同期する F12 linked session 専用の表示であり、独立 session の ownership と矛盾していた。事前分析と Codex のコード裏取りが一致し、新規 bool / Option / detached 述語、viewport / lifecycle 変更を伴わない構造的な表示責務の修正と判断 |
