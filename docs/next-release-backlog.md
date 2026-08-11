# 次リリース検討バックログ

このファイルは、まだ着手していない作業候補だけを置く恒久バックログ。
完了した項目はコミット履歴・リリースノート・個別設計メモに任せ、このファイルからは削除する。

運用ルール:

- 着手前に `docs/README.md` から該当領域の設計ドキュメントを読む。
- 着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから削除する。
- 判断保留・見送りの理由は、次に再判断する人が困らない最小限だけ残す。
- 依存ライブラリ更新は `CLAUDE.md` のリリース手順チェックリスト Phase 2 と整合させる。

---

## 1. 優先候補

### 1.7 detached 中の発火面解決の残り (? / トースト / スタック) — BA 報告

- 残り (P3、発火面の window_id 粒度が必要):
  1. **? キーのショートカット一覧**: detached 中にグリッドから開くと FS 用一覧が
     出る。修正には発火元の面情報を consume サイトから通す必要がある。
  2. **トーストの面粒度**: 発火面のビューアを閉じた後に完了したバッチのトーストが
     不可視のまま消える / active detached + main fullscreen 同時表示時に Viewer 面が
     両ビューアに出る (2 値 enum の粒度の限界)。許容中。
  3. **複数ウィンドウ×スタックの理想形**: スタックをクリックするとメイン一覧が
     フラット読書ビューに切り替わる (フル機能と同じ挙動)。理想は「メインは集約のまま、
     窓側だけフラット文脈を持つ」だが、窓が自前のフラット items 文脈 (bundle 化された
     stack 状態) を持つ設計が必要 (複数ウィンドウの bundle 化と同系統)。現行でも
     Shift+↓↑ ジャンプは動作する。
- 優先度: P3。

### 1.9 parked 窓のリソース制御 (サムネ pipeline 停止 / VRAM 合算予算) — 角度⑥送り

- 出典: v2.3.0 角度⑥レビュー (Sol/Terra 一致、docs/archive/review-v2.3.0/sol-angle-reviews.md)。
  いずれも bounded な効率問題でデータ喪失は無し。park/再活性ライフサイクルの構造に
  踏み込むため、リリース直前パッチではなく detached リワーク後続で設計対応する。
  1. **P2: park してもサムネ pipeline が止まらない**: `pause_background_work_keep_current_frame`
     は fullscreen/AI 系のみ cancel し、bundle の `cancel_token` / `reload_queue` /
     `heavy_io_queue` に触れない。park 時点で積まれていたデコードが走り切り、結果
     `ColorImage` が誰も poll しない rx に溜まる (窓の再活性化 / close まで保持)。
     対応案 = park 時に queue を drain (worker pool は殺さない)。ただし Requested 状態の
     サムネが再活性化時に再要求される仕組みの確認が必要 (state が Requested のまま
     queue から消えると復帰後にロードされない恐れ)。
  2. **P2: サムネ VRAM 上限が文脈単位**: `update_keep_range_and_requests` の予算は
     mounted 文脈にしか効かず、parked bundle N 個がそれぞれ上限近くまで保持し得る
     (動画サムネは eviction 対象外なのでフォルダサイズ分)。対応 = 全 bundle 合算の
     予算会計 + cross-bundle eviction (リワークの資源予算ステージ)。
  3. P3: `display_px_shared` が App-global のため、detached 文脈のデコード解像度が
     メイングリッドの表示密度に引きずられる (適用ミスは無し、品質/CPU の無駄のみ)。
- 関連: v2.3.0 で対応済みの境界 = bundle Drop の解放 (角度①でクリーン確認)、
  文脈別 channel/cancel/世代 (P2-9)。

### 1.10 終了時の削除 worker 未調整 — 角度⑤送り (P2、実害小)

- 出典: v2.3.0 角度⑤ Sol P2。数百件削除の実行中に終了すると、実行中の
  IFileOperation チャンクは完走するが後続チャンクは開始されず、部分削除の最終報告と
  完了後クリーンアップ (resume purge 等) が走らない。一覧は次回起動の走査で実態に
  収束するため破壊は無し。対応案 = 終了時に cancel を立てて現行チャンクの完了を
  短時間待つ + 次回起動時の注意トースト。v2.2.0 出荷時からの既存挙動。

### 1.12 detached 静止画窓から音声/動画へのフォルダ内ナビ不可 (メディア昇格導線)

- 出典: v2.3.0 実機検証 §5 #33 (2026-07-10)。別窓 (静止画) で ↑↓ ナビ中、音声/動画の
  アイテムへは移動できない (静止画窓はメディア再生セッションを持てない設計境界)。
  音楽ビュー→画像の方向は移動できるため非対称。ユーザー許容済み (「一旦これでもよさそう」)。
- 対応案: detached リワークの後続ステージで「静止画窓がメディアに到達したらメディア
  セッションへ昇格 (または既存メディア窓へ委譲)」の導線を設計。凍結ルール下では
  症状パッチを入れない。

### 1.13 duration 不明/不正 MPEG-PS のシークバー不能

- 背景: `sample.mpg` で実機確認済み。duration が不明または不正な MPEG-PS は
  シークバーで移動できない。VLC は同じファイルをバイトシーク + PTS 再スキャン方式で
  シークできる。
- 対応案: decode EOF で実尺を学習して `VideoInfo` / HUD の duration へ反映するか、
  duration を信頼できない場合にバイト位置シークへフォールバックする。
- 裁定: v2.2.0 以前から同挙動で、ユーザー裁定により次版送り。優先度 P3。
  final-report 追補 5 の P3 参照。

### 1.17 UI レイアウト崩れ (文字はみ出し / 見切れ) の自動検出 QA → その後に英語対応を再検討

- 背景: 英語対応 (UI ローカライズ) を検討したが、AlternativeTo は「英語 UI 必須」
  (FAQ: DB 登録アプリは英語対応が必須) のため、日本語のみの現状では掲載できない。
  一方、多機能ゆえに英語 UI を実機で全画面目視 QA するのは負担が大きく、しかも日本語でも
  文字はみ出し / 見切れのレイアウトバグが繰り返し発生している。そこで **まず言語非依存の
  「レイアウト崩れ自動検出」QA ツールに投資**し、日本語バグに即効かつ将来の英語化の
  「目視 2 倍」問題を先に解消する。英語対応の可否はその後に再判断する。
  (2026-07-13 相談。ユーザー方針: QA 投資を先行 → その後に英語対応を再検討。今は別作業中のため別途着手)
- 現状 (確認済み):
  - egui は文字配置時に galley サイズが取れ、painter は clip rect を持つため
    「galley 幅 > 割り当て幅」で見切れを機械判定できる。
  - UI スナップショット基盤 `tests/ui_snapshot.rs` (egui_kittest) が既にある (回帰検出向け)。
  - 文字列は現状ハードコード (i18n 未 externalize)。見切れ計装は現行日本語のままでも動く
    ので、i18n とは分離して先行できる。
- 方針 (QA ツール先行 → 英語再検討):
  - QA ツール (言語非依存・日本語バグにも効く):
    1. **見切れ計装**: 描画時に galley 幅 vs clip/available 幅を比較し、超過した文字列 +
       ウィジェット位置をログ列挙する。全画面を人が見なくても崩れ候補の一覧が出る。
    2. **疑似ローカライズ (幅ストレス)**: UI 文字列を ~1.4x 長い擬似文字列に置換したビルドで
       幅不足のウィジェットを炙り出す (英語が長くなる前提の先行テスト。日本語のままでも
       将来はみ出しやすい箇所を潰せる)。
    3. 既存 egui_kittest スナップショットを併用 (変化検出。最終は差分画像を目視)。
    4. (任意) スクショ自動キャプチャ + ビジョン判定で「切れ / 重なり」を triage。
    - 限界: 得意なのは見切れ / はみ出し / 行あふれ。位置ズレ・不格好な折り返し・パネルの
      重なりは取りこぼしがあり、多少の目視が残る。
  - 英語対応 (QA ツール整備後に可否判断):
    - 前工程 = 文字列の externalize (i18n 化)。
    - AI 一括翻訳 + **用語集** (見開き=Two-page spread 等を先に固定) + 各文字列の文脈付与で
      用語ゆらぎ / 品詞ズレを抑える。
    - **変数整合チェック**: `%s` / `{}` / 改行 の個数・種類が元と英訳で一致するか機械照合。
    - **逆翻訳 QA**: 重要文字列 (削除 / 上書き / 警告 / エラーダイアログ) に限定して英→日へ
      訳し戻し、元日本語と比較する。英語力が無くても意味ズレを検知できる (日本語同士の比較)。
    - 実機確認は見切れ計装 + 疑似ロカライズで大半を機械化し、目視は残差のみ。
    - "English is machine-assisted; feedback welcome" と明示し、OSS の Issue / PR で継続改善。
- AlternativeTo 連携: 英語 UI が入れば掲載可。申請素材 (Tagline / Description の v2.3.0 版
  = 音楽再生 + VST3 入り / Website / Source / License Free+OSS(MIT) / 画像 = icon.png + ss_*.png
  含む ss_music.png / Alternative to = NeeView・MangaMeeya・Leeyes・ZipPla・Honeyview・
  ImageGlass・IrfanView・XnView MP・nomacs・FastStone) は準備済み。GitHub Topics / 英語 README
  サマリ / egui Showcase / awesome-egui PR は UI 言語非依存の枠なので実施済み。
- 規模 / リスク: QA 計装 = Medium (描画経路への hook + 幅バジェット定義)。英語化 = Large
  (externalize + 翻訳 + QA 工程)。QA ツールは日本語バグにも効くため単独で投資回収がある。
- 優先度: P2 candidate (QA ツール)。英語対応は QA ツール整備後に再判断 (現時点は保留)。

### 1.21 自動選定した親コンテナ代表サムネイルに編集プレビューを反映

- 出典: v2.5.0 リリース前実機確認 (2026-07-17)。ZIP 内画像を編集しても、親階層の
  ZIP セルで自動選定された代表サムネイルには編集結果が反映されない。
- v2.6.0 で手動ピン経路は対応済み。cascade 解決後の固定 leaf (直接 Image / ZipEntry /
  PdfPage、ネスト ZipDir、変換アーカイブ) は canonical page key を親要求へ渡し、編集
  preview を catalog より優先する。固定 leaf の個別色調補正も注釈前の下地へ適用し、
  保存 / 無効化通知を親セルへ伝播する。
- 残件は **自動代表選定だけ**。worker 内で leaf を選定した後に page key を組み立て、
  対応する編集 preview があれば raw decode / catalog より優先する。自動選定結果と親
  キャッシュの無効化条件も同時に設計する。
- 不変条件:
  - 現行の編集 preview 内容 (erase / local_adjust / conceal / crop / comic 注釈) だけを
    自動選定 leaf へ反映する。自動代表へどの色調補正を適用するかは別途決め、手動固定済みの
    個別色調補正規則を変えない。
  - UI スレッドへ ZIP/PDF 列挙や SQLite 単件照会を追加しない。
- 規模 / 優先度: Medium〜Large / P3。

### 1.30 native video Stage 5 の再投入条件 (revert 済み、原因未確定)

- 出典: 2026-08-01。Stage 5 (`726f838d`) を入れたところ UI が完全停止する P1 が再現し、
  `737c5234` で **revert 済み**。v2.9.1 は Stage 5 を含めずに出す。§1.28 のカーソル問題は
  既知の問題として残る。
- 症状: 動画をフルスクリーン再生中に VST エディタを開閉すると UI が完全に固まる。**動画・音声の
  再生は継続し、EOF で次の動画へも進む。UI だけが死ぬ。**終了も右クリックも不能。2 回再現
  (t=178s / t=18s)。
- 停止点 (cdb `-pv` で採取): main スレッドが `SendMessageW` 経由の wndproc の中で
  `eframe run_ui_and_paint` → `egui_wgpu paint_and_update_textures` →
  `wgpu_hal::dx12::Surface::acquire_texture` → `WaitForSingleObjectEx`。
  他スレッドは全て健全 (`vst-owner-dispatch` は recv で idle、pump は sleep、render / demux /
  decode は稼働)。
- **原因は未確定。候補 2 つとも潰れていない**:
  1. **VST owner handoff**: owner / z-order / visibility の変更が同期メッセージを撃つ。
     ただし hidden anchor は published fullscreen host の破棄時のみ通り、C++ は
     `old_owner == new_owner` で早期 return し、owner 付け替え自体は Stage 5 以前から存在する。
  2. **カーソル所有**: 新実装は pump から **8ms ごとに `WindowFromPoint`** を呼ぶ。同 API は
     hit-test のため対象の wndproc へ **`WM_NCHITTEST` を同期送信**し、相手は UI スレッドの窓や
     **別プロセスの VST エディタ**になり得る。pump からの無制限な cross-thread / cross-process
     同期呼び出しであり、**Stage 4 の「pump は時間上限を保証できない処理を持たない」原則に抵触する**。
     「窓を作らないから同期メッセージを撃たない」は成立しない。
- 次回に取るべき証拠 (今回取れていない):
  - `acquire_texture` の wait が**本当に無限か**。wgpu-core 27.0.3 は **1000ms timeout** を渡し、
    wgpu-hal は `WAIT_TIMEOUT` を `Ok(false)` にして先へ進む。スタック 1 枚では
    「デッドロック」と「1 秒待ちを繰り返す飢餓」を区別できない。**wait の `dwMilliseconds` が
    `0x3e8` か `0xffffffff` かを読み、1.2 秒以上あけて複数回 break する**。
  - 停止直前に main が実際に受けた `msg` (どの同期メッセージが再入描画を起こしたか)。
    スタックにある `in_window_resize_subclass_proc` は全メッセージを通る常設 subclass なので、
    **resize が起きた証拠にはならない**。
  - `old_owner != new_owner` / anchor handoff / presenter retirement が実際に発生したかのログ。
- 再投入の条件: **同一 release profile** で 4 分割 A/B を通すこと。
  (a) Stage 4 baseline / (b) cursor のみ / (c) owner のみ / (d) Stage 5 全体。
  今回の A/B は `dev-runtime` 対 `release` で、出荷判断には十分だが**原因帰属としては profile 差が
  残る**。(b) が VST 開閉 soak を通ることを出荷条件にする。
- 診断用の回避策 (`MIV_WGPU_FRAME_LATENCY=2`、`WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT=DontWait`)
  は**出荷修正にしない**。症状が消えても wndproc 内 GPU wait と Win32 同期依存は残る。
- VST 側の長期案: editor を transient な presenter HWND へ付け替えるのではなく、**editor lifetime
  全体で安定した専用 owner proxy** を使い、topmost / focus / visibility を別の ordered transaction
  として扱う。owner request の dedupe と一括適用も要る。
- 規模 / 優先度: Large / **P2**。カーソル (§1.28) の修正はこれが片付くまで入らない。

### 1.31 wndproc の内側で GPU 待ちのある描画をする構造

- 出典: §1.30 の解析 (2026-08-01、Codex Sol の独立レビュー)。**v2.9.1 の範囲外**。
- 何が問題か: winit は `WM_PAINT` 内で `RedrawRequested` を**同期 dispatch** し、eframe はそこで
  `run_ui_and_paint` を直接呼ぶ。さらに Windows の resize では flicker 防止のため意図的に同期 paint
  する。結果として **wndproc の内側で swapchain acquire と Present を行う**。Microsoft も
  `Present` が message-pump thread を待ち得ると明記している。
  main surface は `AutoVsync` / maximum frame latency **1** なので、前フレームが retire しない限り
  次の acquire が待たされる。
- 効果: 窓の owner / z-order / visibility を動かす操作、DWM 合成の切替 (Independent Flip / MPO)、
  device-loss 周辺のいずれかが絡むと、**同期メッセージ 1 通で UI が任意時間停止し得る**。
  Stage 4 で閉じた側 (presenter スレッドが窓を持ったままブロックする) に対し、こちらは
  「UI スレッドが自分の窓のメッセージ処理中に GPU で止まる」。**同じ上位の破綻の別の面**で、
  Stage 4 の pump 分離では届かない。
- Stage 4 以降の運用制約 (維持すること): pump thread では GPU API を呼ばない。held frame の
  再提示にも re-arm 用 `AcquireSync` を追加せず、reader key の維持と producer 側回復を使う
  ([video-architecture.md](video-architecture.md) の `FramePresentationState` 節)。
- 方向 (Codex 案):
  - wndproc は damage / resize / redraw 要求を記録して**即 return** する。
  - 実描画は外側の event-loop 境界へ post して coalesce する。
  - `Idle / Painting / Pending` の単一 typed render state を置き、**再入 acquire を禁止**する。
  - surface acquire は短時間・nonblocking にし、間に合わなければその frame を捨てる。
  - msg / paint depth / wait handle / timeout / Present 前後 / cloak / visibility を 1 つの trace に残す。
  - 同期 `SendMessage` 中に presentation availability を故意に止め、**wndproc が有限時間で返る**
    Windows 回帰テストを追加する。
  - 必要なら eframe / winit / wgpu を vendor patch し、upstream へ最小再現を出す。
- 併せて直す観測の穴: 今回 `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も書かれ
  なかった。**active native fullscreen 中は main HWND が隠れていても watchdog を suspend しない**
  ようにする (危険な操作の最中こそ黙る、という現状は逆)。
- 規模 / 優先度: Large / **P1 candidate (基盤)**。次版で Web 配信とコンテナ詰め替えを載せる前に
  方針を決めておく。

### 1.35 リネーム移行ジャーナルの永続化 2 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、Codex)。両方とも source inspection で確認済み。
  1. **対応済み (v2.10.0): UI スレッドでの同期 I/O (P2)**: [app.rs](../src/app.rs) の
     `persist_rename_migration_journal` / `ensure_rename_migration_journal_loaded` は
     `rename_key_migration::journal_save` / `journal_load` を同期で呼び、その中身は
     `std::fs::read` / `write` / `remove_file` / `rename`。呼び出し元は
     `poll_rename_migration_pending` などの UI スレッド経路である。小さな JSON なので通常は
     短いが、ウイルス対策・低速ディスク・プロファイル同期の影響下では UI が止まる。
     CLAUDE.md の「UI スレッドで同期 I/O を行わない」方針にも反する。
     対応 = App が in-flight + queue + boot-retry の完全 snapshot を組み立てる所有境界は維持し、
     書き出しだけを単一 latest-value worker へ移した。worker は revision 順に直列保存し、I/O 中の
     中間 snapshot は最新値へ coalesce する。起動時 load は回復 entry を新規保存で消さない lazy guard を
     優先して同期 1 回を維持し、通常 update の保存経路は enqueue のみ。終了時は最新 revision まで flush する。
  2. **部分失敗が再試行されない (P2、仕様判断あり)**: worker が `panicked` を返したときだけ
     ジョブを `rename_migration_boot_retry` へ戻す。`report.errors` に個別ストアの失敗が
     入った場合はトーストを出すものの、ジョブは完了扱いでジャーナルから消える。一時的な
     DB ロックや I/O エラーだと、編集結果やパス依存設定の一部が旧パスに残ったまま再起動でも
     回復しない。
     - **これは現状のコードコメントが明記している意図的な選択**である
       (「per-store エラーは通常経路と同じ best-effort = 再試行しない」、v2.3.0 角度⑤ 検収時の判断)。
       したがって着手前に「一時エラーと恒久エラーを区別できるか」「再試行の上限をどう置くか」を
       決めること。無条件の再試行は、決定的に失敗するストアで無限ループになる。
     - 移行処理自体は冪等に設計されているので、失敗ストアを保持して再試行する形にはできる。
- 規模 / 優先度: 1 = Small〜Medium / P2、2 = Medium / P2 (仕様判断を先に決める)。

### 1.36 ゲームパッドの文字入力ゲートは 3 面の union が必要 (2026-08-01 訂正)

- 前段の誤った前提: viewer 宛なら fullscreen / detached viewport、そうでなければ root viewport
  という排他選択で十分と考えた。しかし gilrs は App が直接 poll するグローバル入力で、
  OS の keyboard routing を通らない。正しい不変条件は **mIV 内のどこかで文字入力中なら止める**。
- 文字入力面は 3 つある。
  1. App root viewport (一覧・ダイアログ)。
  2. App fullscreen / detached viewport (静止画パネル)。
  3. native video / music presenter overlay。これは `egui::Context::default()` で独自 Context を持ち、
     App の `Context::data` からは viewport id を変えても見えない。実機ログでも detached 動画の
     ブックマーク名入力は overlay 側 ROOT (`FFFF`) として観測され、App detached viewport (`39E5`) と別だった。
- keyboard は presenter の `NativeOverlayInputRouting` が `wants_keyboard_input` / `text_input_active` を見て
  App 転送を止めるが、ゲームパッドは presenter を経由しないため以前からこの保護を素通りしていた。
- 対応: root と対象 viewer viewport の IME state を常に union し、さらに presenter が既に公開する
  `NativeOverlayInputRouting::wants_keyboard_input` を既存 output event bus の latest-value snapshot として
  App へ一方向 publish する。3 条件は `gamepad_text_input_active` だけが所有し、呼び出し側へ分散させない。
  presenter Context の統合、presenter→App 直接参照、新しい逆向き channel は行わない。
- 回帰テスト: root composing、viewer composing、presenter `wants_keyboard_input`、全て inactive の 4 状態を
  1 つの predicate test で固定し、snapshot publish が semantic event として App へ漏れないことも固定する。
- 規模 / 優先度: Small / P2。前段差分を本訂正で置換。

### 1.37 トレイ常駐再生まわりの所有境界 3 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  v2.9.1 で「トレイ格納中も再生を続ける」経路を新設したことで、いずれも**この版から初めて
  実際に通る**組み合わせになった。
  1. **対応済み (v2.10.0) / P2: WM_PAINT ブリッジに背圧が無い**。[tray.rs](../src/tray.rs) の常駐ループは 50ms ごとに
     `PostMessageW(WM_PAINT)` を投げるが、**未処理の wake を数えていない**。posted WM_PAINT は
     無効領域由来のものと違って合体されないため、隠れている間の `App::update` が 50ms を
     超えると投函が消費を上回る。戻り値は `let _ =` で捨てているので、スレッドの posted message
     queue が溢れても検出できない。
     対応 = `resident_media_wake_pending` を既存共有 atomic 群へ追加し、tray thread が false→true を
     claim できた 1 件だけ投函、`App::update` 入口で ack する構造にした。可視化・wake 不要化・
     `PostMessageW` 失敗では pending を reset し、失敗ログは状態変化時だけ出す。
  2. **P3: 再生継続中に video processor cache を落とす**。
     [tray_integration.rs](../src/tray_integration.rs) の `release_gpu_resources` は
     detached 早期 return より前で `release_idle_pools()` を呼び、その中の
     `processor_cache.take()` は**無条件**である (`hw_frames_pool.clear()` と
     `shared_output_pool.retain(in_use)` は refcount 上安全)。
     ⚠ **呼び出し順自体は v0.9.1 (`287eea9f`, 2026-05-19) からで、v2.9.1 で動かしたわけではない**。
     新しいのは「格納中も再生が続く」ことのほうで、その結果この経路が再生中に走るようになった。
     トレイ格納の瞬間に video processor の作り直しが 1 回入るはずで、実害は小さい。
  3. **P3: 外部 hide 検出が tray 所有権に追随していない**。[app.rs](../src/app.rs) の
     `IsWindowVisible` 追従分岐は `viewer_session_is_detached_or_switching()` だけで heartbeat
     suspension を決め、`sync_media_presenter_visibility_for_tray()` /
     `sync_retained_viewport_visibility_for_tray()` を呼ばない (これらは `hide_to_tray` /
     復帰経路にしか無い)。同フレーム後半の `sync_tray_resident_media_wake()` が**値が変化した
     ときだけ**自己修復するので実害は出にくいが、hide の所有者が 2 つある状態である。
- 規模 / 優先度: 1 = Small / P2、2 と 3 = Small / P3。

### 1.38 ネイティブ名前ダイアログが `App::update` の内側でモーダルを回す

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 何が問題か: [rename_item.rs](../src/ui_dialogs/rename_item.rs) の
  `show_rename_dialog_window` は `loop` の中で `native_name_dialog::prompt_name`
  (`DialogBoxIndirectParamW` = 独自のモーダルメッセージループ) を呼び、バリデーション失敗の
  たびに再表示する。`App::update` は `WM_PAINT` → `RedrawRequested` の同期 dispatch から
  呼ばれるので、**wndproc の内側で無制限長のモーダルが回る**。
  §1.31「UI スレッドが自分のメッセージ処理中に任意時間止まる」構造の一種なので、
  **§1.31 の設計を決めるときに一緒に見る**のが筋が良い。
  なお rename 自体は `rename_item_async` で worker 化されており、ここは**ダイアログの
  モーダル span だけ**の問題である。
- **副次的な退行は対応済み (2026-08-01)**: 既存 `rename_pending` を左下の進捗 overlay に接続し、
  シェル rename 実行中の「変更中...」表示を復帰した。モーダル構造は変更していない。
- 規模 / 優先度: Medium / P2 (§1.31 に合流)。残るのはモーダル span の構造だけ。

### 1.39 見開き / 先読みの残り 1 件 (`9590b661` の続き)

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  1. **対応済み (2026-08-01)**: paged 側も `current_loading` で sibling pending を cancel した後、
     return 前に `ensure_fs_page_load(current_idx)` で current producer を再確認する形へ統一した。
     current が `NeedsLoad` の状態遷移テストで、cancel だけで終わらず `LoadPending` になることを固定した。
  2. **P3: `fs_page_load_state` が PDF ページで毎フレーム String を作る**。
     `has_retained_pdf_final_ai_for_current_params` → `retained_pdf_page_key_for` →
     `metadata_cache_key(idx)` が String を生成し、HashMap を引く。
     ⚠ **レビュー報告より実コストは小さい**: `effective_params` / `final_ai_key_for_pixels` は
     retained cache に**該当エントリがあるときだけ**走る (無ければ `?` で抜ける)。
     非 PDF は `items.get` + `matches!` で即 return なので無コスト。
     とはいえ PDF フルスクリーンでは `update_prefetch_window` の current、prefetch ターゲット
     全件、連結読み keep 範囲ループ全件、見開きパートナーから毎フレーム走るため、
     key を 1 回作って使い回す形にできるかは見ておく。
- 規模 / 優先度: 残る 2 は Small / P3。

### 1.42 右クリックメニューの表示が遅い (シェル拡張を UI スレッドで同期ロード)

- 出典: 2026-08-02 の v2.10.0 実機確認中の利用者報告。「右クリックメニューを開くのが
  かなり遅い」ため、連続リネームのテストが現実的に行えなかった。
- 壊れている前提: [native_context_menu.rs](../src/native_context_menu.rs) の
  `query_shell_context_menu` は `IContextMenu::QueryContextMenu` を**メニュー構築時に
  同期実行**する ([native_context_menu.rs:288](../src/native_context_menu.rs) と
  [872](../src/native_context_menu.rs))。同 module に worker は無く、**UI スレッドで走る**。
  `QueryContextMenu` はサードパーティのシェル拡張 (ウイルス対策 / クラウドストレージ /
  書庫ソフト等) を列挙してロードするため、環境によっては数百 ms〜秒単位かかる。
- 「Windows のせい」は半分正しい。遅いのは拡張側だが、**それを UI スレッドで待っているのは
  mIV 側**であり、その間アプリ全体が固まる。CLAUDE.md「UI スレッドでの同期 I/O は即
  worker 化する」の対象。
- 未計測: 実際に何 ms かかっているか、どの拡張が支配的かは未測定。**着手前に計測すること**
  (`perf::event` を `query_shell_context_menu` の前後に入れる)。環境差が大きいので、
  遅い環境のログが取れると判断が早い。
- 対応案 (計測してから選ぶ):
  1. **シェル部分を遅延構築**する。mIV 自身のメニューを即座に出し、シェル項目は
     サブメニューを開いた時点で初めて `QueryContextMenu` する。エクスプローラー互換の
     見た目からは離れるが、体感は最も改善する。
  2. mIV のメニューを先に表示し、シェル項目だけ**非同期に差し込む**。項目が後から
     増えるので、開いた直後にクリックすると位置がずれる問題を設計で潰す必要がある。
  3. 直近で使った拡張セットを**キャッシュ**する。COM オブジェクトの寿命と、
     拡張の追加 / 削除の検知が要る。
- ⚠ **症状パッチにしないこと**: タイムアウトで打ち切って一部の拡張を落とす、は
  「どの拡張が出るか環境ごとに変わる」という新しい非決定性を持ち込むので採らない。
- 完了条件 / 回帰テスト:
  - 右クリックからメニュー表示までの UI スレッド占有時間を計測し、改善を数値で示す。
  - シェル項目の実行 (`InvokeCommand`) が従来どおり動く。
  - 拡張が 1 つも無い環境でも従来と同じメニューが出る。
- 規模 / 優先度: Medium / P2。実害は「操作のたびに待たされる」で、データ喪失は無い。

### 1.45 縮小に EWA (Jinc) 経路を検討する

- 出所: 2026-08-04 の利用者経由の意見 (MPC-BE / MPC Video Renderer の Jinc2m 評価) と、
  それを受けた再測定。正本は
  [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) §4.3.4。
- 測定結果: EWA Jinc3 は網点の残留が**角度にほぼ依存しない** (0/15/30/45 度で 11.0〜11.7)。
  現行の分離 Lanczos3 blur1.00 は同条件で 13.5→48.8 と角度で 3.6 倍悪化する。しかも
  0 度の細線コントラストは同等 (123.4 vs 123.5) を保つ。**分離 blur1.00 に対しては全角度で優位。**
- ただし: ①不規則トーン (実スキャン相当) では分離 blur1.30 に負ける (36.38 vs 30.23) ので
  「なめらかさ」設定は残る ②斜めディテールは分離 blur1.00 比 8〜10% 落ちる (円形通過域が
  対角の角を落とす選択的バイアス) ③**コストが約 6.5 倍** (0.41 縮小で約 196 タップ vs 約 30)。
- 着手時に見るべき点:
  - リサイズ中の実測。結果はキャッシュされるので静止時は一度きりだが、リサイズ中は
    毎フレーム走る。ここが採否の分かれ目
  - 現行の分離経路を置き換えるのか、拡大側にだけ入れるのか。MPC-VR 自身は
    「拡大は Jinc2m、縮小は Lanczos」を推奨しており、拡大側の方が素性は良い
  - 経路を 2 本持つ判断は §3.3 の「縮小経路は 1 本化する」と衝突する。増やすなら理由を残す
- 規模 / 優先度: Medium / P3。現行が壊れているのではなく、より良い候補があるという位置づけ。

### 1.46 静止画の拡大品質を上げる (bilinear をやめる) — 実装済み

**2026-08-05 実装。** 比較結果の正本は
[upscale-algorithm-selection.md](upscale-algorithm-selection.md)。動画は表示構造が異なるため
§1.47 の別案件とする。

- 既定の `PostFilter::None` は、静止画を拡大すると既存の GPU Lanczos3 経路で
  表示サイズへリサンプルする。整数倍 / 非整数倍を区別しない
- 物理等倍 (`OriginalOneToOne`) はリサンプルせず、ドットバイドットを維持する
- `PostFilter::Nearest` は、拡大では従来どおり NEAREST で直接描画する。縮小は
  モアレを戻さないため、従来どおり Lanczos3 を通す
- `PostFilter::UpscaleSharp`（シャープ拡大）は、拡大だけ公式 NVIDIA Image Scaling SDK 由来の
  WGSL shader を使う。縮小は `DownscaleLanczos`、物理等倍は元 texture のまま
- `PostFilter::UpscaleAnime`（アニメ塗り拡大）は、拡大だけ x2 VL の多段 WGSL shader を使う。
  可視 source 領域の長辺上限は 2048px / 4096px / 制限なし（既定 4096px）で、境界値は
  処理し、超過時は同じ領域・目標寸法の標準拡大へ戻す。縮小・物理等倍は他の選択式拡大と同じ
- CRT / レトロ / セピア等の効果付きポストフィルタは、拡大では従来どおり LINEAR で描画する。
  標準の高品質拡大と効果の同時指定には対応しない
- 拡大ではカーネルを広げず、支持半径は Lanczos3 本来の 3.0 とする。縮小用の
  「なめらかさ」設定は拡大へ適用しない
- 初回実装の `source × scale` 目標は大判写真で画面外まで生成して上限へ当たったため修正。
  拡大は表示 trim と画面の可視範囲の積だけを表示解像度へ生成し、パン先読みは持たない。
  可視 source UV は拡大 entry だけの cache identity とし、縮小の目標計算・シェーダ・cache は
  変更しない。`gpu/lanczos_regenerate` には生成元 source pixel 領域も記録する
- 拡大出力は一辺 8192px、総画素 4096×4096 相当を上限とし、超過時は表示を止めず
  従来の LINEAR 描画へフォールバックする。フォールバックと生成時間は perf event へ記録する
- 拡大出力キャッシュは 4096×4096 相当の 2 枚分を上限に LRU 解放する。既存の縮小キャッシュ
  上限は変更しない

NIS の「シャープ」と Anime4K の「アニメ塗り」は選択式として実装済み。どちらも標準
Lanczos3 と同じ可視領域・出力上限・cache ownership を使い、branch key で結果を分離する。
これで §1.46 の静止画拡大候補は完了とし、動画は §1.47 の別構造として扱う。

### 1.47 動画の拡大縮小を mIV のシェーダで行う — 設計確定 / 未実装

**正本は [video-upscale-shader-plan.md](video-upscale-shader-plan.md)。** 本項は要約と着手判断のみ。

§1.46 の動画版。**GPU 性能ではなく動画表示の構造が障害**だったので別案件として扱ってきたが、
2026-08-07 に構造・測定方式・UI まで設計を確定した。

- 現状: native presenter は **swap chain を動画解像度で作り**、`CopySubresourceRegion` で 1:1
  コピーし、**`IDCompositionVisual::SetTransform2` で拡大縮小している**。つまり拡大しているのは
  mIV ではなく DWM / DComp で、mIV のシェーダは通っていない (色補正が identity でないときだけ
  grade シェーダが走るが、それも動画解像度で動く)
- 採る構造: **swap chain を「映像の表示矩形の物理ピクセルサイズ」にし、シェーダでソース解像度
  から表示解像度へ直接解決する**。`compute_video_visual_transform` はサーフェスサイズを引数に
  取る作りなので**無改造で正しい**。リサイズ中は差し替えず既存サーフェスを DComp に伸ばさせ、
  静止後に 1 回だけ差し替える (= 全画面再生では差し替えが一度も起きない)
  - 当初案の「swap chain をウィンドウ解像度にする」は黒帯まで描くので上位互換の表示矩形サイズを
    採る。「動画解像度 × 2 の整数倍」も却下 (mIV の Anime4K / NIS は任意倍率へ直接解決できるため、
    整数倍に縛ると 1〜2 倍の中間倍率で情報を捨てる)
- **Phase A (標準 Lanczos3 / シャープ NIS / ニアレスト + 縮小)** と
  **Phase B (Anime4K)** に分ける。Phase A は 4K 出力で +0.4〜0.9ms と実質タダで全解像度に使え、
  **4K 動画をウィンドウで見たときの縮小モアレも同時に直る**。重いのは Anime4K だけ
- Phase B は変種 (S/M/L/VL/UL) を扱うため、**現行の VL 専用ハードコードを表駆動へ一般化**する
  必要がある ([gpu_anime4k.rs](../src/gpu_anime4k.rs) と
  [convert_anime4k_glsl_to_wgsl.py](../scripts/convert_anime4k_glsl_to_wgsl.py))。一般化すれば
  静止画側にも同じ選択肢が生える
- モデル選択は **利用者が Anime4K を選んだ瞬間にモデル × ソースサイズを実測して表を作り、
  以後は表引きで決める**。GPU 性能は 4090 とノート iGPU で 10 倍以上違い、解像度やフレーム
  レートからの固定しきい値では当てられないため。測定結果は GPU + ドライバ版をキーに永続化し、
  回復手段は再測定ボタン。**再生中はモデルを変えない** (自動昇降格はハンチングし、切替のたびに
  リソース再構築が走るため)
- 最重要の不変条件: **切り替えの瞬間にシェーダのコンパイルもテクスチャ確保も一切しない**。
  現行 grade pipeline はレンダースレッドで同期 `D3DCompile` しており、同じ作りで Anime4K UL
  (25 本 + 中間 24 枚) をやると設定を触った瞬間に数百 ms〜秒単位で固まる
- UI は動画左パネル →「画像補正」→「フィルタ」タブに置く (Creative LUT の隣)。制限で実行され
  ない場合は静止画と同じ `processing_size_outside_note` の書式で選択肢直下に出す
- VSR は [video-architecture.md](video-architecture.md) で**スコープ外と決定済み**なので、
  先例として使えるものが無い
- 性能の数字は**すべて静止画実測からの外挿**であり、これを根拠に既定値やしきい値を決めない。
  測定機構自体が「1080p VL が現実的か」を利用者ごとに答えるためのものである
- 規模 / 優先度: **Phase A = Large / P3 (約 2 週間、測定を待たずに着手可)**、
  **Phase B = Large / P3 (約 2 週間、Phase A の後)**。いずれも単独リリースで実機検証を厚く取る
  規模。detached リワーク ([detached-rework-plan.md](detached-rework-plan.md)) と presenter を
  共有するので、着手時期はリワークの進捗と調整する

### 1.58 ページ送り (キー押しっぱなし) が引っかかる — v2.13.0 で入れて削除した。やり直し

- 出典: 利用者メール (pattier、2026-08-07) + `--perf-log` 実測。キーリピート間隔 34ms に対し、
  1 ページの実体化に UI スレッドで upload 21ms + final_composite_build 21ms かかり、
  構造的に追いつかない。連結読みが速いのは実体化済みを見せるだけだから。
- **v2.13.0 で対策 1 (通過表示 = 通り過ぎるページをカタログサムネイルで描く) を入れ、
  出荷前の実機確認で 5 回直して 5 回とも失敗したため削除した (2026-08-11)。**
  最後の状態は v2.12.0 より悪かった (カラー化した本でページごとにカラー / 白黒が入れ替わり、
  ページ間隔が中央値 463ms / 最大 2763ms)。
- **仕様と設計上の学びは [display-pipeline.md](display-pipeline.md) §2.5 に集約済み。
  やり直すときは必ずそこから読む。** 要件 R1〜R4、加工ごとの扱いと根拠、2 軸の分離、
  入力信号の安定性 (§2.5.2.1)、トレース不変条件 I1〜I5 が入っている。
- **失敗の共通形 (5 回とも同じ)**: **ページごとに変わる条件で描画元を選んでいた**。
  隣り合うページで絵が変わり、破綻して見える。
  1. 完成画像がキャッシュに在るか → ページごとに違う
  2. サムネイルが忠実か (カラー化が要るか) → ページごとに違う
  3. 通過レンディションが作れるか (`MonochromeOnly` の判定材料が届いているか) → ページごとに違う
  §1.58 の初期実装も「そのページのサムネイルがあるか」で選んでいた。サムネイルと完成画像の
  差が解像度だけのうちは見えず、カラー化で差が色になった瞬間に露出しただけ。
- **やり直しの設計制約**:
  - **通過表示に入るかどうかは、バーストの開始時に一度だけ決める。** 途中でページごとに
    揺らさない。または、2 つの絵の差が知覚できないことを保証する。
  - 判定の入力は**フレーム間で安定した信号**にする (§2.5.2.1)。edge の有無は 30Hz で振動する。
  - 「何を描くか」と「UI スレッドで重い処理をするか」は別軸として扱う。
- **着手順序 (ここが今回の最大の反省)**: **先にテスト基盤を作る。** 今回は自分で再現できない
  まま 5 回直し、毎回実機確認に依存して 1 往復 1 仮説しか試せなかった。§2.3 の
  「アプリ内蔵テストスクリプト」ができてから着手する。判定は
  `python scripts/analyze_perf.py <jsonl> page-turn --check` (実装済み) を使う。
- 対策 2〜4 は未着手のまま残る: 再デコードの抑止 / UI スレッドの 42ms 削減 (表示解像度へ
  縮めてからアップロード、`edit_result` upload の分散) / 最終合成の先読み。
  **対策 1 より先に 3 を試す価値がある** (42ms が減れば通過表示自体が要らなくなる可能性)。
- 規模 / 優先度: Medium / P2。

### 1.59 360 度ビューに等距離魚眼投影を追加する (提案・採否判断が要る)

- 出典: 利用者メール (pattier、2026-08-06)。「現在の透視投影に加えて等距離魚眼投影があると、
  視野を引いたときに 360 度カメラの絵に近い見え方ができる」。
- 現状: [panorama_wgpu.rs](../src/panorama_wgpu.rs) のシェーダは透視投影固定
  (`tan_half = tan(fov_y * 0.5)` でカメラ方向を作る)。この式は原理的に 180 度へ近づくと発散するため、
  引いた画角そのものが表現できない。
- 見込み: シェーダ側は数行 (半径 → 角度を線形に対応させ、`sin/cos` で方向ベクトルを作る)。
  作業の本体は投影モードの uniform 追加、設定への永続化、切り替え UI / キー割り当て、
  画角上限の再定義 (魚眼なら 180 度超も扱える)、ドキュメント。
- **どの魚眼かを先に確かめること** (2026-08-07 調査): 「魚眼」には複数の写像がある。
  半径 `r` と入射角 `θ` の対応が、透視 `r=f·tan θ` / 立体射影 `r=2f·tan(θ/2)` /
  等距離 `r=f·θ` / 等立体角 `r=2f·sin(θ/2)` と異なるだけで、**シェーダ上はどれも 1 行の差**。
  利用者の言う「引いたときに 360 度カメラっぽい絵」は、周辺の伸びが最も穏やかな
  **立体射影 (いわゆるリトルプラネット)** の可能性が高い。krpano も little planet は
  stereographic を使う。等距離はレンズの物理仕様としての標準表記で、見た目は中央が
  やや膨らむ。**どちらか一方ではなく、方式を選ぶ形にするのが素直** (実装コストがほぼ同じため)。
- 判断が要る点: 投影方式を増やすと画角スライダの意味と上限が方式ごとに変わる。既定は透視のまま、
  切り替えを提供するのか、パノラマ設定に持たせるのかを先に決める。
- 規模 / 優先度: Medium / P3 (提案段階。採否未定)。

### 1.62 お気に入り編集を開いている間、進捗が動いていなくても 100ms ごとに repaint する

- 出典: v2.13.0 の idle health 測定中に判明 (2026-08-11)。退行ではなく既存仕様。
- 観測: `favorites_editor.rs:752` が `ctx.request_repaint_after(100ms)` を**無条件で**呼ぶため、
  ダイアログを開いている間ずっと 11〜12 fps で `App::update` が回る。perf log で
  `prev_frame_causes=['src\\ui_dialogs\\favorites_editor.rs:752']` が連続して確認できる。
- 実害: 利用者は**起動後の索引作成が終わるのを確認するためにこのダイアログを開いたままにする**
  運用をしており、索引作成は 5 分ほどかかる。その間ずっと起きている。ノート PC やタブレットの
  電池には無視できない。v2.13.0 でタッチ対応を入れた以上、タブレット運用は増える。
- 現行コードのコメントは「active が空でも notify-rs が動き出した瞬間に拾えるよう常に呼ぶ」と
  理由を書いており、**意図的**である。直すなら意図を保ったまま頻度を落とす:
  - active が空の間は 100ms ではなく 500ms〜1s へ落とす (動き出しの検出遅れは許容範囲)
  - または watcher 側から `ctx.request_repaint()` を呼び、ポーリング自体をやめる (構造的)
- 同型の確認: 他にも「進捗表示のために無条件 `request_repaint_after`」を持つダイアログが
  無いか探すこと。あれば同じ方針で揃える。
- 規模 / 優先度: Small / P2。

## 2. 一覧 / サムネイル / フォルダ走査

### 2.3 アプリ内蔵のテストスクリプト実行 (実機確認の往復をなくす) — 利用者提案

- 出典: 利用者提案 2026-08-11。§1.58 の実機確認で 5 往復して直せなかったことを受けて。
- **やりたいこと**: 起動引数でテスト用スクリプトを渡すと、mIV 内のスレッドがそれを実行し、
  操作をアプリ内から再現する。別プロセスからのキー注入をやめる。Rhai は
  スタック機能 ([filename-stack-plan.md](filename-stack-plan.md)) で既に使っており、
  サンドボックスの前例がある。
- **なぜ外部からのキー注入では駄目か (実測)**: `scripts/page-turn-smoke.ps1` を書いて試したが、
  `SendInput` が成功を返し (`inserted=1 lastError=0`)、対象ウィンドウがフォアグラウンド
  (`fgPid == ourPid`) でも、**アプリが就寝から起きない** (perf log で t=3.3 秒以降フレーム 0)。
  原因未特定。フォアグラウンド / デスクトップ / セッションに依存する層を挟むこと自体が弱い。
- **設計上の要点 (ここを外すと意味がない)**: スクリプト API は
  **アクションを呼ぶだけでは不十分**。§1.58 の不具合は**入力層**にあり
  (edge の有無 vs 押下状態、§2.5.2.1)、`run_action("FsPageNext")` では再現できない。
  `hold_key("Right", 5000)` が、**アプリが実際に読んでいる押下状態の供給元**
  (現在は `GetAsyncKeyState`) と同じ場所に入る必要がある。テスト用の入力源を差し替え可能に
  する形。**どの層に入れるかを最初に決めること。** 間違えると「テストは通るが実機で壊れる」。
- 最低限の API: `hold_key(key, ms)` / `release_key(key)` / `run_action(name)` /
  `sleep(ms)` / `wait_until(cond, timeout)` (sleep だけでは不安定)。
- **判定はアプリ内に持たない。** 従来どおり perf log を
  `python scripts/analyze_perf.py <jsonl> page-turn --check` に流す。判定ロジックを
  仕様 ([display-pipeline.md](display-pipeline.md) §2.5) と 1 か所に紐づけておくため。
- 有効化は開発用に限定する (feature gate、または `--data-dir` 指定時のみ許可など)。
  任意コードをアプリ内で実行するので、通常利用から到達できないこと。
- 既存資産: `scripts/page-turn-smoke.ps1` の外枠 (隔離 data-dir で起動 → 判定) は再利用できる。
  中身のキー注入をスクリプト指定へ差し替える。
- **これができてから §1.58 をやり直す。** タッチ対応 (§4.7 で出荷済み) の回帰も同じ手法で
  回せるようになる。
- 規模 / 優先度: Medium / **P1** (これが無いと同種の不具合をまた実機往復で追うことになる)。

### 2.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

### 2.2 サムネイルバッジのレーン配置共通化

- 背景 / 現象:
  1. ZIP / PDF / RAR 等の形式バッジだけを従来比 70% に縮小したため、従来サイズを
     維持している緑色のフォルダ名バッジが相対的に大きく見える。フォルダ名は可変長で
     セル幅の 80% まで使うため、面積差も目立つ。
  2. ブックマーク一覧では、通常セルの左上バッジ列 (編集状態 / pin / タグ) と
     ブックマーク時刻を別々に同じ左上座標から描画している。後から描く時刻がタグの
     先頭を覆い、見た目では隠れた領域がタグのクリック判定として残る可能性もある。
  3. 動画の `UP` バッジも左上を独自使用しており、タグや編集状態バッジと同じ衝突を
     起こし得る。右上のチェック / スタック枚数、左下のコンテナ形式 / フォルダ名 /
     評価、右下の絞り込み件数も、現在は個別の座標計算または相互の高さ推定で配置している。
- 原則:
  - 共通化の単位は「全バッジを同じ見た目で描く巨大関数」ではなく、セル四隅の使用領域を
    割り当てる純粋なレイアウトとする。形式、フォルダ名、時刻、評価など意味の異なる
    バッジは、共通の計測 / padding / 配置部品を使いつつ個別スタイルを維持する。
  - 中央の再生アイコンや代替アイコンなど、隅のバッジではないコンテンツ overlay は
    この仕組みに含めない。
- 実装案:
  1. セルの `inner` と表示対象を受け、`TopLeft` / `TopRight` / `BottomLeft` /
     `BottomRight` の各レーンに実測済み `BadgePlacement` (矩形、文字 / style、優先度) を返す
     `ThumbnailOverlayLayout` 相当の純粋なレイアウト層を設ける。
  2. **v2.9.1 完了**: 第1段階は左上を移行し、ブックマーク時刻 → 編集状態 / pin → タグの順に幅を予約する。
     `UP` も同じレーンへ入れ、残り幅が不足する場合だけタグを実測で省略または非表示にする。
     描画と hover / click 判定は同じ `BadgePlacement` の矩形を使い、再計算をなくす。
  3. **v2.9.1 完了**: 第2段階は左下を移行し、フォルダ名 / ZIP / PDF / 変換アーカイブ、評価、ファイル名
     プレートの予約幅・縦積みを同じレイアウト結果から決める。フォルダ名バッジは形式
     バッジと機械的に同じ 70% へ揃えず、長い名前の可読性を残したコンパクトな専用
     スタイルをスナップショット比較で決める。
  4. **未着手**: 第3段階で右上 / 右下も同じ所有境界へ移し、チェックとスタック枚数の排他条件、
     絞り込み件数の配置を明示する。各段階を独立コミット可能にし、全経路を一度に
     書き換えない。
- v2.9.1 実装記録 (2026-07-31):
  - `src/thumb_overlay_layout.rs` に painter 非依存の純粋レイアウトを新設。セルごとに 1 回だけ
    構築した左上 / 左下の配置結果を、通常描画、タグ hit-test、`UP`、ブックマーク時刻へ渡す。
  - 左下は形式 / フォルダ名とファイル名プレートの実測矩形を下段へ置き、その行の上へ評価を
    4px gap で積む。旧 `lower_left_container_badge_height` の高さ推定は撤去した。
  - フォルダ名は旧フォントの 85%、8.5pt 下限・13.5pt 上限、横 padding 0.30em・縦
    padding 0.12em の専用 style とした。長い CJK 名の可読性を残しつつ、70% 形式バッジとの
    面積差を抑える値を snapshot で確認した。
  - 純関数 unit test 5 件と、フォルダ / ZIP / PDF / RAR、ブックマーク時刻 + タグの snapshot を
    追加・更新して目視確認済み。第3段階の右上 / 右下は未着手のまま残す。
  - 左下の item 種別 → コンテナバッジ / ファイル名プレートの対応は `bottom_left_content`
    として painter 非依存に切り出した。旧 `cell_has_lower_left_container_badge` の
    unit test 4 件 (フォルダは Loaded まではファイル名、アーカイブは常に形式バッジ) を
    同じ規則の検証として移設してある。ここは「フォルダ名と ★ が重なる」報告の退行ガード。
- **第3段階と一緒に片付ける残件 (v2.9.1 では未対応)**: 左上レーンは cursor を右へ送るだけで、
  **セル幅でクランプしていない**。時刻 + `UP` + 編集バッジ 6 個が同時に立つ狭いセルでは
  レーンが右へはみ出す (タグだけは実測で省略される)。第2段階以前からある挙動だが、
  時刻を同じレーンへ入れたぶん到達しやすくなった。直すには「溢れたときどのバッジを落とすか」の
  方針が要るので、右上 / 右下を同じ所有境界へ移す第3段階と合わせて決める。
- 完了条件 / 回帰テスト:
  - ブックマーク時刻と `#` から始まるタグが重ならず、表示矩形とクリック矩形が一致する。
  - `UP` + 編集状態 / pin + タグ、狭いセル、長い CJK タグでもバッジ同士が重ならない。
  - 長いフォルダ名 + 評価、ZIP / PDF / RAR + ファイル名 + 評価で重なりやセル外描画がない。
  - レイアウト純関数で矩形の非交差・優先順位・狭幅時の省略を unit test し、代表的な
    フォルダ / 形式バッジとブックマーク時刻 + タグを `docs/ui-snapshot-policy.md` に従う
    snapshot で確認する。
- 規模 / 優先度: Medium / P2 candidate。現在の重なりだけを座標ずらしで直す症状パッチは
  入れず、まず左上レーンの単一所有化から着手する。

### 2.3 サブ展開のオプション (走査階層 / 走査時フィルタ / 相対パス表示) — 利用者要望

- 出典: 2026-08-02〜04 の利用者メール (pattier)。「追加を検討します」と回答済み。
- 使い方: UNC 共有に `ID001` / `ID002` … のフォルダが多数あり、各 ID 配下は
  `ID001/thumb/img001/*.bmp` のように深い階層へ大量の画像が入っている。目当ては
  `ID200`〜`ID300` の浅い階層にある少数のファイルだけで、全階層走査はネットワーク越しの
  コストに見合わない。
- **起点フォルダの絞り込みは既にできる**: 通常フォルダ表示でフォルダをチェックしてから
  サブ展開すると、チェックしたフォルダ以下だけを展開する。要望の 1 点目はこれで足りるため
  実装不要 (回答済み)。
- 対応状況:
  1. [x] **走査階層の上限指定**: `サブ展開` を押すと確認ダイアログを開き、
     「起点のみ / 1 / 2 / 3 / 5 / 10 階層 / 無制限」を選んで実行できるようにした。選択値は
     スカラー設定として保存して次回の初期値にし、ボタンのツールチップにも反映する。サブ展開中は
     同じボタンでダイアログを出さず即時解除する。既定の「無制限」は従来どおり実効上限 40。
  2. [x] **走査時フィルタ**: 実行前モーダルで既存の絞り込みと同じ種類 / サイズ / 更新日の
     語彙を選び、条件外の項目を snapshot へ入れない。サイズ / 更新日はファイルだけへ適用する。
     サイズは従来の 4 区分に加え、数値 + KB / MB / GB で最小 / 最大を片側または両側指定できる。
     条件は保存して次回モーダルへ復元し、展開後のフォルダバーにも表示する。条件なしは従来互換。
     ディレクトリ走査の I/O 自体は減らないが、後段のソート・一覧構築・メタデータ準備を減らす。
  3. [x] **相対パス表示**: 場所 facet、選択情報、ツールチップ、詳細表示の「場所」列を、
     サブ展開ボタンを押した実フォルダからの相対パスで表示する。複数のチェック済みフォルダを
     起点にしても基準は押下位置のままにし、直下は `(直下)`、root 外はフルパスへ戻す。
     詳細表示の場所列は任意表示・ソート可能で既定非表示。詳細表示では表を隠す行テキスト
     ツールチップを止め、列と下部情報バーへ集約する (プレビュー画像 viewport は維持)。
     `DetailsColumnId::Place` / `DetailsSortKey::Place` は `PageCount` と同じ保存用 clone の stash で
     メイン列と専用下部バーの列順・幅・ソートキーから退避し、旧版 quarantine を防ぐ。
- 規模 / 優先度: 1 / 2 / 3 とも対応済み。

### 2.4 CSV / TSV からの一括タグ / レーティング付与 — 保留

- 出典: 同じメール往復。こちらから代替案として提案し利用者も歓迎したが、**利用者の実際の
  使い方 (参照は一時的で、タグを付けても後から参照しないことが多い) とは噛み合わない**ため、
  §2.3 を優先すると回答した。
- 位置づけ: 外部ツールで抽出した結果を mIV へ持ち込む導線としては筋が良い。単独では需要が
  薄いので、タグ運用側の要望が別に出たときに合わせて再判断する。
- 実装するときの前提 (利用者へ明言済み): **明示的な「取り込み」操作のときだけ動く**こと。
  パスの一覧をビューとして開く形 (実体のない仮想フォルダ) は採らない。理由は、他人由来の
  リストで任意パスを参照してしまうこと、UNC パスなら開いた瞬間に外部へ認証情報が飛び得る
  こと、実フォルダ前提の処理 (サムネイルの識別キー、移動 / 削除時の扱い、各種設定の保存先)
  への影響範囲が大きいこと。
- 優先度: P3 / 保留。

## 3. 補正 / AI

### 3.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 3.2 補正パラメータ変更後に AI アップスケールキャッシュが優先される疑い (再現待ち)

- 背景: 5ch レス 792 の追跡項目。「画像補正パラメータを変更しても AI アップスケールキャッシュが
  優先され、ページを行き来すると変更が効いていないように見える」という報告。
- 現状 (2026-06-18): 通常環境と v1.7.0 ポータブル版の追加テストで再現せず。現在の設計では、
  色調補正や AI 設定の変更は final AI / final composite cache のキー差分または明示クリアで反映される。
  一方、最終段スマートシャープなど post-filter 系は final AI cache を再利用して final composite だけを
  作り直す。さらに AI アップスケール出力にはスマートシャープを適用しない固定仕様なので、
  操作内容によっては「変わらない」ように見える場合がある。
- 方針: 具体的な再現手順が出るまではコード修正しない。再報告時は、変更したパラメータが色調補正 /
  AI ON/OFF / デノイズ / post-filter / スマートシャープのどれかを最初に切り分ける。
- 優先度: P3 monitor / 再現待ち。

### 3.3 手動ビットマップマスクの共通強化 (バケツ / 1px 拡張・縮小) — 利用者要望

- 背景: 同じ「手描きビットマップマスク」を持つサブシステムが 3 面あり、ツールの品揃えが揃っていない。
  補正レイヤーだけが 1px 拡張・縮小を持ち、色を見て塗るツール (境界筆) も補正レイヤーだけにある。

  | 面 | マスク表現 | ビットマップツール | 1px 拡張/縮小 | 色サンプル元 |
  | --- | --- | --- | --- | --- |
  | 消しゴム [ui_erase.rs](../src/ui_erase.rs) | `Vec<bool>` (`erase_mask`) | 筆 / 囲み / 多角形 | なし | `erase_base_cache` (raw を黒フラット化) |
  | 補正レイヤー [ui_adjustment_panel.rs](../src/ui_adjustment_panel.rs) | `Vec<f32>` (`RasterVectorMask.alpha`、実質 0/1) | 筆 / 境界筆 / 隙間補完 / 囲み / 多角形 | あり | `current_local_adjust_source_pixels` |
  | 隠蔽加工 [ui_conceal.rs](../src/ui_conceal.rs) | `Vec<bool>` (`conceal_mask`) | 筆 / 囲み / 多角形 | なし | `current_conceal_source_pixels` |

- 対象範囲 (2026-08-11 利用者判断): **3 面すべてに両機能を入れる**。共通ロジックを
  `mask_db.rs` に置けば追加コストは配線とパネル UI だけなので、ここで 3 面の機能差を解消する。

#### (a) バケツツール (許容値以下の同色を塗りつぶす)

- 既存資産: 補正レイヤーの `paint_local_adjust_edge_brush_stamp`
  ([ui_adjustment_panel.rs](../src/ui_adjustment_panel.rs) の `local_adjust_edge_brush_pixel_allowed` /
  `paint_local_adjust_alpha_edge_brush_line` 一帯) が、既に
  「クリック点 RGB を seed → 最大チャンネル差 ≤ 許容値 → 4 近傍 BFS」を実装している。
  バケツとの差は **ブラシ半径 (`radius_sq`) で打ち切っている点だけ**。半径無制限版の stamp を足す形になる。
  色差許容スライダー (`local_adjust_edge_brush_tolerance`、既定 48.0) も UI に既存。
- 消しゴム / 隠蔽側は色を見るツールが 1 つも無いので新規。`Vec<bool>` 版の flood fill を
  `mask_db.rs` に置いて 2 面で共有する (前例: [margin_fit.rs](../src/margin_fit.rs) の 8 連結 flood fill)。
- 塗り範囲モード (2026-08-11 利用者判断): **連結 / 非連結の両対応**。
  「隣接のみ」チェックボックスで切替える。非連結は BFS 不要の O(n) 1 パスで済み、
  マンガの白地一括マスクに効く。
- 境界の扱い: 補正レイヤーは境界判定 (`local_adjust_boundary_pixel_at`) を併用できるが、
  消しゴム / 隠蔽側にその概念が無い。まずは 3 面とも「色差許容のみ」で揃える。
- 書き込み先はビットマップのみ。ベクタ図形 (直線 / 矩形 / 楕円) は塗り範囲の障壁にしないし、
  バケツで書き換えもしない (既存の筆 / 囲みと同じ規約)。
- **性能が唯一の実装リスク**: `fs_cache` は長辺 8192 に clamp されるので最悪 8192x8192 = 67MP。
  素朴な per-pixel BFS では UI スレッドで数百 ms〜秒級のヒッチになり得る
  ([ui-responsiveness.md](ui-responsiveness.md) §4 抵触)。段階的に:
  1. スキャンライン span fill で実装して 8192px 最悪ケースを実測する。
  2. 閾値を超えるなら worker 化する。テンプレは同ファイルの領域分割
     (`local_adjust_segmentation_pending` + `poll_local_adjust_segmentation`) がそのまま使える。
  - 実測値は着手時にこの項へ書き戻す。

#### (b) 1px 拡張・縮小を消しゴム / 隠蔽にも追加

- 既存実体: `local_adjust_morph_alpha_1px` (3x3 の min/max)、ボタンは
  `draw_local_manual_mask_tool_panel` の「1px拡張」/「1px縮小」、適用は
  `apply_local_adjust_bitmap_mask_op`。回帰テストは
  `bitmap_mask_expand_and_shrink_use_3x3_neighbors`。
- 消しゴム / 隠蔽へは `Vec<bool>` 版を `mask_db.rs` に置き、
  `push_undo_snapshot()` → morph → `mark_erase_mask_texture_dirty(None)` →
  `clear_erase_preview(fs_idx)` の順で適用する (隠蔽は対応する snapshot / dirty / キャッシュ破棄)。
- **ベクタ図形には効かない** (補正レイヤーの既存規約と同じ)。期待とズレやすいので
  ボタン近くの説明文で明示する。バケツと組み合わせると、JPEG リンギングで残る
  輪郭のハロー画素を 1px 拡張で潰せる、という使い方が主用途になる。

#### 触る箇所

```
src/app.rs                   EraseTool / LocalAdjustMaskTool / ConcealTool に Bucket 追加、
                             許容値・隣接フラグのフィールド (App 保持で足りる。設定 DB 変更は不要)
src/mask_db.rs               bool 版 flood fill (連結 / 非連結) + 1px morph
src/ui_erase.rs              ツール dispatch / パネル / スライダー / 1px ボタン
src/ui_conceal.rs            同上
src/ui_adjustment_panel.rs   半径無制限 stamp、Bucket アーム、draw_local_tool_settings
src/keymap.rs                EraseToolBucket / LaToolBucket / ConcealToolBucket
                             (ini_name / context / trigger / default_chords / ALL_ACTIONS の 5 点セット)
docs/keymap.ini.default
htdocs/mimageviewer/manual/{erase,local-adjustment,conceal}.html
docs/preset-and-adjustment.md, docs/local-adjustment-layer-v1.1.0-plan.md,
docs/conceal-feature-plan.md
```

- 空きキー: `K` が 3 モードとも未使用 (Photoshop の `G` は補正レイヤーで隙間補完が使用中)。
- スライダー置き場: 消しゴムは `erase_brush_radius` / `erase_line_width` と同じ位置に
  ツール条件付きで追加 (パネル幅 `PANEL_W=190`、2 ボタン/行に収まる)。
  補正レイヤーは `draw_local_tool_settings` の match アーム追加のみ。
- 許容値は `erase_brush_radius` / `local_adjust_edge_brush_tolerance` と同様に
  App フィールド + Default 値でセッション内保持とし、設定 DB のスキーマは変えない。
- 優先度: P2 (利用者要望、既存構造に素直に乗る)。着手前に
  [preset-and-adjustment.md](preset-and-adjustment.md) と
  [ui-responsiveness.md](ui-responsiveness.md) §4 を読む。

## 4. 入力カスタマイズ / マウス / ゲームパッド

### 4.2 音声モードから戻る Z が 1 回だけ効かなかった (未再現・報告者の追加情報待ち)

- 出典: 利用者メール (pattier)、v2.10.0 で確認。
- 経緯: v2.9.1 で毎回再現していた「音声モード中の Z で動画表示へ戻れない」は、`713d36bf`
  (キーエッジに送信元 HWND / viewport を持たせた) で解消し、v2.10.0 では戻れる。ただし
  利用者の試用中に **1 回だけ**戻れない状態が起き、その後は再現していない。
- 確定している事実 (利用者による切り分け): ツールチップに `(Z)` が出る = 割り当ては生きて
  いる。「動画表示に戻る」ボタンの表示条件は `video_audio_mode == Some(fs_idx)` なので、
  ボタンが見えて効いた時点で **キー側 exit の判定条件も成立していた**。状態ではなく入力
  経路の問題として扱う。
- 候補 (再現時に Z 以外のキーも効かないかで割れる):
  - (A) フルスクリーン viewport がフォーカスを持っていない → `handle_fs_key_input` 冒頭の
    `has_focus` 判定で全ショートカットが早期 return する。Z 以外も効かない。
  - (B) 同一フレームで Z が別の consumer に取られた → Z は画像フルスクリーンの `FsZoomMode`
    の既定でもあり、音楽ビューでの取り合いは過去にも起きて明示的に除外した経緯がある
    (`ui_fullscreen.rs` の fs_zoom ラッチ除外コメント)。Z だけ効かない。
- 依頼済み: 再現したら (a) mIV ウィンドウの余白を 1 回クリックして復帰するか (b) そのとき
  Z 以外のキー ([Space] など) も効かなかったか (c) 直前に別ウィンドウ / タスクバーを
  クリックしていないか。
- 優先度: P3 / 再現待ち。**報告が来るまで着手しない** (ユーザー判断 2026-08-02)。

### 4.1 Shift / Alt + ホイールのカスタマイズ再設計

- 背景: v1.7.0 のリングショートカット / マウスボタン実装中に、Shift / Alt + ホイールのペアバインドを
  追加候補にしたが、実機確認で動画まわりの退行リスクが高いと判断した。
- 方針:
  - v1.7.0 では公開 UI / 入力経路から外し、通常ホイール、Ctrl+ホイール、中ボタンドラッグの既存挙動を維持する。
  - 将来再開する場合は、グリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計する。
  - native video overlay の consumed wheel、modifier 転送、動画タイルの Ctrl+ホイール、編集パネル / スクロールパネルとの
    優先順位を先に決める。
- 実装メモ: `ring_shortcuts.shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用フィールドとして残すが、
  現行 UI / 入力経路からは参照しない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

## 5. リリース前確認 / 依存更新

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |
| PDFium | v2.12.0 サイクルで `chromium/7988` へ更新済み (§5.0) | 更新後は通常 / パスワード付き PDF の表示とページ数を実機確認 |
| FFmpeg | v2.12.0 サイクルで `n7.1.5-12-g1fdbca85aa` へ更新済み (§5.0) | DLL・VERSION・LGPL 対応ソース・製品ページの FFmpeg 節を同じ commit に揃えて動画 / 音声を実機確認 |

### 5.2 Rust クレート

- 互換範囲の一括更新とは分け、次のメジャー更新 / rc 脱出は個別判断する:
  - `ort`
  - `pdfium-render`
  - `ffmpeg-the-third`
  - `image`
  - `zip`
  - `sevenz-rust2`
  - `delharc`
  - `unrar`
  - `turbojpeg`
- 更新後に確認するもの:
  - `cargo test`
  - 検索 bench 回帰
  - perf smoke
  - `dumpbin /dependents` で不要な VC runtime DLL が復活していないこと

### 5.3 リリース / テストスクリプトの並行実行耐性とクリーン環境再現性

- 出典: v2.8.0 リリース前確認 (2026-07-26)。別セッションで
  `scripts/build-release.ps1` を実行中に通常並列の `scripts/test-full.ps1` を実行すると、
  `mimageviewer` のライブラリテストが assertion / panic を出さず `0xffffffff` で終了した。
  リリース処理の完了後、同じコミットを競合プロセスなしで再実行すると正式ゲートは
  `[test-full] PASS` で完走したため、製品テストの並列不具合ではなくリリースツール間の
  干渉と判定。v2.8.0 の出荷は止めず、次版でツールを堅牢化する。
- **P2: `build-release.ps1` の停止対象が広すぎる (v2.9.1 対応済み)**:
  - 原因はプロセス名の前方一致 `mimageviewer*`。Cargo のテストハーネスは
    `target\debug\deps\mimageviewer-<hash>.exe` で、**リポジトリ配下にあるため path 判定も
    通過**し、別セッションの `cargo test` ごと `Stop-Process -Force` していた。
  - 停止対象を `mimageviewer` / `mimageviewer-core` / `mimageviewer-vst3-host` /
    `mimageviewer-susie32` の完全名 allowlist に変更した。`build-portable.ps1` も同じ
    欠陥を持っていたので揃えた (`build-dev.ps1` は元から完全名指定で影響なし)。
  - path を読めないプロセス (昇格 / 保護) は従来どおり停止するが、allowlist を通った
    ものだけになったのでテストハーネスは対象外。
  - リポジトリ単位の test / release 相互排他ロックは**入れていない**。名前の取り違えが
    原因だったので、まず allowlist だけで様子を見る。再発したらロックを検討する。
- **P2: クリーン環境の正式テストゲートが release core に依存する**:
  - `scripts/test-full.ps1` の workspace test には launcher が含まれるが、launcher の
    build script は既存の `target\release\mimageviewer-core.exe` を要求する。このため
    release core がないクリーン checkout ではテストゲートを開始できず、既存成果物が
    ある開発環境でだけ成功する。
  - 対応案 = launcher のテストを正式ゲート内で別段に分けて必要な core を明示的に用意
    するか、launcher のテストビルドを埋め込み用 release core から分離する。
  - 完了条件 = `target\release\mimageviewer-core.exe` が存在しないクリーン環境から
    `scripts/test-full.ps1` が完走し、launcher のテストも省略されないこと。
- 規模 / 優先度: Small〜Medium / P2。いずれも製品 runtime の品質問題ではなく、
  同一 worktree で複数セッションを使うリリース運用とクリーン再現性の改善。

### 5.4 idle health の video-pin evidence 窓が狭い

- 出典: v2.9.1 リリース前確認 (2026-08-01)。`-TargetKey` 方式に変えた直後の実測で FAIL。
  原因は**ゲートの窓**で、製品側でもセットアップ手順の誤りでもない。
- 何が起きたか: 対象キーのサムネイル処理は **t≈177-182 に 519 件**あり、そのままアプリが
  就寝して測定区間へ入った。ところが evidence 窓は「Enter プロンプト表示〜測定終了」
  (t≈199.7-219.8) なので、**20 秒前に完了していた keep 範囲入りが窓の外**になり
  `matched=0` で FAIL した。`-NoLaunch` で 3 シナリオを連続実行すると必ずこうなる。
- ずれている前提: ゲートは「evidence 窓の**中で** keep 範囲へ入ったこと」を要求するが、
  シナリオが必要とするのは「測定中にタイルが keep 範囲に**ある**こと」。先に入って
  居残っているタイルも等しく正しいセットアップ。
- 直す方向 (どちらか):
  1. 窓をセッション全体へ広げ、**最後の `nav.load_folder_begin` より後**に対象キーの
     work があることを条件にする (= 今表示している場所のタイルだと言える)。
  2. 窓は現状のまま、「セッション内に対象キーの work はあるが evidence 窓の外」を
     FAIL ではなく warning にし、手順書で「連続実行時は開き直す」を明示する。
- 1 のほうが構造的 (操作者の手順に依存しない)。着手時は
  [idle-health-check.md](idle-health-check.md) の §3 も揃えること。
- 規模 / 優先度: Small / P2。**毎リリース必須のチェックが手順どおりでも落ちる**状態なので、
  次版で片付ける。
- **v2.10.0 (2026-08-02) は 3 シナリオとも PASS**。waiver 不要。今回窓に当たらなかったのは、
  **Enter を押す直前に対象フォルダを開き直した**ため。証拠は
  `matched=170 (enqueue=23, idle_upgrade_enqueue=2, idle_upgrade_ineligible=2, ready=143)` で、
  アイドル高画質化パスが対象タイルを実際に評価したうえで測定区間は完全 sleep
  (perf event 0 件 / perf log 増加 0 バイト / CPU 1.2%)。
  つまり**現状でも手順を「準備 → 即 Enter」に揃えれば通る**。ただしそれは操作者の手際に
  依存する状態であり、上記の対応方針 1 (窓をセッション全体へ広げる) は引き続き有効。
  なお `-TargetKey` が checklist から抜けていた件は `31b45449` で別途修正済み。
- v2.9.1 の waiver 根拠 (2026-08-01 更新)。`static-foreground` / `static-background` の PASS に加え、
  この版の変更が `video-pin-background` が見ている経路 (アイドル高画質化 / 動画ピンのタイル保持)
  に触れていないこと。対象の変更は次のとおり:
  - バッジレイアウト、rename 移行、ネイティブ名前ダイアログ
  - **トレイ常駐中の再生継続** — hidden 中の `App::update` を 50ms で起こす経路を新設した。
    アイドル高画質化とは別の wake 源だが、**静止時の消費は未実測**。次版で
    `tray-resident` シナリオを足すこと (今回は前 2 シナリオが常駐なしの静止を見ている)。
  - **スマートフォルダ セッション** — parked grid が keep 範囲分のサムネイルを保持する。
  - **見開きの表示可否判定 (`FsPageLoadState`)** — 再入ループを止めた側なので、アイドル時の
    work はむしろ減る (`9590b661`)。
  - **入力所有権 (raw key permit / IME の viewport 分離)** と **native window health** —
    後者は native video window が生きている間だけ 1 秒に 1 回 pump へ ping を送る。
    無再生時は送らないが、**動画を開いたまま放置したときのアイドル影響は未実測**。

### 5.7 v2.13.0 リリース前確認の記録 (2026-08-11)

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも最新で更新なし。
  DLL の ProductVersion `n7.1.5-12-g1fdbca85aa-20260803` が `vendor/ffmpeg/VERSION` と一致。
  LGPL 対応ソースの差し替え不要。VST3 bridge は C++ 最終変更 2026-08-01 / exe ビルド
  2026-08-06 なので再ビルド不要 (`-SkipVst3Bridge`)。Susie ワーカーは再ビルドした。
  - ⚠ `setup-pdfium.sh check` は「新しいバージョンあり」と誤検知する。`vendor/pdfium/VERSION`
    が無い環境では現行版を「未インストール」と読むため。**DLL の FileVersion で判定する**
    (`153.0.7988.0` = `chromium/7988`)。FFmpeg の VERSION 腐りと同じ型。
- **CI**: master success (run 31402289417)。ubuntu の `cargo check` を含む。
  今サイクルの 114 commit が初めて CI を通った。タッチ対応は `#[cfg(windows)]` の塊なので
  ここが最大の未検証点だったが、漏れは無かった。
- **署名 / 依存 DLL**: 配布 4 種すべて `Certum Trusted Network CA` + RFC3161 タイムスタンプ。
  `dumpbin /dependents` で launcher / core / portable / susie32 とも
  `VCRUNTIME140.dll` / `MSVCP140.dll` なし。
- **idle health**: `static-foreground` / `static-background` とも PASS。
  測定窓は perf event 0 件・perf log 増加 0 バイト (完全 sleep)、CPU one-core ratio
  0.0219 / 0.0115。
  - **1 回目の `static-foreground` は FAIL (35 fps) だったが退行ではない**。起動から約 2 分の
    時点で、索引作成が進行中かつお気に入り編集ダイアログを開いたまま測っていた。
    同ダイアログは進捗を流すため 100ms ごとに repaint を要求する既存仕様
    ([favorites_editor.rs](../src/ui_dialogs/favorites_editor.rs) の live 更新)。
    ダイアログを閉じて測り直すと 0 フレーム。**「過去 3 回は 0 フレーム」という比較だけで
    退行と断定してはいけない**。アプリが busy でなかったことを先に確認する。
  - `video-pin-background` は **§5.4 の既知のゲート欠陥で不成立**。証拠窓が測定区間そのもの
    (t=352.4-367.4) になり、対象キーの thumbnail work は t=337.5 で 15 秒前に終わっていた
    (`matched=0`)。加えて操作側の誤りもあり、ピン留めフォルダの**中**を表示していたため
    マッチしたのは個々の動画ファイル (`from_cache=false` = アイドル高画質化の候補外) で、
    フォルダタイル (`dir::<path>`) ではなかった。**waiver とする**根拠は、
    v2.13.0 が `idle_upgrade` を含む行を 1 行も変更していないこと
    (`git log -S "idle_upgrade" v2.12.0..HEAD -- src/` が 0 件)、および測定区間自体は
    完全 sleep (CPU 0.0052) だったこと。セッションログは
    `target/idle-health/v2.13.0-idle-health-session.jsonl` に退避済み。
- **perf smoke**: frame 4476、16ms 超のギャップ 132 件 (97.05% が 16ms 未満)。
  **件数ではなく直前の `ui.tail_repaint.action` で判定**した結果:
  - 100ms 超は 18 件。うち 11 件が `none` (repaint 未要求 = 入力待ちで就寝)、
    6 件が `request_repaint_after_idle_upgrade` で `idle_upgrade_delay_ms` がギャップ長と
    一致 (予定どおりの起床)
  - **`request_repaint` が立っていた 100ms 超は 1 件のみ**: 起動直後 t=1.109s の 198ms
    (`reasons=['requested_nonempty']` = サムネイル要求中)。今サイクルの新経路とは無関係
  - 100ms 超の件数は v2.12.0 の 53 件・v2.9.1 の 24 件より少ない
- **検索 bench**: 全文索引に触れていないため未実施。

### 5.6 v2.12.0 リリース前確認の記録 (2026-08-06)

§5.5 の「記録が残らない」を繰り返さないための実測記録。

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも最新。DLL の
  ProductVersion `n7.1.5-12-g1fdbca85aa-20260803` が `vendor/ffmpeg/VERSION` と一致。
  PDFium MAJOR 152 → 153 の実機確認 (通常 / パスワード付き PDF、動画 / 音声) 済み。
- **CI**: master / main とも success (run 31021319941 / 31021332945)。ubuntu の
  `cargo check` を含む。今サイクルの 32 commit が初めて CI を通った。
- **idle health**: `static-foreground` / `static-background` とも PASS。CPU one-core ratio
  0.0073 / 0.0083、測定窓は perf event 0 件 (完全 sleep)。`video-pin-background` は
  条件を満たすフォルダを用意しなかったため未実施。アイドル高画質化経路は今サイクルで
  触っていないので waiver とする。
- **perf smoke**: frame 7968、16ms 超のギャップ 1376 件。**件数ではなく直前の
  `ui.tail_repaint.action` で判定**した結果:
  - 16-100ms 帯 1323 件のうち **1259 件が `none`** (repaint 未要求 = 入力待ちで就寝)。
    p50 85ms は Ctrl+↓ 連打時のキー間隔そのもの
  - 100ms 超 53 件のうち 42 件が `none`、7 件が `request_repaint_after_idle_upgrade`
    (予定どおりの起床)。最大の 33.7s / 30.3s / 20.1s / 19.6s は idle health の測定窓
  - **`request_repaint` が立っていた 100ms 超は 3 件のみ**: 起動直後 356ms、
    PDF を含むフォルダ読み込み 585ms + 106ms (`load_folder_end=55ms` +
    thumb decode + `pdf/pool_promote_visible`)。今サイクルの新経路とは無関係
  - ⚠ 同一フォルダでの v2.11.0 基準値は未取得なので「悪化していない」とは言えない。
    次サイクルで PDF フォルダの folder-load ギャップを基準値化するか判断する
- **検索 bench**: 全文索引に触れていないため未実施 (ファイル名フィルターは一覧の絞り込み)。

### 5.5 v2.9.1 の perf smoke が最終 tree で未取得

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。本セッションで確認済み。
- 何が起きたか: perf smoke の記録は 2026-07-31 の tree のものしか無く (`17231956` は
  CLAUDE.md の判定基準を書き足した commit で、計測そのものではない)、**8/1 に入った構造変更の
  後に取り直した記録が見当たらない**。対象の変更は入力所有権 (`56db11bc` / `596bb65d`)、
  `FsPageLoadState` (`9590b661`)、health watchdog (`2c11e4cd`)。
- ずれている扱い: idle-health 側は §5.4 の waiver を「未実測部分を明示する」形に書き直したのに、
  perf smoke 側は同じ扱いになっていない。CLAUDE.md Phase 2 の 9.6 は「UI 周り / I/O 経路に
  変更を入れたリリースで実施」なので `FsPageLoadState` はまさに対象。
- 対応 (どちらか):
  1. v2.9.1 の tree で perf smoke を取り直す。**PDF フルスクリーンでの計測を含める**
     (§1.39-2 の毎フレーム経路がそこにしか出ない)。
  2. 取らない判断を §5.4 と同じ形で明文化する (何が未実測かを書く)。
- 規模 / 優先度: Small / P2 (プロセス)。次版のリリース前確認までに片付ける。

---

## 6. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| サブフォルダ展開 / フラット仮想ビュー | `docs/subfolder-expansion-view-plan.md`, `docs/ui-responsiveness.md`, `docs/async-architecture.md`, `docs/details-view-and-filter-plan.md`, `docs/virtual-folders.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md`, `docs/operation-customize-share-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
