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

### 1.68 PDF が「表示するファイルがありません」になったとき、理由がどこにも出ない — 利用者報告

- 出典: 利用者報告 2026-08-12 (§1.58 の実機確認中)。PDF を開いたら「表示するファイルが
  ありません」になり表示できなくなった。**再起動したら再現しなくなり、原因不明のまま**。
- 切り分け済み (source inspection、2026-08-12): PDF の item は
  `load_pdf_as_folder` → `poll_pdf_enumerate` → `start_loading_items` が作る。
  **items を空にし得る経路は 3 つ**あり、いずれも**利用者に理由を見せない**。
  1. enumerate worker の切断 (プロセスが落ちた / IPC が切れた)
  2. 通常の enumerate error
  3. パスワードエラー時の placeholder 除去
  同時期に入れた `MonochromeOnly` の変更は既存 item を読む consumer 側で、items を
  空にする経路は持たない (確認済み)。
- **これは「無言の早期 return」そのもので、直す前に観測を足す類の問題**
  (CLAUDE.md「バグ修正の一般原則」)。原因を推測して guard を足さないこと。
- 観測を入れた (2026-08-13、まだ「直した」ではない):
  - [empty_items_reason.rs](../src/empty_items_reason.rs) に `EmptyItemsReason` を新設。
    空にした経路が理由を残し、grid の中央表示とログが読む。
    `App::set_empty_items_reason` がログ 1 行も出すので、型と文言がずれない。
  - 理由は **`items_generation` と一緒に焼く**。消す責任を「空にした側の呼び出し順」に
    持たせると経路が増えたときに消し忘れるので、世代が変われば自動で無効になる形にした。
    **理由が付かない空は「本当に 0 件」**という対応を保つ。
  - 文言は次の行動が分かるものにした。「読み込みが中断されました。開き直してください」/
    「この PDF を読み込めませんでした」/「この PDF を開くにはパスワードが必要です」。
    ワーカー由来の生メッセージ (英語) は UI に出さず、ログにだけ残す。
- **同型の確認 (完了)**: ZIP にも同じ形の無言経路が 2 つあった (ワーカー切断 / 列挙失敗)
  ので同時に対応。`start_loading_items` を空で呼ぶ経路は**この 5 つだけ**で
  (PDF 3 + ZIP 2)、他の呼び出しはすべて実データを渡している。変換アーカイブと
  スマートフォルダはこの funnel を空で通らないため、同じ抜けは無い。
- worker 切断時の再試行 (PDF pool は 5 プロセス構成で 1 つ落ちても他が生きている) は
  **まだ入れていない**。原因が切断なのかどうかがログで確定してから判断する。
- 再現手段が無いので、**この版を配ってログを待つ**。`empty items:` で始まる行が出るか、
  出るならどれかを見る。観測だけで「直った」と判断しないこと。
- 規模 / 優先度: Small〜Medium / P2。

### 1.77 連結読み中に横断方向のキーで動画へ入ると、戻れなくなる — 利用者報告

- 出典: 利用者メール (pattier、2026-08-13)。連結表示 (縦連結なら左右、横連結なら上下) の
  **連結方向と逆のキー**で表示を切り替えると、**ホイールスクロールではスキップされる動画も
  対象になる**。その結果、動画表示から戻れなくなる。
- **2 つの問題が重なっている。片方だけ直して終わりにしないこと。**
  1. **一貫性**: ホイールは動画をスキップするのに、横断キーはスキップしない。
     利用者の提案どおり「連結モードでは動画を黙ってスキップ」で揃えるのが素直
     (連結読みは本を読む文脈で、動画は連結対象にならないため)。
  2. **戻れない**: こちらが本体。動画へ入った後に戻れないのは、**入口を塞げば見えなくなるが
     消えたわけではない**。同じ状態 (連結読みセッション中にメディアセッションへ遷移) に
     到達する他の経路 (スライドショー / 連続再生 / detached / 検索結果からの遷移 /
     ファイル一覧の更新) が無いかを列挙してから直す。
- 未確認: 「戻れない」の具体的な失敗 (キーが効かない / 連結セッションが失われる /
  ナビ対象が空になる) をまだ観測していない。**推測で guard を足さず、まず再現して
  どの経路が壊れているかを特定する**こと。
- **再現できた (2026-08-13)**。手作業ではなくアプリ内蔵ハーネスで自動再現した。
  再現データ `C:\tmp\miv-continuous-video\` (画像 11 + 動画 1 + 画像 6)、
  スクリプト [continuous-video-nav.rhai](../scripts/page-turn/continuous-video-nav.rhai)。
  縦連結で `Right` を 11 回叩くと **`current_is_still_image` が false になり、動画へ入った**。
  横断キーが動画をスキップしないことは、これで観測として確定した。
- **「戻れない」の正体はキーの割り当ての穴だった** (`docs/keymap.ini.default` で確認):
  **`[FsVideo]` には素の `Left` / `Right` の割り当てが 1 つも無い。**
  Left / Right を使う動画の操作はすべて修飾キー付き (`Shift+Left` = 1 秒戻す、
  `Ctrl+Left` = 30 秒戻す、`Ctrl+Shift+Left` = 1 フレーム戻す)。
  ファイル移動は **`Up` / `Down` (`VideoPrevFile` / `VideoNextFile`) だけ**。
  つまり **入るのに使ったキーが、入った先では何の意味も持たない**。
  ライフサイクルの不具合ではなく、素の Left / Right が動画では無割り当てなだけ。
  利用者から見れば「押しても何も起きない = 戻れない」で、報告どおりの体験になる。
- 方針: 利用者提案どおり **連結読みでは横断キーが動画・音声をスキップする**。
  ホイールと挙動が揃い、この状況自体が到達不能になる。
  **`[FsVideo]` に素の Left / Right を足すのは採らない** (修飾キー付きの seek 系と
  紛らわしく、誤爆で再生位置が飛ぶ)。
- **ハーネスの限界 (記録)**: ネイティブ動画ウィンドウは合成キーの target registry に
  登録されないため、**動画コンテキストのキー操作はハーネスで検証できない**
  (`key_target satisfied=False`)。上のスクリプトも check 1 までしか自動化できていない。
- 規模 / 優先度: Small / P2。

### 1.78 動画の回転メタデータを反映して、縦長動画を正しい向きで再生する — 利用者要望

- 出典: 利用者メール (pattier、2026-08-13)。「サムネイルは縦長なのに、動画再生すると横で
  再生される」。
- **確定 (source inspection、2026-08-13)**: `src/video/` に回転メタデータを読む処理が
  **一切無い** (`displaymatrix` / `rotate` の参照が 0 件)。一方 **サムネイルは Windows Shell API
  (IShellItemImageFactory) が生成しており、そちらは回転を尊重する**。この非対称が
  「サムネと再生で向きが違う」の正体。
- 方針: FFmpeg の stream side data `AV_PKT_DATA_DISPLAYMATRIX` を読み、
  `av_display_rotation_get()` で角度を得て提示側で適用する。0 / 90 / 180 / 270 と
  水平反転を含む行列があり得るので、**90 度だけを特別扱いしない**。
- 確認が要る点:
  - 適用場所。native presenter は D3D11 / DComp 経路なので、swapchain の手前で回すのか、
    表示変換で回すのかを決める ([video-architecture.md](video-architecture.md) を先に読む)。
  - 回転で **表示解像度と HUD レイアウトの前提 (アスペクト・fit)** が変わる。
    §1.47 (拡大縮小のシェーダ化) と同じ座標系に載るかを確認する。
  - 音声モード / detached / タイル表示 / キャプチャ / サムネ生成のどこまで回すか。
    **キャプチャした静止画の向き**も揃える必要がある。
- 縦長動画は**スマートフォン撮影で普通に発生する** (センサーは横向きで記録し、
  回転タグで向きを表す)。
- **テスト素材は既にリポジトリにある (2026-08-13 に tkhd の matrix を直接読んで確認)**:
  - `testimage/iphone/IMG_1197.MOV` — rotate=90、格納 1280×720 → 表示は 720×1280
  - `testimage/iphone/PIR-206_3.MOV` — rotate=90、格納 640×480 → 表示は 480×640
  - 同フォルダの他の MP4/MOV と `testimage/movie/*.mp4` は rotate=0 なので、
    **回転 0 の回帰確認**に使える。
- **難易度は見た目より低い (2026-08-13 の調査)**。SAR と同じ座席に座らせられる。
  - `update_video_visual_transform` は既に **`Matrix3x2` + `SetTransform2`** で DComp へ渡して
    おり、**M12 / M21 を 0.0 で埋めているだけ** ([render_core.rs:3406](../src/video/native_presenter/render_core.rs:3406))。
    回転は**既にある枠の 2 フィールドを埋める**話で、シェーダも swap chain 変更も要らない。
  - 刻みを決める純関数 `compute_video_visual_transform()` (unit test 6 件) が既にあり、
    SAR は「display_w = surface_w × sar」でここに入っている。回転 90/270 のときに
    表示寸法を転置し、戻り値を 2x2 へ広げる。**回転 0 で現在と完全一致することをテストで固定する。**
  - `ffmpeg-the-third` 3.0.2 は `Stream::side_data()` / `Frame::side_data()` と
    `Type::DisplayMatrix` を公開済み。`av_display_rotation_get` 相当 (行列の
    `atan2(m[1], m[0])`、16.16 固定小数) は 15 行程度で自前実装できる。**新しい依存は不要。**
  - デコードや swap chain には触らない (SAR と同じく DComp 側で 1 度だけ回る)。
- **⚠ 二重回転の罠**: サムネイルは Windows Shell API が作っており**既に回転済み**。
  サムネ経路・タイル一覧のサムネへ回転を適用してはいけない。回転を足すのは
  「再生の表示」と、必要なら「キャプチャした静止画」だけ。
- **libmpv 置き換えは採らない** (同メールの提案)。理由: ①mIV の再生は native presenter /
  D3D11 共有サーフェス / VST3 チェーン / トレイ常駐と密結合で、エンジン差し替えは
  再生機能の作り直しに等しい ②libmpv は既定 LGPL だが構成次第で GPL 汚染する
  ③今回の要望 (回転) は FFmpeg 側のメタデータ 1 つで解ける。
  GLSL ユーザーシェーダの発想自体は §1.47 と地続きなので、そちらで検討する。
- 規模 / 優先度: Medium / P2。

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

### 2.3 RAR が多いフォルダで、全件判定が終わるまでサムネイルが出ない — 専用スレ >>246, >>249-250

- 出典: 2026-08-18 の利用者報告。ローカル上の RAR でも、フォルダを開いた直後は
  2〜10 秒ほど形式アイコンのままで、サムネイル表示が遅いことがある。
- **再現・計測済み**:
  - `C:\tmp\miv-rar-thumbnail-test-100` に、30,000 entry を持ち代表画像が末尾にある
    RAR を 30 個複製して混在させると、サムネイルが数秒遅れてまとまって出る。
  - perf log の `nav/archive_cache_peek` は 133 RAR に対して 3,280.7 / 3,913.6 /
    3,363.4ms。ある run ではフォルダ一覧の install 後、対応表完成まで約3.32秒、
    最後の重い RAR のサムネイル ready まで約5.91秒だった。
  - 同じ状態でも Enter / ダブルクリックによる RAR open は成功する。下の §2.6 とは別件。
- 根本原因:
  1. `start_converted_archive_cache_paths_refresh` がフォルダ内の変換対象アーカイブを
     1 worker で順番に調べ、RAR ごとに `inspect_for_direct_read` を実行する。
  2. 結果は全候補の確認が終わった後に 1 個の `HashMap` として UI へ届く。途中結果を
     公開しないため、先頭の RAR の判定が終わっていてもサムネイル要求へ進めない。
  3. `make_load_request` は `ConvertibleArchive` の対応表 entry が無い間 `None` を返すため、
     heavy thumbnail queue 自体に要求が入らない。
  4. `rar_loader` の完全判定 cache は 32 件なので、多数の RAR を同じ順番で再走査すると
     folder scan / thumbnail / open 間で判定を十分再利用できない。
     これは `(path, size, mtime)` ごとの直読み可否と集計結果をプロセス内に保持する cache で、
     変換済み ZIP 本体を永続保存する `archive_cache.db` / `archive_cache` とは別物。ネスト RAR は
     有効な変換済み ZIP があっても、現状は先に直読み判定を行い、非 Direct と分かってから
     `fallback_cached_zip` へ進むため、32 件から外れた後や再起動後は全 header scan が再発する。
- 修正方針:
  1. 現在フォルダの各候補を `Pending / Direct / CachedZip / Unavailable` の typed state で
     所有し、**1 件の判定完了ごとに**同世代の結果を UI へ公開する。map entry の有無だけを
     pending / unavailable の兼用 sentinel にしない。
  2. 可視範囲と keep range を先に判定し、残りは距離順で進める。全件完了を最初の
     サムネイル表示条件にしない。同期 header scan を UI thread へ戻さない。
  3. current-folder generation 内で得た RAR 判定は、そのフォルダのサムネイル要求と open
     から再利用する。`DECISION_CACHE_CAPACITY` を単に増やすだけの修正にはしない。
  4. フォルダ切替 / 再読み込みでは cancel と `items_generation` を照合し、旧フォルダの
     途中結果を新しい一覧へ反映しない。既存の heavy I/O 予算を無視して候補数ぶん thread を
     spawn しない。
- 完了条件:
  - 上記 stress folder で、可視 RAR のサムネイルが全133件の判定完了を待たず順次表示される。
  - 100件程度の通常 RAR、直読み RAR、変換 cache 済み RAR、変換が必要な RAR、分割 RARを
    混在させても、形式判定・代表サムネイル・open 結果が従来と一致する。
  - 同一 RAR の header 判定回数を計装または test double で固定し、一覧判定直後のサムネイル
    生成で同じ全 entry scan を繰り返さない。
- 規模 / 優先度: 中 / P2。利用者へ「本日リリース予定版の次で対応」と回答済み。

### 2.5 選択済み項目を、修飾なしの再クリックで開く — 専用スレ >>246

- 要望: 一覧で現在選択されている項目をもう一度シングルクリックしたとき、Enter /
  ダブルクリックと同じ open を実行する。
- 仕様:
  - current click の選択処理前に、そのセルが既に `selected` だったかを snapshot する。
    1 回目のクリックで選択された直後に同じ click で開かない。
  - 修飾なしの primary mouse click だけを対象にする。Ctrl / Shift、右クリック、タグバッジ、
    drag 開始、ダイアログで open が抑止されている状態では発火しない。
  - Explorer 方式を対象とする。チェック方式で checked set を変更するクリックは従来どおり
    選択操作として扱い、open に変えない。
  - touch-derived pointer は対象外とし、`touch-support-plan.md` §5.8 の「再タップ open は
    入れない」を維持する。
  - 同じ pointer release が egui の `double_clicked()` でもある場合、open は 1 回だけ実行する。
    item 種別ごとの open 分岐を複製せず、既存の Enter / ダブルクリックと同じ activation
    boundary へ合流させる。
- 回帰確認: Folder / ZIP / PDF / 直読み RAR / 変換対象アーカイブ / Image / Video、
  Ctrl・Shift 選択、native D&D、タグバッジ、touch tap。
- 規模 / 優先度: 小〜中 / P2。利用者へ「本日リリース予定版の次で対応」と回答済み。

### 2.6 ZIP / RAR のダブルクリックが時々無反応 — 未再現、専用スレ >>246, >>249-250

- 報告条件: サムネイル表示、ZIP / RAR、Enter では開ける。1 回だけダブルクリックして
  約10秒ほかの操作をせず待つと開くことがある。RAR はローカル上の直読み対象。
- **2026-08-18 時点では未再現**。§2.3 の stress folder でサムネイルが約6秒遅れる状態でも
  ダブルクリック open は成立したため、RARサムネイル遅延をこの不具合の原因と決めない。
- **追加の極端条件でも未再現**:
  - `C:\tmp\miv-rar-decision-cache-stress\01-direct-64x-120000` に、各12万 entry の
    直読み可能 RAR を判定 cache 上限の2倍に当たる64個配置した。
  - `03-mixed-128x-120000` には、各12万 entry の直読み可能 RAR 64個と、末尾にネスト
    archive を置いて最後まで走査しないと非 Direct と確定しない RAR 64個を混在させた。
    フォルダ判定 worker と明示 open が同じフォルダで並行する条件でも、待ってから自動で
    開く症状は再現しなかった。別フォルダ間の移動は旧 worker を cancel するので再現条件に
    数えない。
  - この結果から hidden scan には独立した性能改善余地があるものの、ローカル実機での
    ダブルクリック症状の主原因とは扱わない。さらに人工的な entry 数だけを増やさず、
    報告環境の診断ログで「入力未成立 / accepted 後の待機」を分離する。
- コード上の確認候補:
  1. サムネイル比率が `自動` のとき、2 click の間に実際の cell height / scroll offset が
     変わると、2 回目が別セルまたは余白へ当たり得る。判定結果が同じ比率で配置が変わらない
     場合は原因にならない。
  2. セル全体が `Sense::click_and_drag()` なので、小さな pointer 移動で2回目が native D&D
     開始へ変わる可能性がある。
  3. タグバッジの hit-test は item open より先に処理される。ダイアログ / context menu が
     open の場合も `grid_open_from_click_allowed` が item open を止める。
  4. RAR open 自体は direct-read 可否の hidden scan を開始する。重い RAR では無反応に見え得るが、
     click が成立していれば再操作なしで scan 完了後に開くはずなので区別する。
- 利用者から確認済み: フル機能ウィンドウ、1 click目で選択枠あり、症状発生時にセル比率・
  位置の切替なし。起動直後か操作後かは特定できていない。
- **次の一手は診断 perf log の追加**。高頻度イベントを常時出さず、pointer 操作と archive
  request の境界だけを同一 `input_seq` / open request id で相関できるようにする。
  1. cell の first click / `double_clicked` / `drag_started` と idx、item key、pointer position、
     current folder / `items_generation`。
  2. `grid_open_from_click_allowed` の結果。拒否時は modal / context menu / badge hit / D&D 等を
     構造化した block reason で記録し、単なる bool や自由文だけにしない。
  3. activation 要求の accepted / ignored、所有者、item kind、dispatch 完了。
  4. RAR inspection の begin / decision-cache hit・miss / end / cancel / error。elapsed、走査 entry
     数、Direct / Solid / Nested / Encrypted、folder 判定 worker と明示 open の呼出元を記録する。
     entry ごとのイベントは出さない。
  5. `pending_direct_nav` publish / consume、RAR image enumeration の begin / end、一覧 install、
     自動 fullscreen 要求 / paint を同じ相関 id で追えるようにする。
  6. auto-aspect の実切替 old/new と、その frame の pointer stream 状態。
- **診断 ZIP の終端保証も同時に直す**:
  - 現在の perf log は64KiB `BufWriter`で、App frameから約1秒ごとに flush しているが、
    `diagnostics::export_diagnostics_zip` は直前 flush をせずログファイルを読んでいる。そのため
    通常はボタン直前の約1秒分、UI sleep中にworker eventだけが追加された場合は次のframeまでの
    未flush分が ZIP に入らない可能性がある。
  - 「ログを zip にする」受付を perf event に記録し、`perf::flush()` と `logger::flush()` が
    完了してから logs を読み込む。受付イベントを ZIP 内ログの明確な終端 witness とする。
  - 診断 ZIP に含める perf log は現行 `perf_events.jsonl` のみで、rotate 済み `.1`〜`.4` は
    従来どおり除外する。UIにも「性能ログONは次回起動から」「再現後は再起動せずZIP化」を
    維持し、実装テストでは64KiB未満の未flushデータも ZIP に含まれることを確認する。
- 計装入り版で利用者へ依頼する手順: 開発者で性能ログON → 再起動 → 症状再現 → **再起動せず**
  「ログを zip にする」→作成された診断 ZIP を送付。ログにはファイル名 / path が含まれる
  既存注意書きも案内する。
- **§2.5 の再クリック open で症状が見えなくなっても、本項を解決扱いにしない。** 既存の
  ダブルクリック経路は独立して診断し、根本原因が判明してから修正・回帰テストを入れる。
- 優先度: P2 調査。先に計装と診断 ZIP flush を実装し、報告環境のログ待ち。

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

### 3.3 ページ送り中の AI アップスケールを待たない / 打ち切る

- 出典: §1.89 の実 perf log。見開き 1 ページずらしの停止中に AI upscale 1.3s × 3 回が
  逐次実行され、5.22s / 4.30s の停止時間の大半を占めた。
- §1.89 は「元画像を描く frame が加工済み source を readiness として待つ」不整合だけを修正した。
  AI コストそのものは変更しておらず、元画像表示以外でも通過先 / stale target の推論待ちが起こり得る。
- 方針: ページ送りで表示 target から外れた AI upscale を待たず、context-owned producer の cancel / 打切りと
  着地点だけの再開を ownership・generation・完了回収まで含めて設計する。時間窓や `Awaiting` の強制解除、
  AI 結果の silent fallback では直さない。
- 規模 / 優先度: 中 / P2 performance。

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
- ~~優先度: P3 / 再現待ち。**報告が来るまで着手しない** (ユーザー判断 2026-08-02)。~~

### 4.2 の更新 (2026-08-17) — 「1 回だけ」ではなく常時再現。着手可能になった ⚠️

§1.31-A の実機確認中に開発機で再現し、**追加情報を待たずに着手できる状態**になった。

- **再現率**: 「1 回だけ」ではない。**押した Z のほとんどが無視される**。実測:
  - §1.31-A 込みビルド: 音声モード突入後の `Z:down` 12 回すべて無視
  - §1.31-A 無しビルド (`pre-1.31a-master`): 突入後 **21 回**すべて無視 → 6 秒後にようやく exit
  - 別の回では 3 回無視 → 12.9 秒後に exit、8 回無視 → exit したが timeout → fallback
- **§1.31-A の退行ではない**。上記のとおり `pre-1.31a-master` (v3.1.0 + §1.85-A + §1.86、
  描画スケジューリング無変更) でも同じ再現率で出る。**A/B 済み**。
- **候補 (A) は否定された**。`[fs-key] source=fullscreen focused=true foreground=...` が
  無視された Z すべてに出ている。フルスクリーン viewport はフォーカスを持っており、
  `has_focus` 早期 return ではない。`[fs-key]` の要約元は同じ ctx の `egui::Event::Key` であり、
  Z が egui event へ届いている事実までは確定している。一方、実際の exit は keymap の
  Win32 frame 優先判定を通るため、そこで別 viewport の edge や別キー edge による早期 return が
  起きているかは未確定。
- **未記録の併発症状**: exit が成功した 0.4〜1.0 秒後に**勝手に音声モードへ再突入する**
  事象を 2 回観測 (`entered audio mode` が再度出る)。再突入後は Z がさらに効かなくなる。
- **無言の分岐が 7 つある** (どれで消えているか現行ログでは判別不能):
  - [ui_fullscreen.rs](../src/ui_fullscreen.rs) の音楽ビュー Z 経路 6 項 —
    `fs_music_view_active` / `fs_context_menu_idx.is_none()` / `!ime_input_active` /
    `!ctx.wants_keyboard_input()` / `!music_bookmark_modal_open()` /
    `consume_action_no_repeat`
  - [native_video.rs](../src/app/native_video.rs) `exit_video_audio_mode` 冒頭 2 つ —
    `video_audio_mode != Some(fs_idx)` / `video_audio_exit_pending.is_some()`
- **進め方**: 推測で直さない。7 箇所へ型付きの理由ログを入れてから 1 回再現し、
  どこで消えているかを確定してから直す (`enter_video_audio_mode` の呼び出し元ログも
  同時に入れて再突入の出所を割る)。過去に推測で 2 回外し、計装した 1 回で当てた前例に従う。
- 優先度: **P2**。利用者報告のある実害バグで、再現手段が手元にある。

#### 診断計装の再修正 (2026-08-17)

- 最初の §4.2 計装は全 `outcome=` を `diagnostic_peek_action_press` の true 配下に置いたため、
  peek 自体が false の実ログでは 1 行も出ず、診断として機能しなかった。
- `video_audio_mode.is_some()` の間は peek と独立に guard chain を評価する。1 本の
  `[video-audio] exit key diagnostic` 行へ、`outcome`、対象 viewport の Win32 frame 状態、
  viewport 内 / 全 viewport の Z down、送信元 viewport、egui Z event、fallback 2 判定、
  `fullscreen_shortcut_event_summary` の読み取り元と要約をまとめる。
- 診断は初回・理由変化・Z 観測だけを候補化し、候補を 1 秒間隔で最大 1 行出す。同一理由の
  idle frame は畳み、rate limit 中に見えた Z は診断 snapshot のまま次の出力可能時刻まで保持する。
- 全 viewport 走査は current frame の read-only 診断 projection で、consume / Action 成立 / dispatch
  には使用しない。現時点では原因修正も production 分岐変更も行わない。

#### 診断計装の 2 回目の再修正 (2026-08-17)

- 再修正後の実ログも session 全体で 1 行しか出なかった。その 1 行は音声モード突入直後の
  Z 未押下 idle frame で、以後 `[fs-key]` が t=9.321 から `Z:down` を繰り返し報告しても
  `[video-audio] exit key diagnostic` は追加されなかった。
- 原因は候補判定 `saw_z_or_action_press` が `action_peek`、viewport Win32 Z、
  全 viewport Win32 Z、egui Z という**調査対象の 4 probe だけ**で決まっていたこと。
  4 probe が全て false、`outcome` も不変なので、候補が永久に作られなかった。一方、
  同じ record の `fullscreen_summary` は `Z:down` を持っていたが候補判定に未接続だった。
- `fullscreen_summary` が `Z:down` を報告した frame も候補にし、rate limit 中はその snapshot を
  保持する。加えて音声モード中は、候補・理由・Z 観測の有無に関係なく 1 秒ごとに現在 record を
  heartbeat として出す。停止中に通常 repaint が無い場合も 1 秒後の repaint を予約する。
  ログ量増加は許容し、診断が黙ることを禁止する。
- 恒久原則: **診断ログの抑制条件を、調査している信号そのものだけに依存させない。**
  独立した観測経路または無条件 heartbeat を持たせる。本計装では peek gate と 4-probe
  candidate gate の 2 回、この原則違反で失敗した。
- 変更は read-only 診断 snapshot の候補化と出力頻度だけ。production の key consume、Action 成立、
  dispatch、音声モード enter / exit 挙動、原因修正には触れない。

#### 診断計装の 3 回目の再修正 (2026-08-17)

- heartbeat 追加後も、実ログでは `[fs-key] source=fullscreen ... Z:down` と同じ時刻帯に
  `exit key diagnostic fs_summary=None` しか出ず、Z frame を下流から候補化できなかった。
- Z が見えていることを実ログで確認済みの `fullscreen_shortcut_event_summary` 直後へ
  source-side 行を追加する。音声モード中の `Z:down` ごとに、これから
  `handle_fs_key_input` を呼ぶ stage、`video_audio_mode`、`fs_music_view_active` /
  `wants_keyboard_input` / `ime_input_active` / bookmark modal / normalize modal / context menu の
  全 guard 値、`ctx.viewport_id()` と
  `cumulative_pass_nr()` を rate limit なしで記録する。
- 既存の `exit key diagnostic` は残し、source-side 行と viewport / pass を突き合わせる。
  production の key consume / dispatch / audio-mode state は変更せず、原因修正も行わない。

#### 原因修正・完了 (2026-08-18)

- **原因確定**: `update_fs_zoom_mode_keys` が `fs_zoom_mode_context_ok` より先に
  `take_key_hold_edges(KeyAction::FsZoomMode)` を呼び、Video / Audio 上でも FsImage 側が
  <kbd>Z</kbd> の Win32 / egui edge を消費していた。約 400 行後の音楽ビュー
  `VideoToggleAudioMode` が読む時点では edge が残っていなかった。
- **修正**: Video / Audio を表す `fs_video_key_context_active` を独立させ、
  `update_fs_zoom_mode_keys` の消費前に判定する。FsVideo 所有時は edge を残しつつ、
  `fs_zoom_reset` と非消費の hold level 読み取りだけで画像ズームの transient state を畳む。
  連結表示など FsImage 内の unavailable state は従来どおり edge を消費して理由を表示する。
  Ring / マウス経路の Video / Audio 除外も `fs_zoom_mode_context_ok` に残した。
- **回帰テスト**: 修正前に Video / Audio の両方で所有権テストが
  `[(Video, false), (Audio, false)]` となることを確認。所有権、state collapse、連結表示の
  consume + no-op 理由、Ring 経路の拒否 + feedback を handler-level test で固定した。
- 原因調査用の無制限 `stage=before_handle_fs_key_input` probe は撤去した。既存の rate-limit
  付き `exit key diagnostic` は残す。
- **実機確認 (2026-08-18)**: 動画 → Z → 音声モード → Z → 動画の往復、音声ファイル単体の
  no-op、画像の Z ホールドによる全画面ズーム、連結表示の no-op 理由表示をすべて確認。
- 状態: **完了**。P2 close。

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

### 4.3 操作カスタマイズのキーボード図 HOME / BOTTOM が 4px 右へずれる

- **実機確認 (2026-08-18)**: Q 列と A 列が揃っていることを確認。ずれは Tab の幅だけでなく、その後ろに続く `item_spacing.x` の分も足りていなかった。

- **完了 (2026-08-17):** HOME / BOTTOM 先頭の `Spacer(48.0)` を typed `KeyIndent("Tab")`
  へ置換した。`egui::Ui::add_space` は `item_spacing` を足さず、後続キー配置が spacing を足すため、
  QWERTY の実 Tab と同じ label-width 44px だけを代替する。
- 描画は実キーと同じ `keyboard_picker_label_width` から幅を導く。将来の標準キー幅変更でも
  インデントだけがずれない。HOME / BOTTOM の先頭 variant と Tab 幅一致を unit test で固定する。

### 4.4 マウスジェスチャ実行後の通知を非表示にする — 専用スレ >>246

- 要望: ジェスチャ実行後に右上へ出る `[Gesture: ...]` の feedback toast を設定で
  非表示にできるようにする。
- これは右ドラッグ中に登録済み軌跡を示す「操作中のジェスチャガイド」とは別機能。
  ガイドの既存表示設定は変更しない。
- 仕様:
  - 操作カスタマイズで、既存の入力中ガイド設定と同じページへ
    「マウスジェスチャ実行後の通知を表示」を追加する。
  - 既存利用者の挙動を変えないため既定 ON。OFF では割り当て済み action と「なし」の両方で
    `[Gesture: ...]` 通知を抑止する。action 実行先が出すエラー等、別用途の feedback は消さない。
  - Grid / Image / Video / Edit の全ジェスチャ文脈で同じ設定を使う。
- 回帰確認: ガイド ON/OFF との4組み合わせ、成功 toast、未割り当て / 実行不能時の feedback、
  detached / native video 発火面。
- 規模 / 優先度: 小 / P2。利用者へ「本日リリース予定版の次で対応」と回答済み。

## 5. リリース前確認 / 依存更新

> 版ごとに実際に取った測定値 (perf smoke / idle health / bench / 依存確認) は
> [release-verification-records.md](release-verification-records.md) にある。ここには
> **手順そのものの未解決点**だけを置く。

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

### 1.88 見開き 1 ページずらし後にページ送りが停止する — 利用者報告

- **完了 (2026-08-17):** `FsNavigatorTextureSources` の各ページへ typed な `Live` /
  `Holdover` provenance を持たせ、選択元から `FsDisplayUnitTracePage` まで伝播した。
  sequence は target の完全な page set がすべて `Live` として描かれた frame だけで解放し、
  共有ページの texture id が previous にも含まれることから ownership を推測しない。
  previous overlay の実描画、片側だけ ready、不完全 target は従来どおり解放しない。
- `spread_shift_anchor_idx` は input 解決時に target pairing へ更新されるため、navigation の
  previous capture は更新後の mutable pairing ではなく、直前に実描画された
  `fullscreen_page_layout` の unit を優先するよう固定した。LTR / RTL、前後方向、単発 / repeat の
  状態遷移テストで previous / target page set の分離を確認済み。
- 出典: 専用スレ >>239 (2026-08-16)。v3.0.0 / v3.1.0 で再現し、v2.9.1 では
  発生しない。手元でも再現済み。
- 再現:
  1. フルスクリーンで右綴じの見開き (テンキー 4 / 5) にし、
     `見開き表示を右方向へ1ページずらす` (既定 Ctrl+Right) を実行する。
  2. または左綴じの見開き (テンキー 2 / 3) で、左方向へ1ページずらす。
  3. その後、カーソルキーによるページ移動を受け付けなくなることがある。
     連打 / hold で起きやすいが、1 回だけでも起きる。左右どちらの Ctrl でも発生する。
  4. Backspace でフルスクリーンを閉じると復旧する。
- 根本原因:
  - 見開き 1 ページずらしは、旧 unit `[2, 3]` から新 unit `[3, 4]` のように、
    移動前後の表示単位で 1 ページを意図的に共有する。
  - v3.0.0 の `96faeee6` で追加した navigation sequence は、target の完全な page set を
    描いた後、`observe_fs_navigation_sequence_presented` で previous holdover の texture が
    1 枚も見えていないことを確認して sequence を解放する。
  - 現在は描画元を明示的に追跡せず、target の texture id が previous holdover の
    `page_for_texture_id` に存在するかだけで旧表示の残存を推測する。共有ページは正しい
    target として live 描画されても同じ texture id が previous に含まれるため、
    `captured_page_visible=true` と誤判定される。
  - sequence が `Presenting` のまま残り、`blocks_new_target()` が後続ナビを拒否し続ける。
    フルスクリーン終了で sequence が破棄されるため Backspace 後は直る。
  - 当初疑った右 Ctrl 既定の `押している間だけ元画像を表示する` との競合は根本原因ではない。
    右 Ctrl は texture 選択のタイミングへ影響し得るが、左 Ctrl でも同じ ownership 誤判定が
    成立する。
- 修正方針:
  - previous holdover が**実際に描かれた**ことと、target の live 描画が previous と同じ
    texture handle を共有したことを区別する。texture id の包含だけから描画所有元を推測しない。
    `FsDisplayUnitTracePage` まで live / holdover の provenance を明示的に通し、完全な target
    page set が live source として描かれた frame で sequence を解放する方向を第一候補とする。
  - previous overlay を本当に描いている間、片側だけ target が揃った間、target page set が
    不完全な間は従来どおり sequence を解放しない。ちらつき防止の atomic unit 契約を弱める
    条件緩和では直さない。
  - `spread_shift_anchor_idx` の更新と previous unit capture の順序も確認し、遷移前 unit と
    遷移後 unit の所有者を同じ mutable pairing から取り違えていないことを固定する。
- 回帰テスト:
  1. previous `[2, 3]` / target `[3, 4]` で 3 の texture id が同一でも、3 と 4 が target の
     live source として完全に描かれたら sequence が解放される。
  2. 同じ page set でも共有ページを previous holdover から実際に描いた場合は解放しない。
  3. 左綴じ / 右綴じ、左右へのずらし、左 Ctrl / 右 Ctrl、単発 / repeat を確認する。
  4. 既存の disjoint な通常ページ送り、片側だけ ready、不完全 target、旧 texture の
     index 再利用を検出するテストを維持する。
- 関連: [display-pipeline.md](display-pipeline.md) §2.5.4 の atomic display-unit 契約。
- 規模 / 優先度: 小〜中 / **P1 (次バージョンで修正)**。

### 1.89 元画像プレビュー中の見開き 1 ページずらしが 4〜5 秒停止する

- **実機確認 (2026-08-18)**: 単発タップと分析モードの双方で 4〜5 秒停止は再現しない。AI アップスケールとカラー化の ON / OFF いずれの組み合わせでも停止なし。なお §1.91 の続報修正でホールド中は元画像表示自体が成立しなくなったため、元の再現手順 (右 Ctrl ホールド + Ctrl+→) からはこの経路に入らない。

- **完了 (2026-08-17):** navigation target の `materialized_ready` を、その frame の描画 resolver が
  実際に選ぶ source と一致させた。`fs_display_bypasses_final_pipeline` が true の元画像表示 / 分析モードは
  target 全ページの `resolve_original_preview_tex` を、false の通常表示は従来の加工済み source を要求する。
  元画像が無い target は `Awaiting` のまま、見開きは両ページの `all` が成立するまで ready にしない。
- 根本原因: 描画側は raw 元画像を選んでいた一方、readiness だけが AI + カラー化済み source を待っていた。
  右 Ctrl + Ctrl+Right の実 log では同じページ対で 5.22s / 4.30s 停止し、5.22s の間に 54 入力を consume
  して target は進まなかった。内訳は PDF decode 約150ms、AI upscale 1.3s × 3、final composite 73ms。
- OS キー状態は frame-local sample を producer gate / readiness / draw で共有し、同一 frame 内の source 判定を
  分裂させない。`original_preview` 専用 carve-out、`blocks_new_target()` の迂回、通過表示 blocker の解除、
  AI producer、時間窓 / timeout は変更していない。
- 回帰テスト: bypass raw ready、通常表示の加工済み待ち、raw 不在 `Awaiting`、分析モード、見開き atomic の
  5 条件を追加。§1.88 の4本と既存 atomic 契約テストは無修正で通過。
- AI upscale のコスト自体は範囲外で、別項 §3.3 に継続記録。
- 正本: [codex-original-preview-readiness-brief.md](briefs/codex-original-preview-readiness-brief.md)。
  関連: [display-pipeline.md](display-pipeline.md) §2.5.4。

### 1.92 別ウィンドウの動画再生中に外部アプリから戻ると Z だけ効かない

- 出典: 利用者報告 (2026-08-18)。**v3.1.1 の変更が原因ではない**。利用者が以前のバージョンでも
  再現することを確認済み。
- 症状: 別ウィンドウで**動画を再生している**状態で、他アプリからそのウィンドウをクリックして戻すと
  <kbd>Z</kbd> (`VideoToggleAudioMode`) だけが無反応。P やカーソルキーは効く。上部 HUD から
  マウスで音声モードへ切り替えると、以降は <kbd>Z</kbd> も効く。**音声モード表示中に戻した場合は
  起きない**。
- 分かっていること: 失敗する経路は native presenter の `handle_native_video_key`。egui 側の音楽ビュー
  経路 (§4.2 で直した側) ではない。Z の分岐は P / カーソルと同じ match の中にあり、表面上の特別扱いは
  無い。ただし到達先の `enter_video_audio_mode` にだけ「音声トラック無し / detached / 切り替え中は
  弾く」という追加条件がある。
- **今のログでは切り分けられない**: 「キーが native 側へ届いていない」のか「届いたが match しない」のか
  「match したが `enter_video_audio_mode` が弾いた」のかを区別する記録が無い。`enter request
  source=native_key` は成功時にしか出ない。**最初の一手は観測**で、native key の入口に無条件の記録を
  置き、外部アプリから戻った直後の 1 打鍵を追う。抑制条件を調査対象の信号に依存させないこと
  (keymap-spec.md の原則)。
- detached viewer のリワーク凍結対象領域。症状パッチを入れず、原因に対応付けてから直す。
- **観測結果 (2026-08-18、計装 `b9448ca8` を入れて 1 回再現)**:
  - 効かなかった Z は `[fs-key] source=fullscreen keys=Z:down` として **egui 側**に届いていた。
  - `[native-video-key]` はセッション全体で **1 行だけ** (終了時の Escape)。**Z も P もカーソルも
    native の `handle_native_video_key_event` には届いていない。**
  - native 行は `foreground_hwnd=0x3BC25CC` != `presenter_hwnd=0x7982452` を記録しているが、
    **これは host focus の証拠にはならない** (detached presenter は `WS_CHILD` なので、子に
    focus があっても foreground は top-level host になる。Codex 指摘)。証拠は
    **`[fs-key]` が出ていて native event が無いこと**そのもの。
  - HUD 経由の入場は `source=native_output_event` として記録され、音声モードからの Z は
    egui の音楽ビュー分岐が処理していた (`exit fs_idx=9`)。
- **原因**: キーが奪われているのではなく、**`VideoToggleAudioMode` の動画→音声方向が egui 入力
  経路に持ち主を持たない**。この action は 2 方向 × 2 経路あるのに、ハンドラが片方ずつしかない。

  | | native 経路 | egui 経路 |
  | --- | --- | --- |
  | 動画 → 音声 | あり (`handle_native_video_key_event`) | **無い** |
  | 音声 → 動画 | — | あり ([ui_fullscreen.rs](../src/ui_fullscreen.rs) の音楽ビュー分岐) |

  報告の 3 点がこれで説明できる。P とカーソルが効くのは egui 側にハンドラがあるから。Z だけ
  効かないのは誰も拾わないから。音声モードなら起きないのは presenter が居らず全て egui を通り、
  音声→動画のハンドラは存在するから。**§4.2 と同じ形の鏡像**。
- なお §4.2 の修正 (画像側が Video / Audio 上で Z を消費しないようにした) は本件の**前提**でもある。
  あれが無いと、egui 経路に持ち主を足しても `FsZoomMode` が先に edge を消費してしまう。
- 修正前は利用者向け [known-issues.html](../htdocs/mimageviewer/manual/known-issues.html) に掲載していたが、
  下記完了に伴い削除した。
- **完了 (2026-08-18):** `VideoToggleAudioMode` を `toggle_video_audio_mode` の単一 semantic owner
  へ集約し、native key / native HUD / egui 動画 / egui 音楽ビューの各入口から呼ぶようにした。
  分岐順は音声 VST 表示 → 音声モード → 通常動画で、VST 表示中に `fs_music_view_active == false`
  でも `enter_video_audio_mode` の `AlreadyActive` gate へ落ちず、先に `exit_video_audio_vst` する。
  egui 経路は既存 guard の下で `consume_action_no_repeat` を使う。3 状態 × 2 経路の対象 5 テストと
  §4.2 の画像 / Video / Audio 所有権テストで固定した。detached / focus / placement / presenter
  lifecycle の述語・状態・時間窓は追加していない。

### 1.93 `VideoAdjustSlot1..10` の egui 動画入力 parity を確認する

- §1.92 の同型 action 監査で、`VideoAdjustSlot1..10` の key dispatch は
  `src/app/native_video.rs` の native dispatcher にだけ存在し、`handle_video_input` の egui 経路には
  mapping が見当たらなかった。
- in-window、detached host focus、focus handoff など egui に動画キーが届く場面で
  `Ctrl+1..0` が無反応になる可能性がある。実ログまたは handler-level test で parity 欠落を確認し、
  load/save 修飾子、repeat、context-menu / IME / modal guard、画像側との ownership を固定してから
  action owner を決める。
- 今回は `VideoToggleAudioMode` だけを対象とし、本項の修正は行わない。

### 1.90 アニメーション先読みの全フレーム展開で archive 閲覧が停止する — 利用者報告 >>241

- **実機確認 (2026-08-18)**: `animeted.zip` 内の画像がアニメーションすることを確認。一覧から Enter で開いた直後も右上の展開中表示が出る。

- **完了 (2026-08-17):** fullscreen canonical decode に typed `AnimationPolicy` を追加し、通常ファイル /
  archive entry と GIF / APNG / WebP の全組み合わせで、先読みは第1フレームだけ、現ページは全フレーム
  とした。archive 内 GIF / APNG も WebP と同じくアニメーション再生する。
- 先読みしたアニメ第1フレームは cache entry に形式付きで記録する。現ページ化したら第1フレームを
  表示したまま全フレームへ昇格し、items generation / target idx が一致する結果だけを差し替える。
  ページを離れた worker と upload backlog は cancel / 破棄し、失敗時は静止した第1フレームを残す。
- 昇格が150ms以上続く場合だけ、in-flight 状態から「アニメーションを読み込み中…」を右上3段目へ
  表示する。時間で消える feedback toast は使わず、完了時には同じ state owner から即座に消える。
- 回帰テストは FirstFrameOnly の3形式の先頭画素一致、FullFrames の file / archive 3形式、spawn 方針、
  現ページ昇格、移動 cancel + stale 拒否、失敗時の第1フレーム維持、150ms表示述語を固定した。
  既存 `fs_animation` の10テストは無修正で通過。
- 現ページ1本の全フレーム常駐は確定仕様。ストリーミング / リングバッファ化は今回の範囲外で、
  将来必要になった場合は GIF / WebP のフレーム依存とループ再decodeを含めて別設計にする。
- **表示述語追補 (2026-08-17):** 進行表示を `AnimationPromotion` 専用から「現ページの
  アニメーション全フレーム展開が in-flight」へ統一した。初回 `Display` と先読み後の昇格の
  typed load purpose が同じ開始時刻 projection を公開し、150ms gate と右上3段目の位置は変更しない。
  アニメーション非対応拡張子と PDF は表示対象にしない。
- 正本: [codex-animation-prefetch-policy-brief.md](briefs/codex-animation-prefetch-policy-brief.md)。
  関連: [display-pipeline.md](display-pipeline.md) §4.1.1。

### 1.91 元画像ホールド併用の連続ページずらしで戻り方向だけ 2〜3 倍遅い

- **完了 (2026-08-17):** `original_preview_active` は、既存の
  `fs_navigation_sequence_blocks_new_target()` が true の間だけ false を返す。シーケンス終了後は
  `FsOriginalPreviewHold` の effective modifier が成立すれば従来どおり元画像を表示する。
- 実測は元画像プレビューなしの左右と preview ありの Left が 14.2〜17.2 手/秒だったのに対し、
  Right + preview だけ 5.2〜5.8 手/秒。遅い区間の texture 選択 878 件は全て
  `source=nav_holdover` で、4.5 秒間、補正済みの前表示単位を描き続けていた。
- 機構は、元画像要求に矛盾する加工済み pass-through を正しく無効にした結果、設計どおりの
  非対称先読み窓 12:4 の戻り側が露出し、target ready まで holdover が残るもの。画面にあるのは
  現ページでなく前の表示単位なので、「現ページの元画像を見せる」約束自体が進行中は成立しない。
- 利用者判断: 元画像ホールドは静止して画像を見ているときの確認用途であり、移動中に補正が
  乗っていることは仕様として問題ない。判定は割り当て chord でなく typed navigation sequence
  だけを使う。時間窓、pass-through 復活、先読み窓変更、`blocks_new_target()` caller 例外は追加しない。
- 回帰テストは `FsOriginalPreviewHold` を Ctrl へ再割り当てして、sequence in flight 中の false と
  sequence 不在時の true を固定する。既存の元画像 preview / readiness テストは変更しない。
- **実測後の判別計装 (2026-08-17、原因修正なし):** 修正後の perf log でも
  `page_turn_decision reason=original_preview` と holdover が支配的だったため、blocker 到達ごとに
  memo の cache hit / fresh evaluation、memo 書込時と blocker 時の frame / pass / frame内 call order、
  両時点の `fs_navigation_sequence_blocks_new_target()` を read-only で記録する。1秒ごとの
  `fs/original_preview_blocker_summary` は sequence in-flight 中の original-preview return 数を集計し、
  return が0件でも heartbeat を出す。抑制条件を original-preview / sequence の成立自体には依存させない。
- **続報と修正 (2026-08-18):** 4 属性付き perf log では、右 Ctrl ホールド中の
  `pass_through` は 0 件、`right_ctrl_held=true` / `original_preview_active=true` /
  `context_blocker="original_preview"` は 1,888 frame だった。typed sequence の間だけを抑止すると、
  auto-repeat のシーケンス間の隙間で元画像表示が復帰し、blocker が burst を解除していた。
- 利用者判断により、ページ送り入力が物理的に押されている間も元画像表示を無効にする。既存の
  sequence 抑止は単発タップの release 後から提示完了までを守るため残す。物理レベルは blocker を
  一切参照しない helper に分離し、edge の `still_held`、または focused viewport permit 下の
  configurable / fixed page-turn chord を解決して `(frame_nr, viewport)` 単位で memo 化する。
  `original_preview_active` と `fs_page_turn_input_held` は同じ sample を共有し、時間窓と先読み窓変更は
  追加しない。マウス / リング由来のページ送りと元画像要求の矛盾を防ぐため、`original_preview`
  blocker 自体は残す。
- **実機確認 (2026-08-18)**: 修正後の perf log で、右 Ctrl ホールド中の `pass_through` が
  **0 件 → 130 件** (materialized 31)。左 Ctrl は 46 / 9 で、両者の比率がほぼ一致した。
  auto-repeat がページを進めた割合は戻り方向で **17% → 72%** (左 Ctrl は 69%)。
  利用者確認でも「速さも元表示も OK」。押しっぱなし中の一瞬の元画像は出ない。
  矢印を離して右 Ctrl だけ保持したときは従来どおり元画像へ戻る
  (`right_ctrl_held=true` / `original_preview_active=true` が 1,452 frame 記録されている)。
- 関連: [display-pipeline.md](display-pipeline.md) §§2.3, 2.5.4、
  [keymap-spec.md](keymap-spec.md)「画像フルスクリーン」。

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

### 5.4 idle health の video-pin evidence 窓が狭い

- **v3.1.1 (2026-08-18) の実測。3 回走らせて 3 回とも FAIL したが、いずれも製品側ではない。**
  1. **接続先を取り違える**: `-NoLaunch` が `%APPDATA%\mimageviewer
untime.1.1\` の core
     (ランチャー版が展開した別 instance、`--perf-log` 無し) に接続した。perf を 1 行も書かない
     instance なのに「同一 session PID は確認済み」と表示し、`events=0` を完全 sleep として
     扱った。**perf log growth が 0 のまま measured window を評価したら、sleep ではなく
     「記録していない」を疑う**べき。接続先の exe path が測定対象として妥当か
     (script が起動した instance か) を確認する条件が要る。
  2. **起動直後の初回索引と競合する**: 起動 11 秒後に測ると、全文検索の initial scan
     (walker が 52 万 / 11 万 / 6 万ファイル) が並列で走っていて CPU one-core ratio 2.302 で FAIL。
     索引は text log にしか書かないので perf 側は静かなまま、CPU だけが跳ねる。
     **「起動後 N 秒」ではなく、初回スキャン完了 (name_index / indexer の initial scan done) を
     待ってから測る**のが正しい条件。
  3. 索引完了後に測り直すと **CPU one-core ratio 0.0094 / perf event 0 件 / repaint streak 0 /
     同一 work 0** で、製品側は完全に静止していた。残った FAIL は本項の evidence 窓だけ
     (同一 session の 1 つ前の窓では `matched=64 (enqueue=32, ready=28,
     idle_upgrade_ineligible=4)` を記録済みで、タイルは keep 範囲に入っていた)。
  - 結論: v3.1.1 は **evidence 窓の既知欠陥による FAIL** として通した。実体の測定は PASS。

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
