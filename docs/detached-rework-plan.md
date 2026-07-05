# Detached viewer 構造リワーク マスタープラン (正本)

作成: 2026-07-05 / ClaudeCode (Fable)
体制: **実装 = Codex / 検収 = ClaudeCode (Fable) / 実機検証 = ユーザー**

対象: F12 別ウィンドウ表示 (detached viewer) と複数ウィンドウ (passive / pin /
always-new) のライフサイクル全体。

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
を持ち、Codex が実装し Fable が検収する。

## 2. 憲法 (全ステージ共通の不変条件・禁止事項) ⚠️ 最重要

実装セッション (Codex) は、**着手前に本節を読み、作業中に矛盾する誘惑が生じたら
手を止めて報告する**こと。これらは過去 15 ラウンド以上の失敗から抽出したルールであり、
「目の前の症状を早く消す」ためにこれらを破ると必ず別経路が壊れる。

1. **rect 一致捕捉に条件を足さない**: `find_visible_thread_window_matching_rect` /
   `_excluding` (src/dwm_transitions.rs) へ、新しい除外・閾値・スコアリング・
   リトライを追加してはならない。滑るケースを見つけたら、直さずに報告して止まる。
   この機構は R1 で detached 経路から**撤去される予定**のもの。
2. **geometry 由来の host_lost を recreate トリガにしない** (提案書 BA-3)。
   viewport の再生成はユーザー/明示イベント起点のみ。
3. **App に新しい detached 用 bool / Option フラグを足さない** (提案書 BA-7)。
   状態が必要になったら R2 で導入する `DetachedWindowRuntime` / 状態 enum に足す。
   R2 より前のステージでどうしても必要なら、追加前に Fable に相談する。
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
   仕様変更で赤くなるテストは、指示書に列挙されたもの以外は Fable に確認してから
   書き換える。

## 3. 体制とステージ実行プロトコル

各ステージは次のサイクルで回す:

| # | 誰が | 何を |
| --- | --- | --- |
| 1 | Fable | ステージ指示書 `docs/detached-rework-stage-<ID>.md` を作成 (完了条件・触ってよいファイル・テスト要件を明記) |
| 2 | ユーザー | Codex に投入。プロンプト例: 「`docs/detached-rework-plan.md` の §2 (憲法) と `docs/detached-rework-stage-<ID>.md` を読んでから、指示書どおりに実装して」 |
| 3 | Codex | 実装 + テスト。同一ステージ内の往復は同一セッション (resume) で継続 |
| 4 | Codex | 完了条件の機械チェック (grep / cargo test) 結果を含む完了報告を書く |
| 5 | Fable | 検収: diff を指示書の完了条件と照合。指摘があれば 3 に戻す |
| 6 | ユーザー | 実機 smoke (指示書が指定する [smoke-matrix](detached-viewer-smoke-matrix-20260630.md) のケース) |
| 7 | — | 緑になったら master へ統合、次ステージへ |

- 検収 (5) は Fable の利用量節約のため「指示書との照合」に絞る。設計判断が
  必要な逸脱が見つかった場合のみ Fable が構造判断をする。
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
- 指示書: [detached-rework-stage-r0.md](detached-rework-stage-r0.md)
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

### R2: 状態の集約 — `DetachedWindowRuntime` + placement 一本化 (提案書 S1 / BA-6, BA-7)

- window_id ごとの `DetachedWindowRuntime { window_id, state, placement, hwnd,
  initial_placement_applied }` を導入し、App に散在する detached フィールド (34 個)
  のうち HWND / placement / 遷移状態に属するものを移す。
- `DetachedWindowState { Opening, Active, Parked, Resuming, Closing }` enum を導入し、
  遷移を 1 つの reducer に集約。one-shot bool 群は遷移の副作用として閉じ込める。
- placement の single source of truth 化。settings は「最後にユーザーが置いた位置」の
  永続 seed のみ。Phase B の既定サイズ拒否ヒューリスティックを削除。
- 規模が大きいため、指示書段階で 2〜3 サブステージ (R2a: Runtime 導入と HWND/placement
  移設、R2b: state enum + reducer、R2c: ヒューリスティック削除) に分割する。

### R3: viewport identity を window_id に一本化 (提案書 S3 / BA-4)

- active / passive / fullscreen の ViewportId を window_id 由来に統一。
  `fs_viewport_generation` は content 世代専用に格下げ。
- active↔passive 切替・folder-nav reopen で OS 窓が作り直されないことをログで確認。

### R4 (実施判断はゲート C で): deferred viewport 化 + state machine 完成 (提案書 S4)

- `show_viewport_deferred` へ移行し「毎フレーム描かないと死ぬ」制約から解放。
  keep-alive / holdover / backstop の特殊フレーム分岐を撤去。
- リスク最大。R3 まで到達した時点の安定度と残バグの性質を見て、実施可否を
  ユーザー + Fable で判断する。R3 までで smoke が安定していれば見送り (= 出荷) も可。

## 5. ゲート (ステージ間の判断点)

- **ゲート A (R0 完了後)**: 取得方式の確定。Fable が R1 指示書を方式に合わせて作成。
- **ゲート B (R1 完了後)**: 「窓の誤同定」クラスのバグが実機で消えたかを確認。
  ここで振動・小窓フラッシュ・host_lost ループの再発が観測されたら、原因を BA に
  対応付けて計画を修正する (先に進まない)。
- **ゲート C (R3 完了後)**: R4 の実施可否 + 出荷判断。出荷基準は §7。

## 6. スコープに関する未決事項 (ユーザー判断待ち、リワークをブロックしない)

1. **動画 detached を第 1 弾リリースに含めるか**: native presenter の host 追従が
   detached で最も脆い部位。静止画のみ先行リリースする選択肢がある。
2. **active↔passive 往復 (bundle swap) の簡素化**: 「passive はスナップショット閲覧
   専用、再アクティブ化 = 同ウィンドウでの再オープン」に割り切ると状態空間が
   大幅に減る。R2 の設計時までに方針を決めたい。

## 7. 出荷ゲート (リリース基準)

- [smoke-matrix](detached-viewer-smoke-matrix-20260630.md) の 3 設定セット × 全ケースを
  **連続 2 回**グリーン。
- その後 **2 週間の実機常用で新規 P1 ゼロ** (P2 以下は backlog 化して出荷可)。
- `panic.log` に detached 起因の新規 panic なし (Y-32 / OOM 含む)。
- 満たせない場合は「設定既定 OFF の実験的機能」としての出荷、または動画 detached の
  部分封印 (§6-1) へフォールバック。

## 8. 現在の未コミット作業の扱い

振動バグ対応 (rect 捕捉への passive HWND 除外リスト、2026-07-05 時点で Codex が
作業中・未コミット) は**応急処置として完成させてコミットしてよい**。ただし:

- コミットメッセージに「stopgap: detached-rework R1 で除外リストごと撤去予定」と明記。
- これが landed してから R0 に着手する (作業ツリー衝突を避ける)。

## 9. 進捗記録

| ステージ | 状態 | 指示書 | 完了日 / メモ |
| --- | --- | --- | --- |
| R0 | 未着手 | [stage-r0](detached-rework-stage-r0.md) | |
| R1 | 未着手 (指示書はゲート A 後に作成) | — | |
| R2 | 未着手 | — | |
| R3 | 未着手 | — | |
| R4 | 実施判断待ち (ゲート C) | — | |

ステージ完了ごとに Fable が本表を更新する。
