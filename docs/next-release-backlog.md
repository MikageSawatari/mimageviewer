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
- **保留 (2026-08-13、利用者判断)**。revert 後は**一度も再現していない**。原因未確定のまま
  再投入の A/B に工数を割く段階ではない、として棚上げする。再発したら上の「次回に取るべき
  証拠」から再開する (特に `dwMilliseconds` が `0x3e8` か `0xffffffff` かの確認)。
  §1.28 のカーソル問題は既知の問題として残したまま。
- 規模 / 優先度: Large / **P2 (保留)**。カーソル (§1.28) の修正はこれが片付くまで入らない。

### 1.31 wndproc の内側で GPU 待ちのある描画をする構造

- 出典: §1.30 の解析 (2026-08-01、Codex Sol の独立レビュー)。**v2.9.1 の範囲外**。
- **進捗 (2026-08-16)**: **§1.31-A は完了、§1.31-B は未着手**。A で vendored eframe に
  per-window / viewport / surface-generation 単位の typed scheduler と about_to_wait 外側
  paint drain を新設し、通常 RedrawRequested、bootstrap、AccessKit、不可視窓の要求済み work が
  message dispatch 中に renderer へ入る経路を除去した。inline 例外は event provenance が
  非ゼロ Resized の InteractiveResize frame と、その親 frame が作る immediate viewport render
  subtree だけ。reducer 11契約、既存 hidden throttle 3件、別 thread の SendMessageTimeoutW が
  RedrawWindow で同期 damage を発生させる Windows process test を gate 化した。
  **A が除去したのは「同期メッセージ自身が GPU 待ちを開始する経路」だけ**であり、外側 paint 中の
  GPU 待ちに後着の同期メッセージが巻き込まれる問題は B のまま残る。
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
- **v3.0.0 の判断 (2026-08-13)**: **構造修正は v3.0.0 をブロックしない**。
  - 根拠: この問題が実際に牙を剥いた唯一の事例は §1.30 で、それは **VST エディタの開閉が
    窓の owner / z-order を動かす**ことが引き金だった。v3.0.0 の Web 配信は、HTTP サーバと
    トランスコードを**UI スレッド外**で回すもので、**窓の所有関係にも同期メッセージにも
    手を出さない**。§1.31 の露出面を新たに増やさない。
  - その §1.30 も revert 後は再現していない (上記のとおり保留)。
- **観測の穴は解消済み (v3.0.0 前)**。`ui_heartbeat_should_stay_active_while_hidden`
  ([app.rs](../src/app.rs)) が keep-alive の述語に **mounted media session** (フルスクリーンの
  動画 / 音声、動画→音声モード、`FsCacheEntry::Video`) を含めるようになり、ネイティブ動画
  フルスクリーン中に main HWND が隠れても watchdog が armed のままになる。以下は当時の記録。
- ~~ただし観測の穴だけは v3.0.0 の前に埋める (Small)~~。§1.30 のとき
  `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も残らなかった。
  [app.rs:62646 付近](../src/app.rs:62646) を見ると、main HWND が不可視になった時点で
  watchdog を suspend し、**例外は `viewer_session_is_detached_or_switching()` だけ**。
  ネイティブ動画フルスクリーン中は main HWND が隠れるため、**最も危ない操作の最中に
  watchdog が黙る**。keep-alive の述語にアクティブなメディアセッションを含める。
  これだけ入れておけば、次に野生で固まったときにログが残る。
- message-dispatch / render 位相分離は §1.31-A として完了。nonblocking acquire / Present と
  message service latency の上限制約は §1.31-B として後続する。
- **着手順の確定 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  `§1.85-A → §1.86 → §1.31 本体` の 3 段。根拠と逆順の失敗モードは
  [codex-texture-delivery-test-brief.md](briefs/codex-texture-delivery-test-brief.md) §0。
  §1.31 は frame drop を通常経路にするので、その経路の delta 配送を先に契約化する。
  **§1.86 は障害影響としては P3 のままだが、本項の依存関係上は必須前提**として扱う。
- **前提充足 (2026-08-16)**: §1.86 の単一 delta transaction owner と non-submit outcome の
  finalization 契約が完成し、その上で §1.31-A の位相分離まで完了。通常 frame drop と readiness
  wake を足す §1.31-B は未着手。
- **本体を A / B に分割 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  - **§1.31-A**: message-dispatch 位相と render 位相の分離。**同期メッセージ自身が GPU 待ちを
    開始する経路**を除去する。vendored eframe だけで完結する (winit / wgpu は vendor しない)。
    ブリーフ = [codex-render-phase-separation-brief.md](briefs/codex-render-phase-separation-brief.md)。
  - **§1.31-B**: presentation と message service latency の上限制約。
    **A を merge しても §1.31 は完了しない**。UI スレッドが外側の paint で GPU を待つ間も
    message pump は止まるため、他スレッドの `SendMessage` は依然ブロックする。A が消すのは
    「同期メッセージが GPU 待ちを**開始させた**」経路だけ。
  - **B の実現可能性は確認済み (2026-08-17、コードで実地確認)**。acquire 側は
    **wgpu の vendor 不要**で組める:
    - `Dx12UseFrameLatencyWaitableObject` の既定は **`Wait`**
      ([wgpu-types instance.rs](https://docs.rs/wgpu-types/27.0.1/))。つまり現状の
      `acquire_texture` は frame-latency waitable object を待ち、wgpu-core の
      `FRAME_TIMEOUT_MS = 1000` で打ち切られても**その結果を捨てて先へ進む**。
    - `DontWait` は doc に「**This is useful if the application wants to wait for the
      waitable object itself**」と明記されており、まさに B の用途。swapchain は
      waitable flag 付きで作られ、handle は取れるが wgpu は待たない。
    - handle への到達経路も public: `wgpu::Surface::as_hal::<Dx12>()` (surface.rs) →
      `wgpu_hal::dx12::Surface::waitable_handle()`。
    - → 設計: 起動時に `DontWait` を設定し、paint 前に `WaitForSingleObject(handle, 0)` で
      非ブロッキング判定。未 signal なら damage を保持したまま `DeferredNotReady` として
      その frame を捨て、signal されたら wake する。**時間窓ではなく OS の事実で判定する**
      ので憲法 5 に適合する。
    - 注意: DX12 専用。backend が DX12 でない場合 `as_hal` は None を返すので、その場合は
      現行挙動へフォールバックする (新しい失敗経路を作らない)。
    - 参考: mIV は既に `MIV_WGPU_FRAME_LATENCY` で `desired_maximum_frame_latency` を
      設定しており ([lib.rs](../src/lib.rs))、native presenter は自前 swapchain で
      `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT` を既に使っている
      ([render_core.rs](../src/video/native_presenter/render_core.rs))。機構は既知。
  - **Present 側は別問題で、これは vendor 不要では閉じない**。`SurfaceTexture::present()` は
    戻り値が `()` で、HAL の FIFO Present は interval 1 かつ `DXGI_PRESENT_DO_NOT_WAIT` 無し
    (`DO_NOT_WAIT` は blocked 時に `DXGI_ERROR_WAS_STILL_DRAWING` を返す契約)。
    厳密に bounded にするなら wgpu / core / hal の patch か render thread 分離が要る。
  - ~~**B は acquire だけ先に閉じて、Present は A 後の実測を見てから判断する**のが妥当。~~
    **2026-08-17 撤回。acquire も先に測る**。下記参照。
- **B は「計測 (B0) が先」に変更 (2026-08-17、Codex Sol の設計レビューで差し戻し)**。
  正本 = [codex-acquire-readiness-gate-brief.md](briefs/codex-acquire-readiness-gate-brief.md)。
  差し戻しの決め手 2 つ (どちらもコードで実地確認済み):
  - **acquire を閉じても message service latency は上限化されない**。非ゼロ `Resized` は
    message-dispatch 中に `on_window_resized` → `configure_surface` を通り、wgpu-hal DX12 の
    `configure` は `ResizeBuffers` の前に `wait_for_present_queue_idle()` を呼ぶ。その中身は
    **`WaitForSingleObject(event, INFINITE)`** (`wgpu-hal-27.0.4/src/dx12/device.rs`)。
    unconfigure / drop 側にも同じ待ちがある。**§1.31-A が残した inline resize 例外は、
    acquire より手前で無期限 GPU wait に入れる。** 「acquire は最大 1 秒」も強すぎた
    (1000ms は frame-latency wait の timeout であって acquire 全体の上限ではない)。
  - **§1.31-A の後、acquire が実害を起こしている証拠が無い**。§1.30 の実スタックが本当に
    この wait だったかも未確定のまま。`configure` の `INFINITE` / acquire / Present の
    3 候補があり、どれが効いているか不明な状態で 1 つを選んで直さない。
  - 旧 acquire gate 案には他に P0 が 4 件あった (ブリーフ §3 に訂正付きで保存):
    `waitable_handle` は `unsafe` で「swap chain が生きている間だけ有効」→ worker へ raw handle を
    渡すのは未定義動作 (`DuplicateHandle` が要る) / UI と worker を同時 waiter にすると signal の
    取り合いになる / §1.31-A の `surface_generation` は handle generation の正本ではない
    (`RecreateSurface` で増えず、`set_window(None)` は全 surface を clear する) /
    immediate viewport は親の pass 内で自前 surface を acquire するので outer gate では捕まらず、
    gate の実体は `egui-wgpu::Painter` の per-surface seam に要る。
  - **API 到達性の結論は正しかった**: wgpu 27 に `hal` feature は無いが native build では
    `cfg(wgpu_core)` で `wgpu::hal` が re-export され、`Surface::as_hal` も public (`unsafe`)。
    mIV の依存構成で `wgpu/dx12` / `wgpu-hal/dx12` が有効、`wgpu-hal 27.0.4` 一つに解決される。
    **acquire handle へ到達するための wgpu vendoring は不要**。
  - **`NativeEguiOverlay` は別の `wgpu::Instance`** (`src/video/native_presenter/render_core.rs`)
    で既定 `Wait` のまま動く。`src/lib.rs` の設定は届かないので、
    「mIV の acquire 全体を直した」とは言えない。
- **§1.31-A 着手前の 2026-08-16 に訂正した認識**
  (前セッションの読み違い。同じ誤りを繰り返さないため履歴として残す):
  - **外側の paint 位相は現在存在しない**。`check_redraw_requests` は
    [run.rs:198](../vendor/eframe/src/native/run.rs:198) `handle_event_result` の末尾と
    [run.rs:396](../vendor/eframe/src/native/run.rs:396) `new_events` から呼ばれ、
    `WinitAppWrapper` には `about_to_wait` の実装が無い (trait 既定の no-op)。
    不可視窓の direct paint を「既に外側」と読まないこと。
  - **`RepaintNow` は resize 専用ではない**。producer は初回 `resumed` (bootstrap) /
    非ゼロ `Resized` / AccessKit initial tree の 3 つで、`EventResult::RepaintNow(WindowId)` は
    理由を失っている。位相を分けるには reason の型付けが要る。
  - **無限 `WM_PAINT` の懸念は撤回してよい**。winit は `RedrawRequested` を送った後に
    `DefWindowProcW(WM_PAINT)` を呼び、これが update region を validate する。
    eframe が描画せず戻っても再送ループにはならない。
  - **厳密な `MARKER_IN_SIZE_MOVE` 限定は採らない**。winit の非公開状態であり、
    取りに行くと winit の vendor patch か全 HWND subclass が要る。A の inline 例外は
    「非ゼロ `Resized`」とする (現行は全 redraw が inline なので、これでも厳密に狭い)。
- **設計時に確定すべき点 (2026-08-16 に追加で判明したもの)**:
  - **`WM_PAINT` 内の同期 `RedrawRequested` だけでは足りない**。Windows の resize では
    [eframe run.rs:121](../vendor/eframe/src/native/run.rs:121) が `RepaintNow` を受けて
    その場で `run_ui_and_paint` を直接呼んでいる (flicker 対策)。同期 paint の入口は 2 つある。
  - **acquire を短くしても Present 側の上限が未規定**。wgpu-core の acquire timeout は
    `FRAME_TIMEOUT_MS = 1000` 固定 (present.rs)。DX12 backend は frame-latency waitable object の
    `wait(timeout)` が返す `false` を捨てて先へ進む (`unsafe { sc.wait(timeout) }?;` で bool を破棄)。
    Present は FIFO なら interval 1 で `DXGI_PRESENT_DO_NOT_WAIT` 無し。
    **公開 wgpu API の設定だけでは短時間 try-acquire にできない**。lower-layer patch /
    waitable-object の明示 readiness gate / 非ブロッキング Present 戦略のいずれかまで設計に含める。
    なお §1.30 の「wait は本当に無限か」は、コード上は 1000ms 上限側と整合する
    (= デッドロックより飢餓)。ただし実際に観測された `WaitForSingleObjectEx` がこの wait かは
    `latency_waitable_object` の設定次第なので、live evidence の項目は開いたままにする。
  - 通常の frame drop は `Skipped` と**別の outcome** (`DeferredNotReady` 等) にする。
    通常動作なので warning 対象ではなく、damage の保持と readiness wake が要る。
  - `Idle / Painting / Pending` を global scalar にすると複数 viewport の dirty 状態を失う。
    viewport ID / dirty set / 最新 resize / surface 世代を持たせる。
  - drop 後に即 repost すると spin する。waitable signal / event / 明示的 scheduler wake で再 arm する。
  - `WM_PAINT` を即 return する際の `BeginPaint` / `EndPaint` または validation 規則を決めないと
    無限 `WM_PAINT` になる。
  - immediate viewport は UI callback 中に同期再帰する
    ([wgpu_integration.rs](../vendor/eframe/src/native/wgpu_integration.rs) の nested 経路)。
    フレーム全体を単純に `Painting` にすると detached / immediate viewport を塞ぐので、
    state の適用範囲を定義する。
  - dropped frame でも `textures_delta` は配送する (= §1.86 の契約)。screenshot request は
    成功まで保持 / 再投入する。
  - Windows 回帰テストは 2 つの性質を分ける: (a) `WM_PAINT` / 同期 message handler が有限時間で
    返ること (b) presentation 不能中も外側 render step と message-pump の service latency が有限で
    あること。ハング防止に raw `SendMessage` ではなく `SendMessageTimeout` を使い、実 DWM stall では
    なく決定的な readiness gate を止めて再現する。
- 規模 / 優先度: **残るのは §1.31-B の上限制約 = Large / P1 candidate (基盤)**。
  watchdog の穴 (Small / P2) は上記のとおり解消済み。

### 1.35 リネーム移行ジャーナルの部分失敗が再試行されない

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、Codex)。source inspection で確認済み。
- 機構 (仕様判断あり): worker が `panicked` を返したときだけ
  ジョブを `rename_migration_boot_retry` へ戻す。`report.errors` に個別ストアの失敗が
  入った場合はトーストを出すものの、ジョブは完了扱いでジャーナルから消える。一時的な
  DB ロックや I/O エラーだと、編集結果やパス依存設定の一部が旧パスに残ったまま再起動でも
  回復しない。
  - **これは現状のコードコメントが明記している意図的な選択**である
    (「per-store エラーは通常経路と同じ best-effort = 再試行しない」、v2.3.0 角度⑤ 検収時の判断)。
    したがって着手前に「一時エラーと恒久エラーを区別できるか」「再試行の上限をどう置くか」を
    決めること。無条件の再試行は、決定的に失敗するストアで無限ループになる。
  - 移行処理自体は冪等に設計されているので、失敗ストアを保持して再試行する形にはできる。
- 規模 / 優先度: Medium / P2 (仕様判断を先に決める)。もう 1 件あった「UI スレッドでの同期 I/O」は
  v2.10.0 で latest-value worker へ移して close 済み。

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

### 1.37 トレイ常駐再生まわりの所有境界 2 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  v2.9.1 で「トレイ格納中も再生を続ける」経路を新設したことで、いずれも**この版から初めて
  実際に通る**組み合わせになった。
  1. **P3: 再生継続中に video processor cache を落とす**。
     [tray_integration.rs](../src/tray_integration.rs) の `release_gpu_resources` は
     detached 早期 return より前で `release_idle_pools()` を呼び、その中の
     `processor_cache.take()` は**無条件**である (`hw_frames_pool.clear()` と
     `shared_output_pool.retain(in_use)` は refcount 上安全)。
     ⚠ **呼び出し順自体は v0.9.1 (`287eea9f`, 2026-05-19) からで、v2.9.1 で動かしたわけではない**。
     新しいのは「格納中も再生が続く」ことのほうで、その結果この経路が再生中に走るようになった。
     トレイ格納の瞬間に video processor の作り直しが 1 回入るはずで、実害は小さい。
  2. **P3: 外部 hide 検出が tray 所有権に追随していない**。[app.rs](../src/app.rs) の
     `IsWindowVisible` 追従分岐は `viewer_session_is_detached_or_switching()` だけで heartbeat
     suspension を決め、`sync_media_presenter_visibility_for_tray()` /
     `sync_retained_viewport_visibility_for_tray()` を呼ばない (これらは `hide_to_tray` /
     復帰経路にしか無い)。同フレーム後半の `sync_tray_resident_media_wake()` が**値が変化した
     ときだけ**自己修復するので実害は出にくいが、hide の所有者が 2 つある状態である。
- 規模 / 優先度: どちらも Small / P3。3 件目だった「WM_PAINT ブリッジに背圧が無い」は
  v2.10.0 で claim / ack 構造へ変えて close 済み。

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

### 1.39 `fs_page_load_state` が PDF ページで毎フレーム String を作る (`9590b661` の続き)

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 機構: `has_retained_pdf_final_ai_for_current_params` → `retained_pdf_page_key_for` →
  `metadata_cache_key(idx)` が String を生成し、HashMap を引く。
  ⚠ **レビュー報告より実コストは小さい**: `effective_params` / `final_ai_key_for_pixels` は
  retained cache に**該当エントリがあるときだけ**走る (無ければ `?` で抜ける)。
  非 PDF は `items.get` + `matches!` で即 return なので無コスト。
  とはいえ PDF フルスクリーンでは `update_prefetch_window` の current、prefetch ターゲット
  全件、連結読み keep 範囲ループ全件、見開きパートナーから毎フレーム走るため、
  key を 1 回作って使い回す形にできるかは見ておく。
- 規模 / 優先度: Small / P3。

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

### 1.47 動画の拡大縮小を mIV のシェーダで行う — 設計確定 / 未実装

- **これが入ると 1.112 (360 度動画) の前提が揃う。**投影を差し込む場所を作るのがこの項目であり、
  正距円筒投影は同じシェーダステージ上のもう 1 枚として載る。設計を変えるときは 1.112 も見る。

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
- **先送り (2026-08-13、利用者判断)**。提案として妥当だが、v3.0.0 では扱わない。
  却下ではないので、要望が重なったら再評価する。
- 規模 / 優先度: Medium / P3 (先送り)。

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

### 1.69 変換対象アーカイブのキャッシュパスが未解決のとき、ZipDir タイルだけ黙って失敗し得る

- 出典: §1.66 の修正 (`ff56abea`) の周辺を洗って見つけた。**利用者報告ではなく、
  実際に到達するかも未確認**。今回の不具合ではなく以前からある形。
- 形: `converted_archive_cache_paths` (= 変換キャッシュ ZIP へのパス表) は worker が
  非同期に作るので、表が空の窓がある。この窓での振る舞いが item 種別ごとに違う。
  - `GridItem::ConvertibleArchive`: `?` で **LoadRequest を作らない** → 後のフレームで
    再要求される。安全。
  - フォルダピン経由: `ff56abea` で「表が届いたら依存タイルだけ組み直す」ようにした。
  - **`GridItem::ZipDir`: `unwrap_or(zip_path)` で元アーカイブのパスへ落ちる**。RAR は
    `zip_loader` が `rar_loader` へ振り分けるので読めるが、**7z / ソリッド RAR は ZIP として
    開こうとして失敗**し、そのサムネイルは失敗のまま残る。
- 到達性: コードのコメントによると、`ZipDir` が元 RAR/7z/LZH のパスを持つのは
  **レーティング一覧に復元されたとき**。実際にその状態を作って確認していない。
- 方針: **まず到達するかを確かめる。** 到達しないなら、3 経路で fallback の作法が違うこと
  自体をコメントで明示して閉じる。到達するなら `ff56abea` の依存追跡
  (`pin_archive_dependencies`) を ZipDir にも広げる。**確認前に直さないこと。**
- 規模 / 優先度: Small / P3。

### 1.79 比較表示中の元画像表示 (押している間だけ両側を元画像にする) — 利用者要望

- 出典: 利用者実機確認 2026-08-13。比較 (ワイプ) 表示中に元画像表示 (`FsOriginalPreviewHold`、
  既定 RightCtrl) が効かない、という報告。**期待する挙動は「ワイプはそのままに、ワイプ元と
  比較対象の両方が元画像になる」** (利用者判断)。片側だけ元画像にしたり、比較をやめて
  元画像 1 枚を出すのは求められていない。
- **現状 (source inspection、2026-08-13)**: 比較表示中は**全モードで**元画像表示が効かない。
  `compare_requested` は `compare_view_mode != Off` だけで決まり、
  `CompareFramePrimaryDraw::resolve` は元画像表示を入力に取らない
  ([ui_fullscreen.rs:11708](../src/ui_fullscreen.rs:11708))。今回の Ctrl まわりの変更以前からこう。
- **実装コストは Medium。小さくない。** 比較の合成は加工済みピクセルから作られており、
  「元画像版の対」を新たに用意する必要がある。
  1. **prepared pair は非同期 worker が作る**。identity は
     `(current_idx, pinned_source_idx, output)` なので、**加工済み / 元画像の次元を足す**必要がある。
  2. **比較対象 (pin した側) は、pin した時点の加工済みピクセルの snapshot** (`slot.pixels`) で
     保持している ([ui_fullscreen.rs:22982](../src/ui_fullscreen.rs:22982))。元画像版は存在しないので、
     **pin 時に未加工の snapshot も持つ (メモリ倍) か、押された時点で再デコードする (遅延)** かの
     選択が要る。再デコード対象は現在ページとは限らず、`fs_cache` に無いこともある。
- 方針の案:
  - 元画像版の対は**押されたときに遅延生成**し、準備できるまでは**加工済みの比較を出したまま**にする
    (途中で別の絵に切り替わるちらつきを作らない)。既存の `WaitingForSource` / 「比較表示を準備中」
    の枠組みに乗る。
  - 一度作った元画像版は、対の identity が変わるまで保持して 2 回目以降を即時にする。
- 着手時に読む: 上記 `ensure_compare_prepared_pair` / `ComparePrepareOutput` / `pinned_compare_slot`。
- 関連: 線を消す修飾は左 Ctrl 限定にしてあるので (`88b028e1`)、既定 RightCtrl の元画像表示とは
  物理的に競合しない。この項目を入れても修飾キーの整理は不要。
- **当面の決着 (2026-08-13、利用者判断、`42212e9d`)**: 実装コストが高いので、
  **比較表示中は元画像表示を明示的に無効**にした。有効なままだと画面は変わらないのに
  「比較表示を準備中」トーストが出るなど副作用だけが起きるため、`original_preview_active`
  の入口で断っている。本項目を実装するときはこの早期 return を外す。
- 規模 / 優先度: Medium / P3 (比較表示は加工結果を見比べる機能で、元画像表示が効かないことが
  データを壊すわけではない)。

### 1.95 OS 側でコピー・移動したファイルへ編集内容を引き継ぐ (内容ハッシュ照合) — 利用者要望 ✅ Phase 1 実装済み (2026-08-22)

- 出典: 利用者要望 2026-08-20。エクスプローラー等でファイルを移動・コピーすると、
  補正 / 消しゴム / モザイク / 注釈 / トリミング / ★ / タグが引き継がれない。
- **正本: [edit-content-identity-plan.md](edit-content-identity-plan.md)。Phase 1 (A1-A6) 実装済み・実機確認済み。**
  複数コピー元の選択 (A6) まで完了。残りは同計画の「未着手」節 (アプリ内コピー / ページ移動の
  同じ失効漏れ、復元を通っていないファイルの注釈サムネイル) を参照。
  着手前に必ず読む。以下は要旨だけ。
- 現状の穴: 中央 DB は絶対パスキー。フォルダ側サイドカーは**フォルダごと**の移動しか救えず、
  リネーム移行は**アプリ内**リネームだけ。さらにサイドカーがミラーするのは
  `adjust / mask / conceal / local_adjust / export_crop / comic / tags` のみで、
  **★ / 回転 / 見開き / トリム / 本の続きはフォルダごと移動でも失われる**。
- 方式: 編集の確定点で物理ファイルのハッシュを台帳 (`content_identity.db`) に記録し、
  フォルダ読み込み時に **size → 先頭 64KB → 全体** の 3 段で照合。size はフォルダ走査で
  既に取得済み (`image_metas`) なので、候補が無いフォルダでは **I/O ゼロ**。
  照合と復元は worker (`GlobalIoSemaphore` Low、フォルダ切替でキャンセル)。
- コピー実処理は `rename_key_migration::STORES` 駆動の `copy_store` を新設して共有する
  (`App::copy_book_page_edit_key` は UI スレッド前提で worker から呼べない)。
- **変換アーカイブ (RAR/7z/LZH) を最初から対象にする**。キャッシュ ZIP のパスは元パスの
  純関数なので、再変換を待たずキーを付け替えられる。Source / ConvertedCache の 2 基底 ×
  exact / prefix の **4 面**をコピーする。
- 決定済み: 移動もコピーも既定で確認する / 復元範囲は★・タグ込みで一括 /
  「すべて選ぶ・すべて解除」ボタンを置く / 「このフォルダ以下では聞かない」は作らず
  全体 OFF 設定 1 個 (既定 ON、OFF で照合を完全停止) / リモートは対象外 /
  既存編集の遡りは一括スキャンせず訪問時に少しずつ。
- 規模 / 優先度: Medium / P2 (データ損失ではないが、編集した資産が黙って失われる体験は重い)。

#### 追加項目: 取り消した編集が復元元として残り続ける ✅ 修正済み (2026-08-25)

- 出典: Susie クラッシュ検証中に、編集していないはずのテストファイルで復元ダイアログが出た。
- **索引側は正しい。** `ContentIdentityIndex::upsert` は `has_restorable_content == false` の
  エントリを索引に入れない ([content_identity.rs:221](../src/content_identity.rs:221))。
  「中身が同じだけ」で候補になることは設計上ない。
- **記録側が実データと同期していない。** 台帳では 5 ファイルが
  `has_restorable_content = 1` かつ `last_edit_at` が全て同一だったが、**編集データは
  どのデータベースにも無かった** (`content_identity.db` 以外の全 DB を走査して 0 件)。
- **経路**: 利用者が `Ctrl+Alt+1` (`FsAdjustSlotDefault1` = 補正プリセットスロットを標準設定へ
  読み込む) を実行 → 編集として記録 → 対象が 8x8 単色画像で結果が標準値と変わらず、
  保存時に破棄 (`edit_preview_close: outcome=delete_no_edits`) → **台帳の記録だけが残る**。
- **構造**: `has_restorable_content` は 0 → 1 の単調遷移として実装されている
  ([content_identity.rs:219](../src/content_identity.rs:219))。遅れて届いた detection cache が
  先の復元元更新を消さないための設計だが、その結果**「編集を入れてすぐ取り消した」ファイルが
  永久に復元元として残る**。テストデータが特殊だったから目立っただけで、通常の画像でも
  「補正をかけて元に戻した」後に同内容のコピーを開けば同じことが起きる。
- **修正 (2026-08-25)**: 記録側で flag を下げる経路は作らなかった。編集を取り消す経路は
  `delete_mask_with_sidecar` / `remove_view_trim_page_override` / `remove_folder_thumb_pin` など
  複数あり、そのどれもが「編集した」として同じ record を通る。全部を数え上げて `Absent` を
  渡す形にすると、1 つ漏らしただけで**本物の復元元を消す**側に倒れる。
  - 代わりに**読む側で確かめる**。復元が運ぶのは `copy_stores_at` が `STORES` の unique 行を
    写すことが全てなので、そこに 1 行も無い復元元は選ばれても no-op にしかならない。
    **候補から外しても失われるものは無い**、が根拠。
    `rename_key_migration::ledger_keys_with_restorable_rows_at` が同じ表・同じ正規化で引く。
  - 台帳自身 (`content_identity.db` / `edit_origin`) は `STORES` に載っているが**除外する**
    (`content_identity::LEDGER_DB_FILE`)。台帳の行はこの問いの対象であって答えではない。
    最初これを数えてしまい、常に「行がある」になって probe が効かなかった。
  - 仮想ページの `<container>::<entry>` も見る。`LIKE` はパスに普通に現れる `_` / `%` を
    ワイルドカードとして拾うので、半開区間 (`::` 〜 `:;`) で引く。
  - **読めない store があった key は「行がある」側へ倒す**。余計な確認が 1 回出る方が、
    復元元を黙って消すよりはるかに軽い。
  - ついでに flag も下ろす (`clear_restorable_if_unchanged`)。単調遷移の例外はここだけで、
    根拠は「遅れて届いた 0」ではなく「実データがもう無い」。probe 中に UI スレッドが
    新しい編集を記録していたら `last_edit_at` の一致条件で弾かれる (compare-and-swap)。
    既に台帳へ残ってしまった stale 行も、次に該当コピーを開いた時点で自動的に消える。
  - probe は候補が出たときだけ走り (= full hash 一致が必要なので稀)、背景 I/O の枠内で行う。
- **既存テストの fixture を直した**: `folder_backfill_then_copy_...` と
  `book_byte_copy_is_declined_...` は、実データを 1 行も持たないファイルを backfill して
  候補を期待していた。本番の backfill は編集済みファイルにしか走らないので、fixture 側に
  実際の ★ 行を置いて production と同じ形にした (`rated-only.jpg` という名前が既に
  そう主張していた)。
- 再現データ: `C:\tmp\miv-susie-crash-test` (中身が同一の `MIVOK` ファイル 5 個)。
- 規模 / 優先度: 小〜中 / P3 (実害は余計な確認ダイアログ。データは失われない)。

### 1.99 複数ウィンドウモードで RAR を開くとメイングリッドまで書庫一覧へ切り替わる — 専用スレ >>270 ✅ 実装済み (2026-08-22)

> 実装は detached-rework の `21c3dc0d` / `d7e139d0` に入っており、2026-08-22 の master マージで
> 取り込まれた。型付き宛先意図 `OpenRequestOwner::DetachedGridArchive` →
> `ArchiveConvertCompletionPolicy::DetachedGridArchive` を、ダブルクリック / Enter /
> 直読み完了 / 変換完了 / cache hit のすべてが引き継ぐ。visibility 述語・`show_viewport_*`・
> R2e は不変。**実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>270 (2026-08-20)。利用者環境・手元とも再現済み。
- 症状: 複数ウィンドウモードで RAR を開くと独立した画像ウィンドウだけでなく、
  メインウィンドウのグリッドも RAR 内の一覧へ切り替わる。ZIP では発生しない。
- 期待する不変条件: 複数ウィンドウモードから画像 / 本を開く操作は、表示先の独立ウィンドウだけを
  更新し、メイングリッドの場所・一覧 generation・検索 / 絞り込み・選択状態を変更しない。
- 修正方針:
  - RAR の open request が、どの入口からメイングリッドの一覧遷移へ流れているかを確認する。
    RAR だけを後段で戻す guard は入れず、表示要求の所有 context と dispatch 先を正す。
  - 直読み RAR / CBR と変換対象 RAR の両方を確認し、ZIP / PDF を対照経路として open request の
    差を棚卸しする。
  - detached 述語 / viewport 経路へ触れる場合は [detached-rework-plan.md](detached-rework-plan.md)
    §2 に従い、症状パッチでないことを ClaudeCode / Codex の双方で確認し、同書 §11 に記録する。
- 回帰確認: 複数ウィンドウモードで RAR / CBR / ZIP / PDF を順に開いてもメイングリッドが不変で、
  各独立ウィンドウには正しいページが表示される。フル機能ウィンドウの従来の一覧遷移は壊さない。
- **根本原因 (2026-08-21 特定、v3.1.3 では未修正)**: ZIP / PDF はダブルクリック・Enter・
  ゲームパッドのどの入口でも共通の detached 振り分け ([ui_main.rs:13017](../src/ui_main.rs:13017)、
  [app.rs:34249](../src/app.rs:34249)) を先に通るが、**typed open plan が本コンテナとして認識するのは
  PDF / ZIP だけで、`ConvertibleArchive` である RAR / CBR は対象外**
  ([app.rs:37317](../src/app.rs:37317))。RAR は振り分けを素通りして通常ナビゲーションへ落ち、
  所有者が `OpenRequestOwner::Navigation` に固定される ([app.rs:18298](../src/app.rs:18298))。
  RAR は同期的に ZIP として開けず、直読み可否を App-global で probe した**後**に
  メイン一覧へ遷移する ([archive_convert.rs:675](../src/ui_dialogs/archive_convert.rs:675) /
  [854](../src/ui_dialogs/archive_convert.rs:854))。
- **「RAR を ZIP と同じ descriptor に足す」では直らない。** RAR は非ソリッド・入れ子なしだけが
  直読みで、それ以外は変換対象という分岐がある ([virtual-folders.md:116](virtual-folders.md))。
- **正しい修正 (2026-08-21 に具体化)**: 完了時点では detached 窓も bundle もまだ存在しないので、
  「既存 context の所有者」ではなく **「この要求のために新しい detached context を作る」という
  型付きの宛先意図**を非同期完了へ持たせる。**同型の実装が既にある**:
  ブックマーク経路は `OpenRequestOwner::Bookmark(owner)` →
  `ArchiveConvertCompletionPolicy::Bookmark(owner)` ([archive_convert.rs:47](../src/ui_dialogs/archive_convert.rs:47))
  を持ち、直読み完了と変換完了の両方から
  `open_converted_bookmark_in_detached_context` ([app.rs:39993](../src/app.rs:39993)) へ着地して
  `ViewerContextDescriptor::Zip { path: 実体, archive_source_override: 元アーカイブ }` で
  **新しい detached context を作る**。グリッドからの `ConvertibleArchive` 開きにも、
  `DetachedGridItemOpenPlan::FolderCandidate` ([app.rs:1857](../src/app.rs:1857)) と同じ形で
  candidate variant と着地関数を用意する。
  入口は既に共通化されているので、ダブルクリック ([ui_main.rs:13017](../src/ui_main.rs:13017)) と
  Enter ([app.rs:34249](../src/app.rs:34249)) の両方が同じ gate を通る。
  **visibility 述語や `show_viewport_*` builder の変更は不要。**
  変換キャッシュ命中時の同期経路 (`open_archive_via_cache_owned`) も同じ宛先意図を通す。
- **R2e (viewer context registry) には依存しない** (正本:
  [detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5)。
- 規模 / 優先度: Medium / P1。次版修正候補。

### 1.100 複数の画像ウィンドウを開くと、先に開いたウィンドウでマウスジェスチャが効かなくなる — 専用スレ >>270

- 出典: 専用スレ >>270 (2026-08-20)。利用者環境・手元とも再現済み。
- 症状: 複数ウィンドウモードで画像ウィンドウを複数開くと、後から開いたウィンドウでは
  マウスジェスチャを使えるが、先に開いたウィンドウへ戻っても認識されない。
- 期待する不変条件: 右ドラッグの入力状態と発火先は、入力を開始した viewer window / viewport が
  所有する。アクティブウィンドウを切り替えても別ウィンドウの状態に上書きされず、閉じた
  ウィンドウの状態も残らない。
- 修正方針:
  - pointer event の source、ジェスチャ状態機械、実行 context の各所有先を追い、グローバルな
    active target や最後に開いた viewport へ誤って寄っている境界を正す。
  - open / activate / deactivate / close / cancel の lifecycle と、リングショートカット・短い
    右クリックの兄弟経路も同じ owner 規則になっているか確認する。
  - detached 述語 / viewport 経路へ触れる場合は §1.99 と同じく
    [detached-rework-plan.md](detached-rework-plan.md) §2 / §11 の規約に従う。
- 回帰確認: 3 枚以上の独立ウィンドウを開き、前後のウィンドウを交互にアクティブ化して同じ
  ジェスチャが発火する。最新ウィンドウを閉じても先行ウィンドウで継続し、メイングリッドと
  native 動画のジェスチャを壊さない。
- **根本原因 (2026-08-21 特定・訂正、v3.1.3 では未修正)。これは R2 そのもの**:
  - **非アクティブな静止画ウィンドウ上の右ドラッグは、アクティブ化もせずジェスチャ状態機械にも
    届かない完全な no-op である。** 非アクティブな静止画窓は `show_viewport_deferred` で描かれ、
    コールバックが root pass へ報告するのは描画 / focus / close 要求 / placement だけで、
    ポインタ入力を一切拾っていない ([ui_fullscreen.rs:11622](../src/ui_fullscreen.rs:11622)、
    [ui_fullscreen.rs:11175](../src/ui_fullscreen.rs:11175))。アクティブ化を担う OS watcher は
    **`VK_LBUTTON` しかサンプルしない** ([detached_window_manager.rs:326](../src/app/detached_window_manager.rs:326))。
    左クリックでアクティブ化した後ならジェスチャは効く (§4.5 の「アクティブにすれば動く」と一致)
  - `MouseGestureState` が持つ識別情報は表示種別だけで、**window ID / viewport ID / session ID を
    持たない** ([ring_shortcut.rs:53](../src/ring_shortcut.rs:53))。状態は
    `ViewerContextBundle` ではなく **App に 1 個だけ**ある ([app.rs:10005](../src/app.rs:10005))。
    発火時に owner を再解決せず、その時点で mount されている context へ適用する。
    複数の画像ウィンドウはどれも `RightDragContext::ImageFullscreen` なので区別できない
  - `last_input_surface` は `MainWindow` / `Viewer` の二択で全 viewer window を区別しない
    ([detached_window_manager.rs:452](../src/app/detached_window_manager.rs:452))
  - **訂正**: 当初「非アクティブな窓が集める pointer 情報は `any_pressed` / `any_released` だけ」
    「アクティブ化は press ではなく release で起きるので最初の右ドラッグが失われる」と記録したが、
    これが当てはまるのは `show_viewport_immediate` で描く `ParkedLive` の**動画 / 音声窓だけ**
    ([ui_fullscreen.rs:11785](../src/ui_fullscreen.rs:11785)、[app.rs:39217](../src/app.rs:39217))。
    静止画窓はそこへも到達しない
- **利用者決定 (2026-08-21)**: 「ジェスチャを認識し、ジェスチャされた場合は自動でアクティブ化した
  上で、ジェスチャコマンドを実行する」。**両方のウィンドウが見えている以上、ジェスチャが受理される
  のが期待動作**という判断。右押下だけでアクティブ化する案 (1 回目のドラッグを消費する / 単なる
  右クリックで窓が前面化する) は採らない。
- 必要なもの (3 つ、いずれも R2 の守備範囲):
  1. 非アクティブ窓のポインタ列を、その窓の identity 付きで root pass へ届ける
     (`DeferredDetachedImageWindowEvent` は既に window id を持つ)
  2. ジェスチャ状態を **window 所有**にする (= R2b 残件の「散在 state の typed 集約」)
  3. 成立時に「アクティブ化 → コマンド実行」を**型付きの順序**として表す (guard / 遅延ではなく)
- **R2 (`DetachedWindowRuntime` + 状態の集約) の一部として実施する。** ジェスチャ状態を window
  所有にするのは R2 の守備範囲であり、App 側に識別子を足す小修正は憲法 3 に抵触する。
  一方、コマンドはアクティブ化**後**に実行するので、マウントされていない context へ適用する必要は
  無く、**R2e (viewer context registry) の完成には依存しない**
  (正本: [detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5)。
- **状態 (2026-08-21)**: 実装済み・検収合格・**実機確認済み**。静止画 (`8db282e3` + `5b83df3f`)、
  ガイド表示 (`d2c19796`)、動画 ParkedLive (`c71d8c08` + `4c5260a2` + `12f60c97`) のすべてで動作確認済み。
  指示書は [detached-rework-stage-passive-gesture.md](detached-rework-stage-passive-gesture.md)、
  進捗記録は [detached-rework-plan.md](detached-rework-plan.md) §9.5。
  **実機で確認が取れて出荷するまで本項は消さない** (根本原因・利用者決定・訂正履歴の正本)。
- 規模 / 優先度: Medium / P1。

### 1.101 動画の上部 HUD / 下部シークバーを個別に固定表示できるようにする — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> 設定は動画専用 bool `video_top_bar_locked` / `video_seek_bar_locked` の 2 つ。既定は両方 `false`。
> 静止画の `fullscreen_*_locked` は共有せず、余白だけ `fullscreen_fixed_bar_gap_px` を再利用する。
> 固定状態は可視性純関数への**入力**として渡し、描画と HUD HWND の hit-test region は
> `render_once` が算出した `top_bar_drawn_visible` / `bottom_hud_visible` の**同じ snapshot** を読む。
> 上部の固定は下部へ漏らさない。tile grid / navigation preview 中は上下とも抑止する。
> 固定バーは映像へ重ねず、`compute_video_visual_transform` の target からバー領域と余白を除外する。
> compact は除外後の残り領域の右上 1/4 へ合成する。鍵アイコンは静止画のベクター描画を共有する。
> 固定表示は external drag 中も維持し、touch latch 自体は変えない。
> 音声専用設定は増やさず、通常の音楽ビューは従来どおり常時表示。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。動画シークバーを常時表示したい要望。
- 方針: 静止画と同じ考え方で、動画の上部 HUD と下部シークバーをそれぞれ独立して
  固定 / 自動表示へ切り替えられるようにする。動画専用設定とし、既定は現在の自動表示を維持する。
- 実装先は native presenter の HUD overlay
  (`src/video/native_presenter/{render_core.rs,overlay_draw.rs}`) とし、旧 egui 動画 UI へだけ
  設定を足さない。通常の音楽ビューは対象外とし、VST 用 audio-only native shell だけ動画設定を共有する。
- 固定中は映像 target からバー領域を除外し、VST editor、タッチ操作、HUD HWND の hit-test region /
  z-order を確認する。固定解除後は既存の自動表示へ戻ること。
- 規模 / 優先度: Medium / P2。

### 1.102 YouTube 型の精密シーク用サムネイル列 — 専用スレ >>271 / >>277

- **1.113 (音声波形ストリップ) と同じ場所を使う。**ストリップの導線 (シークバーから上へドラッグ) を
  共有し、中身をサムネイル列と波形で切り替える前提で設計する。片方だけ先に作らない。

- 出典: 専用スレ >>271 (2026-08-20)。シークバーへサムネイルを並べたい要望。
- **意図確認済み (専用スレ >>277)**: YouTube でシークバーを上へドラッグしたときに出る、
  動画を見たまま複数の場面を横一列で選べる「精密シーク」相当が希望。常時表示する
  サムネイル付きシークバーではない。
- 既存機能との境界:
  - `S` / 上部 HUD のタイルボタンで、動画全体の場面を複数タイル表示してシークできる。
  - `B` の動画ブックマークは、左パネルへサムネイル・時刻・名前を並べる。
  - シークバー hover では、その位置のプレビューを 1 枚表示する。
- YouTube デスクトップ版の観測 (2026-08-20、実装仕様の公式保証ではなく参考値):
  - シークバーから上へドラッグすると、160x90px 程度のサムネイル列を現在位置中心に表示し、
    列を横へ動かすことで動画全体をたどる。通常幅では約 5〜6 枚、広い画面ほど表示枚数が増える。
  - ストーリーボードはキーフレーム列ではなく固定時間間隔。観測例は 3:33 = 2 秒間隔 / 108 枚、
    8:14 = 5 秒 / 100 枚、20:17 = 10 秒 / 123 枚、2:00:53 = 10 秒 / 727 枚。
  - YouTube は事前生成済み JPEG sprite を配信するため、ローカル動画からその場で作る mIV と
    抽出コストの前提が異なる。見た目だけをそのまま真似て全時間分を先に生成しない。
- mIV での UI 方向:
  - シークバーから上方向へ一定量ドラッグしたときだけ、下部 HUD 上へサムネイル列を開く。
    中央の時刻を選択位置とし、左右ドラッグで列を送る。固定シークバーでも通常時は列を隠す。
  - 初期生成は画面に見える枚数 + 少量の前後だけとし、移動方向へ逐次追加する。動画全体の
    サムネイル生成完了を待ってから表示する構造にはしない。
  - 既存 `tile_thumb_cache`、seek hover cache、タイル抽出 worker のどこまでを共有できるか確認する。
- **抽出方式は未決定 (性能との妥協を追加調査する)**:
  - 表示時刻どおりの任意フレームは、素材によって 1 枚約 1 秒かかる。一方、直前のキーフレームを
    そのまま採る場合は約 50ms。既存 §1.104 の実測でも精密側は最大約 1 秒、キーフレーム近傍は
    約 40〜80ms で、GOP 距離が支配項と確認済み。
  - キーフレームだけを列にすると高速だが、GOP によって時刻間隔が不均一になり、長い GOP では
    内容が粗くなる。最終 UI をキーフレーム列に固定するとはまだ決めない。
  - 比較候補は、(A) 固定間隔を精密復号、(B) キーフレームのみ、(C) キーフレームを先に表示して
    固定間隔の精密画像へ非同期差し替え、(D) 表示範囲の先頭側キーフレームへ 1 回 seek して
    順方向にまとめて復号、の 4 方式。素材 / GOP / HW decode 別に初回表示時間、列を送ったときの
    待ち、CPU / GPU 負荷を測って決める。
  - サムネイル画像が近似時刻でも、クリック後の本編 seek は選択した表示時刻へ行う。近似画像と
    ラベル時刻のズレをどこまで許容するかは §1.104 の設定と共有するかも含めて判断する。
- 規模 / 優先度: Medium〜Large / P3 (抽出方式の計測・設計後に着手)。

### 1.103 ごみ箱へ移す場合に mIV の削除確認を省略できる設定 — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> 省略の述語は `設定 ON && DeleteConfirmKind::RecycleBin`。
> **フォルダと ZIP / PDF / 対応アーカイブも省略対象に含む。** ごみ箱から復元できることを前提に、
> タイル 1 つが一覧に見えていない多数のファイルを持ち、そのすべてを確認なしでごみ箱へ移すことは
> 設定画面の説明で明示する。
> `MayPermanent` は従来どおりキャンセルが初期選択、`FOF_WANTNUKEWARNING` も不変。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。削除確認を省略したい要望。
- 仕様:
  - 設定は既定 OFF。ON の場合も、mIV の事前判定が `DeleteConfirmKind::RecycleBin` のときだけ
    mIV の確認ダイアログを省略する。
  - リムーバブル / ネットワークドライブ、ボリューム設定、容量等から
    `DeleteConfirmKind::MayPermanent` と判定した場合は、従来どおり確認を表示し、初期選択を
    キャンセルにする。
  - Shell 側の最終判断は mIV から確定できないため、`IFileOperation` の
    `FOF_WANTNUKEWARNING` は維持する。Windows が出す完全削除警告までは抑止しない。
  - 複数選択時は 1 件でも `MayPermanent` があれば省略しない。全件 `RecycleBin` なら、フォルダや
    一覧に見えていない内容を持つアーカイブも確認を省略する。
- 回帰確認: 通常のローカルファイル、フォルダ、ZIP / PDF / 対応アーカイブをごみ箱へ移す場合は
  確認が省略され、完全削除候補、混在選択、Shell がごみ箱へ移せないケースでは警告導線が残る。
  設定 OFF は従来動作と同じ。
- 規模 / 優先度: Small / P2。

### 1.105 動画のフルスクリーン上部バーにも「…」オーバーフローを出す — 利用者要望

- 出典: 利用者要望 (2026-08)。静止画のフルスクリーン右上に「…」を追加したので、動画にも
  同じものがあると UI として揃う、という指摘。
- 現状: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `overflow_available` が
  `!state.is_video && !is_music_view && !panorama_mode_active_now && ...` で、
  **画像のときだけ**「…」を出す。動画・音楽・パノラマでは出ない。
- 仕様の論点: 動画の上部バーには既に動画専用ボタン (タイル一覧 / Perf グラフ 等) が並ぶ。
  「…」に何を入れるかを決める必要がある。
- **1.104 とは切り離す (2026-08-20 決定)**。「ズレ許容」をフルスクリーン中から変えられると
  体感を確かめながら調整できるが、動画側の上部バーへ overflow を通す修正量が大きい。
  1.104 の初回リリースには**含めない**。設定は環境設定からのみ変更できる形で出す。
  報告者へもこの件は伝えていないので、実施を約束したものとして扱わない。
- 規模 / 優先度: Medium / P3。

### 1.106 リングショートカット / マウスジェスチャを左クリックで取り消す — 利用者要望

✅ 実装済み (2026-08-22)。グリッドセル / グリッド背景 / active viewer / passive detached
viewer の 4 開始面で、進行中の右ドラッグを左ボタン press から既存 cancel helper へ流す。
active 面では対応する左 release と右ボタンの後続 release を通常 click として再利用しない。
passive detached は通常どおり release で窓を activate するが、選択 / open / viewer action には
再利用しない。固定 mouse chord の理由は `keymap-spec.md` に記録した。

- 出典: 利用者要望 (2026-08)。右ドラッグで開始した後にやめたいとき、取り消す手段がない。
  現状の回避は「リングは中央の円へ戻して離す」「ジェスチャは割り当てのない軌跡にする」。
  提案は**右ドラッグ中に左クリックを押すと取り消し**。
- 実装の見通しは良い。取り消し自体は既にあり ([ui_main.rs](../src/ui_main.rs) の
  `cancel_mouse_ring_flick` / `cancel_mouse_gesture` をダイアログが開いたときに呼んでいる)。
  足すのは発火条件だけ。
- 確認すべき点:
  - 開始点は 3 か所。[ui_main.rs](../src/ui_main.rs) に 2 か所 (グリッドと、もう 1 か所)、
    [ui_fullscreen.rs](../src/ui_fullscreen.rs) に 1 か所 (フルスクリーン / native 動画)。
    **同型の入口をすべて塞ぐ** (片方だけ直すと「一覧では取り消せるがフルスクリーンでは効かない」になる)。
    タッチからの開始経路は無い (右ドラッグのみ。`touch_input.rs` / `touch_correlation.rs` に
    ring / gesture の参照が無いことを確認済み)。
  - 取り消しに使った左クリックが、離した時点で通常のクリック動作 (選択 / 開く) として
    発火しないこと。リング / ジェスチャが active な間は左ボタンの press と release を
    両方消費する必要がある。
  - 取り消し自体は既に多くの場所から呼ばれている (gamepad / native_video / app / fullscreen /
    main)。追加するのは左ボタン押下という発火条件だけで、取り消し処理は既存のものを使う。
- 規模 / 優先度: Small / P2。

### 1.107 Z 照準のカーソル写像を「画面帯」から「実際に描いている画像領域」へ — 利用者要望
 ✅ 実装済み (2026-08-21)

> 実装は [briefs/z-aim-cursor-mapping.md](briefs/z-aim-cursor-mapping.md)。写像と描画は
> `ZAimBasis` を 1 つ共有し、basis は **実際に描画しているスケール**を持つ (単ページは Z 中に
> contain を強制するので contain、見開きは表示モード由来の `fit_scale`)。見開きのカーソル写像も
> 同時に直った (それまで描画位置でない contain 矩形へ写像していた)。
> **実機確認済み (2026-08-21)**: 意図どおりの動作。以下は着手前の記録。

- 出典: 利用者報告 (2026-08)。3 パターンの切り分けまでいただいた。
  1. 上下左右に余白がない画像 → 枠とカーソルのズレは小さい (あっても枠内)
  2. 縦長画像 (左右に余白) → 上下端への枠移動は 1. と同じだが、**左右端へ動かすには
     ウィンドウ端付近までカーソルを運ぶ必要があり、左右方向に大きくずれる**
  3. パノラマ画像 (上下に余白) → 2. と逆で、左右は違和感なく、**上下のズレが大きい**
- 原因: [displayed_image_transform.rs](../src/displayed_image_transform.rs) の
  `z_cursor_image_px` が **`pan_band` (画面側の帯) 内の比率**で画像座標を決めている。
  帯は viewport 全体から上下の HUD 分だけ詰めたもので、**画像の縦横比を見ていない**。
  一方、照準枠を描く `z_aim_frame_rect` は `view_rect` に縦横比を保って収めた
  `content_rect` を基準にしている。**写像と描画で基準が違う**ので、余白が大きい方向ほど
  カーソルと枠が離れる。報告の 3 パターンはこの差でそのまま説明できる。
- 決定 (2026-08-20): **写像の基準を `content_rect ∩ pan_band` にする**。
  - `content_rect` = いま実際に描いている画像領域 (`view_rect` に縦横比を保って収めたもの)。
    `z_aim_frame_rect` が既に使っているものと**同一の値を共有する** (別々に計算すると
    また乖離する。共通 helper へ出す)。
  - `pan_band` との交差を取るのは、上下の HUD ホバー帯へカーソルが入る前に画像の上端・
    下端へ到達できるようにするため (実機 FB 2026-06-21 で入れた既存の意図)。これは維持する。
  - 効果: 縦長画像では横方向が画像領域基準になるので 2. が解消。パノラマでは
    `content_rect` が帯の内側に収まるので交差が `content_rect` そのものになり、3. も解消。
    1. は元から差が小さく、変化もほぼ無い。
  - 縮退: 交差が空、または極端に細い場合 (小さいウィンドウ + 大きい HUD 余白) は
    従来どおり `pan_band` を使う。狙えなくなる状態を作らない。
- **同型の入口**: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `zip_cursor_image_px`
  (連結表示 / 見開き合成) が**同じ式の複製**になっている。片方だけ直さない。
  可能なら 1 つの helper に寄せる。
- カーソル非表示 ([ef9d8b0b](../src/ui_fullscreen.rs)、Z を押している間は隠す) とは独立。
  この写像を直した後にカーソルを出す方が自然かどうかは、実機で見てから判断する。
  当面は非表示のままでよい。
- 回帰確認:
  - 既存テスト `zip_cursor_image_px_maps_band_to_image_and_clamps` と
    `zip_pan_band_reaches_image_edge_before_top_hover_zone` は前提が変わるので更新する。
    後者が守っている「上部ホバー帯へ入る前に画像上端へ届く」性質は**新しい写像でも維持する**
    ことをテストで明示する。
  - 縦長・パノラマ・余白なしの 3 形状で、カーソル位置と照準枠の対応を確認する。
  - トリム表示中 (`content_bbox` あり) と回転ページでも枠と写像が一致すること。
- 規模 / 優先度: Small〜Medium / P2。

### 1.108 開くときは黒地、切り替えるときは前の画像 — 表示先の占有を routing 境界で判定する

- 出典: 2026-08-20 の利用者報告 (グリッドで PDF をダブルクリックすると、1 ページ目が遅いときに
  無反応に見える) と、それに続く仕様整理。
- **確定した規則 (利用者判断)**: **表示先に既に中身があるか**で分ける。
  - **A (中身が無い)**: 新しいウィンドウが開く / F12 デタッチで新規窓 / 同一ウィンドウ内で
    ファイルを開く → **黒地を挟んでフルスクリーンへ移る**。開く操作の結果が即座に返る
  - **B (中身がある)**: Ctrl+↑↓ / メイン側から既存 detached を切り替え → **前の画像を保持**。
    黒を挟むとちらつく
  - どちらも待ちが 500ms を超えたら中央に「読み込み中」(v3.1.3 で実装済み)
- **却下された導出**: `FsNavigationSequence::previous.is_none()` では分離できない。詳細と
  全経路の分類は [docs/briefs/fs-open-black-vs-holdover.md](briefs/fs-open-black-vs-holdover.md)
  の「第 1 段階の結果」。要点:
  - `previous` は「直前の表示単位を texture として捕捉できたか」であり、表示先の占有ではない
  - **A の production 経路のほとんどに navigation sequence が無い** (`open_fullscreen` を直接通る)
  - **B の多くにも sequence が無い** (Home/End、スタック Shift+↑↓、連結読みのシーク、
    スライドショー、native 動画の前後移動、passive snapshot クリック、remote 復元 等)
  - 反例: グリッド由来・`from_explicit_open == true` でも、linked detached の既存窓を
    更新するなら B
- **正しい判定地点**: teardown 後の `open_fullscreen` ではなく、**表示先の surface / context を
  選ぶ routing 境界**。`ViewerPresentation`、viewer session、detached runtime state、
  mounted bundle の `fullscreen_idx`、native presenter の owner を見る必要がある。
- **リワーク R2 (状態の集約) が所有する領域と重なる。**独立した作業として着手しない。
  `previous.is_none()` を viewport 側へ足す実装は §2 の症状パッチに当たる (Codex 判定)。
  条件を増やす形の部分対応も、単一の所有者を持たない routing 判断へ条件を足すことになるため
  採らない (利用者判断)。
- 現状: 中央の「読み込み中」により**無反応に見える問題は解消済み**。残るのは
  「グリッドが見えたまま待つ (黒地にならない)」という見た目の差のみ。
- 規模 / 優先度: 中〜大 / P3 (実害は見た目。R2 で routing の所有者が決まってから)。

### 1.109 複数ウィンドウ・見開き表示でページ戻りを長押しすると操作不能になる — 専用スレ >>276 ✅ v3.1.3 で出荷済み

> 修正は 2 段。`9b84c265` が「rendition sequence が active というだけで全 upload を止める」decision を
> 廃し、塗り元 (paint source) で 3 状態に分けた (`defer_ui_uploads` は後方互換の計測フィールドへ降格、
> 実判定は `ui_work_admission`)。`c3a65a6b` が、その時「別原因」として残した「detached context が
> 自前の thumbnail 結果を捨てる」を直した。**本項の登録はこの 2 コミットより後** (08-21 00:00 対
> 08-20 22:24 / 23:24) なので、調査の記録として書かれたまま完了印が付いていなかった。
> 実機確認済みで、**v3.1.3 (2026-08-21) の更新履歴に記載して出荷済み**。v3.2.0 の対象ではない。
> 以下は着手前の記録。

- 出典: 専用スレ >>276 (2026-08-20)。v3.1.2 の複数ウィンドウモードで報告。
- 報告条件:
  - 見開き表示でカーソルキーを長押ししてページを戻すと、その後操作不能になる。
  - JPEG ではかなり多く戻ったとき、WebP では数ページで発生しやすい。
  - **手元では、複数の静止画ウィンドウを開いた状態で、静止 WebP 600 枚の見開きを
    長押しで往復すると再現した**。テストデータは
    `C:\tmp\miv-spread-webp-portrait-600-20260820` (1200x1800、連番入り)。
- 状態: **再現・perf log で原因確認済み**。WebP のデコード負荷や UI thread の停止ではなく、
  通過表示用 navigation sequence と full-size upload pacing の循環待ち。
- 実ログの時系列 (`perf_events.jsonl` / `mimageviewer.log`、2026-08-20):
  1. 見開き `[415, 416]` の full-size load を開始し、静止 WebP の decode は両方とも
     約 20〜21ms で完了した。
  2. `idx=416` は `fs/ready` まで進んだが、`idx=415` は `fs_upload_backlog` に残ったまま、
     次のページ戻り入力で `[415, 416]` が navigation target になった。
  3. `idx=415` の thumbnail は `thumbnail_not_loaded` のため pass-through rendition を作れず、
     `materialized_ready=false` / `rendition_ready=false` / `still_awaiting` になった。
  4. 一方、`page_turn_decision_for_inputs` は rendition 対応 sequence が active というだけで
     `defer_ui_uploads=true` にする (`src/ui_fullscreen.rs:8832-8845`)。
     `poll_prefetch` はこの値で早期 return するため (`src/app.rs:63201-63207`)、すでに decode
     済みの `idx=415` を backlog から GPU へ載せる経路まで止まった。
  5. target が完成しないので sequence は退役できず、直前の `[417, 418]` を
     `nav_holdover` で表示したまま循環した。ログでは `UI uploads deferred` が 32768 frames、
     `no stand-in` が 131072 frames まで継続した。
- **根本原因**: rendition が未準備の navigation sequence が、target を materialize するために
  必要な upload 自体を抑止できる状態モデルになっている。見開きの片側だけ upload backlog に
  残るタイミングで自己解除不能になる。複数ウィンドウはこの順序を再現しやすくする明確な条件だが、
  共有 navigation / upload 経路の問題なので単一ウィンドウで絶対に起きないとは仮定しない。
- 期待する不変条件: 長押し中に受理したページ移動が順に処理され、キーを離した後は最後に
  表示したページで必ず settled になる。前方向 / 後方向、素材形式、先読み完了順にかかわらず、
  次の入力を塞ぐ pending / transition 状態が残らない。
- 修正方針:
  - timeout、強制 reset、repaint 追加では直さない。
  - pass-through rendition が未準備でも、対象見開きの完成済み full-size result を
    `fs_upload_backlog` から反映して materialized target を settle できる状態遷移にする。
    `rendition_sequence_active` だけで全 upload を止める現在の decision を見直し、
    rendition の待機と target materialization の所有関係を typed phase で明確にする。
  - 1 フレームあたりの upload pacing と「現在ページ + 見開き相方」の優先順位を確認し、
    片側だけ反映された直後の入力でも完成経路を失わないようにする。
  - navigation sequence、`fs_pending`、`fs_upload_backlog`、viewer context の mount / park / activate
    を同じ context owner と generation で棚卸しし、前後移動・open / switch / close / cancel / error
    の兄弟経路にも同じ循環がないか確認する。
- 回帰確認:
  - JPEG / PNG / 静止 WebP / Animated WebP、同一枚数・同一寸法で比較する。
  - 左綴じ / 右綴じ、左右キー、前進 / 後退、先読み枚数、サムネイルキャッシュ有無を振る。
  - 複数ウィンドウモードを主対象にし、フル機能ウィンドウと F12 detached を対照にする。
  - 少なくとも「target の片側に thumbnail が無い」「もう片側だけ full-size ready」「残り片側の
    full-size result は upload backlog」という実ログの順序を固定し、target が materialized で
    settle して backlog が drain されることを検査する。
  - page-turn request / accepted / materialized / settled、表示 generation、cache source、
    key level / release、active viewer context を同じ perf log で追い、同一 target の
    `still_awaiting` が無期限に継続しないことを確認する。
- 修正時の注意: 複数ウィンドウの入力 / 表示状態所有に触れるため、
  [detached-rework-plan.md](detached-rework-plan.md) §2 の禁止事項に従う。再現した症状だけに
  reset を足さず、前後移動と全素材で共有する state transition の破れを所有境界で直す。
  detached predicate / viewport 経路へ変更が及ぶ場合は ClaudeCode / Codex 双方で構造的修正と
  合意し、`detached-rework-plan.md` §11 に変更範囲と理由を記録する。
- 規模 / 優先度: Medium / P1。

### 1.110 別ウィンドウの題が、表示ページ不定のときに一覧の先頭ファイル名になる

- 出典: 2026-08-20、keep-alive backstop の所有権修正 (`detached-rework-plan.md` §11) の
  フェーズ 1 調査で Codex が指摘。**その修正とは別件なので、同時に直さず切り離した** (憲法 7)。
- 機構: detached の viewport builder は題の index を
  `self.fullscreen_idx.unwrap_or(0)` で決める ([ui_fullscreen.rs:12791](../src/ui_fullscreen.rs:12791))。
  **表示ページが定まっていない正規のギャップ** (列挙待ち、パスワード待ち、フォルダ移動の途中など) では
  `None` になり、**その一覧の先頭ファイル名**が題になる。
- 症状: 一瞬だけ、開いているものと無関係なファイル名がタイトルバーに出る。
  backstop の所有権修正で「別 context の名前が出る」方は消えたが、**こちらは残る**
  (同じ context の中で、たまたま先頭のファイル名が出る)。
- 直す方向: **`unwrap_or(0)` をやめる。** 表示ページが定まっていないことは「先頭ページ」ではない。
  ギャップ中は直前に表示していた題を保つか、アプリ名だけにするかを決める。
  **`Option` を握りつぶさずに、ギャップを型で表す**のが筋。
- 同型の入口: 静止画スナップショット窓の題
  ([app.rs:40534](../src/app.rs:40534) 付近の `detached_image_snapshot_title_for_idx`) も
  `items.get(idx)` が空振りしたとき `"mimageviewer"` に落ちる。こちらは先頭へ落ちないので
  挙動は穏当だが、**ギャップの表し方を揃えるなら一緒に見る**。
- 規模 / 優先度: 小 / P3 (実害は一瞬の見た目。所有権の方が先)。

### 1.111 フルスクリーンで動画へ入る瞬間に前面を失い、押しっぱなしのキーが他アプリへ流れる — 利用者報告

- 出典: 利用者報告 (2026-08-21、v3.1.2 で確認)。
  - 全画面モードで静止画を表示し、**上下キーを押しっぱなしで高速送り**している最中、
    次が動画だと**一瞬ウィンドウが消え、フォーカスが別のウィンドウへ移り**、押しっぱなしの
    上下キーがそのウィンドウへ入力される。
  - **ウィンドウモード / 別ウィンドウモード / 別ウィンドウのフルスクリーンでは起きない。**
  - 併発: ランダムに**カーソルが mIV 上で非表示のまま**になる。次の画像 / 動画へ移ると戻る。
- 原因の見当 (コード確認済み、未確定):
  - フルスクリーン表示では静止画が **egui フルスクリーンビューポート**、動画が
    **別 HWND の native presenter** (D3D11 + DirectComposition)。静止画→動画の遷移で
    前者を `ViewportCommand::Visible(false)` で隠し ([ui_fullscreen.rs:12739](../src/ui_fullscreen.rs:12739))、
    後者を別途 materialize する。
  - **この症状は既に疑われていて計装がある**。同じ場所の直前に
    `log_main_flash_probe("fs_visible_false", …)` ([ui_fullscreen.rs:12735](../src/ui_fullscreen.rs:12735))。
  - [native_video.rs:2842](../src/app/native_video.rs:2842) のコメントが症状そのものを書いている:
    「foreground 奪還まで 80ms 待つと、**その間だけ外部ウィンドウが見えることがある**」。
  - つまり **viewport を隠してから presenter が foreground を取るまで、mIV のどのウィンドウも
    前面を持っていない**。Windows は z-order 上の次のウィンドウへ前面を渡すので、キーリピートが
    そちらへ流れる。報告と一致する。
  - 他モードで起きない説明も付く。ウィンドウ内動画はメインウィンドウの子として扱われ
    (`set_in_window_video_child`、[native_window_host.rs:536](../src/video/native_window_host.rs:536))、
    専用フルスクリーンビューポートと presenter HWND を入れ替える構造になっていない。
  - **カーソル非表示の固着も同根**。`cursor_hidden` はラッチで、解除は (a) egui フルスクリーン
    フレーム内で観測したポインタ操作、(b) HUD が出て `clean` が false になったとき、の 2 つだけ。
    presenter 側は `update_cursor_icon` で別経路の解決をしており、**カーソルの所有者が 2 つある**。
    次の項目へ移ると直るのは、そこで `open_fullscreen` がラッチを落とすため。
- **着手条件: 複数ウィンドウ / キー入力所有権の整理 (別 worktree、`docs/briefs/modifier-ownership-design.md`) の完了待ち。**
  症状パッチ (遷移前後の追加 `SetForegroundWindow`、遅延、キーリピート抑止) を入れない。
  問われているのは「viewer の遷移中、どのウィンドウが前面と入力とカーソルを所有するか」であり、
  1.100 や過去のキー所有権報告と同型。整理後に、遷移を**所有権の受け渡しが切れない 1 手**として
  設計し直す。
- 回帰確認の観点: 静止画→動画・動画→静止画の双方向、キー押しっぱなし中と単発、
  4 表示モード (フルスクリーン / ウィンドウ / 別ウィンドウ / 別ウィンドウのフルスクリーン)、
  遷移後のカーソル可視状態。
- 規模 / 優先度: Medium / P1 (実害あり)。**整理待ち**。
- **v3.2.0 からは外すと確定 (利用者判断 2026-08-22)。** 着手条件 (キー入力所有権の整理) が
  未達で、症状パッチを入れれば整理時に解きほぐす手間が増えるため。次リリースの筆頭候補として残す。

### 1.114 PDF の表示解像度合わせが毎フレーム自分を殺し合ってライブロックする

- 出典: 2026-08-21 の実機確認中に遭遇 (別ウィンドウで PDF の先頭 / 末尾へジャンプ)。
  症状は左下の「PDF 再レンダリング中...」が消えず、そのページの高解像度化が永久に完了しない。
- **今回の detached 修正が原因ではない。** 同じループが `mimageviewer.log.bak`
  (2026-08-20 12:09、detached 作業の着手前) に **2051 回**記録されている。
  別の PDF / 別の日 / 同じ機構。今日のログでは 93 秒間で **15883 回**。
- **機構 (ログで確定)**:
  1. ページを読み込み、保持ページキャッシュへ `long=1179` で格納する
  2. 表示解像度合わせが `target_px=1702` を要求 → キャッシュは 1179 なので
     `reason=request_pdf_rerender/resolution_mismatch` で miss
  3. [`request_pdf_rerender_at_target`](../src/app.rs:51535) が
     **進行中の再レンダを無条件にキャンセルして**新しいリクエストを投げる
     (「既に同じ (idx, target_px) が飛んでいるか」を見ていない)
  4. 呼び出し元の `ensure_pdf_display_resolution`
     ([ui_fullscreen.rs:13897](../src/ui_fullscreen.rs:13897) / [:13906](../src/ui_fullscreen.rs:13906)) は
     **毎フレーム**評価される
- PDF のレンダは数百 ms かかるのに、リクエストは約 6ms ごとに投げ直される。
  **どのリクエストも完了する前に次に殺される。**
  ログの裏付け: `pdf rerender done` が **1 回も出ておらず**、
  spawn された全スレッドが `pdf rerender: cancelled (channel closed)` で終わっている。
- **修正方針**: 「同じ `(idx, target_px)` のリクエストが既に in-flight なら投げ直さない」判定を入れる。
  時間窓ではなく、**進行中リクエストの identity** で判定する。
  in-flight を無条件に潰す現在の実装 ([app.rs:51535](../src/app.rs:51535) 付近) がライブロックの直接原因。
  - 副次的に確認すること: 1 回でも再レンダが完了すれば miss が解消するのか。
    今回は一度も完了していないため、要求解像度 1702 を満たす結果が返るかは**未確認**。
    dedup を入れた後に、完了した結果の long がまだ target を下回るなら別の原因が残っている。
- **⚠ 着手前に調整が要る**: 2026-08-20〜21 に master 側で PDF 周辺が活発に動いている
  (`f2c88357` 同時 open 数の上限 / `c817a0d3` 列挙の相乗り / §2.13〜§2.16)。
  同じ領域を触る別作業と衝突しないか確認してから着手する。
- 規模 / 優先度: Small / P1 (利用者の目に見える固着)。**別の PDF 修正が落ち着いた後に再確認する** (利用者判断 2026-08-21)。

### 1.112 360 度動画 (正距円筒) の 360° ビュー — 利用者要望

- 出典: 利用者要望 (2026-08-21)。静止画でできる 360° ビューを動画でも。報告者は
  insta360 / GoPro の素材を持っている。
- **現状で不可能な理由は「難しい」ではなく「投影を差し込む場所が無い」**。presenter は
  デコード済みフレームを動画解像度の swap chain へ 1:1 コピーし、拡大縮小は DComp が担当する
  ([video-upscale-shader-plan.md](video-upscale-shader-plan.md) §1)。シェーダが走るのは色調 /
  Creative LUT のときだけで、それも動画解像度で走る。
- **着手条件: §1.47 (動画の拡大縮小を mIV のシェーダで行う) が入ること。** §1.47 は
  swap chain を表示矩形サイズにし、シェーダでソース解像度から表示解像度へ解決する構造にする。
  **そのステージができれば、正距円筒投影は同じステージ上のもう 1 枚のシェーダ**になる。
- §1.47 が入った後に残る作業:
  1. **シェーダ移植**: 静止画側は WGSL ([panorama_wgpu.rs](../src/panorama_wgpu.rs))、presenter は
     実行時 HLSL コンパイル ([grade_pipeline.rs](../src/video/native_presenter/grade_pipeline.rs) の
     `D3DCompile`)。投影の数式はそのまま移せる。
  2. **ミップ生成**: 静止画側はフルミップ + `textureSampleGrad` で品質を出している
     ([panorama_wgpu.rs](../src/panorama_wgpu.rs))。毎フレーム 5.7K のミップ生成が現実的かは
     **実測してから決める** (代替は品質を落とした単純フィルタ)。
  3. **入力**: 見回しドラッグと FOV を presenter 側に持たせ、シークバードラッグ / ジェスチャ /
     HUD と共存させる。報告者の「動画には拡大縮小ドラッグの処理がなさそう」は正しい観察で、
     それを作るのが §1.47。
  4. **UI 導線**: 投影方式の選択・視野角・リセットを動画 HUD から出す。§1.105 (動画の「…」) と
     関係する。
- **自動判定は可能**。FFmpeg が `AV_SPHERICAL_EQUIRECTANGULAR` を出す
  ([spherical.h](../vendor/ffmpeg/include/libavutil/spherical.h))。回転メタデータと同じ扱いにできる。
- 規模 / 優先度: Large / P3。**§1.47 待ち**。

### 1.113 動画シークバー近傍のストリップを「サムネイル列 ⇄ 音声波形」で切り替える — 利用者要望

- 出典: 利用者要望 (2026-08-21)。動画視聴中に `Z` で出る音声波形を、シークバー近傍にも出して
  波形を手がかりにシークしたい。サムネイルと併せて場所を探せると便利、という趣旨。
- **§1.102 (YouTube 型のサムネイル列) と同じ場所を使う。** 2 つの UI を競合させず、
  **1 つのストリップのモード切替**にする。§1.102 の「シークバーから上へドラッグしたときだけ開く」
  導線をそのまま共有し、開いた中身をサムネイル列と波形で切り替える。
- データ面は好条件:
  - `TimelineAnalysis` ([crates/music-core/src/analysis.rs:186](../crates/music-core/src/analysis.rs:186)、
    bins + ビートグリッド) が**動画ファイルでも `Z` 波形モードで既に生成されている**。
  - 描画も `draw_music_timeline` のラスタキャッシュがある ([ui_music_timeline.rs](../src/ui_music_timeline.rs))。
  - **タイムライン全体あたりのコストはサムネイル列より安い**。1 回のデコードで全尺分の波形が
    得られるので、§1.102 が悩んでいる抽出方式 (1 枚 1 秒 vs 50ms) の問題が無い。
- コスト面で決めること:
  - **全尺の音声デコードが要る** (背景スレッド・progressive)。長い動画では埋まるまでの時間がある。
    途中経過をどう見せるか決める (progressive partial の既存挙動を流用できるか)。
  - **永続キャッシュを持たない設計** ([app.rs:8238](../src/app.rs:8238)「永続 DB はやめて直近 N 曲だけ
    メモリに載せる」)。開くたびに解析し直す。動画でも同じでよいかを判断する。
  - **ストリップを開いたときだけ解析を起動する** (常時ではない)。現状 `ensure_music_analysis` は
    音楽ビューが有効なときだけ呼ばれる ([app.rs:38518](../src/app.rs:38518)) ので、その条件を広げる形。
    常時起動にすると、動画を開くたび全尺デコードが走る。
- 規模 / 優先度: Medium / P3 (§1.102 と同時に設計する)。

### 1.115 別ウィンドウ表示で静止画を開閉すると、フルスクリーンと一覧が何度も入れ替わってちらつく — 利用者報告

> ⚠ detached viewer リワーク中の領域。**症状パッチ (delay / guard / 追加 repaint) を入れない**。
> BA-5 (gap フレームで passive 窓が破棄される) と BA-7 の境界にまたがる。

- 出典: 2026-08-23 (v3.2.0 公開後)。`g:\home\comfyui\202608-29_焼き肉レストラン` を開くと、
  フルスクリーンと一覧の切り替えが激しくちらつく。**ポータブル版では同じ操作で起きない。**
  これは初回比較時の観測で、その後タイトルバーから detached 窓を最大化するとポータブル版でも再現した
  (下記「最大化後の追試」で訂正)。
- **初回推定 (最大化後の追試で訂正)**: ポータブルで起きない理由は build flavor ではなく設定。ポータブルは既定設定なので
  フルスクリーンが別ウィンドウにならず、下の `detached_viewer_cleanup` 経路自体を通らない。
  `portable` feature が変えるのは native 依存の解決先と data_dir だけで、この経路に分岐は無い。
- **ログで確認できた事実** (`mimageviewer.log`、1 セッション 14 open / 10 cleanup):
  1. 別ウィンドウのフルスクリーンを閉じるたびに
     `[viewport] cleanup_visible_false: presentation=Some(DetachedWindow)` →
     `[ui-fonts] schedule main font atlas resync: detached_viewer_cleanup` →
     **連続 5 フレームの pass 破棄** (`discard pass for font atlas resync`、generation が
     +1 ずつ、`egui_frame` は連番)。実測 3.503s→3.761s = **258ms 分の描画が捨てられる**。
  2. この「cleanup 1 回 = discard 5 回」は **3 世代のログすべてで一定** (v3.1.3 期の
     `mimageviewer.log.bak` も 4 cleanup / 20 discard)。**5 フレーム破棄そのものは v3.2.0 で
     入ったものではない。**
  3. close の約 0.55 秒後、**キー入力の記録が無いまま `=== open_fullscreen ===` が再発火**し、
     別ウィンドウが**新しい HWND で作り直される** (3.341→3.903 / 37.311→37.927 /
     38.528→39.086)。再オープンは毎回**discard 窓が終わった 0.14〜0.17 秒後**に来ており、
     3 回とも同じ内部イベントからの一定オフセット。人が 2 度押ししたにしては揃いすぎている。
  4. `keys=Enter:up,Enter:up,Enter:up,Enter:up` のように**同一フレームに同じキーイベントが
     4 つ並ぶ行がある** (F12 も同様)。同じ入力が複数回配送されている。
- **コードで確認できた構造**:
  - `MAIN_FONT_ATLAS_RESYNC_REPEAT_FRAMES = 5` ([app.rs:392](../src/app.rs:392))。
    発行側のコメントが理由を明記している ([app.rs:37598](../src/app.rs:37598)):
    「close 直後の 1 フレームだけメインウィンドウの wgpu surface が消えて full upload が
    捨てられる。**数フレーム再発行して、surface が戻ったフレームで確実に届かせる**」。
    = **どのフレームで surface が戻るかを観測せず、時間窓で race を吸収している。**
    5 フレームぶんの描画破棄はその代償で、別ウィンドウ経路ではそれが目に見える。
  - `maybe_defer_for_main_font_atlas_resync` は 1 OS フレーム 1 発行にゲートされているので、
    5 回は必ず**別々のフレーム**を消費する ([app.rs:37855](../src/app.rs:37855))。
  - `should_defer_main_paint_for_font_atlas_resync(_reason) -> bool { true }`
    ([app.rs:3130](../src/app.rs:3130)) は**引数を無視して常に true**。reason で絞る余地が
    潰れている (縮退した経緯を先に読むこと)。
- **直す方向**:
  1. **surface が戻ったことを観測して 1 回だけ発行する。** 固定 5 フレームをやめる。
     no-surface early return の地点で「full upload を捨てた」ことを型で記録し、次に surface が
     ある フレームで 1 回だけ resync する。**時間窓で race を吸収しない** (この 5 フレーム窓は
     v1.8.0 の黒サムネ修正で入ったもので、同じ形が別の症状で再発している)。
  2. **再オープンの正体を先に確定させる。推測で直さない。** close 後に `open_fullscreen` を
     呼んだ呼び出し元と契機 (replay されたキーか、遅延していた要求か) をログに出す。
     事実 3 と 4 から「discard 窓中に同じキーが複数回配送され、main 側が Enter を
     もう一度 open として解釈している」が第一候補だが、**未確定**。
  3. 1 と 2 は独立に進められる。1 だけでも破棄フレームが 5 → 1 になり、ちらつきの大半は消える。
- **初回推定 (最大化後の追試で訂正): 再現条件は「設定」ではなく「実行時の状態」**
  (2026-08-23 追記、利用者の settings.db を読んで確認):
  報告者の永続設定は `detached_viewer_enabled=false` / `detached_viewer_open_images_in_window=false` /
  `fullfeature_media_window=true` / `video_in_window_mode=true`。**静止画を別ウィンドウで開く設定は
  どれも OFF** なのに、ログでは静止画 open のたびに `presentation=Some(DetachedWindow)` になっている。
  `requested_viewer_presentation_for_open` ([app.rs:43006](../src/app.rs:43006)) の 4 分岐のうち、
  永続設定によるもの (`detached_viewer_enabled` / `effective_media_in_media_window`= メディア限定) は
  すべて偽なので、**残る実行時分岐**しかない:
  `viewer_session_is_detached() && detached_viewer_independent_active` か、F12 が立てる一度きりの
  `detached_viewer_open_next_still_detached_once` ([app.rs:41528](../src/app.rs:41528))。
  → **環境設定を合わせても再現しない。F12 で作った独立 detached セッションが閉じても残っている状態が要る。**
  利用者がポータブルで同手順を試すと、閉じた後の再オープンで detached が維持されず
  `non_detached_viewer_presentation()` へ落ちた (= `video_in_window_mode=true` なので MainWindow)。
  **同じ操作で detached セッションの寿命が 2 環境で違う**こと自体が、この不具合と同じ所有権の問題。
- **追加観測の指定 (実施済み)**: `MIV_DETACHED_WINDOW_DEBUG=1` で起動すると `[ui-fonts][diag]` /
  `[detached-window-debug]` が `presentation` / `session` / `active_context` / `passive_windows` /
  `fullscreen_idx` を毎回出す ([app.rs:38773](../src/app.rs:38773))。**どの分岐で DetachedWindow に
  なっているかはこれで確定できる。** 再オープンの契機と併せて 1 回の再現で両方取れる。
- **最大化後の追試と訂正 (2026-08-23、Codex、ポータブル v3.2.0、
  `MIV_DETACHED_WINDOW_DEBUG=1`)**:
  1. **再現を可視化する条件はタイトルバーの最大化状態**。最初の通常サイズ open は
     `rect=(110,110 1462x1136)` / `window_id=20` / **16.1ms**。その窓をタイトルバーで最大化して
     close すると、settings.db は
     `detached_viewer_window_placement.maximized=true` を保存し、次の open は
     `rect=(-11,-11 3862x2110)` / `window_id=21` / **157.7ms**、その次も別 HWND の
     `window_id=22` / **147.6ms** になった。通常サイズの約 10 倍を UI thread の
     `pre_grid` で費やしている。builder が保存 placement の `maximized` を新 viewport へ適用する
     ([ui_fullscreen.rs:17331](../src/ui_fullscreen.rs:17331)) ためで、portable feature や F11 の
     borderless fullscreen は必要条件ではない。
  2. **1 close ごとに HWND / viewport identity を捨て、次の open で作り直している。** ログは毎回
     `cleanup_visible_false ... recreate=true` の後に host を clear し、generation を進める
     ([ui_fullscreen.rs:13083](../src/ui_fullscreen.rs:13083),
     [ui_fullscreen.rs:13111](../src/ui_fullscreen.rs:13111))。そのため最大化した新しい全面 HWND の
     生成・surface 初期化が、一覧との切替時にそのまま見える。
  3. close 後は既報どおり **5 回の font-atlas resync discard** が必ず走った。今回の 3 回は
     157〜161 = 229ms、162〜166 = 240ms、167〜171 = 231ms。つまり最大化 host の
     148〜158ms の再生成に加え、一覧側も約 0.23 秒を固定回数の race 吸収に使っている。
     **画像 decode は 10〜11msで完了しており主因ではない。**
  4. 上記「内部イベントによる自動再オープン」「同一 Enter の 4 重配送」は今回のログで訂正する。
     open 時の `input_seq` は 59 → 61 → 63。viewer の Enter close が 1 回 increment
     ([ui_fullscreen.rs:14144](../src/ui_fullscreen.rs:14144)) し、一覧の Enter open がさらに 1 回
     increment してから `open_fullscreen` を呼ぶ
     ([app.rs:35015](../src/app.rs:35015)) 契約と一致する。`[fs-key]` は fullscreen context の
     生存中しか一覧側 Enter を記録しないため、「キー行が無い」ことは内部 open の証拠にならない。
     4 個並んだのは `Enter:up` のみで、close/open を発火する `down` の重複は無かった。
- **確定した原因**: 最大化そのものは増幅条件で、根は次の 2 つの時間窓 / lifecycle 不整合。
  1. 明示 close が linked detached host を terminal teardown し、次の明示 open が保存済み
     `maximized=true` の全面 HWND / surface を同期的に新規作成する。
  2. teardown 後の main surface 復帰を観測せず、固定 5 フレームの discard で font atlas の
     full upload を再送する。結果として、利用者が Enter で viewer と一覧を往復するたび、
     「全面 host の作り直し」と「一覧側の 5 pass 破棄」が交互に露出する。
- **構造的な解消方針 (実装前に detached R4 / ゲート C で方式を確定する)**:
  1. font atlas は固定 5 回再送を廃止し、main surface の no-surface / acquire / present と
     full-upload 完了を既存 owner の typed state で acknowledgment する。surface が使える最初の
     1 pass だけ resync し、delay / retry / 追加 repaint で吸収しない。
  2. linked detached の content close と OS host lifetime を分離し、`DetachedWindowManager` が
     hidden host の再利用可否を所有する。再利用できる方式なら同じ `window_id` / HWND を
     hidden → content remount → visible と遷移させる。egui の制約で terminal teardown が必須なら、
     新 maximized host は HWND 作成・placement 適用・surface/content-ready acknowledgment が揃うまで
     hidden のまま保持し、`Visible(true)` を 1 回だけ発行する。App-level bool / Option、rect heuristic、
     固定待ち時間は追加しない。
  3. `maximized=false` への強制リセット、最大化 placement の保存停止、windowed 強制は機能劣化なので
     修正に使わない。タイトルバー最大化と F11 borderless は別状態のまま維持する。
- **回帰条件**: 保存 placement が `maximized=true` の linked 静止画窓で Enter close/open を反復し、
  (a) 1 操作につき visibility 遷移が 1 回、(b) 再利用方式なら `window_id` / HWND が不変、teardown
  方式なら ready 前に visible にならない、(c) main font atlas が surface acknowledgment 後の 1 回だけ
  full upload、(d)通常サイズ・F11・folder-nav reopen・always-new / passive / ParkedLive の所有権を
  変えないことを state-transition test + debug log smoke で固定する。実装時は
  [detached-rework-plan.md §11](detached-rework-plan.md#11-リワーク外の変更ログ) に合意と変更境界を記録する。

#### 追記 (2026-08-23): 最大化が再現条件、原因は placement の毎フレーム書き戻し

`MIV_DETACHED_WINDOW_DEBUG=1` で取り直したログで確定した。**ポータブル版でも、別ウィンドウを
タイトルバーで最大化すれば再現する。**

- **訂正**: 前段の「キー入力の記録が無いまま再オープンしている」は**誤りだった**。`[key-debug]` を
  出すと、open も close も**すべて実際の Enter 押下**である (`raw main down vk=0x0D` →
  main 窓なら `pressed GridOpenSelected [Grid]`、detached 窓なら `consume FsClose [FsImage]`)。
  非 debug ログの `[fs-key]` はフルスクリーン viewport が見たキーしか出さないので、main 窓の
  Enter が「無い」ように見えていた。**出ていないログを根拠に推論していた。**
- **新しい事実**: **最大化された別ウィンドウが表示されている間、placement 更新が毎フレーム走る。**
  1 セッションで `runtime_placement reason=active_placement_update_maximized` が **82 回**、
  そのすべてが `from == to` (中身が変わらない書き込み)。同数の
  `placement_trace event=builder_no_position` が対になっている。**82 フレーム = 別ウィンドウが
  開いていた全フレーム**で、1 フレームも休んでいない。
- **構造**: maximized 分岐 ([app.rs:43782](../src/app.rs:43782)) は**実測した `outer_rect` を捨てて**
  `detached_window_seed_placement()` (= restore 用の w/h) を書き、`maximized=true` だけ立てる。
  実測 `rect=(-11,-11 3862x2110)` に対し placement は `w:2560, h:1369.33` のまま。
  つまり `DetachedViewerWindowPlacement` 1 個に**「今のサイズ」と「元に戻したときのサイズ」が
  同居**していて、最大化中は前者を記録する場所が無い。だから書き込みは**永久に収束しない**
  (毎フレーム同じ値を書き直す)。終端状態が無いという意味で
  v3.1.2 の `nav/archive_cache_peek` 退行と同型。
- **Codex の §2.20 分析と根が同じ。** あちらは「snapshot bake が placement の w/h で画像矩形を
  正規化するのに、passive draw は最大化 viewport の `full_rect` へ X/Y 独立で戻す」ことを
  指摘している。**症状 (縦横比の圧縮 / ちらつき) は違うが、原因はどちらも「最大化中の placement が
  実ジオメトリを表していない」こと**。片方だけ直しても、もう片方の入力は歪んだまま残る。
- **解消方法** (A が本命、B は A の一部として自然に消える):
  - **A. 現在ジオメトリと restore ジオメトリを別の場所に持つ。** maximized のとき実測 rect を
    捨てない。restore 用 seed は maximize に入った時点の値として別に保持する。これで §2.20 の
    bake / draw も同じ数字を見られるようになる。
  - **B. 変化が無ければ書かない。** 現状 82/82 が no-op。A を入れれば「実測値が変わったときだけ
    書く」が自然に成立する。**変化検出を後付けの guard として足すのではなく、A の帰結にする。**
  - **C. font atlas resync の 5 フレーム再発行を、surface 復帰の観測に置き換える** (上記の直す方向 1)。
    A/B と独立。close 後の破棄が 5 → 1 になる。
- **検証の観測点**: 最大化した別ウィンドウを開いている間、①`runtime_placement` が毎フレーム出ない
  こと ②`builder_no_position` が毎フレーム出ないこと ③close 後の `discard pass` が 1 回に
  なること ④最大化 → 復元でウィンドウが元のサイズに戻ること (A で restore 値を壊していない証明)。

#### Codex クロスチェック: 2 つの欠陥を同一原因として扱わない

ClaudeCode の追加ログと Codex の portable / normal ログを併記して source まで照合した結果、
**最大化が再現を可視化する条件**、**open / close は実入力**、**placement が毎フレーム同値更新される**
という観測は一致した。ただし、ちらつきへの因果は次のように分ける。

- `active_placement_update_maximized` の `from == to` は runtime manager 内の
  `runtime.placement = Some(placement)` であり
  ([detached_window_manager.rs:538](../src/app/detached_window_manager.rs:538))、viewport command、
  generation 更新、visibility 更新、repaint 要求を発行しない。ログの `builder_no_position` も
  `apply_placement=false`、すなわち生存中の窓へ position / maximize を再適用していない。
- settings への永続化は毎フレームではなく、runtime を remove する close 境界で 1 回だけ行う
  ([app.rs:2174](../src/app.rs:2174))。したがって「82 回の同値更新」だけから
  **ウィンドウが82回再表示された**、または**それがちらつきの直接原因**とは結論できない。
- 一方、画面遷移と一対一に対応する事実は、各 close の `recreate=true` + 5 discard と、各 open の
  **新 `window_id` / 新 HWND / 113〜177ms の最大化 host 生成**である。よって §1.115 の直接原因は
  上記「確定した原因」の lifecycle + surface acknowledgment 欠如とする。
- 現在 geometry と restore geometry を1値に混在させる placement model は、§2.20 の座標破綻、
  不要な runtime 書き戻し、将来の placement command 振動を招く**独立した構造欠陥**であり、
  ClaudeCode の A/B は同じ R4 設計で直す価値がある。ただし A/B だけ適用しても close ごとの
  host 再生成と 5 discard が残る限り、§1.115 の完了条件は満たさない。
- 実装レビューでは (1) placement A/B のみで HWND 再生成 / discard が残るケース、
  (2) lifecycle + surface acknowledgment 修正で最大化 placement を維持するケースを分けて測り、
  「ログ量が減った」ことを「ちらつきが直った」ことの代用にしない。

- 規模 / 優先度: 中 / **P1** (常用操作で目に見える。ただし凍結領域なので構造修正が前提)。
- 関連: §2.20 (**同じ根**。最大化中の placement が実ジオメトリを表していない)、
  [docs/detached-rework-plan.md](detached-rework-plan.md) の BA-5 / findings-12 D1
  (font resync の discard パスが passive 窓を破棄した実害) / findings-14
  (毎フレーム seed による振動を `builder_placement_latch` で止めた前例)。

### 1.116 メインウィンドウの起動状態を選べるようにする — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。ZipPla からの移行を検討している利用者が、終了時のウィンドウサイズ復元、
  最大化状態の維持、最大化起動の設定が見つからないと外部SNSへ投稿した。直接受けた実装要望では
  ないため、需要候補として記録する。
- **現状の整理**: 通常ウィンドウの位置とクライアントサイズは既に `window_pos` / `window_size` へ
  保存し、起動時に復元している ([lib.rs:1124](../src/lib.rs:1124),
  [app.rs:54576](../src/app.rs:54576))。最大化中は restore 用の通常矩形を壊さないため
  `track_window_rect` の更新対象外だが、**最大化して終了したという状態自体は保存していない**。
  そのため「サイズ復元なし」ではなく、「次回も最大化」と「常に最大化で起動」が不足している。
- 仕様候補: 環境設定へ `起動時のウィンドウ状態 = 前回の状態 / 通常 / 最大化` を追加する。
  現行互換の既定値は `通常`。restore 用の通常位置・サイズと、終了時の最大化 flag を別々に持つ。
  必要性があれば `--window-state normal|maximized|restore` も同じ resolver へ通す。
- 実装条件:
  - `pending_initial_size` の mixed-DPI 起動補正が、最大化適用後に通常サイズへ戻さない順序にする。
  - 最小化終了は前回の有効な通常矩形を維持し、起動時に最小化は復元しない。
  - 設定 round-trip と起動状態 resolver の unit test に加え、複数モニター / 異なる DPI /
    トレイ終了で restore rect と最大化状態を手動確認する。
- 規模 / 優先度: Small (0.5〜1.5日程度) / P2。コード量より Windows / DPI 実機確認が主なリスク。
- **実装済み (2026-08-25、実機確認待ち)**:
  - `Settings.startup_window_state` (`Normal` 既定 / `Maximized` / `RememberLast`) と、
    終了時の最大化 flag `Settings.window_maximized` を追加。flag は復元矩形
    (`window_pos` / `window_size`) と**別フィールド**にして、最大化を解いたときの
    戻り先を残す (detached 側 §1.115 と同じ根を作らない)。
  - `resolve_startup_maximized` で起動状態を決め、`ViewportBuilder::with_maximized` へ渡す。
    初回フレームで `ViewportCommand::Maximized` を送る形にすると通常サイズが一度見えて
    ちらつくため、生成時に指定する。`--window-size` は設定より優先して通常ウィンドウ。
  - `pending_initial_size` の mixed-DPI 補正は、最大化起動では**最大化が解けるまで保留**する
    (`deferred_initial_size_ready`)。egui が「最大化ではない」と明示報告した最初のフレームで
    1 回だけ流し、そこで復元矩形を矯正する。報告が無い `None` を「最大化ではない」と
    読まないこと自体が条件。
  - 追跡側 (`tracked_window_maximized`) は最小化中と報告欠落時に直前の状態を保つ。
    Windows は最小化中の `GetWindowPlacement().showCmd` を `SW_SHOWMINIMIZED` にするため、
    素直に読むと最大化して最小化しただけで flag が落ちる。
  - 環境設定のページ名を「起動時に開く場所」→「起動時の動作」に変更し、同ページへ
    「起動時のウィンドウ状態」節と検索索引エントリを追加。
  - unit test: 起動状態 resolver / 設定 round-trip / 未知値の既定落ち / 環境設定 OK が
    選択を巻き戻さないこと / 補正の保留条件 / 最小化中の追跡 / 保存が復元矩形を触らないこと。
  - **既定は `RememberLast`** (2026-08-25 利用者判断)。v3.2.0 以前の設定には field も
    `window_maximized` も無いため、更新直後の初回起動は通常ウィンドウのまま。次に最大化して
    終了したときから効き始めるので、更新した瞬間に驚かせない。
  - 既定の変更なので `version_highlights.rs` へ `must_read` を追加済み。
    **⚠ 版数は暫定 `"3.3.0"`。次のリリース版数を決めるときに合わせること** (リリース手順 Phase 1)。
  - 実機確認済み (2026-08-25): 設定 UI / 検索、前回状態の復元と解除後のサイズ、別 DPI モニター、
    最小化終了、トレイ終了、通常ウィンドウ。

### 1.117 外部ツール連携を設定画面へ出し、引数・複数選択へ拡張する — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。移行元の ZipPla / NeeView にある外部ツール設定が mIV に無いとの外部SNS投稿。
  直接受けた実装要望ではないが、既存機能の発見性不足と機能差の両方があるため候補として記録する。
- **現状の整理**: 右クリックの「アプリケーションで開く…」から Windows 関連付けアプリと任意 exe を
  追加・実行できる ([context_menu.rs:2037](../src/ui_dialogs/context_menu.rs:2037))。ただし登録場所が
  コンテキストメニュー内だけで見つけにくく、実行は `exe + 物理ファイル1件` 固定
  ([open_with.rs:142](../src/open_with.rs:142))。引数、作業フォルダー、複数選択、ZIP / PDF 内ページ、
  任意ツールの直接キー割り当ては未対応。動画の <kbd>Shift+Enter</kbd> は OS 既定アプリを開く別経路。
- 段階案:
  1. 環境設定の「起動と連携」に外部ツール管理を追加し、既存の任意 exe 登録を改名・並べ替え・削除
     できるようにする。操作カスタマイズにはまず動的 action を増やさず、「外部ツールを選んで開く」
     という汎用 picker action を追加する。
  2. `ExternalToolDefinition { id, name, executable, arguments, working_directory, selection_mode }` の
     typed 定義へ移行し、`{file}` / `{folder}` / `{files}` 等の明示 placeholder と複数物理項目を扱う。
  3. ZIP / PDF / 変換アーカイブ内ページを対象にする場合は、一時実体化、キャンセル、外部プロセスが
     読み終えるまでの lifetime、終了後 cleanup を別 phase で設計する。
- 安全性 / 応答性:
  - コマンド文字列を `cmd /c` へ渡さず、exe と `OsString` 引数列を分離して `Command` を使う。
  - ネットワーク上の exe / 作業フォルダー確認や仮想ページ抽出を UI スレッドで行わない。
  - 起動失敗を黙って捨てず、ツール名と OS error を通知する。
- 規模 / 優先度: 管理UIのみ Small〜Medium (1〜2日)、物理項目の引数・複数選択まで Medium
  (3〜6日)、仮想ページ込み Large (追加1〜2週間) / P2。
- **段階 3 まで一度に出す (2026-08-25、利用者判断)。** 段階 2 で切って出す案は採らない:
  - 段階 2 までだと現状 (実ファイル 1 件を関連付けで開く) から用途があまり広がらない。
  - **設定画面を一度出すと、それが互換の約束になる。** `selection_mode` と `{file}` の
    意味は仮想ページが入ると変わるので、後から段階 3 を足すと「同じ設定なのに挙動が違う」
    形になる。設定の形を決めるのは仮想ページまで見えてからにする。
  - 出典は要望ではなく不満の声なので**優先度は高くない**。急いで段階を刻む理由が無い。

### 1.118 任意の実ファイルを参照だけで束ねる「コレクション」 — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。ZipPla / NeeView からの移行に「仮想ディレクトリ」が不足するとの外部SNS投稿。
  直接受けた要望ではなく、相手が期待する詳細操作は未確認。ZipPla の仮想フォルダや NeeView の
  プレイリストに相当する、手動で選んだ分散ファイルの参照一覧を想定して候補化する。
- **既存機能との差**:
  - スマートフォルダは複数 root と条件から毎回結果を作るため、ZipPla の「スマートフォルダ」相当。
  - 製本は追加時点のページを実ファイルとしてコピー / 焼き込みするため、元ファイルへの参照一覧ではない。
  - 任意項目を手動登録し、元の場所に置いたまま1フォルダのように見せる機能は無い。
- MVP 案:
  - 内部名も ZIP / PDF の `virtual folder` と衝突させず、利用者向け名称を「コレクション」とする。
  - `TopLevelGridSurface::Collection` を追加し、専用 DB worker が
    `CollectionDefinition` / `CollectionEntry { id, collection_id, source_path, order }` を所有する。
    設定 JSON の巨大配列や UI thread の同期存在確認にはしない。
  - 対象はまず実フォルダ / 実ファイル / アーカイブ本体のみ。コレクション入れ子と ZIP / PDF 内の
    仮想ページは対象外。追加、手動順序、コレクションから外す、元の場所を開くを提供する。
  - 「コレクションから外す」と「元ファイルをごみ箱へ移す」を明確に別操作・別確認にする。
- identity / lifecycle:
  - mIV 内の rename / move は既存 path migration の同じ transaction から entry を更新する。
  - OS 側で移動された項目は推測で別ファイルへ結び付けず、missing 表示と削除 / 再リンクを提供する。
  - 外部から任意パス一覧を読み込む機能は、UNC への意図しないアクセスや他人由来リストの危険があるため
    MVP に含めない。追加する場合は明示 import と確認を別途設計する。
- 規模 / 優先度: 物理項目だけの MVP Medium〜Large (1〜2週間)、仮想ページ・移動追跡・完全な
  D&D 管理まで Large (2〜4週間) / P3。着手前に利用者が期待する「仮想ディレクトリ」の操作例を確認する。

### 1.119 横長画像1枚を左右の表示ステップへ分割して読む — 外部SNSでの移行検討者の指摘

- 出典: 2026-08-24。ZipPla / NeeView からの移行に「見開き分割」が不足するとの外部SNS投稿。
  NeeView の「横長ページを分割する」(横長画像を左右へ分けて順番に読む) に相当する機能を想定する。
- **採用する簡略仕様 (2026-08-24)**: 分割した左右を永続的な論理ページにはしない。元の item index を
  正本のまま維持し、フルスクリーン内だけで `PageSlice::Full | Left | Right` に相当する一時的な表示位置を
  持つ。同じ texture の UV を左右半分へ crop し、1つの元ページを2回の表示ステップとして読む。
- 設定 / 分割判定:
  - 既存のページ構成プルダウンへ排他的な表示モードとして「横長分割 左→右」「横長分割 右→左」を
    追加する。「1ページ表示 / 通常の見開き」と組み合わせる独立 bool にはせず、組み合わせ状態を増やさない。
  - 保存済み回転を反映した後に横長となる静止画ページだけを 50% 位置で分割する。左→右は左半分から、
    右→左は右半分から表示する。分割率の手動調整は MVP に含めない。
  - 自動表示トリムは分割対象ページでは無効にする。分割の適用条件と UV crop は1つの resolver に集約し、
    ページ送り、描画、ナビゲータ、ルーペ等で左右の解釈を重複させない。
- 元ページ単位のまま維持する機能:
  - ★、タグ、ブックマーク、読書位置、補正、注釈、切り取り等はすべて分割前の元ページへ記録する。
    編集画面も分割前の画像全体で開き、左右別の編集データは持たない。
  - サムネイルは分割前の画像を使う。ブックマーク、履歴、検索、シークバー等から再び開いた場合は、
    選択した分割方向の最初の半分へ着地する。左右どちらを見ていたかは永続化しない。
  - シークバーのページ数とノブ位置は元ページ単位のままとし、左右間の移動では変えない。スクラブ時も
    対象ページの最初の半分へ着地する。表示中だけ「12ページ・左側」のように片側を示して混乱を避ける。
- ナビゲーション / 連結読み:
  - 通常のページ送りは `(source_idx, PageSlice)` の typed な一時状態を1か所で進める。分割状態を複数の
    bool / `Option` へ分散させず、同じ元ページ内の左右移動と次の元ページへの移動を区別する。
  - 縦連結では、分割対象ページを同じ texture 由来の2つの crop 領域として縦に並べる。幅フィットで
    スクロール読みする用途を対象とする。横連結と通常の見開き表示には MVP では分割を適用しない。
- **現状の構造差**: 現在の `SpreadDisplayUnit` は物理 item index だけを単位とし、ページ送り、通過表示、
  表示確定も `fullscreen_idx` の変化を前提にする ([ui_fullscreen.rs:8169](../src/ui_fullscreen.rs:8169))。
  元 index が同じ左右移動でも表示変更として扱う typed presentation step と、縦連結の layout-only な
  2領域展開が必要。ただし seek / resume / bookmark / DB / サムネイルを論理ページ化する必要はない。
- detached 制約: 別ウィンドウ経路へ到達する前に [detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  owner 外へ分割状態を足す症状修正にしない。通常の本体フルスクリーンを先に完成させ、detached は R4 後に
  同じ presentation step を所有できる場合だけ拡張する。Remote は当初 MVP 対象外としていたが、
  **スマートフォンの縦画面で一番効く**ため 2026-08-26 に実施した ([web-remote-plan.md](web-remote-plan.md) §15)。
- 回帰条件: 左→右 / 右→左の往復、横長と縦長の混在、回転後の分割判定、先頭 / 末尾、キー長押し、
  縦連結、シーク、編集画面への出入り、ブックマーク再表示で元 index と表示片の不変条件を固定する。
- 規模 / 優先度: 通常ページ表示 Medium、縦連結まで含めて Medium〜Large (1〜2週間程度)。当初の
  logical page 全面対応 (2〜8週間想定) より範囲を大きく限定できる / P3。

**実装プランは [page-split-plan.md](page-split-plan.md)** (当たりを付けた結果・範囲の根拠・
次に触る場所)。以下は着手時の記録。

#### 純ロジックだけ先行 (2026-08-25、[page_split.rs](../src/page_split.rs))

配線はレーン A の `app.rs` 一括切替の後に回す
([next-cycle-work-lanes.md](next-cycle-work-lanes.md) §6.2)。待つ間に、共有ファイルを
触らない部分だけ作った。単体テスト 9 本。

- `PageSlice { Full, Left, Right }` + `uv_rect()` (50% 固定) + `is_half()`
  (自動表示トリムを無効にする条件でもある)
- `SplitDirection { LeftFirst, RightFirst }` の `first()` / `second()`
- `PresentationStep { source_idx, slice }` — **元 index が正本**で、slice は表示だけ
- `presentation_steps(nav, direction, is_split_idx)` — nav をステップ列へ広げる。
  **既存の見開きユニット生成と同じ述語渡しの形**にした。回転を反映した縦横比と
  「静止画か」は `is_landscape` / `is_spread_pairable_item` が既に持っているので、
  ここで読み直さない。寸法が未取得の item は 1 ステップになり、届いた後に組み直される
  (既存の見開きが `is_landscape` に対して持つ性質と同じ。分割のために読み込みを待たせない)
- `landing_step` — しおり / 履歴 / 検索 / シークから**分割方向の最初の半分**へ着地
- `step_forward` / `step_backward` → `StepMove::{WithinPage, ToAnotherPage, AtEnd}`。
  同じ元ページ内の左右移動と別ページへの移動を**型で**区別する (呼び出し側で
  `before.source_idx != after.source_idx` を組み立てると判定が散る)
- 縦連結も同じステップ列を使う。「同じ texture 由来の 2 領域を縦に並べる」順序は
  ページ送りの順序と同じものなので、別の列を作らない
#### モードと per-viewer 状態まで (2026-08-25、レーン A マージ後)

- `SpreadMode::SplitLtr` / `SplitRtl` を追加。**`all()` には入れていない**ので
  プルダウンには出ない (選べるのに何も起きない状態を master へ置かない)。
  見開きとは排他で、`is_spread` / `is_rtl` / `has_cover` はすべて偽のまま。
- `App::fullscreen_page_slice` + bundle 側 + `swap_field!` を追加。**元ページと同じ
  context 所有**にした。片方だけ App に置くと、context を切り替えたときに前の viewer の
  左右が次の viewer に残る。`viewer_context_audit` は通過 (既知 1 件のみ)。
- **回転したページも分割する** (利用者判断で最初のリリースに含める。回転したページだけ
  分割されないのは、使う側からは不具合にしか見えず報告が来る)。
  - 当初は「型付き `SourceCrop` へ作り替えるしかない (215 箇所)」と見立てたが、**外れ**。
    実際の原因は `DisplayedImageTransform::resolve` が**同じ矩形を 2 つの座標系として
    使っていた**こと (fit 倍率では回転後の `display_size` に掛け、UV では元画像空間の
    まま渡す)。回転すると両立しないので丸ごと捨てていた。**UV 側は元々正しかった。**
  - 修正は用途ごとに座標系を分けるだけだった。`rotate_bbox_to_display` を足し、fit と
    paint rect は表示空間、UV は元画像空間にした。写像は screen ↔ source と同じ
    `forward_uv` を使う。**`content_bbox` の型も 215 箇所も動かしていない。**
  - `effective_bbox` が降ろすのは**自由回転中だけ**になった (傾いた矩形の外接が広がる分の
    拡大量を解けないため)。自由回転は保存しない一時値なので、やめれば戻る。
  - 旧テスト `fit_rotation_and_trim_share_paint_and_hit_geometry` は「回転時 UV は全体」を
    固定していた。**制限を仕様として固定していたテスト**なので期待値を更新した。
  - 詳細は [page-split-plan.md](page-split-plan.md) §2、正本は
    [display-pipeline.md](display-pipeline.md)「部分矩形 (content bbox) の座標系」。
- **表示トリムはまだ回転ページで効かない** (2026-08-25 訂正。一度「効くようになった」と
  書いたが誤り)。変換側で扱えるようになっただけで、**トリムを作る側に同じ規則の複製が
  4 か所残っている** (`capture_fs_display_unit_*` の単ページ / 見開き、Z ズーム、連結読み)。
  そのコメントは「描画側が bbox を使わない**ので**」と消費側を理由に挙げており、対の片方
  だけが変わった状態。**外すのは別件** —— 見開きの左右そろえが表示空間で定義されていて、
  180 度回転で左右が入れ替わるページに素直には適用できない。分割は自前の resolver
  (`fs_page_content_bbox`) から矩形を出すのでこのガードを通らない。
- **ページ送り / 描画 / 着地 / 縦連結まで実装済み** (2026-08-25)。通常表示は実機確認済み。
  - 描画の解決は `draw_fs_image` が所有する。最初は呼び出し側で解決させていて、
    `content_bbox` を作る 6 か所のうち**通常表示の 1 か所を通し忘れ**、ページ送りだけ
    半分ずつ進んで絵は横長のまま、という状態を実機で出した。数えて塞ぐのではなく、
    実際に描く 1 か所が所有する形へ変えた。
  - 縦連結は同じステップ列から段を組む。現在位置は左右まで見て選ぶ (同じ元 index の段が
    2 つ並ぶため)。連結の寸法計算にも `content_bbox` と座標系のずれがあり、同じ形で直した。
  - **スクロールで段が変わったときに左右を書く人がいなかった** (実機で発覚)。現在位置が
    毎フレーム元の段へ戻され、スクロールが引き戻されて先へ進めなくなっていた。
    表示は正しく入れ替わっていたので、**描画は合っていて現在位置の追従だけが
    取り残されていた**形。`reanchor_continuous_reading_viewer` が `fullscreen_idx` と
    同じ場所で左右も書くようにした。
  - **同じ形の取りこぼしを 2 回踏んだ**: 「元 index は変わらないが表示位置は変わった」
    という状態を、元 index しか見ていない既存コードが取りこぼす。1 回目は描画側
    (`content_bbox` の producer)、2 回目は現在位置側。分割のように**既存の識別子を
    細分化する機能**では、その識別子を読んでいる場所を先に数えるべきだった。
- **「分割を選んだのにページがつながったまま」の実機報告を解決** (2026-08-25)。
  再現手順が分からなかったので、**先に理由を型で残す計装を入れた** (`SplitDecision`)。
  次の再現で `[split] idx=0 decision=dimensions_unknown` が出て、そこから確定した。
  - `is_landscape` は寸法が未取得でも `false` を返す。見開きのペアリングでは困らないが、
    分割では「まだ分からない」と「縦長」が別の意味を持つ。探索と判定を分けた。
  - 真因は**収穫側**。PDF を開き直すと items 世代が変わり `page_dims_cache` が失効する。
    そのページは retained composite から復元されて表示できるので `fs_cache` にも
    サムネイルにも載らず、**供給源がゼロ**になっていた。寸法自体は retained 側が
    持っている (ログに `source=1512x1921` と出ていた)。
  - `harvest_page_dims_from_fs_cache` → `harvest_page_dims` に変え、**表示できる 2 経路**
    (live cache / retained composite) の両方から収穫するようにした。
- **残り**: 縦連結の実機確認、製品ページへの追記、リリース時の更新履歴。

### 1.120 Susie 32bit ワーカー異常終了後の自己回復 — 外部SNSでの不具合言及 ✅ 実装済み (2026-08-25、実機確認済み)

- 出典: 2026-08-24。mIV の Susie 中継 exe が繰り返しエラーで落ちるとの外部SNS投稿。
  プラグイン名、対象画像、ログ、並列設定は不明で再現未確認。直接の報告ではないため、まず条件採取が必要。
- **コード上確認できた問題**:
  - Susie プラグインを32bit子プロセスへ隔離しているため、プラグインの access violation 等で本体まで
    落ちない設計は維持されている。
  - 起動成功後に worker が想定外終了しても自動 respawn しない
    ([async-architecture.md:757](async-architecture.md#54-pdf-ワーカー--susie-ワーカーの想定外終了))。
  - `run_dispatcher` は `send_recv` の transport error 後も loop を抜けないため、死んだ pipe の dispatcher が
    共有 queue から後続 job を取り、エラーを返し続ける可能性がある
    ([susie_loader.rs:794](../src/susie_loader.rs:794))。
  - README の「プラグインクラッシュ時は自動再起動」という説明は現行実装と食い違う。実装を合わせるまで
    説明を訂正するか、既知の問題として明示する。
- 修正方向:
  - worker slot ごとの generation と状態 (`Starting / Ready / Failed / Backoff / Shutdown`) を持つ
    `SusieWorkerSupervisor` が child / dispatcher の lifetime と実効 worker 数を所有する。
  - EOF / BrokenPipe 等の fatal transport error ではその dispatcher を即座に退役させ、残り queue を
    奪わせない。回数上限と backoff を付けて同じ slot を再 handshake する。
  - クラッシュを起こした in-flight request は自動再送しない。同じ不正画像 / プラグインで crash loop に
    なるため、その item はエラーとして返し、後続 request のためだけに worker を補充する。
  - 通常のプラグイン decode error と worker transport failure を型で分け、診断画面へ plugin / extension、
    worker id、再起動回数、最終失敗理由を出す。並列実行 OFF で回避できる plugin-global race も案内する。
- 回帰: handshake 後に意図的に exit するダミーワーカーで、in-flight 1件だけが失敗し、後続は再生成された
  worker で成功すること、shutdown / cancel と respawn が競合しないこと、再起動上限後は queue を
  無限消費せず persistent notice になることを統合テストする。
- 規模 / 優先度: Medium (3〜7日) / P2。利用者のプラグイン名・画像・ログが得られれば優先度を再判断する。

#### 実機で再現させた結果 (2026-08-25)

検証手段が無かったので、**落ちるプラグインを自作した** ([susie-crash-plugin.md](susie-crash-plugin.md))。
`C:\tmp\miv-susie-crash-test` の 8 ファイル (正常 5 / クラッシュ 3) を一覧で開いた結果:

- **ワーカーはクラッシュのたびに 1 つずつ減り、再生成されない。** 起動時 3 本
  (`susie: 3 workers ready`) が、クラッシュ 3 回で **0 本**になった。この間 mIV 自身は動作を続けている。
- 枯渇後は Susie 形式が**一切読めない**。利用者からは「しばらく使っていたら Susie が全部読めなく
  なった」に見える。**アプリを再起動するまで戻らない。**
- 一覧の初回表示では 8 枚中 6 枚が成功した。生き残っているワーカーが処理できたためで、
  **1 回のクラッシュで全滅するわけではない**。枯渇するまでは部分的にしか見えない。
- 表示が残っていた 1 枚はサムネイルキャッシュ由来 (`state=DisplayReady(LiveCache)`) で、
  キャッシュが切れると失敗に変わった。

**この結果を受けて段階分けを変更する。** 当初案の「段階 1 = dispatcher の退役だけ、respawn は後」
では**直らない**。dispatcher を退役させてもワーカーは減り続け、枯渇後の結末は同じである。
respawn までを 1 つの修正として扱う:

1. transport error でその dispatcher を退役させ、共有キューを吸わせない
2. **backoff 付きで同じスロットを再起動する** (本命。これが無いと枯渇は止まらない)
3. クラッシュを起こした要求は**再送しない** (同じ不正画像で crash loop になる)。その 1 件は
   エラーで返し、後続のためだけにワーカーを補充する
4. 再起動上限に達したら無言で諦めず、利用者へ通知する

**副産物: フルスクリーンが Susie 拡張子を認識しない** (§2.21 として分離)。

#### 実装 (2026-08-25、実機確認済み)

`af6882e5` + `9ecbbec7`。4 つの欠陥を閉じた。**順に実機で確かめ、直すたびに次が現れた**ので、
その経緯も残す。

1. **dispatcher が終了理由を返す** (`DispatcherExit::{Shutdown, WorkerLost}`)。以前は
   `shutdown` でしか抜けず、死んだ pipe を持つ dispatcher が共有キューから後続を取り続けた。
   しかも失敗は即座に返るので、生きているワーカーより速く吸っていた。
   落ちたと扱うのは transport が切れた場合だけで、`InvalidData` 等は含めない。
2. **スロットが backoff 付きで作り直す** (`run_worker_slot`)。上限 5 回、200ms × 試行回数。
   待つのは競合を時間で隠すためではなく、crash loop が CPU を焼き切らないため。
3. **再起動回数は連続失敗で数える** (`restart_count_after_loss`)。累積で数えていたため、
   実機では 3 スロットとも上限に達して Susie が丸ごと死んだ。1 件でも応答を返せた
   ワーカーは働けていたので数え直す。上限が意味を持つのは「起動しても何も返せない」場合だけ。
4. **最後のスロットが閉じたらキューを排出する** (`fail_pending_jobs_no_workers`)。
   これが無いと積まれた要求に誰も答えず、UI が「読込中」で固着した (実機で確認)。
   投入の可否は `is_ready()` の外側チェックではなく**積むのと同じロックの中**で見る。
5. **ワーカーを落とした対象は二度と投げない** (`crashers`)。失敗したデコードはサムネイルに
   残らないので、記録しないと**同じフォルダを開くたびに同じ画像でワーカーが死に続ける**。
   これが 1〜4 だけでは止まらなかった殺し合いの原因。記憶はプールの生存期間だけで、
   起動し直せば忘れる。実ファイルは正規化パス、書庫内は「エントリ名#バイト長」で識別する
   (名前だけだと別書庫の同名エントリを巻き添えにする)。
- `is_ready()` は起動時の本数ではなく**生存スロット数**を見る。全滅しても真を返していた。
- 実機結果: 5 回死んで 5 回とも復帰、スロット喪失 0、上限到達 0。クラッシュする 3 ファイルは
  各 1 回だけ記録され、残る 5 ファイルは何度開き直しても読める。
- README とマニュアルの「自動再起動されます」は**実装が追いついた**形。マニュアルには
  利用者から見える 2 点 (クラッシュした画像は再試行しない / 何度も落ちる場合は打ち切る) を追記。
#### 通知と診断表示 (2026-08-25、実機確認済み)

残っていた 2 点を閉じた。

- **枠が尽きたことを利用者へ伝える**。`SusieWorkerNotice` を one-shot で発行し、
  PDF の `PdfWorkerNotice` と同じ形 (毎 update poll → 閉じるまで残る Window) で出す。
  - **発行条件を 1 か所に閉じた** (`should_notify_workers_gone`)。枠が 1 つ減っただけでは
    出さない (残りが処理を続けられる間、利用者にできることが無い)。終了・再読み込みで
    畳んだ場合も出さない。**最後の枠が諦めたときだけ**。
  - そのために `run_worker_slot` は `SlotExit::{Shutdown, GaveUp}` を**抜けた場所で確定**
    させる。閉じてからフラグを読み直すと、その間に終了要求が来た場合に「自分から畳んだ」と
    誤って読める。
  - 文面が案内するのは**アプリの再起動ではなく「⟳ プラグインを再読み込み」**。`reload()` は
    プールを作り直すので再起動は要らず、案内すると開いている一覧と表示位置を捨てさせる。
    再読み込みボタンを notice 側には置いていない (`reload` は同期でプロセスを 3 つ起こすため、
    新しい UI スレッド同期経路を増やさない)。
- **診断パネルに実績を出す** (`SusieWorkerHealth`)。起動できた枠数 / 生存枠数 / 作り直し回数 /
  打ち切り数 / 落とした対象数 / 最後の失敗理由。0 の項目は出さない (実際に起きた項目が埋もれる)。
  - **副産物の修正**: 枠を使い切った状態が `WorkerSpawnFailed` として出ていた。「起動または
    ハンドシェイクに失敗しました」と書かれるが実際には起動しており、利用者が確認すべきものが
    違う。`WorkersExhausted` を分けた。判定は純関数 `pool_status_from` に出してテストした
    (`a_pool_that_ran_and_then_died_is_not_reported_as_a_failed_start`)。
  - 正常時は何も出さない。復帰の履歴は「読めない画像があった」ときに辿るためのもので、
    常時出す情報ではない。
- **ついでに直した race**: `live_workers` はスロットスレッドを起こした**後**に
  `store(worker_count)` していた。起動直後に落ちたワーカーが先に減算すると 0 を下回って
  戻れない。スレッドを起こす前に `fetch_add` する形にした。
- 変更ファイル: [susie_loader.rs](../src/susie_loader.rs) /
  [ui_susie_diagnostic.rs](../src/ui_susie_diagnostic.rs) /
  [ui_dialogs/susie_worker_notice.rs](../src/ui_dialogs/susie_worker_notice.rs) /
  `app.rs` (4 行) / マニュアル (設定・FAQ) / [async-architecture.md](async-architecture.md) §5.4。
  §5.4 は「Susie も respawn しない」と書いたままだったので、実装に合わせた。

### 2.21 フルスクリーンが Susie プラグインの拡張子を画像として扱わない ✅ 見立ての誤りと判明 (2026-08-25)

- 出典: 2026-08-25。クラッシュプラグインの検証中、サムネイルは Susie 経路を通るのに、
  同じファイルをフルスクリーンで開くと
  `fs load FAIL: The file extension `."miv-crashtest"` was not recognized as an image format`
  で落ちた。ここから「フルスクリーンは Susie に到達していない」と記録した。
- **この見立ては誤り。フルスクリーンは Susie に到達している。**
  - 復号は `decode_canonical_image` の 1 本で、fallback は image → WIC → Susie
    ([canonical_image_loader.rs](../src/canonical_image_loader.rs))。フルスクリーンの
    呼び出し ([app.rs:51540](../src/app.rs:51540)) は `CanonicalDecodeOptions::fullscreen`
    (= `susie_priority: true`) を渡していて、Susie 段を飛ばす分岐は無い。
  - 実プラグイン (`ifpi.spi`) と実ファイル (`C165.PI`) で、製品と同じ入口を通して
    640×400 を復号できることを統合テストで確認した
    (`fullscreen_decode_entry_reaches_susie_for_a_plugin_only_extension`)。
    既存の Susie 統合テストは `susie_loader::decode_file` を直接叩いていて
    **この入口を一度も通っていなかった**ため、疑いを晴らせなかった。
- **本当の問題は診断文だった**。連鎖が全部失敗したとき、返していたのは image クレートの
  エラーだけ。未対応拡張子ではそれが「拡張子が画像として認識されない」になるため、
  **WIC も Susie も試したあとの失敗なのに Susie を試していないように読める**。
  Susie 側のエラーは `.map_err(|_| primary_error)` で捨てられていた。
  - 修正: `CanonicalDecodeError::Decode` を `DecodeChainFailure { primary, susie_error }` に変え、
    `Display` が「WIC も読めず、Susie は〈理由〉で失敗した」まで出すようにした。
    「試したか」の bool は**持たない** (この型が作られるのは連鎖の最後だけで、常に true にしかならない)。
  - 観測された失敗そのもの (クラッシュプラグイン検証中の decode 失敗) は §1.120 の
    ワーカー枯渇と、修正で入れた「一度落とした対象は投げ直さない」記憶で説明が付く。
    どちらも設計どおりで、新しい欠陥ではない。
- 教訓: **エラー文の出所を確かめずに、そこから経路の有無を推定した**。最初の decoder の
  エラーは連鎖について何も語らない。捨てられている失敗理由を先に拾うべきだった
  (§1.121 と同じ教訓)。

### 1.121 RAR を代表サムネにしても親フォルダのタイルに出ない — 専用スレ >>302 ✅ 修正済み (2026-08-24、実機確認済み)

- 出典: 専用スレ >>302。RAR があるフォルダで RAR を `P` で代表サムネに指定し、親フォルダへ移動すると
  そのサムネイルが出ない。利用者の切り分けでは v3.1.0 は正常、v3.1.1 で発生。
- **原因 (計装を入れて 1 回の再現で確定)**。`archive cache unavailable → base_req` と
  `thumb_not_eligible failed` が同じ 1 秒の中に並んで記録された:
  - pin の解決自体は成功していた (cascade → `ArchiveFirstImage` → `dummy-book-002.rar`、pinned_key も正しい)。
    捨てていたのは `converted_archive_cache_paths` に対象 RAR が載っていなかったため。
  - 載らなかったのは、pin-root の収集 ([app.rs:25141](../src/app.rs:25141)) が
    `candidate && !requested && (Pending | Evicted)` を要求していたから。
    **`Loaded` と `Failed` は終端状態で、そこへ落ちたコンテナの pin は二度と解決されない。**
  - **中身がアーカイブだけのフォルダは自動選定が代表画像を選べず (アーカイブを展開しない仕様)、
    必ず `Failed` になる。**そのため出口のないループになっていた:
    ピンが捨てられる → 自動選定が走る → 失敗する → `Failed` → 発見されない → ピンが捨てられる。
  - `Loaded` は反対側から同じ穴に落ちる (画像が 1 枚混ざったフォルダはその画像を描いたまま固定)。
  - 初めて成立させたのは `bc4adbdd` (v3.1.2、prefetch gate と admission 制限の追加)。
    `0f346e20` は下流を整備しただけで、見つからない root は救えない。
- **v3.1.1 の境界は未確定のまま**。v3.1.0..v3.1.1 で `folder_thumb_pins.rs` / `thumb_loader.rs` /
  `rar_loader.rs` / `zip_loader.rs` は blob が同一、`app.rs` の 3 コミットもこの経路に差分が無い
  (ClaudeCode と Codex が独立に確認)。現行の原因は上記で閉じているので、修正には支障しない。
- **修正**: pin-root の発見条件から**サムネイルの状態と進行中要求を外した**。ピンの有無は、
  そのタイルが今どう描かれているかとは無関係な事実である。コストを抑える役目は可視範囲の
  絞り込み (`candidate_indices`) と root ごとの解決結果の記憶
  (`converted_archive_pin_root_states`) が引き継ぐ。解決済みの root は DB も cascade も引き直さない。
- **回帰テスト**: タイルを終端状態にしてから collector を回す 2 本
  (`converted_archive_pin_root_is_found_when_the_folder_tile_already_failed` /
  `..._while_the_folder_tile_is_requested`)。既存の archive-through-folder-pin テストは
  `start_converted_archive_cache_paths_refresh` を直接呼ぶため、描画済みのタイルを一度も見ておらず
  この退行を捕まえられなかった。
- 教訓: **無言の早期 return に理由を型で残してから直した**。事前の推定は
  ClaudeCode / Codex とも `already_requested` か `Loaded` で、**どちらも外れていた** (実際は `Failed`)。
### 1.122 F12 で動画をメイン ⇄ 別ウィンドウへ往復させると重い ✅ 主因は修正済み (2026-08-25、実機確認済み)

- 出典: R2e ②-d の実機 smoke 中の指摘 (2026-08-25)。動作はするが体感でかなり遅い。
- **既存の問題であることは確認済み**: 利用者が **v3.2.0 と v3.0.0 ポータブル**でも同じだと
  確認した。R2e ブランチ由来ではない。**presenter 本体
  ([src/video/native_presenter/](../src/video/native_presenter/)) は R2e で 1 行も触っていない。**

#### ⚠ 最初にここへ書いたログ根拠は誤りだった (2026-08-25 訂正)

当初この項目には `shared output pool exhausted` / `late_drop=88` / `[demux] audio packet
send waited` / `[SLOW FRAME]` を根拠として並べていたが、**保存ログを読み直したところ
別セッションのものだった**。

- 引用元は `mimageviewer.log.bak1` で、**video-strip worktree の dev ビルド**
  (`ffmpeg DLLs expected at C:\home\mimageviewer-video-strip\target\dev-runtime`) のログ。
  **F12 の placement 切替は 1 度も含まれていない** (`switch placement request=` が 0 件)。
- `shared output pool exhausted` は保存ログ全体で **1 回だけ**で、しかも
  **動画の切替 (idx 41 → 42) の瞬間**。直後に旧 decode スレッドが exit しており、
  source 切替時の一過性。F12 とは無関係。
- `[demux] audio packet send waited 20-41ms queue_len_before=64/64` は、同じ区間の
  presenter summary が `fps=59.1 late_drop=0` の**正常再生中に連続して出ている**。
  音声キューが先読みで満杯なだけの通常の backpressure で、症状ではない。
- **F12 往復を含むログは 1 つも残っていない** (該当セッションのログはローテーション済み)。

つまり **§1.122 には現時点で計測根拠が無い**。プールを疑う理由も無くなった。

#### 構造から分かっていること (コードを読んだ結果)

placement 切替は [src/video/mod.rs](../src/video/mod.rs) の `SwitchPlacement` 分岐で処理され、
**placement が変わる場合は presenter を丸ごと作り直す**。`NativeRenderCore::new`
([render_core.rs](../src/video/native_presenter/render_core.rs)) が 1 往復ごとに作るもの:

| 段 | 内容 |
| --- | --- |
| `d3d11_device` | `D3D11CreateDevice` で**新しい D3D11 デバイス**を作る |
| `shader_pipelines` | `VideoGradePipeline::new` + `VideoResamplePipeline::new` (Anime4K 含む全 PS を生成) |
| `swapchain_dcomp` | swapchain + `DCompositionCreateDevice` + visual ツリー + 背景 |
| `egui_overlay` | `NativeEguiOverlay::new` — **wgpu instance / adapter / device (D3D12) を新規作成**し、 |
| | `egui::Context` を作って `configure_fonts_with_settings` で**フォントを構成し直す** |

再生スレッドはこの間フレームを出せないので、**体感遅延 = この区間の実時間**。
「新しいサーフェスを確保してから古いスロットを返す」といった順序の問題ではなく、
**デバイス級のリソースを往復ごとに作り直していること**が疑わしい。

#### 原因 — 実行時の HLSL コンパイル (確定、修正済み)

保存ログに、presenter を作るたびに同じ区間で **毎回 1.8〜2.5 秒**消えている証拠が 15 回分残っていた
(`presenter D3D11 device created` → `Anime4K bytecode loaded` の差)。この間にあるのは
COM cast と `VideoGradePipeline::new` / `VideoResamplePipeline::new` だけ。

単体テストで 7 本の `D3DCompile` を個別に計った結果 (2026-08-25 実測):

| shader | compile |
| --- | --- |
| grade `vs_main` | 2.1 ms |
| grade `ps_main` | 8.0 ms |
| resample `vs_main` | 1.6 ms |
| resample `ps_horizontal` | 6.8 ms |
| resample `ps_vertical` | 6.6 ms |
| resample `ps_nearest` | 2.6 ms |
| **NIS `fs_nis`** | **2138.1 ms** |
| **合計** | **2155.8 ms** |

**NIS の pixel shader 1 本だけで 2.1 秒**。これを `NativeRenderCore::new` の中で毎回
コンパイルしていたので、**F12 片道 ≈ 2.2 秒 / 往復 ≈ 4.5 秒**の固定費用になっていた。
同じコストは **動画を最初に開くときにも**払っている (ウィンドウが出るまでの待ち)。

※ 当初疑ったプール枯渇は無関係だった。

#### 修正

**シェーダを全部ビルド時に FXC で DXBC 化する**。Anime4K は最初からこの経路
(`build.rs` の `find_fxc` / `compile_hlsl_with_fxc`) なので、残りを揃えただけ。

- 手書き HLSL を Rust の文字列リテラルから [src/video/native_presenter/shaders/](../src/video/native_presenter/shaders/)
  の `.hlsl` へ出し、`build.rs` が FXC で `.cso` にする
- NIS はすでに `build.rs` が WGSL → HLSL にしていたので、FXC 工程を足すだけ
- 実行時は `CreatePixelShader` に `include_bytes!` した DXBC を渡すのみ。
  production 経路から `D3DCompile` が消える (残るのは `#[cfg(test)]` の Anime4K GPU 実測テストのみ)
- 「この HLSL はコンパイルできる」テストはビルドが保証するので削除し、
  「実行時に渡す DXBC が実在するか」を見るテストに差し替えた
  (副次効果: lib テストからも NIS の 2.1 秒が消える)

検証は `nis_draw_writes_target_with_default_d3d11_rasterizer` と
`anime4k_chain_writes_target_with_native_fullscreen_vertex_shader` (実 GPU で描画して出力を見る) が兼ねる。

#### 実機確認済み (2026-08-25)。残りの内訳

`shader_pipelines` は **2156ms → 2.8〜5.7ms**。往復 **≈ 5.2 秒 → ≈ 0.95 秒**。
利用者確認でも「速くなりました」。以下は実機ログの実測値:

| 段 | main → 別ウィンドウ | 別ウィンドウ → main |
| --- | --- | --- |
| **total** | **234〜252ms** | **590〜722ms** |
| `host_attach` | 2〜10ms | 45〜48ms |
| `render_core_new` | 166〜176ms | 184〜187ms |
| `prepare` | 17〜26ms | 18〜19ms |
| **`publish`** | 7〜10ms | **289〜424ms** |
| `detach_old` | ≈ 5ms | ≈ 5ms |

`render_core_new` の内訳: `d3d11_device` 27〜31ms / `shader_pipelines` 2.8〜5.7ms /
`swapchain_dcomp` 4〜5ms / **`egui_overlay` 116〜139ms**。
`egui_overlay` の内訳: `wgpu_instance` 0.3〜0.7ms / `adapter` 1.7〜2.2ms /
**`device` 64〜72ms** (wgpu `request_device`) / `renderer` 9.5〜11ms / `egui_ctx` 0.0ms。

⚠ **フォントは無関係だった** (`of_which_fonts=0.0`)。egui の font atlas は初回描画時に
遅延構築されるので、`configure_fonts_with_settings` 自体は払っていない。
事前に疑っていたので記録する — 測らなければここを直しに行っていた。

#### `publish` の非対称は `SetFocus` だった (2026-08-26 実測、確定)

probe を render / pump 両スレッドに入れて F12 を 5 回切替えた。
`publish` の内訳は `hud=0.0 retire=0.0` で、**全部が pump 待ち**。pump 側も
`queue=0.4〜3.7ms` (拾うのは遅くない) / `topology=0.0` / `intents=0.0` で、
残りはすべて `publish_host` → `show_for_placement` の中にあった:

| 呼び出し | main へ戻る | 別ウィンドウへ |
| --- | --- | --- |
| `ShowWindow` | 1.8〜2.1ms | 1.2〜1.7ms |
| `SetWindowPos` | 0.1〜0.2ms | 0.0〜0.1ms |
| **`SetFocus`** | **259.9 / 331.2ms** | **3.8 / 4.4ms** |

**待ち時間は main UI スレッドのフレーム境界の整数倍**で、cross-thread の同期
メッセージが 3〜4 往復していることを示す:

- `18.176s` 開始 → `18.436s` 完了 (260ms)。間の UI フレーム = 18.176 / 18.260 / 18.343 / 18.432 — **3 間隔**
- `10.958s` 開始 → `11.290s` 完了 (332ms)。間の UI フレーム = 11.026 / 11.112 / 11.197 / 11.285 — **4 間隔**

#### なぜ main へ戻る向きだけ遅いのか — placement ではなかった

`SetFocus` は両方向とも同じように呼ばれている (どちらも child window + activate=true)。
違うのは、**その時点で main UI スレッドが遅いかどうか**だけ:

> **動画が detached にある間だけ、main UI スレッドが 1 フレーム 43ms (`pre_grid` 41ms) ・周期 85ms になる。**

- 遅いフレームの区間は `8.456→11.367s` と `14.864→18.432s`。**どちらも動画が
  detached にある区間と一致**し、main へ戻すと止まる (18.432s 以降、ログ終端 25.1s まで 0 件)。
- 同じ理由で `host_attach` も main 方向だけ 40〜45ms (`CreateWindowEx` が親へ
  `WM_PARENTNOTIFY` を送る)。**最初の 1 回だけ 4.2ms** なのは、その時点では
  まだ UI スレッドが速かったから。

つまり **残りは「F12 の問題」ではなく「detached 再生中に main UI スレッドが
1 フレーム 41ms かかる問題」**。これを直せば `SetFocus` と `CreateWindowEx` の待ちが
同時に消え、**detached 再生中のアプリ全体のもたつきも同時に消える**。

⚠ `SetFocus` を遅延・省略・別スレッド化して待ちを隠すのは対策にしない。

#### 次の一手 — 41ms の `pre_grid` の中身

`pg_top` には `selection_info:0.4ms` / `facet:0.3ms` しか出ておらず、**41ms の大半が
名前の付いた区間の外**にある。ただし既存の perf event `ui/slow_frame_breakdown`
([app.rs:67280](../src/app.rs:67280)) が `keep_fullscreen_viewport_ms` /
`render_fullscreen_viewport_ms` / `ensure_native_video_front_ms` / `background_polls_ms` /
`bars_ms` などに分解済みで、`perf::is_enabled() && frame > 30ms` で発火する。
今回のフレームは 43〜45ms なので条件を満たす。
**コード変更なしで、`--perf-log` を付けて同じ操作を 1 回取れば分かる。**

**残りをやるなら優先順は:**

1. **detached 再生中の main UI スレッド 41ms/フレーム** — 上記のとおり、
   `publish` (260〜331ms) と `host_attach` (40〜45ms) の両方の原因。まず内訳を取る
2. **`egui_overlay` (116〜139ms × 2)** — 切替のたびに wgpu device を新規作成している
   (`request_device` 64〜72ms)。presenter 間で device を共有できるかを見る
3. `d3d11_device` (27〜31ms × 2) — 同じく共有候補

- ⚠ **guard / delay / retry で待ち時間を隠さない。**
- 規模 \\ 優先度: Medium / P3 (主因は取れた。残りは体感上実用域)。

### 1.123 スレッド終了中の heap 例外を、例外ハンドラ自身が二次クラッシュで握り潰す — 実機クラッシュ

- 出典: 2026-08-25、R2e ステージ④の smoke 準備中。タグを付けて上のフォルダへ移動した直後に
  `mimageviewer-core.exe` が **0xc0000005 で異常終了**。`panic.log` に記録なし
  (= Rust panic ではない)。**R2e とは無関係の既存不具合**
  ([logger.rs](../src/logger.rs) も [lib.rs](../src/lib.rs) の handler も
  このブランチでは 1 行も変更していない)。
- **WER のフルダンプから取得したスタック** (`cdb -z <dump> -c ".ecxr; kn"`)。下から読む:

  ```
  0b ntdll!RtlUserThreadStart
  0a kernel32!BaseThreadInitThunk
  09 ntdll!RtlExitUserThread          ← スレッドが終了処理に入っている
  08 ntdll!LdrShutdownThread          ← DLL_THREAD_DETACH / TLS デストラクタ実行中
  07 ntdll!LdrShutdownThread
  06 ntdll!RtlFreeHeap+0x726          ← ここで一次例外が発生
  05 ntdll!KiUserExceptionDispatcher
  02 mimageviewer_core!mimageviewer::native_exception_handler+0xa0   ← 自前ハンドラが走る
  01 mimageviewer_core!mimageviewer::logger::current_thread_id_num+0x19
  00 mimageviewer_core!std::thread::current::current+0xf   ← 二次 AV (rcx=0)
  ```

- **問題は 2 つある。**

  **(A) 一次例外**: スレッド終了中の `RtlFreeHeap` で例外。ヒープの不整合か、
  TLS デストラクタ順序に依存した解放。直前のログは**フォルダ移動でワーカー 12 本を
  停止して 12 本を起動**した箇所 (`w0 stopped` … `spawning 10 regular + 2 I/O workers`)。

  **(B) 自前ハンドラが、その文脈で走れない**:
  [lib.rs:693](../src/lib.rs:693) の `native_exception_handler` は
  `logger::current_thread_id_num()` ([logger.rs:272](../src/logger.rs:272)) を呼び、
  その中の `std::thread::current()` が **TLS 依存**である。
  `LdrShutdownThread` 以降は TLS が破棄済みなので `rcx=0` を deref して二次 AV になる。
  **ハンドラが自分で死ぬので `panic.log` に何も残らない。**

- ⚠ **(B) を直しても (A) は直らない。だが (B) を直さないと (A) は永久に見えない。**
  今は例外ハンドラが証拠ごと消している。**(B) を先に直す。**
- (B) の直し方: 例外ハンドラの内側では **TLS に触れる API を使わない**。
  スレッド ID は `std::thread::current().id()` ではなく **`GetCurrentThreadId()`** を使う。
  ハンドラ経路が通るログ整形すべてを同じ基準で見直す (`format!` の allocator も
  ヒープ破損時は危険なので、固定バッファ + `write!` が望ましい)。
- (A) の手掛かり: 一次例外は `RtlFreeHeap` の中。`Application Verifier` の page heap か
  `_NT_GLOBAL_FLAG` の heap tail checking を有効にして再現すると、壊した側が特定できる。
  ダンプは `C:\Users\<user>\AppData\Local\CrashDumps\` に残る (今回は 59.6 GB)。
- 規模 / 優先度: Small (B) / Medium (A)。**P2** — 通常操作 (タグ付け → フォルダ移動) で落ちた。

### 1.124 F12 を連打すると実際の押下が破棄される ✅ 修正済み (2026-08-26、実機確認済み)

- 出典: 2026-08-25、§1.122 修正後の実機確認。別ウィンドウから F12 で main へ戻るとき、
  **連打すると最初の何回かが無反応**。**§1.122 とは別の既存不具合**。
- ログにそのまま残っている (`mimageviewer.log`):

  ```
  30.274  PlacementSwitched request=12 applied DetachedWindow generation=9   ← 切替完了
  30.663  ignore stale native F12 toggle: os_down=false presentation=DetachedWindow
  30.906  ignore stale native F12 toggle: os_down=false presentation=DetachedWindow
  31.203  ignore stale native F12 toggle: os_down=false presentation=DetachedWindow
  32.093  switch placement request=13 -> target=MainWindow                   ← 4 回目でやっと反応
  ```

  切替完了の **390ms 後から** 240〜300ms 間隔で 3 回。rebuild 直後に 1 回だけ来る
  stale 再配送ではなく、**人間の連打そのもの**。1 セッションで 7 件、
  うち 6 件が `presentation=DetachedWindow` (= 利用者の報告と一致)。
- 原因: [native_video.rs](../src/app/native_video.rs) の F12 arm が
  `native_video_key_physically_down()` で **「今この瞬間キーが物理的に押されているか」**を
  `GetAsyncKeyState` に聞いている。これは「この event は rebuild の再配送か」の
  **代用判定**でしかない。presenter の wndproc が WM_KEYDOWN を拾ってから App が
  event を引くまでの遅延よりも押下時間が短いと、**本物の押下も同じく false になる**。
  早押しと stale 再配送をこの検査では区別できない。
- §1.122 で切替が速くなった分、連打しやすくなって顔を出しやすくなっている
  (利用者も「前からかもしれない」と報告)。

#### 修正 — 廃れた proxy を削除した (追加ではなく削除)

当初は「`KeyDown` に presenter generation を乗せる」を提案したが、**Codex の指摘で撤回した**。
rebuild 後に**新しい HWND が decode した**再配送は**現世代**で stamp されるので、generation では
早押しと区別できない。generation は stale key の identity ではない。

実際に入れたのは削除である。F12 arm に届く `repeat=false` は
**現 HWND・現 epoch が生成した first-key-down** であり、probe に仕事が残っていない:

| 軸 | 何が落とすか | 入った時期 |
| --- | --- | --- |
| 旧 presenter 由来 | `window_event_belongs_to_generation` (host ごとの epoch) | **2026-07-30** (`0cf4b6a9` pump 分離) |
| hold による auto-repeat | `WM_KEYDOWN` の previous-key-state (lParam bit 30) → 既存の `!key.repeat` | 当初から |
| 切替中の押下 | `switch_native_video_viewer_presentation` の pending guard | 当初から |

物理 probe は **2026-07-01** 。つまり 1 行目の識別子が入る **1 ヶ月前**の代用判定で、
その後撤去されずに残っていた。

**削除したもの**: `native_video_key_physically_down` / その呼び出し /
`NativeVideoKeyBlockReason::StaleDetachedToggle` / formatter の対応 case。
基盤の `key_input::physical_key_down` は他に 3 箱所の本番利用があるので残す。

**回帰テスト**:

- `window_event_is_accepted_only_for_the_current_presenter_generation` — 述語を切り出して表で 3 通り。
  述語を `true` に変異させて落ちることを確認済み (空振りでない)
- `native_video_f12_does_not_toggle_while_a_placement_switch_is_pending` — キー経路を通して pending guard を固定
- 既存の `native_video_f12_toggles_detached_viewer_mode` は **probe 削除後に初めて意味を持つ**。
  旧コードは `#[cfg(test)] { true }` で probe を迷回しており、**この退行を 2 ヶ月近く隠していた**。
  旧コードで落ちるテストは原理的に書けない (テストビルドでは常に true を返していたため)

憲法 §2 規則 5 はこの probe を好例として名指ししていたので、実機ログで反証されたとして
該当部分を撤回した (一般原則は維持)。触った範囲と判断理由は
[detached-rework-plan.md](detached-rework-plan.md) §11 に記録。

- ✅ **実機確認済み (2026-08-26)**: F12 押下 15 回に対し切替 13 回・意図的な無視 2 回で、
  取りこぼしゼロ・二重トグルゼロ。無視された 2 件はどちらも切替飛行中で、
  `ignore F12 toggle while video placement switch is pending` として記録されていた
  (= 意図した pending guard)。`ignore stale native F12 toggle` は 0 件。

### 1.125 ★固定を押すと、開いている fullscreen が古い idx を指したままになる ✅ 修正済み (2026-08-26、実機確認済み)

- ✅ **実機確認済み (2026-08-26)**: 9 回の判断がすべて説明可能だった。
  追従 7 回 / 閉鎖 1 回 / 対象なし 1 回。閉じたのは `target_len=6` かつ再生中ファイルが
  その 6 件に含まれないときだけ。**往復の対称性も確認** (`57 → 2 → 57`、`5 → 0 → 5`) で、
  固定中に別ファイルへ移動していても解除時に正しく戻る。`no snapshot key` の異常は 0 件。
- ✅ **実装 (2026-08-26)**: 案 C を採用。ただし identity は
  `snapshot_owner_entry` の prefix owner 解決ではなく、`selected` と同じ
  `snapshot_key_from_grid_item` の完全一致とした。hit は generation bump 前に live
  `FsCacheEntry` を退避し、items 交換 / bump / invalidate 後に新 idx へ再挿入して、
  fullscreen と音声 / VST / normalize / loop / EOF / marker / native pending を同じ owner の
  index-space へ追従させる。miss は交換前に正規 `close_fullscreen()` を通す。
- 解除側の at-origin 直接復元にも同じ調停を適用し、activation 時ではなく解除直前に実際に
  開いている item を解決する。media-navigation と generation-stamped marker worker は cancel +
  新 index-space で再開、folder-nav / holdover は解放する。別 context の pending は owner stamp が
  一致しない限り変更しない。
- 回帰テスト 12 件を追加し、activate hit / miss、解除前の viewer 内移動、ZipImage と ZipFile の
  prefix 誤一致、130 item の大規模並べ替え、player Box identity の往復維持、複合 media session、
  late worker result、mounted main / detached / ParkedLive / promoted active / sibling parked の native
  owner 行列、folder-nav / 派生 index lifecycle を固定した。detached 憲法 §11 に判断理由を記録。

- 出典: 2026-08-26、別件 (per-context snapshot) の実機確認中に利用者が発見。
  **動画を再生しながら ★固定 を押すと再生が止まり、
  「読込中...」のまま固着する**。画面には `c:\home\youtube\movie\youtube` のような
  **フォルダに見える path** が出ていた。
- **既存の不具合**。`activate_snapshot` の最終変更は **2026-06-04** (`969ba68f`、★固定 UI 導入時)。
  発見時のブランチは [snapshot_ops.rs](../src/app/snapshot_ops.rs) と
  [ui_fullscreen.rs](../src/ui_fullscreen.rs) を 1 行も触っていない。

#### 原因 — `activate_snapshot` が fullscreen を一切見ていない

[snapshot_ops.rs](../src/app/snapshot_ops.rs) の `activate_snapshot` (165〜330 行) には
`fullscreen_idx` / `fs_cache` / `close_fullscreen` への参照が **1 つも無い**。やっているのは:

1. `items` を snapshot の部分集合へ**丸ごと差し替える**
2. `bump_items_generation()` + `invalidate_idx_state_and_queues()`

しかし **fullscreen ビューアは古い `fs_idx` を指したまま**なので:

- `fs_idx` の先が**別のアイテム**になる。snapshot に含まれない / フォルダだった場合、
  fullscreen はそれを読めず **「読込中...」で永久に止まる**
- `fs_cache` は `ItemsGenerationMap<FsCacheEntry>`
  ([viewer_context_registry.rs:845](../src/app/viewer_context_registry.rs:845)) なので、
  generation を進めた時点で **開いていた動画エントリごと無効化**される → 再生が止まる

**設計の穴はここだけ**: snapshot 側は fullscreen を知らないわけではなく、
`snapshot_current_fullscreen_path()` ([snapshot_ops.rs:711](../src/app/snapshot_ops.rs:711)) を持ち、
**snapshot が既に active な状態での fullscreen ナビ**はちゃんと扱っている。
抜けているのは **「fullscreen を開いている最中に snapshot を activate した」場合の整合**だけ。

`deactivate_snapshot` も同じ形で items を戻すので、**解除側にも同型の穴があるはず**。
修正時は両方を列挙すること。

#### 直し方は仕様判断 — 決めてから着手する

| 案 | 挙動 | 考えどころ |
| --- | --- | --- |
| A | ★固定時に fullscreen を閉じる | 単純で確実。ただし見ていたものが消える |
| B | 同じアイテムを新 idx へ追従させる | 一番自然。`snapshot_owner_entry` が既に path → entry 解決を持っている |
| C | snapshot に含まれるなら B、含まれなければ閉じる | B の自然さを保ちつつ範囲外を定義できる |

⚠ **「読込中のまま固着するなら timeout で閉じる」は症状パッチ**。idx が意味を失ったこと自体を扱う。
⚠ 動画の再生停止だけを見て `fs_cache` を generation 無効化から外すのも不可。
   無効化は「idx の意味が変わった」という正しい事実を表している。直すべきは **見ている側の追従**。

#### 再現手順

1. 動画を含むフォルダを開く
2. 動画を fullscreen で再生する
3. (一覧側に戻らずに) **★固定** を押す
4. → 再生が止まり、「読込中...」のまま戻らない

- 規模 \\ 優先度: Small〜Medium / **P2** (通常操作で到達し、固着する)。

### 1.126 ★固定の items 交換で `image_metas` だけ取り残される — 添字空間の交換漏れ

- 出典: 2026-08-26、§1.125 の設計相談中に Codex が発見。
  **§1.125 とは別の症状**なので別項目にした (憲法 §2 規則 7: ついでに直さない)。
- `image_metas` は `items` と **同じ位置の `Vec`** なのに、`activate_snapshot` が
  subset 化していない。`SnapshotState` にも `saved_image_metas` /
  `list_view_image_metas` が無い。
- 具体例: 元の `visible_indices = [4, 9]` なら、新しい `items[0]` は旧 4 の項目だが、
  `image_metas[0]` は **旧 0 のまま**。
- `invalidate_idx_state_and_queues()` はこれを消さない。同関数の責務コメントが
  **「caller 責任」**と明記している ([app.rs:25872](../src/app.rs:25872))。
- 規模 \\ 優先度: Small / P3 (表示されるメタ情報がずれる。固着はしない)。

### 1.127 ★固定の items 交換後、Details 表示の index state が再構築されない

- 出典: §1.126 と同じ、Codex の指摘 (2026-08-26)。
- `details_order` / `details_tag_prewarm_indices` / `details_meta_pending` は、
  items 交換後に**無条件では再構築・cancel されない**。
- color filter が有効な場合だけ後段の `rebuild_visible_indices()` が偶然更新するので、
  **通常の Details 表示では旧 order が残り得る**。
- 規模 \\ 優先度: Small / P3。

### 1.128 ★固定で範囲外になった別窓を、閉じず自前の一覧へ切り出す — 仕様提案

- 出典: 2026-08-26、§1.125 の実機確認。複数ウィンドウモードで動画再生中に
  ★固定を押し、**そのファイルが固定範囲外だったため動画窓が閉じた**。
  **§1.125 の設計どおりの動作**であり、不具合ではない。ただし体験としては
  **グリッドのボタンを押したら別の窓が消えた**に見える。
- 利用者の希望: **再生はなるべく継続**し、前後移動も納得できる形で残す。

#### 前提の確認 (2026-08-26、コードで裏取り済み)

「★固定していないときは実 FS 順で前後移動する」という想定は **実装と違う**。

```rust
fn build_nav_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize>
```

`get_nav_indices()` は `current_grid_order()` (= `visible_indices`、Details なら `details_order`)
を渡す ([ui_fullscreen.rs:7621](../src/ui_fullscreen.rs:7621) /
[app.rs:46141](../src/app.rs:46141))。つまり **★固定の有無にかかわらず、
前後移動は常にグリッドの表示順・絞り込み結果を辿る**。実 FS 順を辿るモードは存在しない。

したがって ★固定は「別の並びに切り替える」操作ではなく、
**その時点の並びを凍結する**操作である。

#### 提案 — 別窓を自前の context へ切り出す

現状、複数ウィンドウモードの動画窓は **同じ context を別の窓で描いている**
(実機ログ: `main_fs_idx=Some(121) mounted=true`)。だから親のグリッド操作が直撃する。

**器は既にある**:

- 2026-08-26 に `snapshot` を `ViewerContextBundle` の field にしたので、
  **context ごとに別の並び・別の凍結状態を持てる**
- `fork_mounted_live_media_context` が「再生中メディアを自前の context へ切り出す」既存の仕組み

よって §1.125 の miss 経路を「閉じる」から「**切り出す**」へ変えることで、
窓は自分の items を持ち続け、再生も前後移動も維持される。

#### 詰めるべき点

- 切り出した context の一覧は **固定前の並び** を保持する。それを利用者にどう示すか
  (窓のタイトル / HUD に何か出すか、無言でよいか)
- 親が ★固定を**解除**したとき、切り出した窓を親へ戻すのか、独立のままにするのか
- 切り出した context の寿命 (窓を閉じるまで / 親のフォルダ移動まで)
- 静止画でも同じにするか、メディア限定にするか
- ⚠ detached 述語に触れるので §2 の手続きが必要

- 規模 \\ 優先度: Medium / P3 (不具合ではなく仕様改善)。

## 2. 一覧 / サムネイル / フォルダ走査

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

### 2.11 動画サムネイル中央の再生アイコンを目立ちにくくする — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> `video_thumbnail_indicator` の 3 択 (`PlayIcon` 既定 / `BottomLeftBadge` / `Hidden`)。
> **既定は現在の中央アイコンのまま** (利用者確定 2026-08-22: 「既定は現在の動画再生アイコンで OK。
> 設定で変更できれば良い」)。対象確認の回答待ちは解消。
> 中央アイコンと左下バッジは `video_thumbnail_indicator_parts` が両方を返す形で**構造的に排他**。
> **音声は対象外** — サムネイルを生成せず音楽アイコンがセルの中身そのものなので、消すと空の箱になる
> (`bottom_left_content` で Video と Audio のアームを分離済み)。
> バッジの色は `FormatBadgeKind` で決める。**ラベル文字列の一致で決めない** (旧実装は未知ラベルが
> すべてアーカイブの橙に落ちるため、動画も新形式も静かに橙になる)。ラベルは `動画` — 生成中の中央
> プレースホルダと同じ語にし、1 セルに `動画` と `VIDEO` が並ぶのを避けた。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。動画サムネイル上のアイコンを小さくするか、
  非表示にしたい要望。
- まず、対象が動画セル中央の `▶` 再生アイコンで合っているか回答を待つ。
- 対象が合っていれば、中央の代表画像を隠しにくい角の小型表示を既定候補とし、非表示設定も
  用意する方向で検討する。既存 §2.2 の隅バッジ配置とは意味が異なる overlay なので、単に
  同じレーンへ入れず、再生可能であることの視認性と hover / 選択表示との重なりを確認する。
- スナップショットは通常動画、代表画像あり / なし、選択 / hover、狭いセル、形式 / 評価 / タグ
  バッジとの組み合わせを含める。
- 規模 / 優先度: Small / P2 (対象確認後に仕様確定)。

### 2.7 RAR の header 全走査が、一覧判定とサムネイル生成で 2 回走る

- 出典: v3.1.2 で入れた RAR 判定の逐次公開 (旧 §2.3) の caller 監査 (2026-08-19)。その brief の
  §1.4 として着手したが、範囲を超えると判断して停止条件どおり切り出した。**v3.1.2 で閉じたのは
  「全件判定を待つ表示遅延」であって、判定そのものの重複ではない。**
- 機構: フォルダ一覧の eligibility 判定が `inspect_for_direct_read_cancelable` で RAR の
  header を全走査した後、thumbnail worker の代表画像選択が
  `enumerate_image_entries_detailed` で**同じ RAR の header をもう一度全部**列挙する。
  30,000 entry 級の RAR が多いフォルダでは、この 2 回目がそのまま体感時間に乗る。
- なぜ今回入れなかったか: 1 回へ統合するには `RarInspection` が entry / 代表情報を保持し、
  thumbnail と open 双方の列挙契約を変える必要がある。逐次公開とは独立した変更で、
  同じ commit に混ぜると切り分けられなくなる。
- 直す方向:
  - `rar_loader` の判定結果が、そのフォルダ generation の間だけ代表画像選択に必要な情報も
    持つようにする。`DECISION_CACHE_CAPACITY` (32、full inspection 保持) を単に増やす修正には
    しない。メモリが増えるだけで、判定回数は減らない。
  - 統合後は **同一 RAR の header 判定回数を計装または test double で 1 回に固定する**
    (逐次公開側の完了条件として書かれていた項目をここへ引き継ぐ)。
- 計測の起点: `C:\tmp\miv-rar-thumbnail-test-100` (30,000 entry の RAR を 30 個複製)。
  v3.1.2 時点の実測は visible 12 件が 0.801〜0.860 秒、候補 #130 が 4.021 秒
  (worker 累積 3,257.5ms)。2 回目の走査を消せばここがさらに下がるはず。
- 規模 / 優先度: 中 / P2。表示は既に出るようになったので体感の急所ではないが、
  同じ I/O を 2 回やっている事実は残っている。

### 2.6 ZIP / RAR のダブルクリックが時々無反応 — 原因確定・修正済み、利用者の確認待ち

- 報告条件: サムネイル表示、ZIP / RAR、Enter では開ける。最初のダブルクリックでは開かず、
  そのまま待つだけでも開かないが、しばらくして再度ダブルクリックすると開くことがある
  (専用スレ >>257 で訂正)。RAR はローカル上の直読み対象。
- **原因確定 (2026-08-19、報告環境の perf log)**: egui は
  `count = if triple_click { 3 } else if double_click { 2 } else { 1 }` で数え、`is_double()` は
  **`count == 2` の完全一致**。`triple_click` は `max_double_click_delay * 2` を**前々回のクリック**から
  測る (`egui-0.33.3/src/input_state/mod.rs:1213-1222`, `1006-1011`)。つまり **3 回目のクリックは
  triple になり `double_clicked()` が false になる**。1 回目で開かなかった利用者はもう一度
  クリックするので、その 3 回目がちょうどここに落ちていた。
  - 実測 (idx 9、同一セル、同一座標): 離す時刻 165.735 / 166.384 / 166.612。3 回目は前回から
    **228ms** で double 成立圏内だが、前々回から **877ms** で `2 x 500ms` の内側 → triple 判定。
    同 session の成功例は 400ms 間隔の 2 回目。**400ms が成立して 228ms が成立しない**という
    逆転がこれで説明できる。
  - v3.1.2 のダブルクリック時間の OS 追従はこれを**悪化させた** (triple 窓 600ms → 1000ms) が、
    原因ではない。300ms でも 600ms 以内に 3 回クリックすれば同じで、**本項は v3.1.1 以前からあった**。
- **修正 (v3.1.2)**: グリッドは egui の click count を起動判定に使わず、`response.clicked()` で
  click 成立だけを受け取る。同じ `items_generation`・同じセル idx・OS 由来のダブルクリック時間内に
  ある 2 click を自前の単一 pairing state で対にし、item activation 後・セル以外の primary click・
  一覧世代変更で対を切る。「開く → Esc → 単発クリック」で再度開いてしまう追補も同時に閉じた。
  egui 本体は変更していない (triple click は text field の行選択が使うため)。
- **状態: 利用者の再確認待ち**。再発報告が来なければ close する。再発した場合に使える観測手段:
  - `grid/cell_signal` が成功 / 失敗を問わず `time_since_last_click`、`max_double_click_delay`、
    `clicked_by_primary`、`double_clicked_by_primary` を出す。失敗例だけを見ず、同じ session の
    成功例を control として比較する。
  - `grid/activation_request accepted=true` と `grid/activation_dispatch_complete` があれば click は
    成立しているので、以降の dispatch / open 側を疑う。`double_clicked_by_primary=false` かつ
    `first_click=true` なら pointer click として成立していない側を疑う。
  - 依頼手順: 開発者で性能ログ ON → 再起動 → 症状再現 → **再起動せず**「ログを zip にする」→
    診断 ZIP を送付。ログにファイル名 / path が含まれる既存の注意書きも案内する。
- 優先度: P2。原因が確定するまで guard / retry / 閾値再調整の症状修正を入れなかった方針は、
  再発時も維持する。

### 2.10 グリッド以外の egui ダブルクリック消費箇所に場所条件が無い

- 出典: v3.1.2 のダブルクリック対応の追補として行った `Response::double_clicked()` 全箇所棚卸し
  (2026-08-19)。egui 0.33.3 は
  前回クリック位置を持たないため、同じ context 内の離れた widget / 領域のクリックも時間内なら
  ダブルクリックとして各 consumer へ届き得る。
- 今回は利用者報告のあったグリッドセルだけを修正し、本項の consumer は変更しない。現存する
  8 分岐は、詳細表示の列幅 best-fit、360° navigator の視点移動、通常 navigator の zoom 中心移動、
  分析表示の zoom reset、通常 fullscreen の transform reset、360° canvas の視点 reset、native
  presenter egui overlay の音量 reset と再生速度 reset。
- native HWND が `WM_*BUTTONDBLCLK` から作る `double_click` は Windows 自身の時間・距離判定を
  通るため本項の対象外。着手時は一律のピクセル閾値を足さず、各 consumer の意味単位 (同じ
  handle / navigator / canvas / slider など) と、別 widget クリックで pair を切る owner を決める。
- 優先度: P2。latent 誤操作の棚卸し項目で、実害報告が出た consumer から個別に再現確認する。

### 2.4 CSV / TSV からの一括タグ / レーティング付与 — 保留

- 出典: 同じメール往復。こちらから代替案として提案し利用者も歓迎したが、**利用者の実際の
  使い方 (参照は一時的で、タグを付けても後から参照しないことが多い) とは噛み合わない**ため、
  RAR フォルダのサムネイル表示遅延 (v3.1.2 で対応済み) を優先すると回答した。
- 位置づけ: 外部ツールで抽出した結果を mIV へ持ち込む導線としては筋が良い。単独では需要が
  薄いので、タグ運用側の要望が別に出たときに合わせて再判断する。
- 実装するときの前提 (利用者へ明言済み): **明示的な「取り込み」操作のときだけ動く**こと。
  パスの一覧をビューとして開く形 (実体のない仮想フォルダ) は採らない。理由は、他人由来の
  リストで任意パスを参照してしまうこと、UNC パスなら開いた瞬間に外部へ認証情報が飛び得る
  こと、実フォルダ前提の処理 (サムネイルの識別キー、移動 / 削除時の扱い、各種設定の保存先)
  への影響範囲が大きいこと。
- 優先度: P3 / 保留。

### 2.18 リモート: サムネイルカタログが開けないだけでページ表示が失敗する — 利用者報告

- 出典: 利用者報告 (2026-08-23、v3.2.0 出荷前の実機確認)。リモートで PDF を開いたとき、
  端末に「ページ表示グループの読み込みに失敗しました。」が出た。前後ページ移動で復帰。
- **v3.2.0 の退行ではない。** この経路は `732420e3` (2026-07-31) 由来で v3.1.3 以前から出荷済み。
- **サーバ側ログで確定** (`%APPDATA%\mimageviewer-remote
emote-web-log.jsonl`):

  ```
  /api/page  status=500  page.ipc_status="miv_media_internal"
  → client: spread_load_error 「ページ表示グループの読み込みに失敗しました。」
  ```

  本体ログ (`mimageviewer.log`) の同時刻:

  ```
  [216.063s] remote_ipc: container catalog open failed: disk I/O error
  [216.063s] media_operation operation=page source_kind=pdf outcome=error
  ```

- 機構: [container.rs:5222](../src/remote_ipc/container.rs:5222) が**ページ要求のたびに
  `CatalogDb::open` し、失敗するとページ全体を `MediaErrorCode::Internal` で落とす**。
  しかしこのカタログは `decode_remote_source` に渡す**キャッシュの高速化用**で、
  ページ描画には必須ではない。**「速くするための仕組みが開けなかった」だけで表示が失敗している。**
- 発生条件: 直前の 214.28〜214.34s に PDF サムネイル要求が十数件同時にカタログを読んでおり、
  その混雑中に 1 回だけ SQLite が `disk I/O error` (`SQLITE_IOERR`) を返した。
  ログ全体でこの失敗は 1 回だけ。次の要求では成功しており、利用者の「前後移動で復帰」と一致する。
- **同時刻の `/api/page` 503 は別物**。`admission_busy` = IPC 同時実行上限 (heavy 4/4、
  prefetch 3/3) による正常な背圧なので、500 と混同しない。
- 直す方向: この経路でカタログを**省略可能にする** (`Option<Arc<CatalogDb>>`)。
  開けなければログを残してキャッシュ無しで続行する。表示は遅くなるが成功する。
  - **リトライ・待ち時間・閾値調整で吸収しない。**問題は「任意の加速機構の失敗を
    致命的に扱っていること」であって、開けなかったこと自体ではない。
  - 同じ形が他の remote 経路にもないか確認する (`CatalogDb::open` の `?` 伝播)。
  - ついでに、ページ要求ごとに開き直す構造自体も見直す価値がある (§2.13 と同型)。
- 規模 / 優先度: 小 / P2 (頻度は低いが、体験は「表示できない」なので重い)。

### 2.12 サムネイルキャッシュのクリアが `-wal` / `-shm` を残す — 実測 1.4 GB

- 出典: 2026-08-20、利用者がキャッシュクリア後に計測しようとして発見。
- **実測** (`%APPDATA%\mimageviewer\cache` 配下、クリア + 再起動後):

  | 種別 | 件数 | サイズ |
  | --- | ---: | ---: |
  | `.db` (本体) | 2 | 0.2 MB |
  | `.db-wal` | 1,410 | **1,394.8 MB** |
  | `.db-shm` | 1,410 | 44.1 MB |

  サンプルした `-wal` はすべて**対応する `.db` が存在しない孤児**だった。
- 機構: カタログは `cache/<hash 先頭 2 桁>/<hash>.db` (`catalog.rs` の `db_path_for`)。
  クリアが**その `.db` だけ**を消していると思われる。SQLite は正常な close で `-wal` / `-shm` を
  自分で削除するので、**閉じずにファイルを消すと残る**。
- 実害:
  1. **容量が解放されない。**「クリアしたのにディスクが空かない」= クリアがほぼ無意味に見える。
  2. 同じフォルダを再訪すると、その path に新しい `.db` が作られる一方、古い `-wal` が残っている。
     SQLite は salt 不一致の WAL を無視するので即座の破損は起きにくいが、**消したはずの物が残った
     状態で新規 DB を作る**のは設計として健全でない。
- 直す方向: **削除前に接続を確実に閉じる**のが構造的に正しい (閉じれば SQLite が自分で消す)。
  閉じられない経路があるなら、その理由を確認してから `-wal` / `-shm` の明示削除を足す。
  **どの経路が `.db` を掴んだまま消しているのかを先に特定する** (症状として `-wal` を消して回らない)。
- 既存の 1.4 GB は、アプリ終了後に `cache` 配下の孤児 `*.db-wal` / `*.db-shm` を削除すれば安全に回収できる。
- 規模 / 優先度: 小〜中 / P2 (実害は容量。利用者が明示的に「クリア」を選んだのに効いていない)。

### 2.13 PDF は 1 ページ描くたびに文書を開き直している

- 出典: 2026-08-20、PDF ワーカーのレーン容量を直した後、並列度より効く可能性がある側として確認。
- **機構**: `core_render_with_count` ([pdf_loader.rs:1303](../src/pdf_loader.rs:1303))、
  `core_analyze_page` ([1276](../src/pdf_loader.rs:1276))、`core_get_info` ([1165](../src/pdf_loader.rs:1165))、
  `core_enumerate` ([409](../src/pdf_loader.rs:409)) が、**要求ごとに** `load_pdf_from_file` を呼ぶ。
  子ワーカーは `Pdfium` インスタンスは持ち越すが ([1000](../src/pdf_loader.rs:1000) 付近)、
  **開いた文書は持ち越さない**。
- 実害の見積り (未実測):
  1. 1 冊を 10 ページ描くと **10 回開き直す**。開くたびに xref / trailer を読み直す。
  2. ワーカーは 5 個あり、**同じ 1 冊の別ページを並列に描くと、同じ文書を同時に 5 回開く**。
     HDD では xref (ファイル末尾) と本文 (途中) の間で seek が往復する。
  3. ページ数が多い本ほど不利。フルスクリーンのページ送りは同じ文書への連続要求なので、
     いちばん効く場所で毎回捨てている。
- **ファイルハンドルは保持される**: `load_pdf_from_file` は `File::open` したハンドルを
  文書の生存期間ずっと持つ (`FPDF_LoadCustomDocument` に reader を渡す実装)。
  ただし **2026-08-20 に実測した結果、掴んだままでも `rename` / `remove_file` /
  シェル経由のゴミ箱移動はすべて成功する** (Rust の `File::open` は `FILE_SHARE_DELETE` を
  含めて開くため)。動画デコーダが共有違反を起こす件 ([app.rs:26870](../src/app.rs:26870) の
  コメント) とは事情が違う。**掴むことを理由にした解放タイマーは要らない。**
- **本当の危険は ABA**: 削除・差し替えの後もワーカーは古い文書を握り続けるので、
  同じパスに別のファイルが来たら古い中身を返す。
- 直す方向: **ワーカープロセスごとに「直前に開いた 1 冊」だけを保持する。**
  キーは path だけでなく **(path, パスワード, mtime, サイズ)** にし、**要求ごとに stat して
  確認する** (stat は開き直しに比べて無視できる。mtime / file_size は enumerate の応答が
  既に運んでいるので、mIV 内の PDF 同一性判定として一貫する)。
  判定は純関数 (前回のキー × 今回のキー → 再利用 / 開き直し) に切り出せば PDFium 無しで
  テストできる。**時間窓は使わない。**
- アイドル時のメモリ (見終わった本を各ワーカーが抱え続ける) が問題になるかは**先に実測する**。
  効くなら時計ではなく既存の決定的な信号 (フォルダ移動時の `bump_render_context_epoch`) に
  載せて解放する。
- 実装上の引っかかり: pdfium-render の `load_pdf_from_file` は `password: Option<&'a str>` と
  `&'a self` が同じ寿命なので、文書を長く持つとパスワード文字列も同じだけ生かす必要がある
  (PDFium 自身はコピーするので API 側の過剰制約)。パスワード付き PDF のために小さな回避が要る。
- 着手順: **in-process フォールバック削除の後**。`core_*` の呼び出し元が 2 系統から 1 系統に
  なり、この分割の手数が半分になる。
- **測る先**: 現在の perf イベントは main プロセス側の queue / dispatch だけで
  ([pdf_loader.rs](../src/pdf_loader.rs) の `crate::perf::event` は全て pool 側)、
  **ワーカー内部の「開く」と「描く」は分離して測れていない**。
  先に子ワーカー側へ計装を入れて、開くコストの実測を取ってから直す。
- 規模 / 優先度: 中 / P2 (ワーカー数の調整より効く可能性がある。先に実測する)。

### 2.14 PDF のフルスクリーンが PDFium 経路を 2 本持っている

- 出典: 2026-08-20、ワーカー起動失敗の調査 (フェーズ 1) で判明。
- **機構**: 同じフルスクリーン表示なのに、初回ページと再レンダリングで別の PDFium を使う。

  | | 初回ページ | 再レンダリング |
  | --- | --- | --- |
  | 入口 | `render_page_for_display` ([app.rs:50276](../src/app.rs:50276)) | `render_page_async` ([pdf_loader.rs:3502](../src/pdf_loader.rs:3502)) |
  | 実行場所 | ワーカープロセス 5 個 | **メインプロセス内の 1 スレッド** |
  | Critical 予約 | 効く | 効かない |
  | レーン分割・昇格 | 効く | 効かない |
  | context epoch の間引き | 効く | 効かない |

  再レンダリングはズームだけでなく、`ensure_pdf_display_resolution` による表示解像度合わせ
  (現ページ + 見開き相方、[ui_fullscreen.rs:13561](../src/ui_fullscreen.rs:13561)) も通る。
  つまり**通常の PDF 閲覧が日常的にこの経路を使う**。
- **`render_page_async` はフォールバックではない**。`worker_count` を見ずに常に
  `get_worker()` を使う専用経路で、呼び出し元は [app.rs:50765](../src/app.rs:50765) の 1 つだけ。
  ワーカー起動失敗の対応 ([briefs/pdf-worker-startup-failure.md](briefs/pdf-worker-startup-failure.md))
  では**意図的に残した**。
- 移すかどうかの論点 (2026-08-20 に整理):

  **ズーム連打では並列化は効かない。** 新しいズーム要求は前の要求をキャンセルする
  (`fs_pending.remove(&idx)` → `cancel.store(true)`、[app.rs:50710](../src/app.rs:50710) 付近) ので
  キューに積み上がらず、無駄になるのは実行中の 1 件だけ。その実行中の PDFium レンダリングは
  **プールでも中断できない** (`CancelWaitPolicy` は結果の扱いであって PDFium を止める話ではない)。
  要求は互いに上書きするので、並列に走らせても欲しいのは最新の 1 件だけ。

  **効く可能性があるのは 2 ケース:**
  1. **AI 先読みのレンダリング中にズームしたとき。** in-process は `priority_rx` を先に drain
     するが、それはキューの順序を変えるだけで、**既に走っている非 priority の先読みが
     終わるまで待つ** (実測で 1.5 秒級)。プールなら先読み = Normal、ズーム = Critical で、
     予約により最低 1 ワーカーが空いているので即座に始まる。
  2. **見開きの左右ページ。** 2 ページとも必要な独立した処理なのに今は 1 スレッドで直列。
     Critical は lane cap の対象外なので、プールなら同時に走れる。

  **払うコスト: ビットマップの IPC 転送。** ワーカーの応答は**無圧縮の生 RGBA**
  ([pdf_loader.rs:1219](../src/pdf_loader.rs:1219))。ズーム再レンダリングは長辺 8192px まで
  行くので、A4 比なら 8192 × 5794 × 4 ≒ **190 MB をパイプに流す**。in-process は mpsc で
  ポインタを渡すだけ。プールが `serialize_us` / `write_us` / `wire_bytes`
  ([pdf_loader.rs:494](../src/pdf_loader.rs:494)) を計測しているのは、このコストが効くから
  (初回ページも 4K ビューポートで約 33 MB 払っている)。

  | | 得るもの | 払うもの |
  | --- | --- | --- |
  | プールへ移す | 先読み実行中の待ち解消、見開きの並列化 | ズーム解像度で最大 190 MB の直列化 + パイプ転送 |

- **有力な第 3 案: in-process にいる 2 つを分ける** (2026-08-20 追記)。
  `render_page_async` を使っているのは性質が正反対の 2 つで、まとめて扱う理由が無い。

  | 用途 | 呼び出し元 | 性質 |
  | --- | --- | --- |
  | ズーム / 表示解像度合わせ | `request_pdf_rerender(.., true)` ([ui_fullscreen.rs:6960](../src/ui_fullscreen.rs:6960)) | **利用者操作に同期。低遅延が要る。最大 8192px と巨大** |
  | AI 先読み用の native 再レンダ | `prefetch_final_ai` → `request_pdf_rerender(idx, 1.0, false)` ([app.rs:52845](../src/app.rs:52845)) | **完全にバックグラウンド。遅延は問題にならない** |

  なお**通常のページ先読みは既にプール側**なので競合しない
  (`ensure_fs_page_load` → `start_fs_load` → `render_page_for_display`、
  [app.rs:56551](../src/app.rs:56551))。in-process にいるのは上の 2 つだけ。

  **AI 用 native 再レンダだけをプールへ移す**と:
  - ズームは in-process のまま = レスポンス劣化の懸念が無い
  - in-process スレッドがズーム専用になり、**先読みに塞がれなくなる** (= 上の「効く 2 ケース」の
    ①がプール移行なしで解消する)
  - IPC 転送コストは AI 側が払うが、そちらは元々バックグラウンド
  - 見開き並列 (②) は解決しない

  **①が主因だと実測で分かったら、この案を先に検討する。**

- **測るもの** (上の表がそのまま測定項目になる):
  1. **先読み実行中のズーム要求の待ち時間** — in-process (先読み完了まで待つ) vs
     プール (予約ワーカーで即開始)。
  2. **ズーム解像度での IPC 転送コスト** — 既存の `WorkerRenderMetrics` がそのまま使える。
     長辺 2048 / 4096 / 8192 で `serialize_us` + `write_us` を取る。
  3. **見開き 2 ページの完了までの時間** — 直列 vs 並列。

  1 と 3 の得が 2 の損を上回るかで決める。§2.13 の計装と同時に入れると安い。
- 規模 / 優先度: 中 / P3 (現状で壊れてはいない。実測して有意差が出たときに動かす)。

### 2.15 PDF: 利用者が待っている open を、背景の一斉 open に埋もれさせない ✅ 実装済み (2026-08-21)

> §2.16 と同じ 1 つの機構で実装した。設計と経緯は
> [briefs/pdf-open-admission.md](briefs/pdf-open-admission.md)。
> **実機確認済み (2026-08-21)**: 「サムネイルがまだ出ていない状態でも PDF ページを開ける」。
> 同じ本のページ送りの速度に変化なし。以下は着手前の記録。

- 出典: 利用者報告 (2026-08-20)。「複数ウィンドウを開こうとしているとき、ダブルクリックで
  画像がなかなか開かない」。3 回クリックし、待ちが 1.5 → 2.2 → 3.7 秒と積み上がった。
- **実測** (利用者実機の perf log、`worker_open_ms` を分離したので分かった):

  | 全 render の `worker_open_ms` | p50 | p90 | p99 | 最大 |
  | --- | ---: | ---: | ---: | ---: |
  | | **0.3 ms** | 763 ms | 5,730 ms | **10,803 ms** |

  1 秒超の open が 75 件あり、**固まって発生**している:

  ```
  t=11.9 open=5744ms  20230103_008.pdf     t=13.8 open= 7311ms 20230103_010.pdf
  t=12.4 open=6252ms  20230103_001.pdf     t=16.5 open=10314ms 20230103_009.pdf
  t=13.1 open=6955ms  20230103_005.pdf     t=17.0 open=10803ms 20230103_006.pdf
  ```

  **別々の PDF を一斉に開いている区間** (フォルダのカバーサムネイル生成)。対象ファイルは
  HDD 上にあり、5 ワーカーが同じディスクを取り合う。
- **重要**: 遅いのは**ファイルではなく状況**。同じ `20221030_001.pdf` は他の時点で
  **0.4 ms** で開いている (数十サンプル)。当初「病的なファイル」と誤診したが、
  利用者の指摘で確認して訂正した。
- **Critical レーンの予約はワーカーを 1 つ確保するだけで、ディスクは確保しない。**
  他の 4 ワーカーが大きな PDF を読んでいる間、予約されたワーカーも同じディスクを待つ。
- 直す方向: **前面の PDF open が待っている間、背景 (カバーサムネイル生成・先読み) の open を
  走らせない。** スクロール中の先読み抑制 (`decide_prefetch_allowed`、
  [prefetch-suppression-during-scroll-plan.md](prefetch-suppression-during-scroll-plan.md)) と同じ形。
  **時間窓ではなく「前面が待っているか」で決める。**
- **設計原則 (利用者、2026-08-21)**: **利用者の直接の操作に応答することが最優先**。
  資源が競合したら、背景仕事より前面を通す。
- 規模 / 優先度: 中 / P2。
- 関連: §2.16 (同時 open 数の上限)。両者は補完関係で、こちらは**優先度**、あちらは**総量**。

### 2.16 PDF: open と render を別の資源として扱う (同時 open 数の上限) ✅ 実装済み (2026-08-21)

> 実装は `MSG_OPEN` を新設して open を独立した IPC 要求にする形になった。当初案の
> 「job 単位で許可枠を数える」は、枠が render まで覆って SSD の throughput を落とすため
> 採らなかった。詳細は [briefs/pdf-open-admission.md](briefs/pdf-open-admission.md) §3.1。
> 以下は着手前の記録。

- 出典: 2026-08-21、§2.15 の議論から。利用者提案「PDFium 並列度 10、同時 open は 3 まで」。
- **実測が支持している**: 7.4 秒かかった要求の内訳は

  ```
  worker_open_ms   = 7371.8   ← 競合を全部吸収している
  worker_page_ms   =   11.5
  worker_render_ms =  154.9   ← 平常時 (100〜170ms) と変わらない
  ```

  **競合の最中でも描画時間は動かない。** つまり:
  - **open はディスク律速** — HDD で同時に走らせるとシークが往復して待ちが跳ねる
  - **render は CPU 律速** — 並列度を上げた分だけ効く

  **性質が正反対のものを、同じ並列度で縛っているのが現状の構造。**
- 直す方向: **render の並列度は保ったまま、同時 open 数だけを上限で絞る。**
  - **親側の dispatch で絞る** (ワーカー内部で待たせない)。親はどのワーカーへどの文書を
    送ったかを知っているので、**「そのワーカーが保持している文書と違う要求 = open が要る」と
    予測できる**。§2.13 で入れた文書キャッシュがこの予測を成立させている。
    ワーカー間の名前付きセマフォのような仕掛けは要らない
  - **ストレージ種別を自動判定しない。** HDD / SSD を実行時に見分けて挙動を変えるのは
    「実行時状態で挙動を変えない」方針に反する。**固定値か設定値**にする
- 規模 / 優先度: 中 / P2 (dispatcher の資源管理に手を入れるので、出荷直前には入れない)。
- 関連: §2.15 (前面優先)、§2.13 (文書キャッシュ)。

### 2.17 フォルダ代表サムネの既定ソートが一覧の既定と食い違う (番号順 vs ファイル名順) — 利用者報告 ✅ 実装済み (2026-08-22)

> `default_folder_thumb_sort` を `FileName` にし、`schema_meta.folder_thumb_sort_default_v2` marker による
> 一度きりの移行を追加。**値と marker は同じ transaction で commit する** (marker だけ残ると次回起動が
> 「移行済み」と判断して番号順のまま取り残されるため)。**移行の失敗は致命的にしない** — ログを残して
> 保存済み設定のままロードし、marker が立たないので次回再試行する (load 全体を失敗させると
> `FailedFallbackDefault` でそのセッションが既定設定 + save 抑止になり、全利用者が 1 回だけ通る移行の
> 代償として重すぎる)。`natural_sort_key` は変更していない。3.2.0 の `version_highlights` で戻し方を告知。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 2026-08-22。`00表紙.jpg` / `00表紙2.jpg` の 2 枚だけが入ったフォルダで、一覧の先頭は
  `00表紙.jpg` なのに、フォルダタイルの代表サムネだけ `00表紙2.jpg` になるという報告。
- **どちらも既定値のまま**で起きる (利用者が設定を変えたせいではない)。既定が 2 つに分かれている:

  | 設定 | 既定 | 定義 |
  | --- | --- | --- |
  | 一覧の並び順 `sort_order` | ファイル名順 | [settings.rs:5388](../src/settings.rs:5388) (`SortOrder::default()` = `FileName`)、固定テスト [settings.rs:9564](../src/settings.rs:9564) |
  | 代表画像の選択基準 `folder_thumb_sort` | 番号順（区切り無視） | [settings.rs:5126](../src/settings.rs:5126)。導入コミット `e6793dbb` (2026-04-12) から不変 |

- 機構: 代表画像の自動選定は [thumb_loader.rs:2349](../src/thumb_loader.rs:2349)
  `resolve_folder_thumb_image_inner`。**一覧の `sort_order` は見ず** `folder_thumb_sort` で
  並べて先頭 1 枚を採る。実ログでも確認済み:
  `resolve_folder_thumb_image: ... sort=Numeric depth=3 -> ...\00表紙2.jpg` /
  保存キー `folderthumb:auto-v2:numeric:d3:...`。ピン (`folder_thumb_pins.db`) は無関係。
- 番号順で反転する理由: `natural_sort_key` ([ui_helpers.rs:979](../src/ui_helpers.rs:979)) は
  記号・空白を捨てて数字塊 / 文字塊に割るが、**拡張子も直前の文字塊に溶ける**。
  - `00表紙.jpg` → `[数字0, "表紙jpg"]`
  - `00表紙2.jpg` → `[数字0, "表紙", 数字2, "jpg"]`

  2 要素目が `"表紙" < "表紙jpg"` (前方一致で短い方が小) なので、**数字が付いている方が勝つ**。
  `cover.jpg` vs `cover2.jpg`、`a.jpg` vs `a10.jpg` も同型 (連番同士 `001` / `002` は期待どおり)。
  ファイル名順 (Windows ソートキー) なら `00表紙.jpg` が先頭になる (`LCMapStringEx` の
  ソートキー比較・`StrCmpLogicalW` の双方で確認)。
- **直す方向 (利用者判断済み)**: 既定をファイル名順に合わせる。ただし `default_folder_thumb_sort`
  を変えるだけでは**新規インストールにしか効かない**。[settings_db.rs:1550](../src/settings_db.rs:1550)
  `write_settings_kv` が Settings の全フィールドを毎回書くので、既存利用者の `settings_kv` には
  全員 `folder_thumb_sort = "Numeric"` が入っており、**「保存されている」= 「利用者が選んだ」とは
  判別できない**。そこで更新時に一度だけ移行する:
  1. `schema_meta` に一度きりのフラグ (例 `folder_thumb_sort_default_v2`) を置く。既存の
     `bootstrap_complete` / `migrated_from_json_at` と同じ `INSERT OR IGNORE` の形で足せる。
  2. フラグが無い既存 DB では、起動時に一度だけ `folder_thumb_sort` を `FileName` へ書き換えて
     フラグを立てる。クリーンインストールは bootstrap 時にフラグだけ立て、値は触らない。
  3. 副作用: 意図して「番号順」を選んでいた利用者も 1 回だけ戻る (上記のとおり区別できない)。
     §5.0 経由で `version_highlights` の `must_read` に載せ、戻し方 (環境設定 → フォルダ・ファイル →
     「代表画像の選択基準」) を明示する。
  4. 代表サムネのキャッシュキーにソート順が入る (`...:numeric:...` → `...:filename:...`) ので
     選び直しは自動で走る。旧キーのエントリが残って容量だけ増えないかは実装時に確認する。
  5. マニュアルの既定表記 ([settings.html:470](../htdocs/mimageviewer/manual/settings.html:470)) を
     同時に更新する。
- 実装前に決める枝: 「番号順の natural key から拡張子を除く」案でも
  `[0,"表紙"] < [0,"表紙",2]` となり番号順のまま期待どおりになる。ただし `natural_sort_key` は
  一覧ソート ([app/folder_scan.rs:132](../src/app/folder_scan.rs:132))、ファイル名スタック、
  ZIP ツリー、スマートフォルダが共有しており、**番号順を選んでいる全一覧の並びが変わる**。
  既定の食い違いを消すだけなら上の移行案の方が影響範囲が小さい。
- 規模 / 優先度: 小 (既定値 + 一度きりの移行) / P2。

### 2.19 バックグラウンドインデクサの残り時間が「まだ出せない」と「壊れている」を区別できない — 利用者報告

- 出典: 2026-08-23。お気に入り編集ダイアログの「🔄 バックグラウンドインデクサ」に残り時間が出ておらず、
  機能を消したのか不具合かという問い合わせ。しばらく待つと `[残り 02:12 (215件/秒)]` が出た
  (= スキャンから取込フェーズへ進んだ)。**現行仕様どおりの挙動で、退行ではない。**
- 機構: 残り時間は「残件数 ÷ 直近レート」で出しているので、**全体件数 N が確定しているフェーズ**でしか
  計算できない。

  | フェーズ | 呼び出し | 残り時間 |
  | --- | --- | --- |
  | 削除 (n/N) / 取込 (n/N) | `set_msg_and_count` ([ingest_worker.rs:218](../src/ingest_worker.rs:218), [:253](../src/ingest_worker.rs:253), [name_bulk_indexer.rs:150](../src/name_bulk_indexer.rs:150)) | 出る |
  | スキャン: `<dir>` (n 件) | `set` ([search_walker.rs:246](../src/search_walker.rs:246)) | 出ない |
  | 取込待ち (writer 使用中) | `set` ([indexer_supervisor.rs:534](../src/indexer_supervisor.rs:534)) | 出ない |
  | 名前索引の更新 / 更新スキャン | `set` ([name_index_supervisor.rs:386](../src/name_index_supervisor.rs:386), [:673](../src/name_index_supervisor.rs:673)) | 出ない |

  counted フェーズに入った直後もサンプル 2 点かつ件数が進むまでは `remaining_secs=None`、
  取込完了時は `clear_count()` で消える ([indexer_progress.rs](../src/indexer_progress.rs))。
- 問題: 「まだ計算できない」「そもそも総数の概念がない」「壊れている」が**すべて同じ無表示**で、
  利用者から区別できない。大きいフォルダ / HDD・NAS / 操作中で ActivityGate が walk を止めている
  条件ではスキャンが数分続き、その間ずっと空欄のままになる。
- 直す方向: 空欄をやめ、フェーズに応じた理由を出す (例: スキャン中 = `[件数集計中…]`、
  counted 直後 = `[残り 計測中…]`、差分更新 = 表示しない)。
  - **理由は型で持たせる。** 現状 UI は `eta: Option<EtaSnapshot>` の有無しか見ておらず、None の理由
    (カウント未設定 / サンプル不足) を区別できない。`ProgressReporter` 側に理由を持たせ、UI はそれを
    表示するだけにする。**メッセージ文字列の前方一致で "スキャン:" を判定するのはやらない**
    (文言を変えた瞬間に静かに壊れる)。
  - `aggregate_total_eta` ([favorites_editor.rs:938](../src/ui_dialogs/favorites_editor.rs:938)) は
    「残り = max(各 remaining)、速度 = Σ(各 rate)」で集約している。理由付き None が混ざったときの
    集約規則 (1 つでも計測中なら全体も計測中扱いにするか) を決める。
  - タイトルバー側の ETA 表示 ([app.rs:10296](../src/app.rs:10296) のキャッシュ経由) も同じ扱いに揃える。
- 規模 / 優先度: 小 (表示のみ。索引の動作自体は変えない) / P3。

### 2.20 別ウィンドウで動画が切り替わると、静止画ウィンドウの画像が縦横比の狂った状態で固着する — 利用者報告

> ⚠ detached viewer リワーク中の領域。**症状パッチを入れない** (CLAUDE.md 凍結ルール)。
> 分類は **BA-7** (bundle 外の App-global 状態が別 context から汚染される)。
> findings-12 D3 (book open が main context 経由で `auto_aspect` を汚し、メイングリッドが
> アスペクトリフローした) と**同型**。

- 出典: 2026-08-23。静止画をフルスクリーンで見ている最中に、別ウィンドウで動画が切り替わった
  タイミングで、静止画が**圧縮されて縦横比の狂った状態**で表示された。**画像を開き直すと復帰**
  (= 1 フレームのちらつきではなく、再解決まで残る状態)。
- ログで確認できた事実 (`mimageviewer.log`、セッション経過 6284〜6296s):
  - 独立した 2 つの viewer session が同時に開いていた。
    session 31 = 静止画 (`g:\home\comfyui\...`、items_gen=24 / 300 件、表示中 idx=96 =
    `mistblossom_gpt_4_6...png` **896x1152**)、session 4 = 動画 (`c:\home\youtube\...`、
    items_gen=6 / 171 件)。
  - **動画側の切り替えが App-global の `open_fullscreen` を通っている。**
    `[6285.031s] === open_fullscreen: idx=2 ===` は利用者が静止画側で操作した記録ではなく、
    動画ウィンドウの swap ([app.rs:44508](../src/app.rs:44508))。
    9 秒間に 6 回 (6284.9 / 6285.8 / 6290.7 / 6291.7 / 6292.3 / 6293.0) 発生している。
  - つまり静止画ウィンドウの描画中に、`fullscreen_idx` を含む App-global が動画ウィンドウの
    都合で書き換えられていた。
- **コード読みで確認できた構造** (この症状の原因と断定はしていない。候補の絞り込み):
  - `DisplayedImageTransform::resolve` は `full_image_rect = display_size * total_scale` の
    一様スケールなので、**この経路では縦横比は歪められない**
    ([displayed_image_transform.rs:167](../src/displayed_image_transform.rs:167))。
  - 歪みが表現できるのは `from_resolved_rect` だけ。呼び出し側が渡した矩形に対して
    `scale_x` / `scale_y` を別々に出し、**食い違っていても平均して受け入れる**
    ([displayed_image_transform.rs:183](../src/displayed_image_transform.rs:183))。
  - この穴は認識されていて、`resolve_fs_transform_in_layout_rect` が「レイアウト矩形は枠に
    すぎないので、実テクスチャから最終矩形を出し直す」ガードとして用意されている
    ([ui_fullscreen.rs:4094](../src/ui_fullscreen.rs:4094) の doc comment が
    "no caller can publish a non-uniform pixel scale" と明記)。
  - **単ページ ([ui_fullscreen.rs:24707](../src/ui_fullscreen.rs:24707)) と連結読み
    ([:29060](../src/ui_fullscreen.rs:29060)) はこのガードを通る。通っていない呼び出しが 2 つある**:

    | 場所 | 渡している矩形 | 由来 |
    | --- | --- | --- |
    | 見開き左右 ([:10381](../src/ui_fullscreen.rs:10381) / [:10397](../src/ui_fullscreen.rs:10397)) | `rects.left_rect` / `right_rect` | `spread_layout_geometry(left_size, right_size, ...)` = **ページ寸法**。`texture_size` には実テクスチャを渡すので、両者の比が食い違えば歪む |
    | detached の凍結スナップショット ([:10172](../src/ui_fullscreen.rs:10172)、`detached_continuous_frozen_pages_for_snapshot`) | `page.rect` | レイアウトが決めた矩形をそのまま渡している。直前に一様な `logical_scale` を計算しているのに、矩形側には反映していない |

  - 後者は **detached ウィンドウが凍結描画に落ちるときの経路**で、「別ウィンドウ側の操作で
    こちらが凍結スナップショットに切り替わる」という今回の状況と形が合う。**ただし利用者が
    単ページ / 見開き / 連結読みのどれで見ていたかはログに出ていない** (`spread` / `連結` の
    ログ出力が 1 件も無い) ので、経路は特定できていない。
- **Codex 追補 (2026-08-23、ClaudeCode 分析との差分)**:
  - perf ログでは、静止画を開いた直後の `896x1152` final composite は
    `scale_x=scale_y=1.188666`、Lanczos も `1597x2054` で、画像データ・texture・通常の
    transform は縦横比を保っていた。6293.759s の開き直しも同じ cache の hit だけで直っており、
    cache 内容の破損ではない。
  - 時系列では、動画の実 source swap (6285.031s) より先に、ParkedLive 動画窓のクリック復帰が
    6283.984s に queue されている。この復帰入口は、現在の静止画を
    `park_and_close_current_active_detached_viewer_for_media_handoff` で passive snapshot にしてから
    動画 bundle を active にする ([app.rs:40136](../src/app.rs:40136) →
    [:40414](../src/app.rs:40414) → [:42678](../src/app.rs:42678))。したがって、利用者からは
    「動画切替時」に見えても、歪んだ表示矩形が作られる候補時点は source swap より約 1 秒前の
    **active still → passive snapshot handoff** である。
  - この legacy still park は `build_active_detached_image_window_snapshot(None)` を呼ぶ
    ([app.rs:42583](../src/app.rs:42583))。`ctx=None` なので `frozen_continuous_pages` は必ず空になり、
    passive 描画は mode にかかわらず `window.image_rect_norm` の単画像 fallback を使う
    ([app.rs:41752](../src/app.rs:41752)、[ui_fullscreen.rs:10641](../src/ui_fullscreen.rs:10641))。
    今回の経路については、ClaudeCode 分析が未確定としていた見開き / 連結の
    `from_resolved_rect` 候補より、この fallback の方が直接対応する。
  - 静止画 host は 6282.159s に `rect=(-11,-11 3862x2110)` で登録されており、Windows の最大化
    outer rect と整合する。一方、maximized の placement 更新は実測 client size ではなく
    restore 用 seed の `w/h` を runtime に保持する ([app.rs:43781](../src/app.rs:43781))。
    snapshot bake はその placement の `w/h` で画像矩形を fit して正規化する
    ([app.rs:41689](../src/app.rs:41689)、[ui_fullscreen.rs:10242](../src/ui_fullscreen.rs:10242)) が、
    passive draw は正規化矩形を現在の `full_rect` へ X/Y 独立で戻すだけで、保存済み
    `image_dims` による contain / 一様 scale の再解決を行わない。restore 窓と最大化 viewport の
    アスペクトが違えば、この最後の写像だけで texture が非一様に圧縮され得る。
  - したがって、App-global `open_fullscreen` の書換えは handoff の起動条件ではあるが、それ自体が
    passive 静止画の geometry を汚染した証拠はまだない。Codex の第一候補は **BA-7 の global
    state 汚染ではなく、maximized 時の restore placement と live viewport を混ぜた snapshot
    geometry ownership の不一致**。数値の最終確認には、既存案の `from_resolved_rect` 計装に加え、
    snapshot bake の `placement/maximized/image_rect_norm` と passive draw の
    `full_rect/復元後 image_rect/scale_x/scale_y` を同じ window/session id で記録する必要がある。
    `from_resolved_rect` は bake 前の一様な矩形だけを見て正常終了し、その後の
    `rect_from_normalized` で生じる歪みを捕捉できない可能性がある。
- **次にやること (直す前に)**: 無言で受け入れている所を鳴らす。
  `from_resolved_rect` で `|scale_x - scale_y|` が閾値を超えたら、`page_idx` / `texture_size` /
  渡された矩形 / session を添えてログに出す。加えて active still → passive snapshot では、bake
  時の `placement/maximized/image_rect_norm` と draw 時の `full_rect/image_rect/scale_x/scale_y` を
  同じ window/session id で記録する。**推測で直さない** — 上の候補のどれでもない可能性が残って
  いる。再現手順は「静止画ウィンドウを開いたまま、別ウィンドウで動画を数回切り替える」で、
  利用者側では再現している。
- **直す方向**: 特定できたら、`from_resolved_rect` の一様でない矩形を**受け入れずに拒否**するか、
  全呼び出しを `resolve_fs_transform_in_layout_rect` 経由に寄せる。ガードが既に存在して
  文書化までされているのに 2 経路が素通りしている状態を、分岐追加ではなく入口の一本化で閉じる。
- 規模 / 優先度: 中 / P2 (表示の破綻だが開き直せば復帰する。凍結領域なので構造修正が前提)。
- **2026-08-23 追記 — §1.115 と根が同じ。** 最大化された別ウィンドウが表示されている間、
  placement 更新が**毎フレーム走り、82/82 が中身の変わらない書き込み**であることを
  `MIV_DETACHED_WINDOW_DEBUG` のログで確認した。maximized 分岐が実測 rect を捨てて restore 用
  seed の w/h を書き戻すため ([app.rs:43782](../src/app.rs:43782))、最大化中の placement は
  実ジオメトリを表さない。**Codex がここで指摘した「bake は placement の w/h、draw は最大化
  viewport の `full_rect`」というズレの供給源がこれ。** 直し方は §1.115 の案 A
  (現在ジオメトリと restore ジオメトリを別に持つ) で、両方の入力が同時に正しくなる。
- 関連: §1.115 (**同じ根**。最大化中のちらつき)、
  [docs/detached-rework-plan.md](detached-rework-plan.md) §2 (憲法) / BA-7、
  findings-12 D3 (同型の App-global 汚染)。

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

### 3.3 ページ送り中の AI アップスケールを待たない / 打ち切る

- 出典: 元画像プレビュー中の見開き 1 ページずらし停止 (2026-08-17 修正済み) の実 perf log。
  停止中に AI upscale 1.3s × 3 回が
  逐次実行され、5.22s / 4.30s の停止時間の大半を占めた。
- その修正は「元画像を描く frame が加工済み source を readiness として待つ」不整合だけを直した。
  AI コストそのものは変更しておらず、元画像表示以外でも通過先 / stale target の推論待ちが起こり得る。
- 方針: ページ送りで表示 target から外れた AI upscale を待たず、context-owned producer の cancel / 打切りと
  着地点だけの再開を ownership・generation・完了回収まで含めて設計する。時間窓や `Awaiting` の強制解除、
  AI 結果の silent fallback では直さない。
- 規模 / 優先度: 中 / P2 performance。

## 4. 入力カスタマイズ / マウス / ゲームパッド

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
- 補足: マスク編集モードで筆系ツールを選び、キャンバス上にカーソルがある場合だけの
  `Shift+ホイール` は筆半径変更の固定入力として別途実装する。一般の入力経路や上記互換フィールドへ
  接続せず、パネル上・筆以外・動画では消費しないため、本項の横断カスタマイズ再設計を再開したものではない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

### 4.5 非アクティブなウィンドウでは右クリックのリングが効かない — 利用者報告 ✅ 解決済み (§1.100 の修正による)

> 下記のとおり §1.100 と同一原因。§1.100 は実機確認済み (`8db282e3` + `5b83df3f` / `d2c19796` /
> `c71d8c08` + `4c5260a2` + `12f60c97`) なので本項も解決済み。経緯の記録として残す。

- ⚠ **§1.100 と同一症状であることが 2026-08-21 に確定した** (同じ 1 つの原因)。
  本項が持っていた追加事実 2 点 (「**アクティブにすれば動く**」= 非アクティブ窓でのみ効かない /
  「App グローバル状態が塞いでいる」仮説の**反証**) は §1.100 へ移した。
  **修正・仕様判断はすべて §1.100 側で行う。本項は経緯の記録として残す。**

- 出典: 2026-08-20 の利用者報告。複数ウィンドウ表示中、**新しいウィンドウを開くと前のウィンドウで
  リングが効かない**。追加確認で「**アクティブにすれば動く**」= 非アクティブ窓では効かない、と判明。
- **調査で否定された仮説**: `ring_picker` / `mouse_ring_flick` / `mouse_gesture` は App グローバルで
  bundle swap 対象外なので、片方の窓が置いた状態がもう片方を塞ぐ、と考えた。**塞がっているなら
  アクティブにしても効かないはずなので、この説明は成り立たない。**
- 実際の挙動: 非アクティブ窓では、**最初のクリックがウィンドウのアクティブ化に使われている**と
  見られる。mIV は detached ウィンドウに active / passive の概念を持ち、passive 側は egui の
  キーイベントからアクティブ化を判断する作りになっている。
- **これは不具合というより仕様の選択**。利用者の期待は「見えている別ウィンドウに直接ジェスチャしたい」で、
  現行仕様と食い違っている。
- 扱い: detached リワークの **R2 (状態の集約)** が扱う領域。BA-7 (App グローバル状態の per-window 化) に
  隣接する。**症状パッチ (非アクティブ窓でも右クリックを通す guard) を入れない。**
- 併せて記録: 上記のとおり `ring_picker` / `mouse_ring_flick` / `mouse_gesture` /
  `mouse_ring_grid_target_idx` / `mouse_ring_nav` は**現在 App グローバルで per-window 化されていない**
  (`analysis_mode` など 223 個は swap 対象)。今回の症状の原因ではないが、**複数ウィンドウでの
  リング状態の所有者は未整理**であり、R2 で扱う対象。
- 規模 / 優先度: 仕様判断が先 / P3。

## 5. リリース前確認 / 依存更新

> 版ごとに実際に取った測定値 (perf smoke / idle health / bench / 依存確認) は
> [release-verification-records.md](release-verification-records.md) にある。ここには
> **手順そのものの未解決点**だけを置く。

### 5.0 次版の「重要な変更点」に載せるもの

[src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ書く候補。
**既定の挙動が変わったもの**は `must_read` に入れる (更新後初回起動で自動表示される。
ここに載せないと利用者に伝わらない)。載せたらこの節から消す。

| 変更 | 区分 | 入った版 |
| --- | --- | --- |
| (なし) | | |

v3.2.0 ぶんは [src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ記載済み
(必読 3 件 = 代表画像の既定 / バケツの塗り 1px / 消しゴムの色調合わせ、新機能 7 件)。

### 5.11 v3.2.0 出荷前確認の記録 (2026-08-23)

**動画アップスケール (`video-upscale-shader`) をマージした後の最終ビルドに対する記録。**
マージ前のビルドに対する旧記録はこれで置き換えた。

配布ビルド: `build-dist.ps1` (全体テスト込み) → 署名の 1 本目で
SimplySign のクラウド鍵セッション切れにより失敗 → 再ログイン後
`build-dist.ps1 -SkipRustTests` で完走 (同一ソースでゲート通過済みのため但し書きに合致)。

| 項目 | 結果 |
| --- | --- |
| Rust 全体テスト | ✅ 失敗 0 件 (`lib` 単体でも 6,200 passed)。**フルゲート中に mIV を操作すると `ui_fullscreen::tests` が 5 件落ちる**ので触らないこと |
| CI (GitHub Actions) | ✅ 緑 (`f053625d`)。**`build.rs` の fxc 呼び出しが非 Windows で問題ないことを実証**。`native_presenter` が `#[cfg(windows)]` なので生成テーブルを `include!` する経路が Linux に無く、`find_fxc` も `Ok(None)` になる |
| コード署名 | ✅ 単体exe / setup.exe / portable の 3 種とも `Valid` + RFC3161。内包 vendor PE (pdfium / onnxruntime / FFmpeg 6 本) も `Valid` |
| CRT 静的リンク | ✅ `VCRUNTIME140.dll` / `MSVCP140.dll` 依存なし |
| PDFium | ✅ 最新 (chromium/8009) |
| FFmpeg | ⏸ 4 コミット新しい版あり (`n7.1.5-12-g1fdbca85aa` → `-16-g9a4bb2c579`)。**見送り** — 同じ 7.1.5 系で必要な修正が特定できておらず、更新すると LGPL 対応ソースの再掲と製品ページの手書き節の更新が伴う |
| idle health: static-foreground | ✅ PASS (完全 sleep、perf event 0 件 / CPU 1 コア比 0.0396。外部 sampler が同一 session を確認済み) |
| idle health: static-background | ✅ PASS (完全 sleep、perf event 0 件 / CPU 1 コア比 0.0167) |
| idle health: tray-residency | ⏭ 本ビルドでは未実施。マージ前ビルドでは PASS したが**軽い条件のみ** (`evidence_floor` が起動時のままで、サムネイル読込中に閉じた状態では測っていない) |
| idle health: video-pin-background | ⏭ 未実施 (waiver)。動画を代表画像に固定し、かつキャッシュから読み直される状態のフォルダが必要 |
| ポータブル版 smoke | ✅ 実機確認済み (`data\` が exe の隣、APPDATA 不変) |
| 「重要な変更点」表示 | ✅ `--whatsnew-from 3.1.3` で**必読 4 + 新機能 8** を確認 |
| 機能の実機確認 | ✅ 製本の復元抑止 (出ないこと + Explorer コピーでは出ること)、動画アップスケール、復元モーダル / ESC、動画バー固定、設定の移動先、**T キー** (別ウィンドウ動画 → メインで PDF → 動画へ戻って T)、**キー割り当て一覧のラベル 2 件** |

**見送り (実施不要と判断)**: 検索ベンチ回帰 (本版で検索未変更)、perf smoke。

**次回への申し送り**: `Assert-MivSignReady` は証明書ストアの存在確認だけなので、
SimplySign のクラウド鍵セッションが切れていても通過する (証明書の公開情報だけが残り
秘密鍵が外れるため)。**ビルド 40 分を消費してから署名で落ちる。**
使い捨ての PE を 1 本実署名してから配布ビルドに入れば数秒で分かる。
事前チェック自体をその形にする改善は §5.12。

### 5.13 v3.2.0 の残り公開作業 (2026-08-23 時点)

**出荷判断は済んでいる。出荷前確認は 2026-08-23 に全件完了した。**残りは公開手順のみ。手順の正本は CLAUDE.md「リリース手順チェックリスト」。

済んでいるもの: Phase 0 (更新履歴・利用者承認済み) / Phase 1 (版番号・installer・製品ページ・
`changelog.html` 再生成・`version_highlights`) / Phase 2 (PDFium 最新・FFmpeg 見送り判断・CI 緑) /
全体テスト / 配布ビルドと署名。記録は §5.11。

残り:

1. ✅ **配布ビルドの最終版を検証** — 3 種とも `Valid` + RFC3161、CRT 静的。
2. ✅ **実機確認 (利用者)** — アイドル健全性 2 シナリオ、T キー、キー割り当てラベル、
   製本、ポータブル版 smoke すべて完了。
3. ✅ **§5.11 の記録を最終ビルドに合わせて更新。**
4. ✅ **push** — `master` と `master:main` を `09758b90` まで同期。
5. ✅ **Vector 申請用 zip** — `dist\mImageViewer_installer_v3.2.0.zip` (setup.exe + readme.txt)。
6. ✅ **タグと Release** — `v3.2.0` を公開。body は README の v3.2.0 節 (7,111 バイト)。
   Assets 4 点とも `uploaded`。<https://github.com/MikageSawatari/mimageviewer/releases/tag/v3.2.0>
7. ✅ **リリース日** — 2026-08-23 で README 見出し・製品ページとも一致。
8. ✅ **Phase 5** — mikage.to 反映済み / Vector 申請済み (2026-08-23)。
   任意の窓の杜・MS Store は見送り (毎リリース必須ではない)。
9. **公開後の目視確認 (未実施)** — 別マシンで起動し、更新通知ダイアログの body が崩れずに出るか。

**この節はここで閉じる。** 次版のリリース記録は新しい節を起こす。

### 5.12 署名の事前チェックが、鍵の使えなさを検出できない

- 出典: v3.2.0 の配布ビルド (2026-08-23)。`Assert-MivSignReady` を通過したのに、
  実署名の 1 本目 (`vendor/pdfium/bin/pdfium.dll`) が
  `SignTool Error: No certificates were found that met all the given criteria.` で失敗した。
- 機構: SimplySign のクラウド鍵セッションが切れると、**証明書の公開情報は
  `CurrentUser\My` に残ったまま秘密鍵だけが外れる**。`Assert-MivSignReady` は
  証明書の存在を見るだけなので通過してしまう。
- 実害: `build-dist.ps1` は clean からビルドしてから署名するので、
  **40 分ビルドしてから落ちる**。しかも中途半端な成果物は残らないので、
  丸ごとやり直しになる。
- 直す方向: 事前チェックを「証明書が見えるか」から「**実際に署名できるか**」へ変える。
  使い捨ての小さな PE を 1 本コピーして `Invoke-MivSign` し、成功したら消す。
  数秒で済み、セッション切れならビルド前に分かる。
  - 署名対象を汚さないよう、必ず一時ディレクトリのコピーに対して行う。
  - タイムスタンプ取得でネットワークへ出るので、失敗理由を
    「鍵が使えない」「TS サーバに届かない」で区別できると望ましい。
- 回避策 (現状): 配布ビルドの前に手で 1 本署名してみる。
- 規模 / 優先度: 小 / P2 (リリースのたびに 40 分を失うリスク)。

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
- **quick-xml の非推奨 API 追随** (v3.0.0 で 0.39 → 0.41、advisory 対応)。
  `src/xmp_reader.rs` の 4 箇所が非推奨警告を出す:
  `decode_and_unescape_value` → `decoded_and_normalized_value`、
  `unescape_value` → `normalized_value`。
  **0.41 では非推奨側が新実装へ委譲済み**なので、いま呼んでいる限り挙動は
  `normalized_*` と同じ (= XML 仕様どおり属性値の改行 / タブが空白になる)。
  影響するのは**属性値だけ**で、`xtw:*` (ツイート情報の表示用テキスト) と
  `rdf:resource` / `xmp:Rating` / GPano (いずれも URI か数値) にとどまる。
  **タグ (`dc:subject`) は `Event::Text` 経由**で非推奨メソッドを通らないため無関係。
  次の更新で削除される前に呼び出しを置き換える。

### 5.9 リリース手順: 配布ビルド前に core / remote の存在が要る

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`scripts/test-full.ps1` が
  **テストを 1 件も実行しないままビルドエラーで落ちた**。
- 原因: `--workspace` に launcher package が入り、その build.rs が内包対象の
  `target\release\mimageviewer-core.exe` と `mimageviewer-remote.exe` の存在を検査する。
  remote は v3.0.0 で増えたので、それ以前の成果物しか無い環境で初めて表面化した。
- `build-dist.ps1` は test-full を `cargo clean` の**前**に回すため、直前のビルドが
  残っている通常のリリース機では起きない。新しい clone や `cargo clean` 直後に起きる。
- 当座の回避と前提は CLAUDE.md「開発中のビルド・テスト選択」に記載済み。
- 直す方向: test-full が不足分を先に build するか、launcher を `--workspace` の
  テスト対象から外す (launcher 自身に unit test があるかを確認してから決める)。
- 規模 / 優先度: 小 / P3。

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

### 1.85 vendored egui-wgpu のテクスチャ配送に回帰テストが無い

- 出典: v3.0.0 出荷前のクラッシュ修正 (2026-08-14、`ce6616ef`)。
  `paint_and_update_textures` が surface 無し viewport で早期 return し、
  **`textures_delta.set` を丸ごと捨てていた**問題を直したが、**テストは入れていない**。
- **この境界は過去 3 回、別の症状で表面化している**:
  1. サムネイルが純黒で固着 (v1.8.0 回帰) → `poll_thumbnails` 側で resync 窓中の
     upload を先送りする回避
  2. font atlas の `Y 29..44` パニック → `set_fonts` 最大 5 世代リトライの resync
  3. font atlas の `Y 45..126` パニック (今回) → **リトライ 5 世代を回りきって落ちた**。
     リトライ回数では防げないことの実証
- 欲しいテスト (Codex 案):
  1. `Managed(0)` が初期 32px 高
  2. **surface が無い状態で** 128px 以上の full 置換を submit
  3. surface 復帰後に `y=45..126` の partial を submit
  4. full 置換が共有 renderer に届いており validation error が出ないこと
- 障害: vendored egui-wgpu に既存のテスト基盤が無く、GPU device が要る。
  `egui_kittest` の wgpu 経路 (tests/ui_snapshot.rs) が最も近い足場。
- **回避策を撤去するかの判断もここに含める**: 上記 1 の upload 先送りと 2 の resync は、
  境界が直った今は保険であって正しさの担保ではない。撤去は別レビューで
  (今回は同時に触らないと決めた)。
- **2026-08-16 の確定**: §1.31 の第 1 段として着手する (順序は §1.31 参照)。
  ブリーフ = [codex-texture-delivery-test-brief.md](briefs/codex-texture-delivery-test-brief.md)。
  - 足場は `egui_kittest` **ではなく** `vendor/egui-wgpu` の in-crate headless unit test。
    `RenderState::create` の `compatible_surface` が `Option` で全フィールドが `pub` なので
    surface 無しの device / renderer を作れる。`ui_snapshot` 実行体の既知の間欠 AV を避ける。
  - **上記の欲しいテスト 4 は exit 2 (no-surface) しか到達しない**。`surfaces` が空なら
    `get_current_texture` に届かず、`on_surface_error` はエラーを分類するだけで注入できない。
    `RecreateSurface` / `SkipFrame` の coverage は §1.86 で typed outcome の seam を作ってから。
    → 前半を **§1.85-A** として切り出した。
  - **判定は `Renderer::texture_size` の observable な値にする**。mIV の overflow guard が
    範囲外 partial を wgpu へ渡す前に skip するため、「validation error が出ない」だけでは
    full 置換が落ちていてもテストが通ってしまう。
  - `vendor/egui-wgpu` は workspace `exclude` かつ `autotests = false` なので、
    `scripts/test-full.ps1` に専用実行を足さないと**テストが存在するだけで走らない**。
- **2026-08-16: §1.85-A (前半) 完了**。
  `vendor/egui-wgpu` の DX12 headless in-crate unit test
  `paint_and_update_textures_delivers_set_and_free_without_surface` で、surface 無しのまま
  32px seed → 128px full 置換 → `y=45..126` partial を渡し、主判定の
  `Renderer::texture_size` が 128px を保持することと validation error scope が空であることを固定した。
  別 texture の no-surface `free` も `Renderer::texture()` の消失で検査する。
  vendor manifest に明示的な lib test target を設定し、`scripts/test-full.ps1` の専用
  `--manifest-path` 段から実行される。production 挙動と `src/` 配下の既存回避策は変更していない。
- **残り**: exit 3/4 (`RecreateSurface` / `SkipFrame`) の `free` 配送は §1.86 で
  typed `PaintOutcome` seam を作ってから検査・修正する。本項前半からは手を伸ばさない。
- 規模 / 優先度: 中 / P2。

### 1.86 surface 取得に失敗したフレームで `textures_delta.free` が捨てられる

- 出典: v3.0.0 出荷前の font atlas 調査 (2026-08-14) で Codex が併せて指摘。
  **今回のクラッシュの原因ではない** (`set` は既に適用済みの位置にある) が、同じ関数の同型の穴。
- `paint_and_update_textures` は `set` を surface 参照より前に適用するようになったが、
  **`free` のループは描画の後ろ**にある。`get_current_texture` が失敗して
  `SurfaceErrorAction::RecreateSurface` / `SkipFrame` で戻る経路
  (`vendor/egui-wgpu/src/winit.rs`) では free が実行されない。
- 影響は**テクスチャリーク**であってクラッシュではない。egui は id を再利用しないので
  誤描画にはならず、解放されない GPU テクスチャがプロセス寿命で残るだけ。
  サムネイルを大量に流す使い方だと効いてくる可能性がある。
- 直し方: no-surface 早期 return と同じく、これらの経路でも `free` を流す。
  `set` / `free` の適用を 1 つのヘルパに寄せて、描画の成否と独立にするのが素直。
- **2026-08-16 の確定**: §1.31 の第 2 段として着手する (順序は §1.31 参照)。
  ブリーフ = [codex-delta-delivery-transaction-brief.md](briefs/codex-delta-delivery-transaction-brief.md)。
  - **障害影響は P3 のままだが、§1.31 の依存関係上は必須前提**。§1.31 は frame drop を通常経路に
    するので、捨てる経路で `free` が落ちる構造のままだと本物のリークになる。
  - **「1 つのヘルパ」を単一関数と読まない**。`set` は surface lookup の前、`free` は成功経路では
    `queue.submit` の後でなければならず、同じ時点に置けない。
    `begin_delivery` (set をちょうど一度) / inner が typed `PaintOutcome` を返す /
    `finish_delivery` (outcome ごとに free) の**二段階 transaction owner**にする。
    `RenderStateAbsent` は「配送不能」の明示的な例外 outcome として型に残す。
  - **exit 3/4 へ同じ `free` ループをコピーするだけの修正は、「構造的修正」の合意対象外**
    (2026-08-16、ClaudeCode / Codex Sol)。所有構造 (配送責任が各 return に分散している) を
    直さず症状だけ消すため。
- **2026-08-16: 完了**。
  - `begin_delivery` が render state 有りなら surface lookup 前に `set` をちょうど一度適用し、
    inner paint は `RenderStateAbsent` / `SurfaceAbsent` / `SurfaceRecreated` / `Skipped` /
    `Submitted` の typed `PaintOutcome` へ合流する。no-paint 経路も同じ owner を使う。
  - `finish_delivery` が唯一の `free` 適用箇所。非 submit outcome は labeled inner block の
    encoder / command buffer drop 後、`Submitted` は `queue.submit` 後に finalizer へ到達する。
  - 実 driver error は注入せず、小さい acquire classifier seam で `RecreateSurface` / `SkipFrame`
    を分類し、両 outcome で seed texture が消えることを `Renderer::texture()` で検査した。
    §1.85-A の no-surface test は無修正で維持する。
- 規模 / 優先度: 小 / P3 (障害影響)。**ただし §1.31 の必須前提**。

### 1.84 表示キャッシュに item-context 世代の刻印が無い (latent、ABA 危険)

- 出典: v3.0.0 出荷前の holdover 調査 (2026-08-14) で Codex が併せて指摘。
  **今回の不具合 (`docs/briefs/pdf-page-turn-and-stale-composite-plan.md`) の原因ではない**
  が、同じ調査で見つかった同型の穴。
- フルスクリーン表示チェーンの以下が **`idx` だけ**、または item 文脈を含まない世代だけで
  引いている:
  - `current_edit_result_texture` — `idx` で走査し他の `EditResultKey` 世代を無視 (src/app.rs 53781)
  - `current_comic_composite_texture` — `idx` 直引き。コメントに「世代チェックなし」と明記 (src/app.rs 56470)
  - `current_final_composite_texture` — `key.edit_key.idx` のみ (src/app.rs 55823)
  - `conceal_cache` — `idx` + global conceal 世代のみ、`items_generation` を見ない (src/ui_fullscreen.rs 4297)
  - `EditResultKey` / `FinalCompositeKey` に item-context 世代のフィールドが無い
    (src/app.rs 5550 / 6032)
- **今は `close_fullscreen` が各キャッシュをクリアするので救われているだけ**
  (src/app.rs 49757)。軽量な items 差し替え経路がそのクリアを通らないと ABA になる。
- **意図された正しい形が同じファイルにある**: `ContinuousPageTransition` は
  `items_generation` を自分で持ち、`keep_set_evict` でも照合している (src/app.rs 6434)。
- 直すときの注意 (Codex): 「current 系 helper 3 つに条件を足す」では**不十分**。
  exact-key ヒットが src/app.rs 55292 / 55640 にもあるため、
  **key / read / insert のライフサイクル全体**に item 文脈を通す必要がある。
- 規模 / 優先度: 中 / P2。リリース直前に触る範囲ではない。

### 1.87 セッション最初の拡大で UI スレッドが 0.5 秒止まる (拡大パイプラインの初回作成)

- 出典: v3.0.0 リリース前の perf smoke (2026-08-15)。
- 実測: `gpu/lanczos_regenerate` は同セッション 16 回のうち **1 回目だけ
  `encode_submit_cpu_ms=495.1`**。残り 15 回は 0.2〜7.8ms (中央値 0.5ms)。
  同フレームの `ui/fs_viewport_breakdown` は `central_ms=495.3` / `media_ms=495.2` で、
  495ms 全部がフルスクリーン中央の描画に乗っている。セッション中で 50ms を超えた
  UI スレッド作業はこの 1 件だけ。
- 読み: 初回だけという分布は wgpu の pipeline / shader を最初の 1 回で作っている形。
  利用者から見ると「その日はじめて画像を拡大表示した瞬間に 0.5 秒固まる」。
- **今回入った退行ではない**。既定の高品質拡大は v2.12.0 からで、warm-up の構造は
  そのとき入っている (§1.46)。v3.0.0 のブロッカーとしては扱わない。
- 直すなら: 起動後の暇なフレーム、または一覧描画の最初の 1 回で pipeline を先に作る。
  どのタイミングなら利用者の操作とぶつからないかは計測してから決める
  (起動直後に足すと今度は起動が遅くなる)。
- 規模 / 優先度: 小〜中 / P3。

### 1.94 音声モードの退出が期限切れで detach+attach+seek に落ち、音が途切れる

- 出典: 利用者報告 (2026-08-18)。メインウィンドウへ結合した状態で <kbd>F11</kbd> と <kbd>Z</kbd> を
  連打すると、切り替えのたびに**メインウィンドウのグリッドが一瞬見えるちらつき**と、
  **音の途切れ**が起きる。
- **同日 (2026-08-18) に入れた「別ウィンドウの動画再生中に外部アプリから戻ると Z だけ効かない」の
  修正が原因ではない**。その経路は `source=egui_key` で記録されるが、実ログ中に
  2 回しか無く (別ウィンドウでの成功例)、後述のタイムアウト 7 件のいずれとも時刻が重ならない。
  失敗時の enter はすべて既存の `source=native_key`。ちらつき自体はリリース版 v3.1.1
  (本修正を含まないプロセス) でも観測されている。
- **実測した機構** (1 セッションで 7 回発生、`mimageviewer.log`):

  ```
  180.776 exit: placement changed, re-placing via SwitchPlacement
  180.877 exit ignored reason=exit_pending  deadline_remaining_ms=1099
  181.409 exit ignored reason=exit_pending  deadline_remaining_ms=567
  181.986 exit timed out; falling back to detach+attach+seek
  181.986 fallback exit: re-attached presenter, seek=32.298
  ```

  placement 切替を伴う退出が **`VideoAudioExitPending.deadline` 内に完了せず**、
  detach → attach → **seek** のフォールバックへ落ちる。この seek が**音の途切れ**、presenter の
  付け直しが**ちらつき**。連打すると毎回この経路に入る。
- **構造上の問題**: 退出の完了判定に**時間窓 (`deadline`) を使っている**。憲法 §2 規則 5
  「races を時間窓で吸収しない」に当たる。本来は placement 切替の完了イベントで確定すべきところを
  経過時間で見切っているため、切替が遅い状況では必ずフォールバックする。
- **直す方向**: deadline を伸ばさない。`SwitchPlacement` の完了 (または presenter の再配置完了) を
  typed なイベントとして受け、それで退出を確定させる。フォールバックは「イベントが来ない」ことが
  確定した場合の最後の手段に落とす。着手前に、退出の producer / consumer と
  open / switch / close / cancel / error の各 lifecycle を列挙すること。
- detached / native video のリワーク凍結領域。症状パッチを入れず、原因に対応付けてから直す。
  着手時は [detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  ClaudeCode / Codex 双方で「症状パッチではない」ことの合意を取る。
- 再現手順: メインウィンドウのフルスクリーンで動画再生 → <kbd>F11</kbd> と <kbd>Z</kbd> を連打。
  判定はログの `timed out; falling back to detach+attach+seek` の有無で機械的にできる。
- 規模 / 優先度: 中 / P2 (音が途切れるので体感は悪いが、連打しない通常操作では出ない)。

### 5.10 入力テストの共有ロックが poison して、失敗 1 件が全滅に見える

- 出典: 動画補正スロットの入力 parity 確認 (2026-08-19) の mutation 確認中。可視性ガードを外して
  1 件だけ落とすつもりが、
  同じフィルタの 5 件すべてが FAILED になった。実際に落ちたのは 1 件で、残り 4 件は
  `fullscreen fixed-key test lock poisoned: PoisonError { .. }` だった。
- 機構: `crate::key_input::TEST_INPUT_LOCK` は入力テストを直列化するためだけの
  `Mutex<()>` だが、12 箇所すべてが `.expect(...)` / `.unwrap()` で取得している。1 件が
  panic するとロックが poison し、以降このロックを使う全テストが**本来の失敗とは無関係な
  理由で**落ちる。**最初の 1 件を読まないと原因が分からない状態**になり、CI ログでも
  mutation 確認でも切り分けの手間が増える。
- 直す方向: 取得を 1 つの helper に集約し、`unwrap_or_else(|e| e.into_inner())` で poison から
  回復する。この Mutex はデータを保護しておらず、panic で壊れる不変条件を持たないので
  回復が正しい。**ただし共有フレーム状態の後始末は別問題**なので、helper 化のときに
  「各 call site が取得直後に `clear_test_frame()` するか、guard の Drop で戻すか」を
  揃えて確認する (現在は `FullscreenFixedKeyTestGuard` だけが Drop で戻している)。
- 対象: `src/key_input.rs` (定義 + 6 箇所)、`src/app/tests.rs` (5 箇所)、`src/keymap.rs` (1 箇所)。
- 規模 / 優先度: Small / P3。製品には影響しないテスト基盤の可読性問題。

### 5.8 検索ベンチのゲートに絶対時間の下限が無い

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`check_bench_regression.py` が 10 件中 7 件を
  劣化と報告したが、**製品側の劣化ではなかった**。
- 何が起きたか: 判定が `+30%` の**比率だけ**で、対象クエリの絶対時間を見ていない。
  実測はこう:

  | クエリ | baseline | 3 回の実測 | 実行間ばらつき |
  | --- | --- | --- | --- |
  | `rare_jp` | 1.03ms | 3.61 / 1.13 / 0.89 | **+304%** |
  | `super_generic` | 0.01ms | 0.02 / 0.01 / 0.01 | +184% |
  | `rare_jp_and` | 0.15ms | 0.31 / 0.42 / 0.23 | +79% |

  **同一バイナリの実行間ばらつきが閾値 (+30%) を大きく超える**。1 回目は 25 分の
  配布ビルド直後で、全クエリが一様に遅い (ディスクキャッシュと CPU の状態)。
  3 回の最良値なら 10 件中 9 件が合格し、3 件は baseline より速かった。
- 唯一残る `rare_jp_and` も 0.15ms → 0.23ms で、**差は 0.08 ミリ秒**。この規模に
  比率の閾値を当てても意味がない。
- ずれている前提: ゲートは「1 回の計測が真の性能」と仮定しているが、ミリ秒未満の
  クエリでは計測ノイズのほうが大きい。
- 直す方向 (組み合わせる):
  1. **絶対時間の下限**を入れる。`+30%` かつ `+N ms` (N=1〜2 程度) の両方を満たしたときだけ
     劣化とする。ミリ秒未満のクエリが騒がなくなる
  2. ベンチ側で **1 クエリを複数回まわして最良値**を JSON に書く (現在も batch 内では
     `best=` を出しているので、外側にも同じ考え方を広げる)
  3. baseline に**測定条件** (日付・機種・同時実行の有無) を残す。現在の
     `vendor/bench_baseline.json` は `num_docs` と `version` しか持たず、
     いつ何の上で取った値か分からない
- 当座の運用: 失敗したら**同じバイナリで 3 回まわして最良値で判断する**。
  ノイズと判断した場合に `--save` で baseline を上書きしないこと (ノイズを基準値にすると
  次から本物の劣化を見逃す)。
- 規模 / 優先度: 小 / P3 (リリースのたびに人が判断すれば回るが、毎回時間を取られる)。

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

### 1.124 見開き表示のまま Ctrl+G で補正レイヤーへ入ると、左右の切り替えもマスク編集も効かない — 実機報告

- 出典: 2026-08-26、利用者報告。**見開き表示のページ**で <kbd>Ctrl</kbd>+<kbd>G</kbd> を押して
  補正レイヤー画面へ入ると、左右ページの切り替えが動かず、マスク編集も含めて操作が
  まったく効かなくなる。
- **未調査。着手前に確認すること** (推測で直さない):
  1. 補正レイヤー画面は「1 ページ 1 編集対象」を前提にしているはず。見開きで入ったとき
     **編集対象がどちらのページに決まるのか**、あるいは決まらないまま入っているのかを
     まず観測する。決まっていないなら、入口で単ページへ落とすのか、左右を選ばせるのかは
     仕様判断になる。
  2. 「すべて効かない」= 入力が別の所有者に吸われている可能性がある。キー・ポインタの
     消費経路 (`consume_key` / キャンバス入力) を、見開きと単ページで比べる。
  3. 同型の入口を数える。<kbd>Ctrl</kbd>+<kbd>G</kbd> だけでなく、メニュー・ツールバーから
     補正レイヤーへ入る経路、連結読み中、横長分割中 (v3.3.0 で追加) でも同じか。
     [CLAUDE.md](../CLAUDE.md) 「バグ修正の一般原則」に従い、再現した経路だけで終えない。
- 関連ドキュメント: [local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md)、
  [preset-and-adjustment.md](preset-and-adjustment.md)、[display-pipeline.md](display-pipeline.md)。
- 規模 / 優先度: 未見積 / **v3.3.0 に含める** (利用者判断、2026-08-26)。
