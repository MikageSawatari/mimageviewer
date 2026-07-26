## 1. サマリ

監査対象は clean な `HEAD 2cb405c7`。ファイル変更・テスト・ビルド・実機起動は行っていない。

- docs↔コード不一致: **16 件**
  - すべて「文書が古い／不完全」
  - コードが文書化済み仕様に違反しているものは、不一致ではなく下記バグ 2 件として分離
- リファクタ候補: **8 件**
  - P1: 1
  - P2: 2
  - P3: 5
- バグ: **2 件**
  - 一時停止中の grade 再提示漏れ
  - `present_retire.back()` による GPU frame の placement prime 失敗

重要な結論は、`present_retire` は「最後の表示フレーム」ではなく、GPU コピー完了までの一時的な廃棄待ちキューであること。さらに GPU frame は一度 present すると keyed mutex が writer key に戻るため、そのまま再 present できない。したがって、バックログ案の「`present_retire.back()` 経路を一般化」は構造的には不十分。

## 2. 不一致リスト

1. **`video-engine-redesign.md` が草案・目標構成と実装履歴を混在させている**

   文書は現在も「草案 v0」で、`TransportController`、command channel、専用 EngineActor worker thread を採用構成としている。[video-engine-redesign.md:1](../video-engine-redesign.md:1) [同:48](../video-engine-redesign.md:48) [同:191](../video-engine-redesign.md:191) [同:235](../video-engine-redesign.md:235)

   実装は `VideoPlayer` が `Arc<Mutex<EngineActor>>` を持ち、UI tick がイベントを drain し、操作も同期的に `apply_command` する。[video/mod.rs:123](../../src/video/mod.rs:123) [同:5015](../../src/video/mod.rs:5015) [同:5215](../../src/video/mod.rs:5215)

   **判定:** 文書が古い。コードの仕様違反ではない。  
   **修正案:** 「恒久仕様」ではなく「設計案＋採否履歴」と明記し、冒頭に現在の Mutex/UI-tick topology を記載する。未採用の `TransportController` と専用 actor thread は将来案へ隔離する。

2. **EngineState の型定義が一致しない**

   文書は `Buffering { resume_target: Option<f64> }`、`Seeking { target: f64 }`。[video-engine-redesign.md:137](../video-engine-redesign.md:137)

   実装は payload なしの `Buffering` と `Seeking { target_secs }`。コードコメントも文書との差を明記している。[engine/state.rs:23](../../src/video/engine/state.rs:23)

   **判定:** 文書が古い。実装側は resume target を actor context に置く意図的設計変更。  
   **修正案:** enum を実装と一致させ、resume intent は state payload ではなく actor context が所有すると記す。

3. **ReadinessLatch の式に `NoVideo` がない**

   文書は `FirstFrameReady ∧ (NoAudio ∨ BufferReady)`。[video-engine-redesign.md:161](../video-engine-redesign.md:161)

   実装は audio-only 対応のため `(NoVideo ∨ FirstFrameReady) ∧ (NoAudio ∨ BufferReady)`。映像・音声の両方がない場合は never-ready。[engine/state.rs:176](../../src/video/engine/state.rs:176) [同:188](../../src/video/engine/state.rs:188) [engine/actor.rs:514](../../src/video/engine/actor.rs:514)

   **判定:** 文書が古い。コードが正しい。  
   **修正案:** 式、遷移図、疑似コードへ `NoVideo` を追加し、audio-only と playable stream なしを分ける。

4. **BufferReady の wall 時刻を Playing anchor に使う記述が古い**

   文書は `audio_anchor=(pts, wall_now)` を保存し、その wall で `ClockAnchor::audio` を構築する。[video-engine-redesign.md:352](../video-engine-redesign.md:352) [同:380](../video-engine-redesign.md:380)

   実装は保存 wall を捨て、Playing 入場時の `Instant::now()` を使う。FirstFrameReady が数秒遅れた場合の開始時ジャンプを防ぐためである。[engine/actor.rs:673](../../src/video/engine/actor.rs:673)

   **判定:** 文書が古い。コードが正しい。  
   **修正案:** 「PTS は latch、wall は Playing 遷移時に採取」と書き換える。

5. **READY_THRESHOLD が 500ms のまま**

   [video-engine-redesign.md:332](../video-engine-redesign.md:332) は 500ms を規定する。

   実装は 100ms。[video/audio.rs:815](../../src/video/audio.rs:815) 恒久正本側は既に 100ms と説明している。[video-architecture.md:1007](../video-architecture.md:1007)

   **判定:** `video-engine-redesign.md` が古い。  
   **修正案:** 100ms へ更新し、500→100ms の変更理由は `video-architecture.md` へ一本化する。

6. **非 PLAYING 中の decoder pacing/drop 規定が逆**

   文書は Buffering/Seeking の `video_tx` Full を正常 drop、audio decode は state 不問とする。[video-engine-redesign.md:462](../video-engine-redesign.md:462)

   実装は非 PLAYING 中に Full なら frame を保持して retry する。GPU/CPU 両経路で同じ。[video/decoder.rs:4464](../../src/video/decoder.rs:4464) [同:5087](../../src/video/decoder.rs:5087) Paused/Eof では audio decode も park する。

   **判定:** 文書が古い。`video-architecture.md` 側の規定が正しい。[video-architecture.md:1155](../video-architecture.md:1155)  
   **修正案:** redesign 文書の「唯一の規定」を削除または現行規定へ置換する。

7. **動画 channel の型・容量・sample rate が文書間でも矛盾**

   `video-architecture.md` は一方で `video_tx=24`、後段で `8` と書く。[video-architecture.md:447](../video-architecture.md:447) [同:824](../video-architecture.md:824)

   `async-architecture.md` は packet queue 各256、`VideoWorkerMsg::{Packet,Flush,Eof}`、`video_tx=24`、再生音声48kHz固定としている。[async-architecture.md:39](../async-architecture.md:39)

   実装は packet 32/64、control 各8、frame `video=8` / `audio=32`、packet/control は別 channel。[video/decoder.rs:121](../../src/video/decoder.rs:121) [同:1699](../../src/video/decoder.rs:1699) [同:2569](../../src/video/decoder.rs:2569) [同:2632](../../src/video/decoder.rs:2632) 再生系 sample rate は出力デバイスに合わせ、48kHz は fallback。[video/mod.rs:4814](../../src/video/mod.rs:4814)

   **判定:** 文書が古い。  
   **修正案:** 両文書の channel 表を同時更新する。解析用 decoder の48kHz固定と、再生用 decoder の device-rate を区別する。

8. **モジュール規模と負債評価が現状を大幅に過小評価**

   [video-architecture.md:352](../video-architecture.md:352) と負債表 [同:2258](../video-architecture.md:2258) は v0.9.0 時点。

   現在は `video/mod.rs` 6,996行、`decoder.rs` 8,297行、`native_presenter/mod.rs` 9,054行、`overlay_draw.rs` 6,363行、`actor.rs` 2,302行、`native_window.rs` 1,345行。特に [video/mod.rs:1920](../../src/video/mod.rs:1920) の出力 loop と [native_presenter/mod.rs:3979](../../src/video/native_presenter/mod.rs:3979) の overlay impl が増大している。

   **判定:** 日付付き履歴としては正しいが、恒久正本の現行構造表としては古い。  
   **修正案:** 現行 snapshot を別節に置くか、行数表をスクリプト生成する。「native_window 問題なし」「engine tests 9件」等の評価も再監査する。

9. **倍速 UI 文書が撤去済み legacy egui path を要求**

   [playback-speed-design.md:43](../playback-speed-design.md:43) [同:299](../playback-speed-design.md:299) は legacy/native 両方への実装を要求し、存在しない `src/video/native_presenter.rs` を参照する。

   実装は native presenter 必須。[video/mod.rs:4895](../../src/video/mod.rs:4895) egui 側 `draw_video_hud` はエラー表示のみ。[ui_fullscreen.rs:22832](../../src/ui_fullscreen.rs:22832) 速度 UI は native overlay にある。[overlay_draw.rs:5786](../../src/video/native_presenter/overlay_draw.rs:5786)

   `video-architecture.md` の「eframe版と切替可能」も古い。[video-architecture.md:2366](../video-architecture.md:2366)

   **判定:** 文書が古い。  
   **修正案:** UI節を native-only の実装済み仕様へ変更し、パスを `native_presenter/mod.rs` / `overlay_draw.rs` に直す。

10. **VST3 文書に私有 `file:///` リンクが残る**

    [vst3-integration.md:8](../vst3-integration.md:8)

    **判定:** 文書側の問題。リポジトリ外の個人環境に依存する。  
    **修正案:** `docs/archive/vst3/` の履歴文書への相対リンクに置換するか削除する。

11. **VST bridge のプロセス分離単位が文書内で矛盾**

    冒頭は「1 plugin = 1 bridge、他 plugin に波及しない」とする。[vst3-integration.md:14](../vst3-integration.md:14) 後段は正しく「1 chain = 1 bridge」とする。[同:161](../vst3-integration.md:161)

    実装は最初の slot の bridge を後続 slot が共有する。[video/dsp/mod.rs:819](../../src/video/dsp/mod.rs:819) 音声 IPC も chain 単位で1回。[同:2322](../../src/video/dsp/mod.rs:2322)

    **判定:** 冒頭が古い。  
    **修正案:** 「chain 内 plugin crash は chain bridge 全体へ波及し得る」に統一する。

12. **存在しない `src/video/dsp/shm.rs` を記載**

    [vst3-integration.md:219](../vst3-integration.md:219)

    shared memory 型と Win32 mapping は `bridge.rs` 内。[video/dsp/bridge.rs:39](../../src/video/dsp/bridge.rs:39) [同:748](../../src/video/dsp/bridge.rs:748)

    **判定:** 文書が古い。  
    **修正案:** module map を実ファイルに合わせ、「新規」等の実装前ラベルも除去する。

13. **raw_pending back-pressure を未実装としている**

    [vst3-integration.md:393](../vst3-integration.md:393)

    実装は `raw_pending >= 5s` で audio intake を非破壊的に停止し、bounded queue を通じて demux へ逆圧を返す。[video/audio.rs:819](../../src/video/audio.rs:819) [同:919](../../src/video/audio.rs:919)

    **判定:** 文書が古い。  
    **修正案:** 実装済みにし、PLAYING/preroll とも現在は5秒 cap であることを記す。

14. **VST3 の既知負債が削除済みコードを前提にしている**

    `chain_process`、`scratch_a/b`、60フィールド級 `PluginSlot` を現存する負債としている。[vst3-integration.md:536](../vst3-integration.md:536) [同:551](../vst3-integration.md:551) [同:589](../vst3-integration.md:589)

    現在の `DspBridgeInner` / `PluginSlot` はより小さく、音声経路は `process_block` だけ。[video/dsp/mod.rs:188](../../src/video/dsp/mod.rs:188) [同:199](../../src/video/dsp/mod.rs:199) [同:2331](../../src/video/dsp/mod.rs:2331)

    **判定:** 文書が古い。  
    **修正案:** 削除済み項目を履歴へ移し、現在の責務混在に基づく負債表へ書き直す。

15. **normalize scan worker が async worker 一覧にない**

    [async-architecture.md:8](../async-architecture.md:8) の一覧に該当行がない。

    実装は `normalize-scan` thread、cancel atomic、progress、Provisional/Done channel を持つ。[app/native_video.rs:4259](../../src/app/native_video.rs:4259) 600秒 provisional も実装済み。[video/normalize_scanner.rs:46](../../src/video/normalize_scanner.rs:46)

    **判定:** 文書の追記漏れ。  
    **修正案:** worker 数、所有者、cancel、結果採用条件、provisional 継続中の lifecycle を追加する。

16. **VST plugin GUI worker の場所と本数が古い**

    `async-architecture.md` は Rust の `vst3-plugin-gui` thread が「表示中のみ1本」とする。[async-architecture.md:45](../async-architecture.md:45)

    現在の editor は bridge 内の per-slot STA thread。plugin loader ごとに lazy 生成される。[plugin_loader.cpp:726](../../crates/vst3-host/src/plugin_loader.cpp:726) [同:888](../../crates/vst3-host/src/plugin_loader.cpp:888) `vst3-integration.md` 後段は正しく説明している。[vst3-integration.md:466](../vst3-integration.md:466)

    **判定:** `async-architecture.md` が古い。  
    **修正案:** bridge 内 per-slot STA thread とし、表示した plugin 数まで増え得ることを記載する。

## 3. リファクタ候補リスト

### P1: visible/hidden/retire を統合した typed FramePresentationState

**なぜ問題か:** 現在は `present_retire`、`hidden_latest_frame`、`source.queue` がそれぞれ「使えそうな最近の frame」を部分的に表現している。しかし `present_retire` はコピー完了待ちであり、表示内容の永続所有者ではない。[video/mod.rs:2292](../../src/video/mod.rs:2292) この分離が grade 再提示漏れと GPU placement prime 失敗の共通原因。

構造案は native output context が `Empty / Hidden{frame} / Visible{frame,fence}` の単一 typed state を所有する形。`present_retire` は廃棄待ち専用のまま残す。GPU frame を Visible として再利用する場合、present 後に keyed mutex を reader key=1 へ再 arm する処理も必要。新 frame 提示時は旧 Visible を retire へ移す。

- **影響範囲:** `video/mod.rs`、`native_presenter/mod.rs`、`gpu_renderer/d3d11_device.rs`、テスト・文書。約250～450行。
- **回帰リスク:** 高い。GPU pool 枯渇、frame 混入、黒画面、source swap、hidden show、detached placement、音声モード復帰。
- **テストで担保できるか:** typed transition と fence/key state は unit test 化可能。D3D11 keyed mutex、複数 presenter device、実 detached は実機確認必須。
- **規模:** Medium
- **優先度:** P1

detached では App-global bool や geometry state を追加せず、既存 native output context 内に閉じるべき。凍結ルールに抵触する症状パッチは不要。

### P2: BufferReady anchor payload と MasterClock 書き込み境界の型による固定

**なぜ問題か:** `ReadinessLatch.audio_anchor` は `(pts, wall)` を保持するが、actor は wall を意図的に捨てる。型が古い設計を許し続け、将来 `wall` を再利用すると開始時ジャンプが再発する。また `MasterClock::set_anchor` はコメント上 actor 専用なのに `pub(crate)`。[engine/state.rs:159](../../src/video/engine/state.rs:159) [engine/clock.rs:163](../../src/video/engine/clock.rs:163)

- **影響範囲:** engine state/actor/clock、audio event 生成、文書。3～4ファイル、50～120行。
- **回帰リスク:** 低～中。open/seek 後の初期 A/V anchor。
- **テストで担保できるか:** BufferReady→遅延FirstFrameReadyで Playing 入場 wall が現在時刻になる actor test で担保可能。
- **規模:** Small
- **優先度:** P2

### P2: VST GUI/owner/HUD teardown の単一 policy 化

**なぜ問題か:** fullscreen close、fullscreen watcher、music VST exit、動画→音声 enter、video-audio VST exit に同種 teardown が複製され、HUD/owner/tracker/settings の処理に差がある。[app.rs:45601](../../src/app.rs:45601) [同:57911](../../src/app.rs:57911) [app/native_video.rs:7192](../../src/app/native_video.rs:7192) [同:7348](../../src/app/native_video.rs:7348) [同:7650](../../src/app/native_video.rs:7650)

片方だけ修正すると orphan editor、誤TOPMOST、stale owner が残り得る。

- **影響範囲:** `app.rs`、`app/native_video.rs`、必要なら小 helper module。150～250行。
- **回帰リスク:** 中。VST GUI visibility、z-order、owner復元、設定保存。
- **テストで担保できるか:** teardown policy の pure test/mocked bridge は可能。実 plugin editor と外部 foreground 遷移は実機確認が必要。
- **規模:** Medium
- **優先度:** P2

### P3: 動画→音声モードを単一 VideoAudioLifecycle へ集約

**なぜ問題か:** `video_audio_mode`、`video_audio_vst`、entry target、exit pending、source-swap one-shot、pending 内の `audio_mode_after_swap` が分散している。[app.rs:2021](../../src/app.rs:2021) [同:8884](../../src/app.rs:8884) [app/native_video.rs:163](../../src/app/native_video.rs:163)

実装内にも EOF swap と exit の競合、entry 中の placement pending、複数フィールド一括 clear に関する過去不具合コメントがある。[app/native_video.rs:7281](../../src/app/native_video.rs:7281) [同:7422](../../src/app/native_video.rs:7422) [同:7513](../../src/app/native_video.rs:7513) index 削除時にも各フィールドを個別 shift している。[app.rs:22525](../../src/app.rs:22525)

- **影響範囲:** App/context bundle、native_video、source-swap、detached round-trip、テスト・文書。4～7ファイル、600～1,200行。
- **回帰リスク:** 高い。連続再生、EOF、VST、hidden presenter、F11/F12、context park/restore。
- **テストで担保できるか:** reducer/state-transition table、index shift、bundle round-trip は自動化可能。実 presenter/VST/detached は実機確認が必要。
- **規模:** Large
- **優先度:** P3

detached 凍結中に新しい App bool を足す形では実施せず、既存 bundle state の置換としてレビューされた段階で行うべき。

### P3: EngineActor と AvClock の二重 clock ownership 解消

**なぜ問題か:** state の source of truth は EngineActor だが、実再生位置は AvClock が所有し、速度変更も両方へ別々に書いている。[video/mod.rs:123](../../src/video/mod.rs:123) [同:5194](../../src/video/mod.rs:5194) `AudioRendered` 等が production 未配線のため compat shim も残る。[video-architecture.md:1144](../video-architecture.md:1144)

新しい pause/seek/speed 経路ごとに両 clock の同期が必要で、片側更新漏れが A/V drift や state/position 不一致へ直結する。

- **影響範囲:** engine、clock facade、audio、decoder、VideoPlayer。5～8ファイル、800～1,500行。
- **回帰リスク:** 非常に高い。seek、pause、EOF replay、倍速、PDC、A/V同期。
- **テストで担保できるか:** actor/clock unit test は可能だが、実動画・実音声デバイスの perf/実機確認が必須。
- **規模:** Large
- **優先度:** P3

### P3: `video/mod.rs` から native output runtime を分離

**なぜ問題か:** `run_native_video_output` が約2,600行あり、command処理、source switch、placement rebuild、hidden、frame選択、retire、perf、HUDをローカル変数群で所有する。[video/mod.rs:1920](../../src/video/mod.rs:1920)

今回の holdover 欠落のように、相互作用する寿命状態を一望できず、別経路の bookkeeping を同期させる保守コストが高い。

- **影響範囲:** 3～6ファイル新設、2,500行以上の移動。VideoPlayer public facade は残す。
- **回帰リスク:** 高い。native presenter 全経路、source/placement switch、shutdown。
- **テストで担保できるか:** 最初は機械的移動＋state helper unit testが可能。Windows native 動作は実機確認必須。
- **規模:** Large
- **優先度:** P3

### P3: `native_presenter/mod.rs` の device/surface/overlay 分離

**なぜ問題か:** `NativeVideoPresenter` impl は [native_presenter/mod.rs:1540](../../src/video/native_presenter/mod.rs:1540)、`NativeEguiOverlay` impl は [同:3979](../../src/video/native_presenter/mod.rs:3979) から始まり、1ファイル9,054行。D3D device、swapchain、grade、HUD input、VST、各panel、IME状態が同居する。

z-orderやDPI修正が grade/present lifetime と同じファイルに入り、レビュー範囲が過大になる。

- **影響範囲:** core/device、surface、overlay state、panel単位など6～10ファイル。5,000行以上移動。
- **回帰リスク:** 高い。DComp、DPI、HUD入力、VST、IME、fullscreen/detached。
- **テストで担保できるか:** layout/snapshot/pure input test は可能。HWND/DComp/IMEは実機確認必須。
- **規模:** Large
- **優先度:** P3

### P3: DspBridge の chain/GUI/persistence/audio hot path 分離

**なぜ問題か:** `DspBridge` が chain mutation、bridge lifecycle、GUI/z-order、永続化、audio hot path を同じ moduleで扱う。[video/dsp/mod.rs:114](../../src/video/dsp/mod.rs:114) GUI command helperには inner Mutex を保持したまま closure を呼ぶ箇所もあり、lock規約の判断コストが高い。[同:2311](../../src/video/dsp/mod.rs:2311)

旧文書の `chain_process` 等は消えているが、責務混在自体は残っている。

- **影響範囲:** slot/chain、GUI registry、audio I/O、persistenceなど5～7 module。1,000行以上移動。
- **回帰リスク:** 高い。chain rebuild、bridge poisoning、plugin GUI、audio RT遅延。
- **テストで担保できるか:** chain snapshot/command生成は自動化可能。実VST3 pluginとbridge crashは実機検証が必要。
- **規模:** Large
- **優先度:** P3

## 4. バグリスト

### Bug 1: 一時停止中に grade/LUT 変更が画面へ反映されない

- **観測できる症状:** 動画を一時停止し、色調補正またはCreative LUTを変更しても、表示中の映像は古い grade のまま。再生再開または次の present まで更新されない。
- **壊れている不変条件:** 表示設定変更後、現在表示中の静止フレームも新設定で再提示されるべき。
- **原因経路:** App は grade command を送る。[app/native_video.rs:253](../../src/app/native_video.rs:253) output handler は `set_video_grade` を呼ぶだけ。[video/mod.rs:2496](../../src/video/mod.rs:2496) presenter は定数/LUT textureを更新するが `Present` しない。[native_presenter/mod.rs:3226](../../src/video/native_presenter/mod.rs:3226) Paused中は decoderがparkするため次frameが来ない。
- **同型経路:** 通常fullscreenとdetachedの可視 presenter は同じ問題を共有する。video-audio VSTで presenterを再表示したままpauseした場合も同型。hidden presenter中は表示がないため症状は出ず、show時に最新gradeで初回presentされる。audio-onlyは対象外。
- **確認状態:** コード経路上は確定。Windows実機での手動再現は本監査では未実施。

### Bug 2: `present_retire.back()` の GPU frame は placement prime に再利用できない

- **観測できる症状:** 一時停止/EOF等で `source.queue` が空の状態で placementを作り直すと、GPU decode経路では新 presenterのprimeが失敗する。コードは失敗後も新windowを表示するため、次frameが来るまで黒または未初期化表示になり得る。pausedのままなら継続する可能性がある。
- **壊れている不変条件:** placement swapは、新windowを現在frameでprimeしてから旧windowを破棄するべき。
- **原因経路:** fallbackは `present_retire.back()` を新 presenterへ渡す。[video/mod.rs:3079](../../src/video/mod.rs:3079) しかし通常present後、keyed mutex guardは `ReleaseSync(0)` してwriter keyへ戻す。[native_presenter/mod.rs:1000](../../src/video/native_presenter/mod.rs:1000) 次の `present` は `AcquireSync(1)` を要求する。[同:3613](../../src/video/native_presenter/mod.rs:3613) producerが最初にreaderへ渡すときだけkey=1へreleaseする。[d3d11_device.rs:1131](../../src/video/gpu_renderer/d3d11_device.rs:1131) held frameはpool slotを占有しているのでproducerによる再armも起きない。prime成否にかかわらずwindowは表示される。[video/mod.rs:3113](../../src/video/mod.rs:3113)
- **同型経路:** `hidden_latest_frame` と `source.queue.front()` は未present frameなのでkey=1のまま、同問題はない。CPU frameにも keyed mutex はなく再提示可能。問題は「既に一度presentされたGPU frame」のfallback。通常再生中はqueue側が使われやすいが、pause/EOF/paused detachedのplacement switchで踏みやすい。
- **確認状態:** keyed mutex状態遷移とコード経路は確定。実機での黒画面発生は未確認。

## 5. 総評

この領域の文書は、**動作の恒久正本としては `video-architecture.md` が最も信頼できる**。EngineStateのFrozen不変条件、epoch付きReadinessLatch、非PLAYING中のframe保持、native presenter必須化、normalizeの現在設計は概ね実装に追随している。ただし、channel表、module規模、抽象化評価には古い記述と文書内矛盾がある。

特に信頼できないのは次の2文書。

- **`video-engine-redesign.md`**: 草案、未採用の理想構成、実装途中の規定、完了履歴が追記式で混在している。EngineState schema、Readiness式、anchor wall、閾値、pacing、actor topologyのいずれも冒頭側を現行仕様として読めない。
- **`vst3-integration.md`**: 後半のchain bridge説明は比較的新しいが、冒頭、module表、実装状況、負債表が旧per-plugin bridge時代のまま。節によって正しさが異なる。

`playback-speed-design.md` はclock/audio会計の設計は現在も有用だが、UIとファイルパスは古い。`audio-normalize-scan-bench.md` は今回確認した範囲では実装とよく一致している。`async-architecture.md` は全体として有用だが、動画channel・VST GUI・normalize workerの行は現状を反映していない。

EngineActor本体については、epoch検査、readiness payload確認、anchor→stateのpublish順、非Playing時のFrozen化に明確な違反は見つからなかった。今回の優先課題は state machineそのものより、native output側の「最後に表示したframe」の所有不在である。