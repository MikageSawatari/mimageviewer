# v2.3.0 出荷前レビュー — 横断的な防御漏れハント

担当: 横断パターン (panic 可能性 / エラー経路の後始末 / リソースリーク / 永続データ互換 /
非 Windows ビルド)。対象 = `7eff5a9e..01910684` + 未コミット差分 (現 working tree を正とする)。
調査方法: 差分から追加行 59k 行を抽出し、パターン別 grep + 呼び出し経路の Read 照合。

指摘 6 件: P2 × 2、P3 × 4。

---

## [P2] detached context の bundle 破棄時に per-context サムネワーカープールが永久残留する (スレッド/メモリリーク)

- 場所:
  - src/app.rs:2129-2135 (`ViewerContextBundle` に tx/rx/cancel_token/reload_queue/heavy_io_queue を bundle 化 — **未コミットの review-v2.3.0 P2-8/P2-9 修正で導入**)
  - src/app.rs:20542-20722 (`spawn_thumbnail_workers` — worker ループの終了条件は `cancel_w` のみ。queue 空なら `cvar.wait()` で無期限ブロック)
  - src/app.rs:2471 (`pause_background_work_keep_current_frame` — fs_pending/AI 系は cancel するが **bundle の `cancel_token` には触れない**)
  - src/ui_fullscreen.rs:4963-4968 (`close_detached_image_windows_by_ids` — `retain` で snapshot ごと bundle を drop、cancel なし)。同型: src/app.rs:26066 (`detached_image_windows.clear()`、モード変更)、src/app.rs:26710-26711 (`close_parked_live_media_windows_for_new_media`)
  - `cancel_token.store(true)` の全出現は app.rs:14996 / 15752 / 20484 の 3 箇所のみで、いずれも**マウント中の `self.cancel_token`** を対象にする。bundle として drop される token を立てる経路は存在しない。`ViewerContextBundle` に Drop impl も無い。
- シナリオ: detached book window (ZIP/PDF を別窓で開く) を開く → book load (`load_zip/pdf_as_folder` → `start_loading_items` app.rs:12428-12441 / 17262-) が detached context の queue + token に紐付く worker pool (parallelism 設定により通常 5〜14 スレッド) を spawn → ユーザーが窓を閉じる (× ボタン / 新メディア再生による parked 窓の強制 close / マルチウィンドウモード切替) → bundle が drop され、token は誰も true にしないため worker はキューを掃き切ったあと `cvar.wait()` で**永久にブロックしたまま残る** (queue の Arc は worker 自身が保持するので解放もされない)。detached 窓の開閉を繰り返すたびに 1 プールずつ蓄積する。
  - 付随: parked (passive) 状態の bundle は `poll_thumbnails` も `drain_thumb_results_discard` も回らないため、worker が処理し終えた `ThumbMsg` (デコード済み ColorImage 入り) が bundle の rx に溜まったまま保持される (drain は active mount 中のみ = src/app.rs:27597-27601)。
- 根拠: 上記の各行を Read で照合。旧実装 (HEAD 時点) は token/queue が App-global で、次のロード (どの文脈でも) が旧プールを cancel + wake していたためこのリークは無かった。bundle 化で「最後のロードのプールを止める者」が居なくなった。
- BA マッピング: BA-7 系 (detached 状態の所有権・ライフサイクル)。findings-12 D3 (queue/worker のグローバル汚染) の対策として入れた未コミット修正の副作用であり、**コミット前に修正可能**。方向性としては bundle の破棄経路 (close/clear/retain) で `cancel_token.store(true)` + 両 queue の `notify_all()` を行う teardown ヘルパを通すか、`ViewerContextBundle` に Drop を実装する。
- 確度: 高 (静的解析で経路確認済み。実機での再現は未実施)

## [P2] `FacetItemKind::Audio` の永続化が v2.2.0 ダウングレードで設定全体を Corrupted → quarantine/bak 巻き戻しに落とす

- 場所:
  - src/settings.rs:444-456 — `FacetItemKind` に `Audio` variant を追加 (v2.2.0 には無いことを `git show 7eff5a9e:src/settings.rs` で確認)。この enum には `#[serde(other)]` の受け皿が**無い**
  - src/settings.rs:626-629 — `FacetFilter.kinds: BTreeSet<FacetItemKind>` は `Settings.facet_filter` (settings.rs:1765) として settings_kv に永続化される
  - src/settings_db.rs:1675-1677 — 読み込みは全 kv を 1 つの JSON object にまとめて `serde_json::from_value::<Settings>` する方式で、**1 フィールドでも deserialize に失敗すると全体が `SettingsDbError::Corrupted`** → quarantine + bak fallback 経路 (spec §5 decision tree)
- シナリオ: v2.3.0 でスマートフィルタの「種類 = 音声」を有効にしたまま終了 → `facet_filter` に `"Audio"` が保存される → v2.2.0 に戻して起動 → v2.2.0 の `FacetItemKind` は `Audio` を知らず facet_filter の deserialize が失敗 → 設定 DB 全体が Corrupted 扱いで quarantine → bak1..bak10 を順に試すが、直近 bak も Audio 入りなら連鎖して失敗 → **設定リセットまたは古い世代への巻き戻り** (ユーザー視点: ダウングレードしただけで設定が消える/戻る)。
- 根拠: 同じファイルの `ToolbarFacetFilterItem` (settings.rs:830-832) には「将来バージョンが書いた変種を旧バイナリが読んだ場合」用の `#[serde(other)] Unknown` が用意されており、プロジェクトはダウングレード耐性を設計方針にしている。`FacetItemKind` だけその防御が漏れている。
- 補足 (v2.3.0 側で取れる対策の例、提案のみ): 保存時に `kinds` から `Audio` を除いて書く / facet の kind 永続化を文字列 + 不明値 skip 方式にする。v2.2.0 側は出荷済みで変更不能。
- 参考 (同種で問題なしと確認したもの): `rating_db::RatingItemKind::Audio=9` は末尾追加 + v2.2.0 の `from_db` が `and_then` で None に落とす読み方 (rating_db.rs:428-446) なのでダウングレード安全。keymap / ring / メニュー構成の新アクションは文字列 ID 永続化で不明値は無視される。`SnapshotEntryKind::Audio` (enum 中間挿入) は in-memory 専用で永続化されない (reading_history_db は kind TEXT + `unwrap_or(Folder)`、本リリースで変更なし)。新規 settings キー 7 個 (`quick_folder_drive_current_dirs` / `slideshow_continuous_{wait,scroll}_secs` / `slideshow_continuous_scroll_percent` / `auto_fullscreen_image_folders` / `music_open_resume` / `music_nav_resume` / `detached_viewer_open_images_in_window`) は全て `serde(default)` 付きでアップグレード安全、ダウングレードでも未知キーは無視される。
- 確度: 高 (v2.2.0 実バイナリでの再現は未実施だが、読み込みコードは両バージョン同一経路)

## [P3] keep range / scroll_hint の共有 atomic が App-global のまま per-context worker pool と組み合わさり文脈間汚染する (未コミット修正の残穴)

- 場所:
  - src/app.rs:5509-5510 — `keep_start_shared` / `keep_end_shared` (と `scroll_hint` / `visible_end_shared`) は App-global。`swap_viewer_context_bundle` の swap_field 対象に**入っていない**
  - src/app.rs:20566-20567, 20631-20633 — 各 worker pool (main 用も detached 用も) が同じ atomic を読んで out-of-keep skip を判定
  - src/app.rs:12339-12340 — `start_loading_items` 冒頭で `keep_*_shared` を 0,0 に store (detached context のロードでも同じ atomic を潰す)
- シナリオ: (a) main グリッドのサムネロード中に detached book window を開く → detached 側ロードが共有 keep range を 0,0 に落とす → その瞬間に main pool の worker が pick した項目は `out_of_keep` skip → canceled 送信 → 再エンキュー、という 1 フレーム程度の churn。 (b) 恒常側: detached pool の queue 項目は以後 **main の keep range で** gate され続ける (main が毎フレーム自分の範囲を store するため)。detached はグリッドを描かないので実害は無駄 skip / rx への canceled 蓄積に留まる。
- BA マッピング: BA-7 系。P2-8/P2-9 bundle 化 (未コミット) が channel/token/queue を per-context 化した一方で、worker が参照する keep/scroll atomic 群を global に残した非対称が原因。
- 確度: 中 (経路は確認済みだが、タイミング依存で実害の頻度・大きさは未計測)

## [P3] 非 Windows ビルド破壊: 音楽 chrome 系の cfg(windows) 項目を unguarded 参照

- 場所 (確認できた具体例):
  - src/ui_fullscreen.rs:4166-4167 — `active_music_chrome_view_state` が `#[cfg(windows)]`。呼び出し元 src/ui_fullscreen.rs:21578 (`draw_fs_music_view` 内) は**ガードなし**。同関数は 21502-21505 で `music_shell_active` に cfg(not(windows)) fallback をわざわざ用意しており、この呼び出しだけ漏れている
  - src/ui_fullscreen.rs:1548-1570 — `MusicChromeViewState` 型自体が `#[cfg(windows)]`。src/ui_music_panels.rs:19 の `use` と :727-732 `draw_music_bottom_hud(chrome: &MusicChromeViewState)` はガードなし
- シナリオ: 非 Windows target で `cargo check` すると music view 周りが型未定義でコンパイル不能。現状 CI は ubuntu で `cargo fmt --check` のみ (compile なし)、開発は Windows のみなので**実害は今のところ無い**が、プロジェクトは他所 (app.rs:4999 の「非 windows lib stub 想定」コメント、多数の cfg(not(windows)) stub) で非 Windows ビルドを意図的に維持しており、方針と不整合。
- 補足: 本レンジで `#[cfg(windows)]` 直下の新規宣言は機械抽出で 719 件あり、全数の unguarded 参照照合は静的 grep では不可能 (cfg スコープ解析が要る)。構造的な再発防止は「非 Windows target の cargo check を CI に足す」以外に無い。上記 2 系統は目視確認済みの確実な破壊箇所。
- 確度: 高 (コンパイル理屈。手元に非 Windows toolchain が無く実 check は未実施)

## [P3] music-core `fft_power_window_into` が空 / 奇数長入力で index out-of-bounds panic (出荷バイナリからは到達不能)

- 場所:
  - crates/music-core/src/analysis.rs:662-665 — `stereo_samples[frame_idx * 2]` / `[frame_idx * 2 + 1]` に境界ガードが無い
  - crates/music-core/src/analysis.rs:547 — `ensure_windows` の `size = spec.size.min(frame_count).max(1)`: `frame_count == 0` でも `.max(1)` により size=1 の窓が作られ、空 slice への `[0]` アクセスで panic。奇数長入力 (`len == 2k+1`) では `[frame_idx*2+1]` が OOB
- 根拠: 同型の `analyze_chroma_frame` (analysis.rs:932-939) は `if frame_idx < frame_count` ガード + `hann.get(i)` で防御しており、非対称。
- 到達性: アプリ内の唯一の呼び出し元 `ui_music_spectrum::compute_spectrum` は `MusicPcm::copy_window` → `spectrum_window_range` (ui_music_spectrum.rs:642-665) が `available_frames == 0` で None を返すため空入力は渡らない。PCM は常に偶数長 append (interleaved stereo) なので奇数長も現状発生しない。**panic するのは library API として直接叩いた場合のみ** (tools/music_lab や将来の呼び出し元)。
- 確度: 高 (コードの性質として)。到達性: 低。

## [P3] `ComInitScope` が S_FALSE 時に CoUninitialize を呼ばない (COM 参照カウントの単調増加)

- 場所: src/dwm_transitions.rs:36-53 — `needs_uninit: hr == S_OK`。COM の契約では CoInitializeEx が **S_FALSE を返した場合も CoUninitialize で釣り合わせる必要がある** (S_FALSE = 既に初期化済み、カウントは +1 されている)。
- シナリオ: UI スレッドが既に STA 初期化済みの環境では `move_window_to_desktop_of` (detached 窓を owner と同じ仮想デスクトップへ移す処理、F12 / detached open のたびに呼ばれる) が呼び出しごとに COM 初期化カウントを +1 して返さない。実害はプロセス生存中のカウンタ増のみで、リソースリークとしてはほぼ理論値。
- 確度: 高 (契約違反として)。実害: 低。

---

## 調査したが問題なしと判定した主な項目 (検収側の重複調査防止用)

- **panic ハント**: 追加行の `unwrap()/expect()` 全 69 件を照合。app.rs の 26 件は全て v2.2.0 から存在する移動コード (`git show 7eff5a9e` で snippet 一致確認)、それ以外はテストモジュール内か、ガード済み (`ui_fullscreen.rs:21624` の `music_analysis.as_ref().unwrap()` は 21573-21577 の `show_timeline` ガード下)。`src/audio_decode.rs` / `MusicPcm` は try_reserve + cancel + layout 正規化で防御済み。`fs_animation.rs` の WebP/RIFF 手動パースは checked_add / read_exact ベースで健全。壊れファイルの巨大 duration → timeline 行数 OOM は**未コミット修正で対策済み** (`TIMELINE_MAX_ROWS`、ui_music_timeline.rs:698-712)。f64→usize の `as` キャストは Rust の飽和変換なので UB/panic なし。
- **エラー経路の後始末**: `with_active_detached_viewer_context` は catch_unwind で suppression depth / bundle swap を panic 安全に復元 (app.rs:10930-10947)。`enter/exit_video_audio_mode` / `poll_video_audio_exit_pending` / `enter_music_vst_shell` / `activate_parked_live_media_window_snapshot` の abort 経路はいずれも状態復元 (snapshot 再 insert 等) を確認。`ensure_music_analysis` の spawn 失敗はエラー表示に落ちる。
- **リソース**: spectrum worker (`MusicSpectrumState`) は Drop + cancel_worker で teardown、timeline raster worker は tx drop → Disconnected で終了、`DetachedActivationWatcher` はプロセス単一スレッド、`NativeComApartment` (video/mod.rs:1196-1221) は RAII 対、`music_analysis_lru` / `VIDEO_RESUME_PREVIEW_SESSION_CACHE` は上限付き。
- **音声再生の音切れ/固着対策の未コミット修正** (SeekCompleted 再送 / BufferReady EOF 例外 / delete 時の idx shift 群) 自体に新たな防御漏れは見つからず。
