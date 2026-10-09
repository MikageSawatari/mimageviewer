# §1.359 EffeTune Mixwright v0.13.0 更新と Now Playing 連携

状態: **実装・自動検証・確認用build完了、実機／公開前gate待ち**。2026-10-09。§7は全項目回答済み。
対象は **v4.5.0 の次のリリース**。Coordinator は ClaudeCode、実装担当は Codex。
調査基準は `next-effetune` / `33a018417e13a271be2da2c109fa571f64079684`。
初稿の引き継ぎは `target/F-design-msg.txt`、R2は `target/F-design-r2-msg.txt`。
R3は設計commit `45d65a17e` の再レビューを反映し、`39b293a75` の独立再レビューで新規指摘無し。
v4.5.0公開済みのmaster `840c309ec` を `ba5367125` で本branchへmerge済み。
利用者の2026-10-09回答により§5のA/Bを実装する。coherent chunkごとの検証・引き継ぎは
`target/D359-<chunk>-msg.txt`。アプリ起動とコミットは行わない。署名・公開はrelease leadの担当。

## 0. 前提と証拠の読み方

- 利用者決定: 音楽／動画は同時に一つだけ再生する。Remote 配信もその一つとして扱う。
  選択中の一覧項目、最後にフォーカスした窓、ユーザー VST チェーンは送り元／送り先にしない。
- 作者の契約は `target/effetune-host-now-playing.md:10`。初期化後、plugin UI thread の
  `IConnectionPoint::notify` に `EffeTune.NowPlaying` を渡す。追加の接続 handshake は不要。
  title / album / artist は各4096 UTF-16単位、画像は8,388,608 bytesまで、MIMEは
  png / jpeg / gif / webp。完全な snapshot を置換し、欠落／空欄は消去する
  (`同ファイル:20`, `:33`)。`kResultOk` は受理であり画像のdecode成功保証ではない (`:26`, `:39`)。
  editor を開く必要はなく、毎回のplugin初期化で再送する。state／presetには保存されない (`:44`, `:101`)。
- 新版の読取専用資料の基点を **N** と表す:
  `C:/home/miv-tools/effetune-mixwright-0.13.0/extract/EffeTune Mixwright.vst3`。
  **W** は `N/Contents/Resources/webview`。本書の `W/...:行` はこの実ファイルを指す。
  `C:/home/miv-tools` は変更しない。実装後の同じ相対pathは vendor 内に配置される。
- C++の短いファイル名は `crates/vst3-host/src/`（`protocol.h` は
  `crates/vst3-host/include/protocol.h`）、`bridge.rs` / `dsp/mod.rs` は `src/video/dsp/` を指す。
  `mod.rs` は `src/effetune/mod.rs`、remote／sfzのJS短名は直前に示したW内directoryを指す。
  製品ページの短名は `htdocs/mimageviewer/` を基点とする。
- 初稿の「コード参照」は静的読取時点の行番号。実装後の自動検証は§8へ記録する。
  新版pluginの実機表示・通信・保存動作は未確認。
  バイナリの文字列／import検査はソースの代わりでも、ネットワーク不使用の証明でもない。

## 1. バンドル更新を完了させる手順

### 1.1 原本、配置、承認manifest

1. ClaudeCode が利用者の許可を得て取得した
   `C:/home/miv-tools/effetune-mixwright-0.13.0/EffeTune-Mixwright-windows-x64.zip` を正本にする。
   本調査で再計算した SHA-256 は
   `1ad197b57380503fb04d3773a60525e338153182834f96ae6634d4ff8cd25da0`。
   作者release notesとの一致は依頼文による ClaudeCode の検証記録。
2. ZIP内の全448ファイルと N の相対path／バイト列は本調査の非起動比較で一致。
   現vendorは424ファイル、新版は448ファイル、追加24・削除0。
   新版bundleの合計は **23,662,343 bytes**。ZIP entryは450（directory entryを含む）なので
   450をファイル数に使わない。件数一致だけで承認せず全ファイルhashを固定する。
3. 実装時に現vendorの実ファイルを退避してから、以下の二階層を完全に置換する。
   DLL単体のコピー、旧bundleへの上書き混在、junction／symlinkは禁止。

   ```text
   vendor/effetune-mixwright/
     VERSION                         # v0.13.0 + LF、UTF-8 BOMなし
     EffeTune Mixwright.vst3/
       Contents/x86_64-win/EffeTune Mixwright.vst3
       Contents/Resources/...        # stamp・notice・WASMを含む全448ファイル
   ```

4. 新しく `third_party/effetune-mixwright/v0.13.0/` を作る。`VERSION` はvendorと同じバイト列、
   通知は以下の3本を上流のままコピーする。
   `Contents/Resources/THIRD-PARTY-NOTICES.txt`、
   `Contents/Resources/webview/THIRD-PARTY-NOTICES.txt`、
   `Contents/Resources/webview/plugins/dsp/NOTICE.txt`。
   `manifest.sha256` は **VERSION + bundle448 = 449行のhash entry**、
   `sha256小文字 + 半角空白2個 + slash相対path`、ordinal順、raw bytesから作る。
   現形式は `third_party/effetune-mixwright/v0.12.0/manifest.sha256:1` と
   `scripts/effetune-distribution.ps1:5`。新しい承認済みZIPとの照合時だけ作成し、gate失敗を
   通すためにローカルvendorから承認manifestを作り直さない。
5. `.gitattributes:31` の `third_party/effetune-mixwright/** -text` は新版にも適用される。
   新しいVERSION／通知／manifestのcheckout後hashまで確認する。旧v0.12.0の追跡資料は履歴として保持する。

### 1.2 通知の差分と補足通知

3本をvendor v0.12.0とN v0.13.0で行単位比較した結果。既存ライセンスの削除・置換はなく、追加だけ。

| ファイル | v0.13.0で追加された通知 | 方針 |
| --- | --- | --- |
| `N/Contents/Resources/THIRD-PARTY-NOTICES.txt:50` と `W/THIRD-PARTY-NOTICES.txt:50` | lie 3.3.0（Calvin Metcalf / Jordan Harband、MIT）、immediate 3.0.6（Barnesandnoble.com等、MIT、`:75`）、setImmediate 1.0.5（同、MIT、`:101`）、pako 1.0.11内のzlib由来コード（Jean-loup Gailly / Mark Adler / Vitaly Puzrin / Andrey Tupitsin、zlib、`:127`） | 従来mIVが補っていた4成分の通知を上流が収録。JSZipは従来から同梱されているので「4ライブラリを新しく追加した」とは説明しない |
| `W/plugins/dsp/NOTICE.txt:46` | Emscripten runtime（MIT または University of Illinois/NCSA、Node.js由来部分のMITも収録）、musl libc（MIT、`:156`、関連由来コードと例外の説明を含む） | WASM DSP／SQLite資産を対象とする上流通知をそのまま追跡・同梱・about表示する。今回新しくライセンス条件を変更したとは断定しない |

`third_party/effetune-mixwright/supplemental/NOTICES.txt:1` は「v0.12.0の上流通知に無い」と
説明している。推奨は **v0.13.0では補足通知の配布／about埋込を終了し、元ファイルは履歴として保持**。
上流3通知と実JSZip資産で4成分の全文充足を再照合したうえで、aboutの4番目の項目と補足専用assertを
更新する (`src/ui_dialogs/about.rs:85`, `:376`, `:398`)。`EFFETUNE_NOTICES[..3]` のような
旧件数依存も新しい実数に合わせる。上流bundle内の通知は一切編集しない。
製品ページの「補足通知」案内も更新する (`htdocs/mimageviewer/index.html:1456`)。

### 1.3 ビルド、署名、世代公開

- 原本検証: `scripts/build-release.ps1:105` / `scripts/build-dist.ps1:55` が
  `Assert-MivEffetuneSource` を呼ぶ。承認VERSION、全ファイル集合／hash、PEと通知を検査する
  (`scripts/effetune-distribution.ps1:120`)。新版でも追加・欠落・改変の拒否を維持する。
- staging: `New-MivEffetuneStage` は固定の `target/effetune-dist-source` へ実コピーする
  (`scripts/effetune-distribution.ps1:162`)。reparse祖先／subtree拒否を維持する。
  原本vendorは署名しない。
- `build-dist.ps1` は既定で署名ON (`:85`)。`build-release.ps1 -Sign` がstaging内の
  **拡張子に依存しない全PE**を列挙し、`.vst3` PEもlauncher埋込前に署名する (`:315`, `:337`)。
  stage検証は非PEの完全一致、PEのchecksum／証明書追加以外の一致、指定発行元の有効署名を要求する
  (`scripts/effetune-distribution.ps1:57`, `:92`)。署名hashを原本manifestへ書き戻さない。
- launcher生成は `crates/launcher/build.rs:28` → `build_effetune.rs:11`。
  `MIMV_EFFETUNE_DIR` にstagingを渡し (`scripts/build-release.ps1:402`)、VERSION、
  ordinal相対名、size、SHA-256から実配布内容のmanifestを生成して全ファイルを埋め込む
  (`crates/launcher/build_effetune.rs:50`)。原本承認hashと署名後のruntime hashは区別する。
  `build_effetune_source.rs:75` の署名前／署名後検証を新版へ更新する。
- runtimeはmIVの版別directoryの中の `effetune/<hash12>-<generation>/` に構築し、
  全検証後に小さい `current` pointerだけを置換する。plugin版文字列だけでは公開世代を決めない。
  既存世代のin-place置換／削除をしない (`crates/launcher/src/effetune_bundle.rs:280`, `:328`)。
  最深pathのUTF-16長259以下のgateも維持する (`同ファイル:349`)。
  新しい448ファイルで相対最長名と想定APPDATA長を再計算する。
- coreはlauncher指定世代を解決・生存期間中固定し、leaseを保持する
  (`src/effetune/mod.rs:658`, `:762`)。異常な世代を古いbundleで黙って代用しない。
- 旧世代清掃は起動後の既存workerに任せる (`src/runtime_cleanup.rs:51`, `:131`)。
  現在版／pinned／pointer先、in-use lease、publisher lock、旧coreのprocess画像、
  canonical直下関係とreparse拒否を維持。busy／削除失敗は次回起動へ回し、更新専用清掃を足さない。
  EffeTune設定／IR／SFZ／プリセット／WebView保存領域は清掃対象にしない。
- bridge protocol v6のC++変更もあるため、現ソースmarker付きhostをCMakeで作り直す。
  host／CRT→core／remote／EPUB worker→launcher→installerの既存署名順を守る
  (`scripts/build-release.ps1:282`, `:335`, `:395`, `:416`)。
  PE依存gateに同じstagingを渡す (`同ファイル:436`, `scripts/build-dist.ps1:220`)。
- `build-dev.ps1:136` はloose bundleをdev-runtimeへコピーする既存経路。
  本機能はlauncherの埋込／署名にも依存するため、最終検証はrelease launcherが必要。
  portableはEffeTuneとVST hostを同梱せず、UI非表示とUnavailable no-opを維持
  (`docs/effetune-integration-plan.md:525`, `src/effetune/mod.rs:713`)。

### 1.4 バックアップと保存済み状態

実装前にv0.12.0の原本全体＋VERSION＋承認manifestを実ファイルで退避する。
実装後にはv0.13.0の正本ZIP、vendor原本全体、manifest／通知、再buildしたVST host＋CRTを
ツリー外に保存する。既存hostバックアップ規則は `CLAUDE.md:400`。
本worktreeからツリー外へ書く権限は無いため、その更新はClaudeCode／利用者が行う。
`C:/home/miv-tools` に上書きしたり、今回の調査でバックアップを作ったことにはしない。

v0.12.0は出荷済み。同梱前のv0.11.1についての「移行不要」決定
(`docs/effetune-integration-plan.md:674`) を今回へ流用しない。
保存済み効果、IR、GUI設定、プリセットの読込・再保存を新版で検証する。
Now Playingは保存状態と分離し、復元失敗で旧stateを消さない既存方針を維持する。

### 1.5 `0.12.0` / `424` 検索による変更一覧

追跡テキスト全体を検索し、gitignoreのvendor原本VERSIONも別途読んだ。
`424`を直接要求する実コードgateは見つからないが、VERSION込み**425**をassertする
`crates/launcher/build_effetune_source.rs:173` がある。manifestの全path/hash固定と件数assertを両方更新する。
**一括置換しない**。以下の現行pinは更新し、過去の証拠と無関係な数字は残す。

| 現在の参照 | 実装時の変更 |
| --- | --- |
| `scripts/effetune-distribution.ps1:37`, `:41`, `:52`, `:95`, `:132`, `:133` | 承認版、復旧文、manifest参照をv0.13.0へ。任意VERSIONを受け入れる緩和はしない |
| `crates/launcher/build_effetune_source.rs:11`, `:75`, `:160`, `:168`, `:173`; `build_effetune.rs:23` | 原本pin／実vendor test／復旧文を更新。test名の425と`assert_eq!(manifest.len(), 425)`も449へ |
| `scripts/test-effetune-distribution.ps1:46`, `:54`, `:67`, `:139` | synthetic fixtureを新しいpinへ。欠落／追加／非PE改変／偽署名等の負例を維持 |
| `src/ui_dialogs/about.rs:68`, `:75`, `:82`, `:398` | 3通知のinclude／VERSION一致検査と補足項目の見直し |
| `CLAUDE.md:1029`, `:1725`, `:1747`; `docs/release-operations.md:231`, `:310`; `docs/effetune-integration-plan.md:10`, `:381` | 現行配置・件数・配布契約を更新。release-operationsの署名前425照合は449へ。旧§10.2／§10.3の実績は履歴として残し、新版節から現行手順を示す |
| `scripts/gen-effetune-docs.py:4`, `:27`; `docs/effetune-docs-overrides/README.md:8`, `:101`; `selection.json:27` | 新版に対応する上流日本語snapshotの正確なcommitを取得・記録し、DEFAULT_SOURCEとoverrideを再照合。旧snapshotを新版とラベルだけで称しない |
| `scripts/test_gen_effetune_docs.py:153`, `:157`, `:161`, `:177`, `:207`, `:234`, `:257`, `:268`, `:274`, `:283`以降 | 新版生成／入口／snapshot対応testを更新。旧版不一致の負例は残せる |
| `htdocs/mimageviewer/manual/effetune.html:92`, `:93`; `manual/effetune/index.html:8`; `htdocs/sitemap.xml:74`以降 | 新版HTMLをconverterで生成して入口を切替、sitemapを再生成。旧v0.12.0の版別HTMLは既存converter方針どおり保持 |
| `src/effetune/mod.rs:2087`, `:2092`; `src/video/dsp/bridge.rs:1950` | v0.12.0のstate fixture／codec検査は互換回帰証拠として保持。新版fixtureを別途検証し追加する。数字だけの変更で新版実測を名乗らない |
| `third_party/effetune-mixwright/v0.12.0/*`; `third_party/effetune-docs/03bffda3/SOURCE.md:5`; `ja/dsp-library.md` | verbatim履歴は編集せず新版directoryを追加。補足通知原文も保持 |
| backlog §1.359、backlog-on-holdの旧ソース調査、book-resume-meter等の過去gate、archive／review log | 観測時点を保つ。§1.359は実装段階で本planへ誘導し、新版公開済み／同梱状況を区別する |

`Cargo.lock:1143`, `:7125`、vendor egui系lockの0.12.0は別crate。
SVG座標、chart座標、メモリ量、test logの行番号などの424も対象外。
release gate本体 `scripts/build-dist.ps1:142` は版を直接固定せず、上のhelper／fixtureを実行する。

## 2. 新しい資産と公開説明への影響

### 2.1 Remote: デスクトップ用機能とVST版の到達性

追加資産 `W/js/remote/*` と `W/css/effetune-remote.css` はLAN remote control用。
`remote-control.js:2` はserverがElectron mainの `electron/remote-control-host.cjs` にあると説明し、
実起動入口 `:42` は `window.electronAPI.remoteV1` が無ければreturnする。
`W/js/app.js:541` のimport自体は新版VST UIからも到達するが、
`W/vst-bootstrap.js:399` が構築するelectronAPI（終端`:462`）に **remoteV1は無い**。
`remote-control-button.js:22` もAPIが無いとreturn。`W/effetune.html:245` のボタンは初期hidden。

外向きWebSocketを作るコードは存在する: `remote-session.js:23` はHTTPページに限定し、
`:43` は同一hostの `ws://.../?t=...` を構築、`:115` がWebSocketを作る。
ただし `remote-client.js:307` の自動起動条件はbodyの `remote-client` class。
bundleの `W/effetune.html:163` はclass無しの通常bodyで、`:273` はstartup.jsを読む。
remote-client.jsを読み込むscriptはこのHTMLに無い。client側のfetchは
`remote-client.js:187` の相対 `package.json`。調査したremote JSにWebRTC／RTCPeerConnection、
HTTP server作成／listen実装は見つからない。Electron serverソースはこのVST bundleに無い。

非起動PE検査: Nのpluginはx64 (0x8664)。通常import tableにはWS2_32／WinHTTP／WinINet／URLMonが
無く、文字列 `EffeTune.NowPlaying` は有り、`remoteV1` / `WebSocket` / `listen` は無し。
ただしWebViewや動的ロードによる通信はimport tableで証明できない。
**結論の範囲は「同梱JSのVST入口から新Remote server／clientを起動する経路は確認できない」**。
「プラグインの全native codeは絶対に通信しない」とは断定しない。実機のlisten／外向き接続は未観測。
EffeTuneのRemoteとmIV Remoteは別物であり、新版のLAN remote controlをmIVの機能として案内しない。

### 2.2 SFZ: 利用者が選んだ外部ファイルを読む

`W/plugins/others/sfz_note_player.js:1` はSFZ Note Player。
requiresWasm (`:2`) と音階検出による楽器再生の説明 (`:5`) を持つ。
native serviceのfolder選択は同ファイル`:353`、browser方式は`:360`。
`W/vst-bootstrap.js:198` は `sfz/select`, `list`, `remove`, `openRead`, `readChunk`, `closeRead`
をnativeへ渡し、`:253` でsfzLibraryV1を公開する。これが
`W/js/sfz/service.js:345` → `native-service.js:25` に採用される。
`native-service.js:58` は選択済みIDに対するrelative read、`:87` はsampleのstatを行う。
`parser.js:29` は絶対path、drive prefix、NUL、選択rootを越える `..` を拒否する。
これはJS側の規約であり、native containmentの実装まで監査した意味ではない。

`W/js/sfz/limits.js:1` は64／128／256／512／1024 MiB、既定256 MiB。
W/vst-bootstrapの転送は192 KiB chunk、最大1 GiB (`:194`)。
Now Playing画像の8 MiB上限とは別。SFZ機能を8 MiBに制限しない。
調査したSFZ JSにネットワークfetch／WebSocketは無く、外部ファイルの読取をnative APIへ委ねている。
どの参照／コピー／indexがどのAPPDATA先へ永続化されるかはnative側資料と実機で追加確認が必要。
JSだけから「SFZの全sampleがWebView内にコピーされる」とは書かない。
ユーザーが選ぶ元フォルダーがネットワーク共有の場合のOSファイルアクセスは、クラウド処理と区別する。

### 2.3 Adaptive PredictionとVisualizer

- `W/plugins/resonator/adaptive_prediction_effect.js:20` はrequiresWasm、mono／single／stereo-pair。
  `:35` は音声予測を学びresidual／resonanceを作ると説明し、Learn、Weight Decay、Freeze、Hold、
  Resetを持つ (`:37`, `:57`, `:182`)。JS processorは `return data` (`:50`)。
  WASM実行不能をUIで示す共通処理は `W/js/ui/pipeline/wasm-execution-notice.js:19`。
  汎用Web側はローカルartifact名を選ぶ (`W/js/audio/dsp-wasm-loader.js:240`)。
  VST版のaudio managerはnativePort／fakeNode (`W/vst-audio-manager.js:972`) に処理を橋渡しする。
  よって「mIVでこの効果が必ずWebAssemblyとして処理される」とも断定しない。
- 調査したAdaptive Prediction JSには外部AIサービス接続、model download、ONNX参照はない。
  予測アルゴリズムのnative内部やニューラルネットの有無はこの配布JSだけでは判定できない。
  「AIだからGPUを使う」「クラウドAIを使う」「学習データを送信する」を名前から推測しない。
- `W/vst-bootstrap.js:35`, `:420` はstartup viewのVisualizer選択を許している。
  `W/vst-audio-manager.js:1001` はnowPlayingMetadataを返し、`:1005` はrevisionの新しい
  complete snapshotだけを採用する。新版追加の `W/js/visualizer/visualizer-guitar.js` も同梱する。
  Visualizer表示設定はplugin側で選ぶ。mIVが起動時表示を毎回上書きする設定を増やさない。

### 2.4 `manual` / `privacy.html` /「安心して使えます」

| 現行説明と根拠 | 実装時の扱い |
| --- | --- |
| `htdocs/mimageviewer/index.html:1213`「AI処理はすべてPCの中で」本文は画像AI4種類とGPUを列挙 | 画像AIの説明を維持。Adaptive PredictionまでGPU動作の保証を広げない。音声予測はPC内の音響処理という説明にし、native確認の根拠を別途得る |
| `同ファイル:1219`「通信するのは4つの場面だけ」; `privacy.html:212`通信一覧 | Now Playingはcore→ローカルhost→pluginのIPCであり、新しいHTTP endpoint／通信先を足さない。新Remote JSの存在だけで第5の場面を追加しない。起動／idleとSFZ選択・load・reload、Adaptive Prediction Learn／実音声処理時のhost／WebView子プロセスの通信・I/Oを公開前に観測する（§6.2）。新版同梱は利用者決定済み。通信・保存動作が説明と異なれば同梱を取り下げる代わりにprivacy／製品ページ／manualを事実へ更新する |
| `privacy.html:139`, `:166` と `index.html:1247` のEffeTune保存データ | SFZの楽器／参照情報を追加する必要がある。nativeの正確な保存先確認後に記述し、保存先不明のままIRと同じと決めない。Now Playingはplugin instance内の一時情報でpreset／historyへ保存しない（作者契約）。画像生成で利用する既存サムネcacheとは区別する |
| `manual/effetune.html:92`以降、版別説明書、DSP NOTICE案内 | 新版の説明書snapshot／VST向けoverrideを用意。曲情報／画像、空欄、pause／stop、Remote EOF後の保持、起動時Visualizer、SFZの外部ファイル依存、local処理、portable非対応を平易に説明。EffeTune独立版のRemote／media playerをmIVで使える説明として残さない |

privacy.htmlの英語側（`:315`, `:338`）も同じ内容へ更新する。
実装が未完了の今回、製品ページの断定文を書き換えない。

## 3. Now Playingの構造

### 3.1 所有者を一つにする

`EffetuneController`に専用Now Playing ownerを一つ置く（実装ファイル案 `src/effetune/now_playing.rs`）。
既存runtime (`src/effetune/mod.rs:136`)、音声slot、GUI owner、state保存とは役割を分ける。
Appの各窓へ新しいpending bool／画像bufferを足さない。

```text
確定した単一media owner（Local または Remote）
  → 軽い入力snapshot: media identity / title / album / artist / artwork source descriptor
  → Now Playing owner（Empty または Media、単調revision付き）
      ├ artwork worker: 読取・decode・縮小・encode、古いrevisionは破棄
      └ effetune-host-control: 最新complete snapshotをtyped Cmdとして送信
          → C++ control thread: bounded decode → per-slot GUI post_async
              → IMessage → IConnectionPoint::notify
```

表示中の窓を毎frame探して複数候補へ優先順位を付ける方式は採らない。
mediaを確定／再生再開する共通境界でLocal identityを渡し、RemoteのStreaming確定境界でRemote identityを
渡す。入力元の識別子（Local playerの生存identity、Remote sessionとregistration）を保持し、旧ownerのclose／
error／EOFが新ownerを消去しないよう一致を条件にする。同じpathのA→B→Aも別revisionとする。
先読みplayer生成、cache再利用準備、metadata用だけのplayerは送り元を公開しない。

既存codeでは複数playerは保持でき、実再生clockが止まるseek中もautoplay intentを持つ
(`src/video/mod.rs:9875`, `:9885`)。DSPの所有tokenも metadataの公開ownerの代用にはしない。
単一再生の利用者前提を実装時に共通開始境界で再確認し、矛盾する二重再生経路があれば
優先順位を勝手に新設せずClaudeCodeへ報告する。

### 3.2 Rust IPC: protocol 6、typed Cmd、応答なし

- `src/video/dsp/bridge.rs:40`、`crates/vst3-host/include/protocol.h:91`、独立testerの
  `crates/vst3-host-tester/src/bridge.rs:19` のversionをすべて5→6。
  helloコメント (`protocol.h:13`)、ready例 (`protocol.h:30`) と
  `crates/vst3-host/tests/smoke_hello.py:68` のコメント、`:69` のhello送信値、`:75` の応答version assertも6へ。
  本体／testerのhandshake test、host identityとexeも同時更新する。v5 hostの拒否回帰は維持する。
- `Cmd` はinternally tagged enum (`bridge.rs:73`)。新variantは明示的renameで
  `set_now_playing`。wire keyの順は `cmd`, `slot_id`, `revision`, `title`, `album`, `artist`,
  `artwork`, `artworkMimeType` とする。revisionは**slot incarnation内の順序判定watermark**であり、
  診断logにも使う。artworkはbase64文字列。
  MIMEは `image/png`, `image/jpeg`, `image/gif`, `image/webp` のいずれか（無しなら空）。
  Rustのsnake_case fieldはserde renameで上記wire keyに固定する。
  **`cmd`を最初にserialize**し、全keyを固定する。画像なしは空bytes／空MIME、clearは全空欄。
  empty snapshotも省略せずtyped Cmdで送る。
- `Bridge::send(&Cmd)` (`bridge.rs:1093`) を使い、`send_value(json!)` へ迂回しない。
  現C++のfield抽出は名前探索であり (`main.cpp:654`, `:659`)、ユーザーtitle内の
  `cmd`／`slot_id`風文字列に誤認しないserialization／抽出testを付ける。
  必要な新コマンドのfieldはJSON-awareなtop-level抽出で検証し、既存汎用parserをついでに全置換しない。
- 4byte little-endian長 + UTF-8 JSON。以下の三つの上限を別々に検査する。
  JSON frame bodyは**33,554,432 bytes（32 MiB）**まで。4byte長prefixは含めず、確保前に検査する
  (`protocol.h:100`, `main.cpp:119`, `bridge.rs:1179`)。
  wireのbase64文字列は標準alphabet／padding付き、**11,184,812 ASCII文字（同bytes）**まで
  (`4 * ceil(8,388,608 / 3)`)。JSONから取り出した文字列の長さであり画像bytesではない。
  decode後の画像は**8,388,608 bytes**まで。paddingから実decoded長を確保前に計算し、decode中も検査する。
  8,388,609 bytesの画像もbase64長は11,184,812になり得るため、wire長の検査だけでは拒否できない。
  3文字列の最大escapeを加えた正常な最大画像frameは32 MiB内。decoded画像の8 MiB上限を
  base64文字列へ適用しない。JSON frame内のescape分はframe上限で別途扱う。
  malformed base64、型不正、MIME不一致ではnotifyしない。
- **reply eventなし**を推奨。受理／拒否／slot不在はC++ stderr logだけ。
  既存の応答なしGUI命令は `main.cpp:950`。共有event_rxへ汎用Error／ackを投げない。
  Rustのsend成功は「IPCへ書き終えた」でありnotify受理とは呼ばない。
  stderrは既存pumpがcallbackへ渡す (`bridge.rs:619`)。受理はC++のtresult logとnative試験で確認する。
  将来ackが必要になった場合だけ
  request ID専用routeを使う（現PluginState専用route `bridge.rs:692` に倣う）。
- `DspBridge`にslotを指定する小さいAPIを追加し、inner mutex下で実Bridge Arcとslot_idを
  snapshotして**lockを解放してから**送信する。`with_slot_bridge` (`dsp/mod.rs:2948`) は
  closure中もmutexを保持するので約11 MiBのwriteに使わない。音声block permitをmetadata送信のために待たない。

### 3.3 C++: per-slot plugin UI threadで通知

新命令は§3.3.1のslot単位mailboxへ渡し、軽量wakeだけをcontrol threadから
既存 `post_async` (`plugin_loader.cpp:702`, `:1753`) へ投げる。snapshotごとのtaskは作らない。
**audio threadや音声state fenceでは実行しない**。GUI未表示でもload時のper-slot threadがある。

GUI task内で次を行う:

1. initialized componentから `IConnectionPoint` をquery、続いてcontroller側の候補をquery。
   作者契約はeitherなので **componentを先にnotifyし、受理されれば終了**。
   componentにinterfaceが無い／未対応 (`kNoInterface` / `kNotImplemented` / `kResultFalse`) の場合だけ
   controllerを試す。異なるオブジェクトにも無条件に二重配信しない。
   canonical `FUnknown` identityで同一objectを判定し、同一ならnotifyは一度だけ。
   `kInvalidArgument`はpayload拒否としてlogし別receiverで隠さない。ここは §7の推奨設計。
2. `allocateMessage(host_app_)`／SDKの参照カウントを使い、IDは `EffeTune.NowPlaying`。
   hostは既にHostMessage／HostAttributeListを生成する (`host_app.cpp:78`)。
   既存component↔controller connection (`plugin_loader.cpp:1087`) は維持し、追加connectは不要。
3. UTF-8→well-formed UTF-16変換を行う。Rustで先にNULを除去・scalar境界で4096単位へ切り詰める。
   C++でもstrict変換後4096単位へ切り詰め、末尾high surrogateを残さずNUL終端を付ける。
   4096はterminatorを含まない。malformed UTF-8や残った埋込NULは拒否する。
   path用strict helper (`crates/vst3-host/include/utf8_paths.h:14`) はNUL拒否なのでmetadataにも同条件を明示する。
4. title / album / artistをsetString、nonempty画像はsetBinaryとMIMEをsetString。
   allocation、各setter、notifyのtresultをチェック。notifyが終わるまでpayloadを保持する。
   logはslot、revision（順序watermark）、receiver、tresult、画像byte数のみ。title／画像base64をlogへ出さない。
5. slot破棄後へraw pointerだけのtaskを残さない。既存unloadはGUI queue上で行う
   (`plugin_loader.cpp:2532`)。§3.3.1のClosed状態と同じcontrol順序のtask→unloadを検証し、
   quarantine／thread未準備／component無しはlogして破棄する。UIを開くための同期waitを追加しない。

#### 3.3.1 bridge内も実行中一件＋最新pending一件に限定する（R2 / P2-2）

現受信readerは全JSONを `cmd_queue_` に積む (`main.cpp:314`, queue宣言`:1876`)。
GUI taskも無制限のdequeへ積む (`plugin_loader.cpp:734`)。Rustだけのlatest mailboxでは
この二箇所のMB級滞留を防げない。新commandは**readerの受信・検証境界から**専用mailboxへ分岐し、
完全なJSON／画像を既存cmd_queueやGUI taskのcaptureへ入れない。

- live slotごとに、slot incarnationに結び付いたshared mailboxを一つ登録する。
  状態は `Idle / Scheduled / Running / Closed` の単一ownerとし、Running snapshot一件と
  **最新pending snapshot一件**だけを保持する。受信readerはbounded parse／base64 decode後にpendingを
  置換し、古いbufferを解放する。未知slot／Closedへの入力はlogして捨て、readerからplugin APIを呼ばない。
  初期化完了前にはRegisterBridgeが発行されないため、未登録slotへのmetadataを保存する別queueは不要。
- Idle→Scheduledのときだけ、mailboxへのshared handleを持つ**軽量wake一件**をcmd_queueへ積む。
  control側は同じtokenをGUIへ移す。GUI開始待ち／Running中の追加snapshotはpending置換だけで、
  wakeを増やさない。GUI task開始時に最新pendingをRunningへmoveし、lockを解放して通知する。
  完了時にpendingがあれば次の軽量wakeを一つcontrolへ返し、無ければIdleへ戻す。
  GUI thread上のpost_asyncはinline実行する (`plugin_loader.cpp:703`) ので、そこで自己再帰／無限drainせず、
  control経由で次taskをqueueへ渡して他のGUI処理へ機会を返す。未実行wakeは全段合計一件まで。
- clearも同じmailboxの空snapshotであり、別queueへ逃がさない。revisionは同一slot incarnation内で
  単調増加する順序値で、受信済み最大値をwatermarkとして、
  clearより古いrevisionを再採用しない。より新しいmediaでclearが置換されることは許す。
  既に実行開始した旧notifyは取り消せないが、その後はclearまたはそれより新しいsnapshotだけを通知する。
  新incarnationではwatermarkを新規に作り、RegisterBridgeによる最新snapshot再送を旧slotの値で拒否しない。
- remove／unload／resetでslot自体を廃棄する際は、readerがその制御命令を受け付けた順序で
  旧mailboxへのadmissionをClosedにしpendingを捨て、既存制御命令をqueueへ渡す。
  未開始wakeはshared ownerのClosedを見て何もしない。Closedはadmissionの終了であり、既にRunningの
  payload／開始時刻は完了まで保持し、Closed後もwatchdogの監視対象に残す。完了callbackは再起床しない。
  Runningを途中解放せず、既存unloadのGUI直列経路で
  完了後にloaderを破棄する（hangなら§3.3.2でprocess終了）。同じslot_idを再利用しても新incarnationと
  新mailboxを作り、旧wake／旧完了callbackから新slotへアクセスしない。taskはbare loader pointerだけを捕まえない。
  このsealはslotを実際に廃棄する命令だけに適用し、通常seekのDSP resetやpreset reloadではClosedにしない。
- mailboxの画像保持はrunning8 MiB＋pending8 MiB＝**16 MiB以下**。
  readerの一時frameは32 MiB以下、一時decode候補は8 MiB以下。frameを二つのqueueへ複製せず、
  JSON fieldはframe内view／有界抽出で扱う。置換中の旧pendingと候補の一時共存はこの読取予算に含める。
  IMessageへのコピー一件も最大8 MiBなので、Now Playing用途の専用host（送り先一slot）が所有する
  画像／frame部分はこれらの合計
  **64 MiB以下**（text／小さい構造の予算は別）。plugin／WebView自身の内部コピーはこのhost保持予算には含めない。
  slow-GUI headless試験で件数・byte数のhigh-waterと、slot破棄後の解放を実測する（§6.1）。

この共用mailboxは二段の各queueに別pendingを置く設計ではない。slow receiverでは中間snapshotを
省略し最新完全snapshotへ収束するため、各更新のFIFO配送を保証するという初稿の表現は撤回する。
clear／incarnationの順序と、適用revisionが後退しない不変条件を保証する。

#### 3.3.2 通常の拒否とGUI hangを分ける（R2 / P2-1）

`post_async`には期限監視が無く (`plugin_loader.cpp:702`)、既存watchdogは
Running getStateだけを監視する (`main.cpp:460`)。notifyが戻らないと後続getStateもQueuedのままで、
この監視には掛からない。新しいRunning mailboxにはGUI task開始時刻を持たせ、
既存独立watchdog (`main.cpp:447`) から**実行開始後5秒**を監視する。
上限は `kPluginStateTimeout` (`plugin_loader.cpp:599`) と同じ固定値。
message構築／receiver query／setter／notifyを含むtask開始前後で登録・解除し、
plugin API呼出し中はmailbox／watchdogのlockを保持しない。queue待ちはnotify実行時間に含めない。

- setter失敗、payload拒否、interface無し等でtaskが戻る通常失敗はlogし、音響処理を維持する。
- taskが戻らず期限超過した場合は既存GUI timeoutの**process終了方針**
  (`plugin_loader.cpp:602`, `:616`) に合わせ、診断を残して専用EffeTune hostを終了する。
  既存helperには汎用Error送信もある (`plugin_loader.cpp:613`) が、Now Playingのtimeout経路では
  それを通常応答として流用せず、stderr診断→process終了にする。終了検知をHostLostへ一意に流す。
  実装レビューで、notify背後のclose/open/query/show等が既存の短いGUI sync期限で先に
  汎用Errorを発行できる競合を確認した。slot GUI executorはcanonical mailboxへのweak参照を持ち、
  同期futureの期限満了時にExecuting notifyがあれば、その同一開始時刻＋5秒までCVで完了を待つ。
  notify期限超過はwatchdogと同じstderr専用終了へ収束する。期限内に完了すれば同じfutureを
  操作自身の従来budgetで待ち、元操作のhangは既存方針とする。再度commandをqueueへ積まない。
  baseline待機中に開始・完了してIdleへ戻ったnotifyも、同一mailboxのexecution_count増加で
  待ち時間の消費を判定する。同IDの別incarnationにはこのbudget回復を引き継がない。
  metadata用の別Running flag、sleep／try_lock loop、timeoutへの余裕値追加は使わない。
  GUI threadへのcancel／join／notify retryで解除を待たない。metadataの通常ackは増やさず、
  getState応答を待つ経路にもhostの死亡が伝わる既存経路を使う。
- 既存host monitor (`src/effetune/mod.rs:1534`, `:1550`) → `fail(HostLost)`
  (`:1578`) → EffeTune slot clear →音声素通し／利用者へ一度の故障通知に収束する。
  この実行中は自動再起動しない (`docs/effetune-integration-plan.md:89`, `:281`)。
  専用hostなのでユーザーVSTチェーンを終了させない。§7.6の「効果を止めない」は通常失敗だけに適用する。
  故障の案内はcontrollerのFailedへの遷移で一度だけ発行し、毎frameのruntime参照や複数の未完了要求から
  重複発行しない。timeoutによる未完了の状態取得で保存済みpreset／stateを上書きしない。
  Failedと同pollまたは後から届くmetadata write警告は常にdrainし、logだけへ収束させる。
  停止案内を通常送信失敗の案内で上書きせず、同じhost故障を二度通知しない。
  現Appのpoll入口 (`src/app.rs:29146`) は故障toastを発行していないため、実装ではpoll前後の
  canonical runtimeのFailedへの変化をUIが一度通知する。通知済みboolを窓ごとに増やさない。

### 3.4 worker送信と初期化後の再送

既存 `effetune-host-control` (`src/effetune/mod.rs:1017`) がtransportを一つ所有する。
新しい軽量 `HostCommand::RefreshNowPlaying` は最新値を取りに行く通知だけとし、
MB級画像やbase64をhost_txのunbounded queueへ積み続けない。
ownerのmailboxは最新一件のArc snapshotを置換し、画像bufferをmutex下でclone／decodeしない。
送信側は同じsnapshot revisionを連続送信しない。更新中は旧画像を含めずtext-onlyを先に送り、
画像の成功／無画像終端で同じmediaの**完全なsnapshot**を更新する。

`publish_running` → `HostCommand::RegisterBridge` (`mod.rs:1522`, `:1022`) は現在のWeakを登録する
だけの現実装。ここで**送信履歴を初期化し最新snapshotを必ず再送**するよう変更する。
空snapshot／画像準備中text-onlyも再送対象。初回load前に曲が始まっていても最新値を保持する。
editorのhide／showでは再初期化しないので再送不要。
plugin再作成はRegisterBridge（または同じ意味の初期化完了通知）を必ず通す。

Disableは送信先Weakを外す既存経路 (`mod.rs:1026`) を維持。
Unavailable／portableではsource I/O・artwork job・transport送信を開始しない。
Idle／Loadingは軽いtext／source descriptorだけ保持し、利用可能なロード時に画像jobを始めればよい。
音響調整の未使用だけを理由にpluginを起動しない。通常のmetadata読込／受渡し拒否だけでは
EffeTune段をdisableしない。host終了／notify hangは§3.3.2のHostLostへ収束させる。
利用者への説明は §7.6。

## 4. metadata・画像の選択とlifecycle

### 4.1 送り元の実コード境界

| 境界 | 照合した現コードと追加する処理 |
| --- | --- |
| ローカル動画／音楽open | `src/app.rs:71037` 共通player構築、`:71526` 音楽open、`:71782` 動画open。生成時は公開しない。採用されて再生開始したplayerから軽いdescriptorを渡す |
| metadata準備／再生開始／再開 | `src/video/mod.rs:9703` info、`:9875` clock、`:9885` autoplay。`src/app.rs:88634` tickによる確定後にmetadataを更新。seek／bufferingのclock falseでclearしない |
| main／detached／parked live／tray | `src/app/viewer_context_registry.rs:3595`, `:3795` のcontext identityとread-only参照、`src/app.rs:14056` のresident presenter更新。mountやF12移送だけで別曲／clearにしない。既存playerの生存identityを維持 |
| Remote start確定 | `src/remote_ipc/ui.rs:1562` のStable start→`:1581` publish。`AppRemoteVideoStreaming`のmetadata playerとrequested_path (`:565`) を使う。paused headlessのis_playingを根拠にしない |
| Remote pause／seek／画質／音声track | `src/remote_ipc/ui.rs:2042`, `:2048` Play／Pause、session/generation検証。再生中mediaは維持。配信generationの切替はartwork completionのmedia identityと区別する |
| Remote session終了／cancel／切断／返却／error | `src/remote_ipc/ui.rs:1343`, `:1470`, `:1486`, `:2203` と `src/remote_ipc/session.rs:610`, `:629`。そのownerのmetadataだけclear。media EOF／端末側の最終停止はsession終了と扱わない。退役旧workerの終了で新曲をclearしない |

#### 4.1.1 Remote EOFではclearしない（R2 / P2-3、Coordinator決定）

現実装の `RemoteVideoStreamingSession::reconcile` (`src/video/stream/session.rs:865`, `:876`) は
配信workerのEndedでもActiveを返す。これは生成完了であり端末側の再生完了ではない。
Web側も最終停止は `setPlaying(false)` → Pauseとして送る
(`crates/remote-web/web/video-stream.mjs:2805`, `:2846`)、app側の受理は
`src/remote_ipc/ui.rs:2048`。`onEnded` はloop判定より先に進捗のendedを送る
(`video-stream.mjs:2002`)。進捗handlerはresume更新用 (`src/remote_ipc/ui.rs:2351`) で、
Now Playingの最終停止通知には使えない。

`CLAUDE.md:111`の単純化を適用し、Remote EOFを識別する新command／終端通知／Remote protocol更新は
**追加しない**。以下の表示仕様に固定し、最後の曲が終わった後にその曲名・画像が残ることを許容する。

| Remoteの境界 | Now Playingの扱い |
| --- | --- |
| 配信生成Ended、端末ended進捗、Pause、端末側の最終停止 | 現在のmetadataを保持。clearの条件にしない |
| 同曲loop | 保持。既存seek／再生を使い、配信generationが変わっても別mediaにしない（`video-stream.mjs:2863`） |
| 次曲へ進む | 移行に伴う既存stop／cancelが現ownerに一致すればclearを許す。次のmediaはStable startで採用後に公開する。開始待ちの空表示を許す |
| 次曲が無い | 既存Web処理はPauseへ落ちる（`crates/remote-web/web/app.js:3160`, `:3173`）。metadataを保持 |
| EOF後に同曲を再開 | 同じmediaのまま保持。session／media identityを新規採用した場合は通常start境界で再公開 |
| Remote session終了／media close／stop／cancel／切断／返却／error | 次曲への移行中も含め、現ownerに一致する終了だけclear。次media／Localが採用済みなら旧Remote終了でclearしない |
| Local playbackが採用される | Local snapshotへ置換。表示窓のfocusだけでは置換しない |

Remote progressのendedは、新旧session／owner generation／配信generationを問わずmetadata更新の入口にしない。
旧generationの生成完了、旧session終了callbackも現在owner／registrationを照合し、異なるownerのsnapshotを変更しない。
既存progressの順序／resume処理は維持する (`src/remote_ipc/ui.rs:2282`)。
最終停止だけのclearを追加する境界は、この保持仕様では正しさのために不要と判断した。

**R3決定: 次曲のStable採用まで前曲を保持する要件は置かない。** Webの次曲表示は
`cleanupScreen`で旧viewerを破棄し (`crates/remote-web/web/app.js:7420`)、destroyは旧sessionの
stopを送る (`crates/remote-web/web/video-stream.mjs:4122`)。Rustの新startも旧streamをcancelする
(`src/remote_ipc/ui.rs:2006`)。この時点でmetadata ownerが旧曲なら、既存終了経路でclearするのが正しい。
次曲への移行と明示終了を区別する追加状態／通知や、開始失敗時の前曲表示の復元は作らない。
新曲開始待ち・開始失敗後は空のままでよく、Stableで新曲を公開する。
遅延した旧stopはsession照合 (`src/remote_ipc/ui.rs:2201`, `:2224`) とmetadata owner照合で、
開始待ちの新streamや採用済みの新曲を停止／clearしない。handler-level試験は§6.1に明記する。

detachedの述語／viewportを変える実装が必要なら、事前にClaudeCodeと独立reviewerで構造判断し、
`docs/detached-rework-plan.md` §11へ記録する。単にmetadataを観測するためにwindow再生成や
別contextのcache失効を足さない。設計レビューはmetadataの開始／停止／再送を含む全境界を対象にする。

### 4.2 title / album / artist

`VideoInfo`はtitle／artistを持つ (`src/video/decoder.rs:1761`)。
demuxは `read_metadata_value(&input, &["title", "TITLE"])` とartist／ARTIST／authorを読む (`:2676`)。
同じworker上でalbum／ALBUMを読み、`VideoInfo.album: Option<String>`を追加し、`:2735`の構築と
test fixtures (`src/video/mod.rs:8743`等) を揃える。Now Playing用にUIからFFmpegをもう一度openしない。
`original_url`などの別metadata、ファイルの絶対path、URLを送信しない。
推奨はtitle無しなら再生ファイルのstem、album／artist無しなら空欄（§7.1）。
遅れて届いたVideoInfoはmedia identityが現在と一致する場合だけ採用する。
上流VideoInfoの文字列は変更せず、Now PlayingのUI側descriptor生成前に共有の借用prefix関数で
最初のNUL／4096 UTF-16単位までに制限する。Unicode scalarを途中で切らず、上限以降は走査・複製しない。
毎tickの変更判定もこのprefixだけを比較し、送信されないsuffixの変更ではrevisionを進めない。
snapshotの空title判定も制限後の文字列で行い、trim／cloneを上限検査より先に行わない。

### 4.3 artworkの優先順位

- **音声**: §1.347の共通source選択を使う。同名sidecar（既存設定に従う）→MP3先頭ID3v2の
  PIC／APIC（前面表紙優先）→無し。MP3以外の埋め込み画像対応は今回増やさない。
  出所mapは `src/app.rs:67218`、worker APIは `src/audio_thumbnail.rs:49`, `:77`。
  sidecar処理`:108`、非MP3無画像`:159`、bounded APIC reader`:234`。
  `src/audio_album_art.rs:6` はtag32 MiB／picture16 MiB／候補16枚であり、送信8 MiBとは別の読取上限。
  FFmpeg attached-picture抽出へfallbackしない。既存catalog admission／I/O permitを守る。
- **動画**: 一覧と同じ代表画像。pin WebP→同名sidecar→動画デフォルト代表画像の既存順序
  (`src/app.rs:49376`, `:49386`) を共有する。seek previewを追いかけて毎秒更新しない。
  有効なpin／catalog WebPをworkerで読んで再利用できる。無画像時は空にする。
  `src/video/thumbnail.rs:38` の320×180 seek thumbnailを「1024pxの代表原本」とは扱わない。
- UIのTextureHandleからGPU readbackしない。既存source resolver／decode処理を小さな共通worker関数に
  抽出してLocal／Remote／Now Playingが同じ優先順を使う。Remoteのsource descriptorは認証済みの
  `resolve_start_address` (`src/remote_ipc/video_stream.rs:105`) のpathだけを使い、別URLを取得しない。
- 同名sidecarの発見はUIで親directoryを走査しない。既存source proofが無ければworkerで
  同じsame-parent／stem／設定規則を使い準備する。catalog／sourceのmtimeとsizeもworkerで検証する。
  非active contextから最新一覧idx／settingsを借りて画像を選び直さない。

### 4.4 画像変換と予算

**推奨: 長辺1024px以下、縦横比維持、拡大なし、static一枚**（§7.3）。
PNG／JPEG／GIF／WebPでも画像寸法、orientation、encoded bytesを検査する。
cap内かつ正常なstatic PNG／JPEG／WebPはbytesを再利用可（特に既存cache WebP）。
GIF／animated WebP／APNGは最初の一枚、orientationが必要な原本やJXL／HEIC／RAW等は
既存decoderをworkerで使って縮小し、透過ありはPNG、他はJPEG（quality85目安）へ再encodeする。
RAWは既存executor、色／回転は既存代表画像の規則に従い、画像AI・追加の高解像度現像を始めない。

8 MiBは**decode前入力画像の一律拒否条件にはしない**。
たとえば16 MiB以内のAPICはbounded reader後に1024pxへ縮小して8 MiB以下へできる。
出力はencode後にも8,388,608 bytes以下を検査し、超過／decode失敗／unsupportedは画像無しのtext-onlyを送る。
際限なくquality／sizeを変えるretryを足さない。1024px PNGでも超過した場合の透明画像JPEG化は
不要（画像無しで通知／log、§7.6）。再encodeした画像のMIMEは拡張子でなく実encoderで決める。

画像workerは新規の単一 `effetune-now-playing-art`（案）とし、実行中一件＋最新pending一件まで。
既存thumb poolへの統合も検討したが、背景一覧の多数要求に待たされず、host-controlのGUI操作を
画像I/Oで止めない小さな専用workerを推奨。最新値の通知はchannel／Condvarを使い、sleep pollingを使わない。
decode割当前予算は既存bounded音声経路の160 MiB
(`src/audio_thumbnail/metadata.rs:4`) 以下とし、画素寸法／積を先に検査する。
これはencoded fileの上限ではない。設計案としてsidecar／cacheのencoded読取は64 MiBまでとし、
metadataで長さを検査した後もbounded readerで上限を守る。APICは既存16 MiB上限を維持。
RAWは全fileをこのbufferへ読まず既存executorにpathとcancelを渡し、取得したpreviewにもdecode予算を適用する。
sidecarの全file無制限read、EXIF全展開は新経路へ持ち込まない。
既存のEXIF展開問題はbacklog §1.354にあるので、Now Playingに必要なorientationは
§1.347の有界方式を共有する。既存全画像metadataの全面改修はこのchunkの範囲外。
形式別decoderで割当前制限を証明できなければ、共有経路の必要変更をClaudeCodeへ報告し、
対応形式を黙って削らない。

### 4.5 変更検知、clear、stale結果

- media切替／ownerに一致するclose／errorと、Localの明示stop／最終EOFでownerのrevisionを進める。
  Remoteは§4.1.1のとおりEOF／端末最終停止では保持する。次項目への移行時も既存stop／cancelで
  現ownerが終了したらclearし、次のStable採用またはLocal再生owner採用で公開する。
  明示clearは全空欄snapshot。新曲は旧artwork無しのtextを直ちに公開し、画像完了後に
  title／album／artistも含む完全snapshotを送る。source無しを前曲画像の保持にしない。
- 推奨はpause／seek／buffering／同じ曲のloop／F12移送／最小化では保持。
  EOF後の連続再生は次mediaの採用で置換し、Remoteの移行途中のstop／cancelによる空表示を許す。
  Localの最終停止はclear、RemoteのEOF／最終停止Pauseは保持。
  sourceの明示refresh／pin更新／metadata変更は同じmediaでも新revision。
  定期file pollやframeごとのbase64生成をしない。
- ownerはtypedな `Empty` / `Media { identity, revision, text, artwork: Pending|Absent|Ready }` を
  唯一の正本にする。jobは発行revisionを持ち、I/O段階、decode／encode後、mailbox採用時、
  transport送信前に現在と照合。A→B→A、clear→同path reopenをpath一致だけで通さない。
- revisionはmedia切替だけでなく、text／source変更と画像完了によるsnapshot更新でも進める。
  job完了は発行revisionと現在値の一致を確認してから、新revisionのReady snapshotを公開する。
  これによりtext-only送信済みというdedup判定が後続画像を消さない。実行中にtextが変わったら
  古いjobを破棄し最新入力で再発行する。同じsourceの既存cacheはworkerで再利用できる。
  Pending→Absentのみで送信内容が変わらない場合は再送不要。Clear→同path再開は新revisionを送る。
- completionはmutex下の小さいrevision検査とArc置換だけ。古い画像は今のsnapshotを更新せず破棄。
  host-controlは序列を崩さずC++へ送る。C++は§3.3.1のmailboxで中間値をcoalesceし、
  clearのwatermarkとslot incarnationを守るので、古い画像を新snapshotの後に適用することはない。
  write開始済みの旧snapshotが切替直後に一時的に見える可能性と、
  stale画像が後から戻る不具合は区別する。即時のプロセス間atomic表示まで保証しない。
- registerされた新bridgeはtransportの送信revisionをリセットし最新値だけ再送。
  artwork jobのrevisionはmediaのrevisionであり、GUIの開閉やaudio DSP generationで失効させない。
- UIが行うのは既にmemoryにある少量のmetadata／identity／source descriptorの提出だけ。
  file read／stat／directory scan／DB open／decode／縮小／encode／base64／約11 MiB write／joinはworker。
  `docs/ui-responsiveness.md:431`、`CLAUDE.md:704` の規則を適用する。

## 5. 単純化の検討と実装chunk

`CLAUDE.md:111`の順に検討した。

1. **単一mediaだけを扱う**: 採用。Local／Remote用の別snapshotや「最新窓優先」比較を無くし、
   既存の再生ownerから一つのtyped snapshotへ収束させる。
2. **画像生成中をモーダルにする**: 不採用を推奨。補助的なVisualizer画像のために
   再生開始／次曲／seekを待たせると既存操作が遅くなる。最新一件＋revision破棄で扱う。
3. **窓を閉じて再loadする**: 不採用。作者契約はeditor無しで更新できる。
   再loadは音響state／連続音声を妨げ、snapshot更新で済む。
4. **全応答／retry／配送保証を作る**: 不採用を推奨。notifyの結果はC++ log、
   sendはworker、resendはRegisterBridgeと次の正常media更新だけ。タイマーretry、
   DB保存、rollback、旧画像回復、失敗の細分類を作らない。通常拒否とhost hangは分け、
   hangは固定期限→専用host終了→HostLostへ一本化する。失敗時の見え方は §7.6 で説明する。
5. **Remoteの最終停止だけを識別する境界を作る**: 不採用（CoordinatorのR2決定）。
   EOF／loop／次曲なし／同曲再開では保持し、既存stop／cancel／session終了ではclearする。
   R3では次曲移行に伴う既存終了もclearを許し、Stableまで保持するための状態や復元を足さない。
   最終曲名が残ることと切替待ちで空表示になる仕様は§7.2に明記する。

実装はレビュー可能な二つのchunkに分ける。

- A: bundle／manifest／notice／版pin／manual snapshot更新、既存publisher／cleaner／portableの回帰。
  signing／配布準備はClaudeCode release lead。v0.12.0状態互換を検証する。
- B: C++ command＋protocol v6、typed Rust commandとtransport、単一source owner、artwork worker、
  Local／Remote lifecycle、ドキュメント更新。開始前にClaudeCodeと独立reviewerで所有設計を合意し、
  coherent diff完成後も独立review。共用ファイルは担当writerを一人に固定する。

## 6. 検証と受け入れ条件

設計時にはgit状態／版／source読取、ZIP hash、ZIP対抽出全448ファイル比較、追加／削除一覧、
通知diff、PE通常import／ASCII文字列を非起動で検査した。実装後の結果は§8に追記する。
Aの非署名source／fixture検証とBのRust／C++／画像／lifecycle検証、最終full gateは実装担当が所有する。
ClaudeCodeは署名済み最終artifact／release checklistと利用者のnative受入れ枠を所有し、独立reviewerは
設計とdiffを別contextで確認する。既に同じdiffで通った自動検証は再実行せず証拠を再利用する。

### 6.1 自動・headless

- Rust pure owner／fake transport: text-only→画像付き完全置換、各空欄clear、Local stop／close／error／EOF、
  pause／seek保持、A→B→A、遅い旧画像、旧Remote退役、別context closeによる新曲不変、
  metadata準備遅延、Remote pause headless、Idle→RegisterBridge、再RegisterBridge（同値／emptyも再送）、
  portable／Unavailable（job／sendゼロ）。fake transportで順序、実slot、送信スレッドを検査する。
- Remote handler-level: worker生成Ended、端末ended進捗（loop前も含む）、最終停止Pause、
  次曲なしPause、同曲loop／EOF後再開ではmetadata保持。次mediaのStable採用／Local置換で更新し、
  次曲移行に伴う既存stop／cancelを含む現owner終了でclear。現metadata ownerと一致しない旧sessionの終了や、
  旧owner generation／配信generationのended／退役報告を
  実handlerへ通して、現在のtext／画像／revisionが変わらないことを確認する。
  resume進捗記録の既存順序回帰も維持し、Now Playing用の終端通知を増やしていないことを確認する。
  R3の追加caseは実stop／cancel／start completion handlerとfake transportを通す:
  - **次曲の開始待ち**: AのEOF後は保持→Bへの移行で旧Aのstop／cancel→空snapshot。
    BがOpening／Startingの間は空のまま、Stable後にBのtext／画像を公開する。
  - **開始失敗**: 旧Aをclear後にBのstartが失敗しても空を維持し、Aを復元せずBも公開しない。
  - **遅延した旧stop**: B開始待ちとBのStable採用後の両方で、Aの旧session stopを到着させる。
    Bの開始を中断せず、開始待ちの空snapshot／採用済みBのtext・画像・revisionを変えない。
- serializationはfake Valueだけで済ませず、**実 `serde_json::to_vec(Cmd)` のbyte列**を検査。
  先頭 `{"cmd":"set_now_playing"`、JSON round-trip、空MIME、Unicode／quote／backslash／
  title内のkey風文字列、画像8 MiBのframe長。偽ackをevent_rxへ送らないことも検査する。
  wire 11,184,812文字の最大画像をdecodeして8,388,608 bytesとして受理し、
  **8,388,609 bytes（同じwire長になり得る）**を拒否する。wire文字上限＋1、padding不正、
  JSON frame 33,554,432 bytesのframing受理／＋1の確保前拒否を各層で別々に試験する。
  最大image frameの受理はC++側でも確認し、Rustのserialize成功だけで済ませない。
- C++非plugin fake: component／controller別・同一identity、component受理時の一回、controller fallback、
  no interface、各setter失敗、tresult、GUI thread ID、unload前後queue順とpayload寿命。
  4095／4096／4097 UTF-16単位、emoji境界、NUL、malformed UTF-8／surrogate、base64長／decode超過、
  MIME、unknown slot。SDK HostMessageでsetString／setBinaryの実コピーも検査する。
- C++ slow-GUI headless: fake receiverを期限内で意図的に待たせ、実stdin frame→mailbox→GUI経路へ
  最大画像を多数連投する。generic JSON queueに画像ゼロ、running≤1、pending≤1、未実行wake≤1、
  mailbox画像≤16 MiB、reader候補≤8 MiB／frame≤32 MiB／bridge画像・frame合計≤64 MiBを
  high-water counterでassert。clear後の古いrevision、clear→新media、slot close／同ID再利用、
  closeとRunning完了の競合を通し、古い画像の復活が無く最終値へ収束し、全payloadが解放されることを確認する。
- C++ hang headlessはfake receiverを子host内で戻らなくし、後続getStateがqueue待ちでも
  実行開始後5秒＋watchdog tick余裕でhostが終了することを親harnessから確認する。
  slot close後もRunning期限監視が残る場合を含む。Rust側ではHostLost→Failed／slot clear→素通し、
  利用者通知一回、ユーザーVST不変、自動再起動なしを検査する。通常kInvalidArgumentではhostを終了しない。
- artwork: sidecar優先、設定OFF、MP3前面表紙、非MP3無画像、video pin優先、source変更、
  static WebP再利用、large input縮小、1024px境界、EXIF回転、透過、GIF等first frame、
  JXL／HEIC／RAW、decode／encode失敗、8 MiB境界、bounded入力／割当、cancel後の新source不変。
- 配布: `scripts/test-effetune-distribution.ps1`、launcher source approval／publisher／path tests、
  `src/runtime_cleanup.rs`のlease／publisher／旧版／reparse tests、about全文一致。
  manifest449 entry、欠落／余計なファイル拒否、署名stage改変拒否、portable非同梱も検査。
- protocol gate: 本体／C++／testerのv6一致、旧v5 host拒否、
  `smoke_hello.py`のhello=6／ready=6を実行する。独立testerは
  `cargo check -p vst3-host-tester --bin vst3-host-tester` で検証し、tester GUIは起動しない。
- 狭い `cargo test -p mimageviewer --lib <filter>`、CMake build＋ctestの後、
  `cargo fmt --check`、UI文字列追加なら `python scripts/check_ui_glyphs.py`。
  共有protocol／Local／Remoteを変更するため最終は `scripts/test-full.ps1`。
  日本語manualはconverter test／`--check`／sitemap checkも通す。
  検証担当、diff／commit、コマンド、profile、exit code、件数、ログを記録し有効な結果を再利用する。

### 6.2 native／新版bundle／最終署名launcher

ClaudeCodeが具体的scenario／所要時間／desktop利用／データ範囲を示して利用者の明示承認を得た
検証枠でのみ実施する。今回のdesign依頼は起動の承認ではない。

1. 新bundleのUIを開く前の送信→Visualizerへtitle／album／artist／画像表示。
   UI閉じる→再度開く、plugin初期化し直し→同曲を再送、起動時Visualizer選択。
2. 日本語／emoji、長い文字列、欠落tag、画像無しの次曲、MP3 APIC／同名画像、動画代表pin／sidecar。
   pause／seekで保持し、Localのstop／close／最終EOFで消える。速い曲送りで前曲画像が戻らない。
3. main／別窓／F12移送／tray、Remote開始・切替・pause・seek・stop・切断・返却。
   Remoteでplugin GUIが隠れる既存方針を変えず、EOF／loop／最終停止／次曲なしでは保持する。
   次曲への移行時は既存stop／cancelで空表示になり得る。開始待ち／開始失敗／遅延した旧stop、
   同曲再開、次曲Stable採用、session終了、Local置換で§4.1.1どおりになり、古い曲が戻らない。
4. 同じv0.12.0保存状態とpresetを新版で読んで効果を維持。SFZのfolder選択／再起動後の参照、
   保存先／外部file依存、Adaptive Predictionと既存EQ／oversamplingのnative処理を確認。
5. **新版同梱・公開の通信／I/O gate**。通常起動・UI open／idleに加え、実音声を流しながら
   SFZ folder選択、楽器load／sample読取、reload、UI再open／アプリ再起動後の参照復元、library removeを
   使い捨てfixtureで実使用する。Adaptive PredictionもLearn値変更／学習、Freeze解除、
   実音声処理／Resetを実使用する。各段階で専用hostと全WebView子プロセスのPID／親子関係、
   listen endpoint、外向き接続先／port／時刻、成功・失敗・観測不能の結果を記録する。
   mIV Remoteの明示ON、利用者の外部リンクopen、OSによる選択済み共有folderへのアクセスは別記する。
   観測不能／未実施を「通信無し」と扱わない。
   同じ枠でSFZの保存path、元fileへの参照かcopyか、cache／index／sampleの範囲、再起動後の参照保持、
   library remove／runtime世代cleanupの削除範囲をfile I/Oと前後inventoryで確認する。
   元SFZ／sample、利用者の他のdataが勝手に削除されないことと、mIVのruntime cleanupから
   SFZ保存領域が分離されることを確認する。normal-profileの設定をagentが操作するtest setupは行わない。
   **公開前条件**は、実使用中のlisten／外向き通信と保存／参照／削除範囲を根拠付きで確認し、
   public/privacy/manualの説明を実測に合わせること。新版同梱は2026-10-09利用者決定済み。
   説明と異なる挙動があれば同梱を取り下げるのでなく公開説明を更新する。追加機能を無断で削らない。
6. 署名後の最終launcherで新世代公開、全448ファイル／VERSION／署名hash一致、
   再起動reuse、欠落修復、新旧世代の使用中保護と次回cleanup、長pathの拒否、
   通常版／installer同梱・portable非同梱を検査する。

hostを変えるので、実装時はdev coreだけで引き渡しを終えない。
通常検証buildは `scripts/build-release.ps1`（必要なら `-PreserveRuntime`）を使用し、
最終配布はClaudeCodeが `scripts/build-dist.ps1` の署名gateを担当する。
利用者の手動確認用コマンドは
`Start-Process -FilePath .\target\release\mimageviewer.exe`。
これは通常 `%APPDATA%\mimageviewer` の実settings／dataを更新し得る。
共有single-instance mutexがあるのでinstalled／tray版を先に閉じる。
エージェントはこの通常版を起動しない。自動UIのportable-smokeはEffeTune非同梱という製品境界があるため、
bundleを足して今回のnative受入れの代わりにはしない。新版pluginのnative検証は利用者の手動枠を正本にする。

## 7. 利用者決定（2026-10-09、全六項目回答済み）

1. **曲名がファイルに入っていないとき、ファイル名を出しますか。**
   **決定: 推奨案を採用。** 拡張子を除いたファイル名を出す。アルバム名とアーティスト名は、情報が無ければ空欄。
   絶対パスやURLは渡さない。
2. **一時停止中は曲情報を残し、リモートでは最後の曲が終わった後も残してよいですか。**
   **決定: 推奨案を採用。** シーク中や読み込み待ち、同じ曲の繰り返しでも残す。
   PCでの再生は停止・閉じる・最後まで再生して終了したときに消す。
   リモートは曲が終わっただけでは消さない。次曲へ移るときの停止・取消や再生セッション終了で消し、
   次曲の開始が確定したら新しい情報を出す。切替待ちや開始失敗時は空表示でもよい。
   PC側の再生へ切り替わったらその情報に置き換える。最後の曲名が残ることと切替中の空表示を許す方針。
   利用者補足: Remote使用中はPC画面をほとんど見ないので、実装を簡単にする。
3. **画像は長辺1024ピクセルまで、動く画像は最初の一枚でよいですか。**
   **決定: 推奨案を採用。** 再生を待たせず、先に曲名を出し、画像が用意できたら表示する。
   音楽は同名画像・MP3表紙、動画は一覧と同じ代表画像を使う。
4. **曲情報は音響調整の使用中に自動で渡し、専用のON/OFF設定は増やさなくてよいですか。**
   **決定: 推奨案を採用。** PC内のプラグインに渡すだけで、未使用ならプラグインをこのために起動しない。
   Visualizerで表示するかと起動時の画面はEffeTune側で選ぶ。
5. **新版の同梱と公開前確認。**
   **決定: 新版同梱は確定。** 公開前にSFZの選択・読み込み・再読み込みと、音声予測の学習・処理中も確認する。
   保存されるファイル、元ファイルへの参照、削除の範囲を確かめ、説明はmIVから実際に使える機能に限る。
   独立版EffeTuneのLANリモート操作は、mIVでも使えるとは案内しない。
   必要ならprivacy.html／製品ページ「安心して使えます」／manualを実測に合わせて更新する。
   通信・保存動作を理由に新版同梱を保留する方針は撤回する。実機確認自体は公開前に残る。
6. **通常の受け渡し失敗では音響調整を続け、プラグインが固まったときは音響調整を停止して知らせる扱いでよいですか。**
   **決定: 推奨案を採用。** 画像を読み込めなければ画像無しで曲情報を送る。読込失敗はログと一度の案内で知らせる。
   アプリからの送信に失敗した場合はログと一度の案内で知らせるが、前の曲名や画像が残る場合がある。
   プラグインに拒否された場合も前の表示が残る。返信を受け取らずログだけに記録するため、拒否の画面通知は行わない。
   通常の拒否では効果を止めず、次の曲の変更またはプラグインの次回初期化で送り直す。
   5秒戻らず固まった場合は音響調整用のプロセスを終了し、音声を素通しで再生し、一度知らせる。
   ユーザーVSTは終了させない。自動で繰り返したり、再起動したりしない（Coordinatorの決定済み方針）。
   送信できなければ消去も届かず、notify拒否でも作者契約上は前のsnapshotが残る。この見え方も了承済み。

component優先・必要時controller fallback（同じobjectには一度だけ）、reply無し、
latest一件の画像workerを含め独立設計レビュー完了。実装後の独立completion reviewと§6の検証を行う。

## 8. 実装・検証記録（2026-10-09）

実装基点は `ba5367125`。コミットせず、アプリも起動しない。C++／Rust bridge／App lifecycleを
担当別に実装し、独立reviewerが別contextで確認した。以下の未実施項目は自動検証の成功と区別する。

- A: 原本ZIP hashと全448ファイルのbytes一致を確認し、vendorを実ファイルでv0.13.0へ更新。
  旧原本は `target/D359-v0.12.0-source-backup` へ退避し、旧承認manifest／noticeは別の
  `target/D359-v0.12.0-approval-backup` に保存した。新版承認manifestはVERSIONを含む449 entry。
  `third_party/effetune-docs/6791afdf` は新版の上流EffeTune commit
  `6791afdfc6f4e6c913b9f48c8189a44e29a09bde` の固定snapshotで、旧snapshotも保持する。
  日本語manualは133 HTML＋4画像、113 effects。原本notice三本と追加licenseをaboutへ反映した。
- B: protocol v6、UI thread上のnotify、有界mailbox、5秒hang方針、Rust latest worker、
  Local／Remote owner、album metadata、非同期有界artworkを実装した。
  既存一覧はpin更新中も旧WebPを表示するため、Now Playingのread-only pin選択もPTS一致を条件にしない。
  非空BLOBと64 MiB読取上限は保ち、実 `set_pin` の更新待ち状態と一覧の一致をtempDBで試験する。
- C++ completion reviewで見つかったGUI timeoutの競合を修正。
  同じmailbox／incarnationと既存futureでnotifyの5秒期限を裁定し、実行回数により
  最初の待ち中に完了したnotifyも識別する。再投稿・retry・独立したRunning状態は追加しない。
  Rust/AppのHostLostと同時・直後に届くsend warningは既存Failed通知へ収束させる。
  pin補正もaffected-path reviewで追加指摘無し。
- 追加reviewで巨大tagのUI側全文clone／毎tick比較をP2として確認した。
  共有borrowed prefixでdescriptor生成・変更判定・snapshotを制限し、既存VideoInfoの内容は保つ。
  巨大suffix、Unicode scalar、NUL境界のheadless回帰とaffected-path reviewで解消を確認した。
  最終独立reviewの未解消P指摘はゼロ。

| 検証 | 結果・証拠 |
| --- | --- |
| ZIP／raw source gate | 全448ファイル一致、manifest449、`effetune-distribution.ps1 -ValidateSource` exit 0 |
| 配布fixture | `test-effetune-distribution.ps1` PASS、exit 0 |
| 日本語manual | converter 22/22、137出力＋entryの`--check`成功、sitemap343 URLs一致 |
| C++ Release | CMake exit 0、最終ctest 3/3（17.05秒、`target/D359-vst3-ctest-final.log`）、v6 hello／旧v5拒否／metadata無応答、identity一致 |
| C++ hang／slow GUI | 後続query／unload、watchdog ON/OFFとも約5.002秒で専用終了、stdout空。期限内slow／完了直後の同future回帰成功 |
| 本体check | 最新版 `cargo check -j 1 -p mimageviewer --bin mimageviewer-core` exit 0、`target/D359-normal-check-final.log`（30.02秒） |
| Rust Now Playing | 48/48、`target/D359-now-playing-tests-r5.log`。Local／Remote handler、stale画像、再Register、最大wire、失敗通知、巨大tag境界を含む。r4のtest-only型エラー五件は修正済み |
| 追加画像予算 | RAW返却raster／Shell割当前上限 各1/1、`target/D359-test-bounded_thumbnail.log`／`target/D359-test-shell_bitmap_budget.log` |
| 既存EffeTune回帰 | 105/105、`target/D359-test-effetune-r2.log`。既存fake hostもmetadataを無応答で受信しGUI試験を維持 |
| notice／cleanup | about原本一致1/1、runtime cleanup22/22、`target/D359-test-effetune_notice.log`／`target/D359-test-runtime_cleanup.log` |
| portable／独立tester | 最新core portable check exit 0（19.97秒）、`target/D359-portable-check-final.log`。独立tester check exit 0、`target/D359-tester-check.log`、full gateでtesterもcompile成功（unit caseは0件） |
| C++引き継ぎ | `target/D359-b-cpp-msg.txt`。source marker `2899e71293f96a8ffe5852b1a991b0a6d6aabe67a9c7f61e5e6ca3cd13f52541` |
| launcher | 39/39、`target/D359-launcher-tests.log`。承認449 raw files、実bundle抽出／修復／publisher／lease回帰 |
| Remote Web | 518/518、`target/D359-remote-node-tests-r2.log`。初回runnerはchild spawn EPERMで未実行、Node22のsame-process isolationで全assert成功 |
| 静的context audit | `cargo run --locked -p viewer_context_audit --quiet` exit 0、指摘ゼロ |
| 初回Release build | `build-release.ps1 -PreserveRuntime` exit 0、CRT依存runtime4／embedded12検査成功。巨大tag補正前のcoreなので最終確認用binaryとは区別する |
| 全体gate | `test-full.ps1 -SuppressCrashDialogs` PASS／exit 0、`target/D359-full-gate.log`。lib11457成功／52 ignored、UI snapshot140/140、Remote service137成功／1 ignored、IPC65/65 |
| vendor gate | 同じfull gateでegui25/25・egui-wgpu11/11・eframe18/18。viewer-context crate36/36。全targetの結果は`target/D359-full-results.json` |
| 最新確認用build | 利用者の続行指示どおり `build-dev.ps1 -PreserveRuntime` exit 0、`target/D359-dev-build.log`。core38m38s／Remote3m48s／EPUB2m26s、CRT runtime4／PE3成功 |
| 最終artifact一致 | dev-runtime内のEffeTune全448ファイルを承認manifestのbytesと照合し一致。B source30ファイルのSHA256不変、`target/D359-artifacts-final.json` |

最終 `cargo fmt --all -- --check` はclean、UI glyph lintは危険文字ゼロ。
コミット分割はA194ファイル／B33ファイルで、完全な一覧は `target/D359-commit-files.json`。
正本の引き継ぎは `target/D359-a-msg.txt`／`target/D359-b-msg.txt`／`target/D359-final-msg.txt`。
B内のC++／Rust／App記録は担当別の証拠であり、protocol／album／ownerの依存があるBを別々にcommitする推奨ではない。
利用者指定の最新版確認用起動コマンドは `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常APPDATAの実settings／dataを更新し得るため、installed／tray常駐版を先に閉じる。エージェントは起動していない。
実機・通信／保存の詳細手順は `target/D359-hardware-check.md`。
新版pluginの実機表示／v0.12.0 state互換、SFZ／Adaptive Prediction実使用中の通信・保存観測、
署名後の最終artifactは未実施でrelease leadの受入れ枠に残す。workspace外のvendor backup更新も
release leadへ引き継ぐ。unsigned原本を署名済みstageのhashで上書きしない。
再buildしたhost＋CRTの実ファイルは `target/D359-vst3-host-current` にも保存した。
これはツリー外backupの代わりではない。release leadは `CLAUDE.md` の
`C:/home/mimageviewer_vendor_backup/vst3-host` を現host＋CRTで更新し、新EffeTune原本もツリー外へ保存する。
