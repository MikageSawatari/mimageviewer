# Sol 角度別レビュー (2026-07-10、コンパクト後の未カバー角度 3 本)

視点をずらした複数回レビュー戦略の一環。既存レビュー (mode/race/perf/crosscut/hunt/
batch2/sol-review) が触れていない 3 角度を gpt-5.6-sol の独立新規セッションで並行実施した。
プロンプトはセッション scratchpad の `codex-angle{1,2,3}-prompt.txt`。

| 角度 | thread_id | 結果 |
|---|---|---|
| ① リソース/リーク (長時間セッション) | 019f4a4b-0c0c-7d40-b727-bfaca4ae0437 | P1×1 → 修正済み |
| ② 設定永続化・ダウングレード互換 | 019f4a4b-2335-74e3-ab6c-1f99fb8d951e | P1×1 (既知の再発見) → 修正済み |
| ③ 音声/動画エンジン正しさ | 019f4a4b-2a8d-7c12-81bd-2773d61bffc2 | **P1/P2/P3 ゼロ** (engine 104 + music 22 テストも実行) |

## 角度① P1: 音楽解析ワーカーの非キャンセル区間で PCM が積み上がる

- **機構**: `cancel_music_analysis()` は atomic flag + receiver drop だけで、
  `music_core::analyze_stereo_timeline()` (progressive partial / 最終確定の両呼び出し) は
  内部にキャンセル点が無かった。解析 1 パス実行中の放棄ワーカーは全尺 PCM の
  `Arc<MusicPcm>` (4h クランプで最大 ~5.5GB 予約) を保持し続け、長尺ファイルを解析中に
  連続切替すると重なって積み上がる。
- **判定**: 実在 (コード内コメントにも「analyze_stereo_timeline は cancel 不可」と明記
  されていた)。恒久リークではなく「解析 1 パス分の保持が重なる」bounded transient だが、
  長尺 × 連続切替で RAM を圧迫し得るため修正。
- **修正**: `analyze_stereo_timeline_cancellable()` を music-core に追加
  (bin 集計ループ + chroma FFT 窓ループで `should_abort` を確認、中断で `None`)。
  既存 `analyze_stereo_timeline` は never-abort の wrapper に。呼び出し側
  (`run_music_analysis` の partial/final、`audio_decode::analyze_audio_file_with_config`)
  は cancel トークンを渡す。放棄ワーカーの PCM 保持は最大 1 bin/1 FFT 窓分の遅延で解放。
- **不採用**: Sol 提案のうち「final 解析の bounded executor 直列化」は協調キャンセルで
  重なり自体が消えるため不要。「PCM の全尺 reserve をやめる」は決定性優先の設計判断
  ([[feedback_deterministic_over_adaptive]]) と realloc memcpy 回避のため据え置き。
- **テスト**: `analysis_cancellable_aborts_and_matches_when_not_aborted` (music-core)。
- **検収ラウンド 2 (Sol 追い込み)**: 後処理パス 4 本 (`apply_timeline_summary_metrics` /
  chroma 割当 + `smooth_pitch_confidence` / `apply_novelty_scores` /
  `estimate_simple_beat_grid`) にキャンセル点が無い — 10ms bin × 4h = 144万 bin では
  窓スキャン O(bins×radius) と BPM 自己相関 111 候補 × O(bins) がそれぞれ秒単位、と
  正当な指摘。**全パスに `should_abort` を貫通** (bins ループは `ABORT_CHECK_STRIDE`=4096
  ごと、BPM は候補ごとに確認、中断は bool/Option で伝播して `None` 返し)。
- **検収ラウンド 3: 「The fix is verified. No remaining findings.」** 残る非キャンセル区間は
  bounded な線形パス (vector collect / onset 構築 / BPM 1 候補分 / 結果構築) のみで蓄積
  リスクは消滅、4096 stride も妥当、executor 直列化はもはや不要、と Sol 側で確認。

## 角度② P1: facet_filter の Audio が v2.2.0 ダウングレードで設定 DB を隔離させる

- **機構**: `FacetItemKind::Audio` (v2.3.0 新設) を `facet_filter.kinds` に含んだまま保存
  すると、v2.2.0 (Audio variant も `#[serde(other)]` も無い) の deserialize が失敗 →
  boot decision tree が Corrupted 判定 → settings.db 隔離 + bak 世代へ巻き戻り。
  Audio を含む bak も連鎖隔離され、再アップグレード後もその世代以降の設定変更が失われる。
- **判定**: 既知 (review-v2.3.0 hunt P2、当初は「リリースノート + version_highlights
  must_read で注意喚起」で対応予定) の再発見。ただし Sol の被害掘り下げ (bak 連鎖 +
  再アップグレード後も残る巻き戻り) を踏まえ、注意喚起ではなく保存形の互換化で解消する
  方針に変更。
- **修正**: `FacetFilter::kind_audio_stash: bool` を追加。`save_full` は保存用クローンで
  `stash_kind_audio_for_persist()` (kinds から Audio を除去して bool へ退避)、読込側は
  `Settings::sanitize` の `restore_kind_audio_after_load()` で kinds へ復元。v2.2.0 は
  未知フィールドを無視するのでダウングレードしても壊れない。実行時の正は常に kinds。
  これに伴い version_highlights の v2.3.0 must_read (注意書き) と release-note-drafts の
  ⚠️ ダウングレード注意を削除。
- **テスト**: `facet_kind_audio_stash_keeps_persisted_form_v22_compatible`
  (保存形に "Audio" が現れない + v2.2.0 形状 enum で deserialize 成功 + restore 往復) と
  `save_load_roundtrip` 拡張 (DB 往復で Audio が kinds に復元される)。
- **注意 (将来)**: `FacetItemKind` に新 variant を足すときは同じ退避が必要
  (enum コメントに明記済み)。
- **検収結果**: 修正確認済み (save_full が唯一の書き込み経路 / 全 boot 経路が sanitize
  収束 / 修正前 v2.3.0 dev DB の kinds 直入り Audio も読める、を Sol 側で確認)。
  新規 [P3] = ダウングレード → 再アップグレードで音声フィルタの選択だけ消える
  (v2.2.0 のブートストラップ保存が未知フィールド `kind_audio_stash` を落とすため)。
  これは v2.2.0 側の挙動なので v2.3.0 からは対処不能。破壊・巻き戻りは伴わない表現
  ロスであり**許容** (フィルタを 1 回選び直すだけ)。

## 角度③: エンジン正しさ — 指摘ゼロ

EOF ドレイン / seek epoch / pause-freeze / 音声会計 / source swap の double-close /
hidden presenter のフレーム消費 / VST un-hide 検証 / 解析結果の path ゲート / spectrum
in-flight 制限を確認してクリーン。`cargo test --lib video::engine` (104) と
`cargo test --lib music_` (22) も実行して green。

## 角度④: 破壊的ファイル操作 × 新サブシステム (Sol + Terra 起用実験、2026-07-10)

同一プロンプトで Sol (gate) と Terra (実験) を並行実行。**P2×3 (Sol) + P1×1 (Terra 独自)**
= 停止基準は未達。「新角度が新バグを出す」パターンがまた再現した。

- **(A) 削除後に parked/detached bundle が整合されない** [Sol P2 / Terra P1 相当]:
  `poll_delete_pending` は mounted `self.items` への `remove_items_batch` のみで、
  `detached_image_windows[*].paused_bundle` (および live detached bundle) を訪れない。
  parked 窓は削除済みアイテムを表示し続け、そこへの★/タグは存在しないファイルの
  メタデータを作る。再アクティブ化は stale path を開こうとする。resume position の
  purge も無し。**v2.3.0 新規領域** (複数ウィンドウ)。コード確認済み (app.rs
  poll_delete_pending 完了部)。→ **修正スコープはユーザー判断待ち** (detached 凍結
  ルール対象。最小案 = 削除 path を現在表示中の窓は close + resume purge)。
- **(B) 削除前に対象を再生中の player を止めない** [Sol P2]: `start_delete_files` は
  即 worker spawn。active/parked メディア窓の decoder handle が共有違反で削除を失敗させ、
  teardown は成功後にしか走らないため失敗が繰り返される。エラーは「N 件の削除に失敗」
  のみで原因非表示。メディア別窓 (v2.3.0) で顕在化。→ **ユーザー判断待ち** (最小案 =
  spawn 前に削除対象 path を持つ player の窓を close)。
- **(C) rename が parked bundle と path-keyed DB を移行しない** [Sol P2 / Terra P2]:
  rename 成功時は親フォルダ一致時の pending_reload のみ。タグ/★/回転/補正/サムネ/
  resume が旧 path キーのまま残り、生存 parked player は旧名で resume を書き戻す。
  **DB 移行欠落は rename 出荷時 (v2.2.0, 2026-06-14) からの既存ギャップ**で v2.3.0
  リグレッションではない。v2.3.0 新規部分は parked bundle との相互作用のみ。
  → 完全な rename transaction は次リリース候補 (バックログ)、v2.3.0 では
  「rename 対象を表示中の窓を close」の最小対応をするかユーザー判断待ち。
- **(D) XMP 書込ワーカーと delete/rename の復活競合** [Terra P1・**修正済み**]:
  `write_atomically` が無条件 rename するため、レーティング/タグ書込の read〜rename の
  間に対象が削除/改名されると削除済みファイルを旧 path に復活させる。**既存 (v2.2.0)**
  だが窓が狭くない (大きい JPEG で数十 ms)。修正 = rename 直前の存在確認で破棄
  (TOCTOU は µs 級に短縮、完全排除ではない)。テスト =
  `write_atomically_discards_when_target_disappeared`。

**Terra 実験の所見**: Sol と同じ 2 領域 (A/C) を独立に検出しつつ、Sol が出さなかった (D)
を追加検出。severity 判定は Terra の方が強め (A を P1)。クリーン判定した領域
(tag surface 分離 / 仮想フォルダの rename 到達不能 / undo の path identity) は両者一致。
モデル多様性はこの角度でも有効だった。

## 角度④ (A)(B)(C) のスコープ確定と実装 (2026-07-10 ユーザー判断)

- ユーザー判断: 「削除やリネームしたときに、再生中の窓を一度閉じる動作はそのままで良い」
  = **(A)(B) は最小案 (該当窓 close + resume purge) で v2.3.0 実装**。(C) の rename
  transaction は「可能ならば設定値を引き継ぎたい」= ストア全数調査の上で見積もり報告
  (docs/next-release-backlog.md §1.8 に調査結果と段階案を記録)。
- 実装 (`release_viewer_surfaces_for_removed_paths` / `purge_video_resume_positions_for_
  removed_paths` + `removed_path_key_matcher`):
  - 削除は `start_delete_files` で worker spawn **前**に呼ぶ (decoder handle 解放、(B))。
    リネームは成功後に旧 path で呼ぶ ((A))。
  - 照合 = 正規化キー完全一致 + フォルダ配下 `<key>/` + アーカイブ内 `<key>::`。
    対象 = passive/parked 窓 (stamp / descriptor / paused_bundle の表示アイテム・
    current_folder・Video player) + active detached bundle + メイン fullscreen。
  - **メイン fullscreen は「player が対象 path を掴んでいる」ときだけ閉じる**。静止画は
    閉じない (remove_items_batch の隣スライドという既存再整合があるため。閲覧しながら
    削除する UX を維持)。bundle に含まれるだけのアイテムも閉じない (再活性化 fail-safe
    に任せる)。
  - resume purge = video_resume_positions + video_resume_thumb_last_request を
    succeeded (削除) / 旧 path (リネーム) で retain-out。
  - テスト 4 本 (matcher prefix / parked-live close+purge / still 窓の選択的 close /
    音楽解析+ラウドネス測定の path 一致 cancel)。
- **(A)(B) 実装の Sol 検収 (3 ラウンド)**: R1 = matching/close 手順/順序/スライド維持は
  verified、追加 P2「**音楽解析ワーカーは App-global で窓 close では止まらない** (FFmpeg
  handle が残り削除が共有違反)」→ 修正 (music_analysis_path 一致で clear_music_view_state、
  pending のみ一致は cancel_music_analysis のみ。解析が今日のキャンセル貫通で ms 級に
  閉じる前提)。R2 = 音楽解析対応 verified、追加 P2「**ラウドネス測定 (normalize_state) も
  App-global**」→ 修正 (file_path 一致で take + cancel、fs_idx 不使用)。R3 = 最終確認。

## rename transaction 段階 1+2 の実装とレビュー (2026-07-10 夜)

ユーザー判断「広めに実機検証を行う今回のうちに段階 2 まで」を受けて実装。
実装 = `src/rename_key_migration.rs` (worker、zip_key_migration 方式) + App 配線
(FIFO キュー / in-memory presence set・resume 書換 / 完了時キャッシュ引き直し)。
対象ストア・許容制限の正本はモジュール doc。

**Sol レビュー R1 (P1×1 + P2×2 + P3×1) → 全対応**:
- **P1 連続リネームの順序逆転** (A→B 直後の B→C が並列 worker で逆順実行されると
  中間 path に全メタデータが取り残される) → **FIFO 直列化**
  (`rename_migration_queue` + in-flight 1 本、poll が完了後に次を開始)。
  根拠テスト = `sequential_chained_renames_require_fifo_ordering` (逆順だと B に
  取り残される事実も固定) + App レベル FIFO テスト (tempdir override)。
- **P2 view_trim.db の移行漏れ** (ストア全数調査エージェントの見落としを Sol が補足。
  view_trim_pages = keep-drive / view_trim_books = drive 除去) → 両テーブルを
  ターゲット追加 + 完了時 `clear_page_edit_state()` で in-memory 側も引き直し。
- **P2 実行待ちタグ書込が旧キーを復活** (tag worker はジョブ実行時にキー解決して
  tags.db を書く) → **書込 worker が空になるまで移行開始を遅延**
  (`rename_migration_writers_busy` = tag is_busy || rating is_busy。残ジョブが先に
  旧キーへ着地 → 移行がまとめて新キーへ運ぶ = 順序回復)。RatingWriteHandle に
  submitted カウンタ + is_busy() を追加 (旧名サイドカー再作成の余地も塞ぐ)。
- P3 fmt / 未使用 import → 修正。

**Sol レビュー R2**: FIFO / view_trim / rating ゲート / テストは verified。追加 P2 =
タグゲートの `is_busy()` は worker の DB 書込完了で false になるが、UI 側の結果消費
(`poll_tag_write_results` の sidecar ミラー書込) はフレーム後半に走るため、移行開始後に
旧 path サイドカーが書き直され得る → **`has_unconsumed_batch()` へ変更** (結果消費まで
true)。rating 側は sidecar 書込が worker 内で完結するので is_busy のまま (理由コメント化)。

**Sol レビュー R3: 「Verified. no further findings.」** テスト 9 本 + bin フル 3304 green。

## 角度⑤: 起動 / 終了 / first-run / portable × 新機能 (Sol + Terra、2026-07-10 夜)

Sol と Terra が同一 P1 に収束 + 各自の P2。プロンプトで「終了時の rename 移行キュー」を
要確認事項として明示した箇所を両者とも実証した。

- **P1 (両者一致): rename 移行が終了 / クラッシュ / トレイ Exit に耐えない** — キューと
  in-flight は in-memory のみで、on_exit は drain せず、トレイの「終了」(hidden 時) は
  `std::process::exit(0)` で on_exit 自体を通らない。ファイルは新名なのにメタデータが
  旧キーに永久に取り残される (Sol: release-blocking)。→ **ジャーナル方式で修正**:
  `rename_migration_journal.json` (data_dir、atomic 置換・空で削除) に in-flight+queue+
  boot_retry を同期永続化し、**report を受信できたジョブだけ消し込む**。起動後の最初の
  enqueue/poll 前に lazy 読込 (回復エントリの上書き防止) → キューへ再投入して冪等再実行。
  worker は catch_unwind で panic も report 化。回復ジョブの完了時は presence set /
  resume キーの書換もやり直す (通常経路では no-op)。
- **Terra P2: spawn 失敗で dequeue 済みジョブが消える** → queue 先頭へ戻して再試行
  (ジャーナルにも残存)。
- **Sol P2: 終了時に再生中の位置を resume map に確定しない** → on_exit_inner で
  `save_all_video_resume_positions()` を保存前に呼ぶ。
- **Sol P2 (送り): 終了と削除 worker の未調整** — 部分削除の最終報告と完了後処理が
  落ちるが破壊なし・v2.2.0 既存挙動 → backlog §1.10。
- **検収 R2 (Sol 追い込み)**: catch_unwind で panic を report 化した際に**消し込んで
  しまう**のは誤り (panic = 残ストア未試行の可能性、per-store エラーと性質が違う) →
  `RenameMigrationReport::panicked` フラグを追加し、panic は Disconnected と同じ
  「boot_retry + ジャーナル残置 + 次回起動で冪等再実行」(セッション内リトライなし =
  決定的 panic の無限ループ回避)。テスト
  `rename_migration_panic_keeps_journal_entry_for_boot_retry`。
- クリーン確認 (両者): 空 must_read の highlights 表示 / detached placement のモニタ
  検証 / first_setup / portable の data_dir 一貫性 (view_trim・移行・pdf_passwords 含む) /
  close-to-tray の detached セッション温存 / 単一インスタンス mutex。
- テスト: journal roundtrip / 起動回復 (App) / FIFO のジャーナル消し込み。

## 角度⑥: グリッド / サムネパイプライン × 複数ウィンドウ (Sol + Terra、2026-07-10 夜)

Sol P2×2 = Terra P2×2 (完全一致) + Terra P3×1。**いずれも bounded な効率問題で
データ喪失・誤適用は無し** → park/再活性ライフサイクルの構造対応が要るため
**backlog §1.9 へ送り** (リリース直前の凍結領域パッチを避ける判断):

- P2: park してもサムネ pipeline (cancel_token / reload_queue / heavy_io_queue) が
  止まらず、積み残しデコードが走り切って誰も poll しない rx に溜まる。
- P2: サムネ VRAM 上限が mounted 文脈単位で、parked N 窓が合算で超過し得る
  (動画サムネは eviction 対象外)。
- P3: `display_px_shared` が App-global でデコード解像度がメイングリッドに引きずられる。
- クリーン確認 (両者): 文脈別 channel/cancel/世代で cross-context 汚染なし /
  catalog の Arc+mutex 共有 / sync stamp の item-key fallback / perf 計装の文脈誤り
  なし / Shell 動画サムネの文脈ローカル送信。

## 角度⑦: 修正間相互作用 (Sol + Terra + Luna 同一プロンプト、2026-07-10 深夜)

ユーザーの仮説「コード修正後の再レビューは別の問題を見つける」を検証する回。単純な
再実行ではなく「今日入った修正群 × 相互作用」に絞った新プロンプト。**3 モデルが同一の
P1×2 + P2 に収束** (Luna 起用実験も有効と確認 — Sol/Terra と同じ 2 P1 を独立検出)。

- **P1: 削除がキュー済みリネーム移行を無効化しない** — A→B 改名 (移行がタグ書込ゲートで
  待機) → B を削除 → 後から移行が走り、削除済み B のメタデータ行を A から作り直す。
  ジャーナルにより再起動を跨いでも起きる。→ 修正 = `invalidate_rename_migrations_for_
  removed_paths` (削除成功 path を新側に持つ queue/boot_retry エントリを retain-out +
  ジャーナル書き戻し。in-flight は中断不能・孤児行は通常削除と同性質のため対象外と明記)。
- **P1: 移行完了時の `clear_page_edit_state` が無関係な編集を破壊** — Terra が最悪の帰結を
  特定: ドラッグ中の補正値は in-memory のみ → 全消し後にマウスを離すと release handler が
  「値なし」と見て `clear_page_params` = **保存済み補正の削除**。→ 修正 = 完了時の全消しと
  無条件 pending_reload を廃止し、`rehydrate_contexts_after_rename_migration` で
  「リネームに関係する文脈だけ」既存の `rehydrate_page_edit_state_for_current_items` で
  再構築。main はドラッグ中なら `rename_rehydrate_main_deferred` でドラッグ終了後に繰り延べ。
- **P2: 移行ギャップ中に新 path を開いて hydrate した bundle が恒久 stale** (bundle-local
  の idx presence は再活性化でも再読込されない) → 上記 rehydrate が active detached ctx と
  該当 parked bundle を `swap_viewer_context_bundle` で一時 mount して再構築
  (poll_parked_live と同じ swap 手順。parked は編集中でないため即時で安全)。
- テスト 3 本追加 (delete_invalidates_queued... / rename_completion_preserves_unrelated... /
  rename_completion_rehydrates_matching_parked_bundle)。
- クリーン確認 (3 者): FIFO の A→B→C 連鎖 / ジャーナル回復順序 / タグゲートの
  sidecar ミラー / F12 遷移中の active close / ActionSurface ルーティング。

**学びの更新**: 「同角度の単純再実行はクリーン」だが「**修正が積もった後の相互作用
レビュー (新スコープ) は有効**」— ユーザーの直感どおり。モデル 3 本 (Sol/Terra/Luna) の
一致度も高く、相互裏付けとして機能した。

**検収 (Sol、2 ラウンド)**: R1 = 3 件の修正は verified、追加 P2「合成ビュー (全体検索 /
サブ展開) は current_folder が合成 path のためフォルダ照合に掛からない」→ `items_ref`
(items の drag_source_path 走査、O(items) はリネーム稀のため許容) を 3 文脈すべての
照合に OR 追加。R2 = **Verified**。テスト計 4 本追加 (delete 無効化 / 無関係編集温存 /
parked bundle 再構築 / 合成ビュー再構築)。bin フル 3311 green。

## 実機 FB: メディア窓 P ピンがメイン窓グリッドに反映されない (2026-07-10 深夜)

**症状 (ユーザー実機)**: フル機能モード + 動画別ウィンドウで、メディア窓の P ピンが
メイン窓のグリッドサムネに反映されない。

**原因**: `video_thumb_overrides_dirty` (bool) の消費 = `close_fullscreen` の 1 箇所
だけで、そこで現フォルダを再ロードして pin WebP を snapshot し直す設計。従来の
同一窓フルスクリーンは ESC で必ず通るが、§1.7 構成ではメイン窓のグリッドが開きっ
ぱなしで反映契機が来ない。`folder_thumb_pin_dirty` が過去に踏んだ同型バグ
(Codex Phase D P2) の video pin 版。

**修正**: bool → `video_thumb_overrides_dirty_paths: HashSet<PathBuf>` (producer 4 箇所
が path を挿入)。`consume_video_thumb_overrides_dirty()` ヘルパー化し、close_fullscreen
に加えて `App::update` (`!main_viewer_blocked`) からも毎フレーム消費。ユーザー指定の
「pin した動画がメイン表示中に存在するときだけ再ロード」= 可視性判定付き
(`video_pin_dirty_paths_visible` 純関数 + `video_pin_dirty_visible_in_items`)。
`pending_pin_thumb_refresh` (後追い WebP 補完) 中は consume を保留し、
「古いフレームで一度再ロード → 補完後にもう一度」の二度手間を防ぐ。

**Sol レビュー (新セッション、R1)**: P1×2 + P2×1。
- **P1-1 (修正)**: ピン解除が同 path の pending 補完を破棄しておらず、解除後に
  set_pin が走ってピンが復活する (既存バグ、consume ゲートで顕在化しやすくなる) →
  解除時に path 一致の pending を破棄。
- **P1-2 (修正)**: 可視性判定が Video セルのみで、代表サムネ 📌 の cascade が dirty
  動画へ解決される Folder タイル (親フォルダ表示中) を見逃す → seed と同じ
  folder_pin_map + cascade 解決で Folder タイルも照合 (`video_pin_dirty_visible_in_items`)。
- **P2 (許容)**: 検索/タグ等の合成ビューでは可視でも再ロード分岐に入らずサムネが
  古いまま → bool 時代 (ESC 閉じ) と同一挙動で退行ではない。ビューを閉じてフォルダへ
  戻る load_folder で反映。コメントに既知制限として明記。

**検収 (Sol、R2)**: 全項目 **Verified**・新規問題なし。テスト 3 本追加 (可視性判定 /
consume の破棄+pending 保留 / Folder cascade 可視性)。実機チェックリスト #21b〜21d 追加。

**実機退行 (同日夜)**: 2 回目の P で全動画サムネが黒に固着、再生終了で復帰。ログ解析で
**再ロードストーム**を確認 (`=== load_folder ===` が ~50ms 周期 ×15 秒 ≒ 300 回。毎回
動画サムネスレッドが 20/140 件で cancel され Pending=黒のまま)。ループ構造 =
`persist_native_video_marker_thumbnail` (マーカー帯キャッシュの保守書込) の Pin arm が
無条件に set_pin + dirty 再点火 → consume が再ロード → メイン文脈の load_folder が
**App-global の fullscreen_video_marker_cache を破棄** (メディア窓は再生継続中) →
再構築時の sync が未 decode の cached=None を見て player から再抽出 → pending_saves
再発行 → persist → dirty → …。1 回目の P はキャッシュ decode が先に完了して収束、
2 回目は persist が先行して発火し続けた。

**修正**: ① Pin arm を `lookup_meta` でゲート — pin_pts 一致 (1e-3) かつ
`thumb_is_current()` なら set_pin も dirty も発行せず、in-memory 表示フィールドだけ
同期 (pending_saves の再発行も止まる)。実際の pin 変更後の初回 persist は従来どおり
書込 + dirty (メディア窓モードでは tick_pending_pin_thumb_refresh がメイン文脈の
fullscreen_idx チェックで即 abort されるため、マーカー経路が WebP を埋める本命)。
② consume に **1 秒クールダウン** (`video_thumb_reload_last_at`) — dirty は take せず
保持し、間隔経過後に処理。将来の未知の producer 再点火でもストーム不可能に。
テスト +1 本 (cooldown)。checklist #21c2 追加。

**Sol 検収**: ループ遮断 **Verified** (skip 分岐の in-memory 同期で pending_saves 再発行も
停止、bookmark 再保存は dirty を触らないので load_folder を駆動しない)・クールダウンも
Verified (dirty が恒久 stuck する経路なし)。新規 P1 なし。P2 注記 = メタ照合は blob 内容を
検証しないため、同一パスの動画差し替え等で旧フレームが DB に残り得る → **許容制限**
(byte 比較は抽出フレームの揺らぎで再ループするリスクと引き換えのため不採用。ピンの
付け直しで回復)。コード上にコメントで明記。

## 実機 FB: 右パネルがカーソル移動で早期クローズ (2026-07-10 夜、Sol 委任作業の初例)

**体制**: ユーザー指示によりトークン節約のため **調査・修正の実作業を Sol に委任**
(`codex exec --sandbox workspace-write`)。Claude はアンカー提示 + 検収のみ。

**症状**: 画像ウィンドウ (別窓) で右端ホバー → 右パネルが開く → パネル内へ左移動すると
パネル外に出る前に閉じる。左パネルは正常。

**根本原因 (Sol)**: 端ホバーで補正パネルとの同時表示 (forced 描画経路) に入ると、
`draw_metadata_panel` の forced 分岐が `metadata_panel_hover_active` ラッチを毎フレーム
false に破壊 → activation ストリップ (右端の細い帯) を出た瞬間に sustain 判定 (描画矩形 +
margin) へ遷移できず即クローズ。**同一窓フルスクリーンにも同じバグが存在** (別窓は
viewport が狭くストリップが細いため再現しやすかっただけ)。座標系の不一致は無し。

**修正 (Sol)**: 純関数 `metadata_panel_hover_active_at` (開く=右端トリガ / 維持=描画矩形
+ sustain margin) を新設し、forced/通常の両描画経路が同じラッチ更新を通るように統一。
回帰テスト 1 本 (非ゼロ原点・狭幅 viewport でのラッチ維持)。

**横断監査 (Sol)**: 静止画×右 = 修正 (main/detached とも)。静止画×左 / native 動画×左右 /
音楽ビュー×左右 = 影響なし (動画は presenter 物理px→pt 変換後に描画と同一矩形で sustain、
音楽は矩形を 1 回構築して描画と共有)。

**Claude 検収**: 非 forced 経路のセマンティクス等価を diff で確認。ラッチ参照 2 箇所への
影響も確認 — ①ホイール抑制 `has_right_panel` は forced 中に描画パネル矩形で判定される
ようになり正しい方向 ②カーソル自動非表示は `!adjustment_mode` ガード既存で変化なし。
checklist #21e 追加。

## 実機 FB: 音声メディア窓の波形グラフがフォルダ移動でリセット (2026-07-10 夜、Sol 委任 2 例目)

**症状**: §1.7 でメディア窓の音声再生中、メイン窓のフォルダ移動で再生は継続するが
波形グラフが再表示 (解析リセット) される (checklist §3 #18)。

**根本原因 (Sol)**: 最初のフォルダ移動で音声セッションが `active_detached_viewer_context`
へ退避された後、次の移動の `close_fullscreen` が P2-3 ガードをすり抜けて
`clear_music_view_state` を実行 (解析 / PCM / spectrum / timeline cache / 解析版数を破棄)。
P2-3 ガードは **ParkedLive の paused_bundle しか検査しておらず、active なメディア窓
bundle を見落としていた**。

**修正 (Sol)**: `detached_music_window_exists` 述語を新設 (active bundle + ParkedLive
paused_bundle の両方で音楽ビュー所有を判定) し、close_fullscreen の clear ゲートと
P2-3 の open ガード (`should_clear_music_view_on_open`) で共用。実際のメディア窓 close は
bundle を mount してから close_fullscreen に入るため consumer が残らず従来どおり
teardown される。同一窓の音楽フルスクリーン終了も従来どおり clear (退行なし)。

**Claude 検収**: 述語構造・open ガードの分岐 (音声=不変 / 非メディア=温存 / 動画=従来
どおり破棄 [メディア窓 1 本規則で窓ごと切替のため]) を確認。回帰テスト 1 本追加
(active §1.7 音声 bundle + main close_fullscreen → 解析 path/PCM/版数が生存)。

## 実機 FB: §7 削除中断 + リネーム拡張子喪失 (2026-07-10 深夜、Sol 委任 3・4 例目・並行実行)

**#40 削除中断 (Sol 修正)**: メディア窓再生中の動画を削除 → 窓は閉じるが
「削除に失敗しました」×2 (H:、log = `Shell 操作が中断されました` 29ms 後)。原因 =
decoder teardown が非同期で、`FOF_NOERRORUI|FOF_SILENT` 下の IFileOperation がロック中
ファイルで abort する競合 (「中断」は GetAnyOperationsAborted + 残存の自前マッピング、
delete_worker.rs:401)。修正 = worker 内リトライ `recycle_chunk_with_retry` (abort /
ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION / COPYENGINE_E_SHARING_VIOLATION_SRC/DEST
のみ、200ms×5、成功分確定・失敗分のみ再投入、バックオフ中 20ms キャンセル応答)。
テスト 3 本 (retry 判定 / 失敗分のみ再投入 / チャンク継続)。

**#41 リネームでファイル消失 → 拡張子喪失と確定 (バグではなくデータも無事)**:
ユーザーがダイアログで拡張子ごと編集して消していた (一覧は認識拡張子のみ表示のため
「消えた」ように見えた)。ダイアログは v1.6.0 から同挙動。ユーザー判断で
**Explorer 方式の拡張子保護を実装** (Sol): ①ファイルはダイアログを開いたとき拡張子を
除いた stem だけを初期選択 (TextEditState の CCursorRange、文字数ベースで日本語対応、
one-shot) ②確定時に最終拡張子が変わる/消える場合は確認ステップ (「変更する/戻す」、
Enter=変更・Esc=戻す、IME セーフ既存パターン準拠) ③フォルダ・拡張子なし・dotfile は
対象外、大文字小文字のみの変更は非変更扱い。純関数 2 つ + テスト 2 本。
種別判定はダイアログ open 時に 1 回だけ fs 照会。

**Claude 検収**: 両 diff 精読 (リトライの非対象 HRESULT 即時失敗 / 確定処理の同フレーム
二重発火なし / 今日の移行フック非破壊)。checklist #40 注記 + #41 注記 + #41b 新設。

## 実機 FB: 音声モードのホイール不統一 (2026-07-10 深夜、Sol 委任 5 例目)

**症状**: 動画再生中に ♪ で音声モードにすると、グラフ上のホイールはスクロールするが
それ以外では無効 (音声/動画の通常再生では前後ファイル移動)。

**根本原因 (Sol)**: detached native 動画の二重ホイール抑止述語
(`should_suppress_egui_wheel_for_native_detached_video`) が presenter HWND の存在だけで
判定しており、音声モードでは presenter が hidden でも HWND が残るため egui 側の
ホイールが破棄されていた。

**修正 (Sol)**: 述語に `music_view_active` を追加 (`fs_music_view_active` 中は egui へ
通す)。統一仕様 = 音声ファイルと同じ「グラフ上 = ScrollArea 縦スクロール / それ以外 =
前後ファイル移動 (adjacent_navigable_idx → keep_audio_mode で音声モード維持)」。
通常の detached 動画は従来どおり抑止 (二重入力防止維持)。テスト 1 本追加 (述語)。
メイン窓 fullscreen は元から共通経路で挙動維持。

## 実機 FB: ZIP リネーム後の一覧 stale + 末尾削除で fullscreen クローズ (2026-07-10 深夜、Sol 委任 6 例目)

**#46 ZIP リネーム後 stale (Sol 修正)**: リネーム成功時のリロード条件が
`current_favorite_target()` 照合で、この helper はアーカイブ override / 列挙 state 中に
None を返すため ZIP 絡みのセッション状態でリロードが抑止されていた → `current_folder`
直接照合 (path_eq) に変更。

**#42 末尾削除で閉じる (Sol 修正 + Claude 検収追記)**: **P2-2 (削除 idx シフト) の退行**
と確定。旧実装の数値 clamp が Folder/ZipFile 等のコンテナセルへ落ちると fullscreen が
表示対象を失って閉じていた。新 helper `fullscreen_neighbor_after_removal` = 削除位置へ
詰まった次の表示可能項目 → 無ければ直前 → 全滅なら None (閉じる)。検収で **Audio を
遷移先に追加** (`adjacent_navigable_idx` と同じ「映像なし動画」扱い) + テスト 1 本追記。
Sol テスト 3 本 (末尾→前へ / コンテナ skip / 既存 shift)。

## 収束判定への寄与

- 新角度③ = P1/P2 ゼロ (クリーン 1 本目)。**角度④で P2 群が出たため連続カウントは
  リセット**。(A)(B) 実装 + (C) 方針確定後、さらに新角度 2 本連続クリーンが停止基準。
