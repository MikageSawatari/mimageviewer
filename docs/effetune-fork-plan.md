# 音響調整の派生版 (Mixwright の mIV 用別名ビルド) 計画

状態: **第 6 版。独立設計レビュー 6 回目で ACCEPT (§15)。利用者の未決定 (§11) の回答待ち。実装未着手**。2026-10-24。
Coordinator は ClaudeCode。実装は Codex (GPT-6.1 Sol / high)、独立レビューは別の Sol / xhigh (AGENTS.md)。

## 0. 目的と利用者の決定

mIV は v4.3.0 から EffeTune Mixwright (VST3、MIT、作者 Frieve-A) を**無改変で同梱**し、音響調整として使っている。
これを、**mIV が自分でビルドする改変版を、別の名前で配る**形に変える。

利用者の決定 (2026-10-24):

- 理由は二つ。作者の負担を減らす (mIV の要望や問い合わせを作者へ持ち込まない)。mIV 独自の拡張 (歌詞など) を作者の対応を待たずに入れる。
  ビルドの手間が増えるのは構わない。
- **名称・クラス ID・保存先をすべて変える**。設定・プリセット・WebView の保存領域を APPDATA から mIV のデータフォルダへ移し、
  **ポータブル版でも音響調整を使えるようにする**。
- 利用者の目に入る「EffeTune」の語はなるべく消す。**著作権表示とクレジットは残す** (MIT の条件と礼儀)。
  コード内部の名前と通知 ID は変えない。
- マニュアルも作り直す (mIV で使えない機能を外し、名前を直す)。
- 作者には、方針を変えるときに一度だけ短く知らせる (2026-09-29 に了承を得たのは「無改変の同梱」だったため)。送るのは利用者。

作者の立場 (2026-09-17、利用者が共有): MIT に従えば改変・配布は自由で断りは不要。ただし「EffeTune」を冠した製品名は避けてほしい。

本書では新しい製品名が決まるまで **`<NAME>`** と書く (候補は §2.1)。

## 1. 今の形で起きている問題 (コード読み。実機では未確認)

1. **DAW 用の Mixwright と保存先を共有する**。mIV のユーザー VST の一覧は `%COMMONPROGRAMFILES%\VST3` などを走査し、
   パスで区別する (`src/video/dsp/scanner.rs`)。DAW 用に Mixwright を入れた人は、それをユーザー VST にも追加でき、
   音響調整の同梱版と同時に動く。クラス ID が同じでも別プロセスなので、それ自体は衝突しない。しかし次を共有する。
   - Mixwright の設定・プリセット・SFZ の参照: `%APPDATA%\Frieve\EffeTunePlugin\{config.json, effetune_presets.json,
     effetune_plugin_presets.json, sfz-references.json}`。デスクトップ版の `%APPDATA%\effetune` があればプリセットはそちら
     (上流 `src/bridge/config_store.cpp:26-39`、`preset_store.cpp:40-66`、`plugin_processor.h:556`)。
   - WebView の保存領域 (Visualizer のプリセットと画像、Room EQ の測定、IR、UI の状態): CHOC の `getUserDataFolder`
     (`third_party/choc/choc/gui/choc_WebView.h:2052-2073`) が `%APPDATA%\<host exe 名>` に決める。ユーザー VST 用と
     音響調整用の host は同じ `mimageviewer-vst3-host.exe` (`src/video/dsp/extract.rs:115`、`src/video/dsp/mod.rs:1066`, `:1204`)。
   版が違えば (例: DAW 用が v0.15、同梱が v0.14) 互いのデータを正しく読めないおそれがあり、同時に書けば後から書いた方が勝つ。
2. **mIV の不具合が作者の製品のデータに及ぶ**。上の共有先は作者の DAW 用・デスクトップ版のデータでもある。
3. **ポータブル版で使えない**。上の保存先が APPDATA に固定されている (`SHGetFolderPath`) ため、ポータブル版では音響調整を
   無効にしている (`docs/effetune-integration-plan.md` §10.1)。
4. **上流 v0.15.0 の Counter が mIV では曲の位置にならない**。v0.15.0 の Visualizer Counter は ProcessContext の
   `projectTimeSamples` と `sampleRate`・`kPlaying` を表示に使う (上流 `plugin_processor.cpp:2610-2648`、
   `tools/build-ui-assets.mjs:374-389`)。mIV の host は処理サンプル数を入れ、常に kPlaying、シークで 0 に戻す
   (`crates/vst3-host/src/plugin_loader.cpp:1289-1303`, `:2408-2409`)。Counter は「最後のシークからの経過」になる。

## 2. 名前とクレジット

### 2.1 製品名 (利用者が選ぶ)

条件: 「EffeTune」「Mixwright」(どちらも作者の製品名) を含めない。mIV の機能名「音響調整」と結び付く。英数字で短い
(バンドル名・フォルダ名・WebView のフォルダ名になる)。

| 候補 | バンドル名 | 所感 |
| --- | --- | --- |
| **mIV Sound** (推奨) | `mIV Sound.vst3` | 短く、mIV の一部だと分かる。音響調整の英語表記にも使える |
| mIV Audio Studio | `mIV Audio Studio.vst3` | 機能の広さ (エフェクト・Visualizer) は伝わるが長い |
| mIV Sound Tuner | `mIV Sound Tuner.vst3` | 「調整」に近いが、Tune が EffeTune を連想させる |

画面上の日本語の呼び名は今どおり「音響調整」とし、プラグインの窓のタイトルや About には `<NAME>` を出す。

### 2.2 置き換える範囲

| 種類 | 場所 (調査で確認) | 方針 |
| --- | --- | --- |
| バンドル・DLL 名 | 上流 `CMakeLists.txt:44-52` の `EFFETUNE_PLUGIN_PACKAGE_NAME`、`src/plugin/CMakeLists.txt:20-22`, `:59` | `<NAME>` |
| 工場情報 (host に見える) | `src/plugin/factory.cpp:11-22` (vendor "Frieve"、URL、クラス名 "EffeTune Mixwright") | vendor = "mImageViewer"、URL = mIV のサイト、クラス名 = `<NAME>`。作者の製品と取り違えさせない |
| 版 | `src/plugin/version.h`、`CMakeLists.txt:3`、`src/engine/pipeline_model.h:103` (状態に書く appVersion) | 上流の版・派生版の表示用の版・数値の版の 3 種類に分ける (§7.2) |
| 画面の表記 | `tools/build-ui-assets.mjs:8` の `productName` と `applyProductBranding()` (`:44-48`、HTML・About・locale 10 言語)、初回の案内 (`:9-26`)、`ui-shim/vst-bootstrap.js` (About・ヘルプ・GitHub のリンク `:315-338`, `:383-400`, `:713-763`)、native の文言 (`webview_host.cpp:99-111`, `:210`、`native_file_dialog.cpp:67-71`、`message_router.cpp:662`, `:687`) | `productName` を `<NAME>` にし、既存の置換の仕組みに乗せる。ヘルプのリンクは mIV のマニュアルへ。GitHub のリンクは外すか上流へのクレジットにする |
| 検証の固定値 | `tools/verify-ui-assets.mjs:108-168`, `:299-302`, `:353-357`、`tests/webview/webview_smoke.cpp:530-545`, `:2045`, `:2057`、`tests/CMakeLists.txt:218-243` | 名前と同時に直す |
| 上流のまま残る文言 | 上流の JS の約 25 か所 (`ui-manager.js:400` "Starting EffeTune…" など)。多くはデスクトップ版の機能用 | 画面に出るものだけ置換に加える。出ないものは残す (差分を増やさない) |
| mIV 側 | host の窓タイトル `L"EffeTune"` (`plugin_loader.cpp:526`)、`host_icon.rc:21`, `:25` の FileDescription / ProductName "EffeTune (mImageViewer)"、設定画面・検索 (`preferences/pages.rs:8424`, `:8427`、`search_index.rs:1323-1351`)、`version_highlights.rs`、About (`about.rs:106-120`, `:208`)、利用者向けの理由文 (`effetune/mod.rs:61`) | 「音響調整」または `<NAME>`。host の文字列は音響調整用の host とユーザー VST 用の host が同じ exe なので、窓タイトルだけを変え、FileDescription は中立な「mImageViewer audio host」などにする |

**変えないもの** (利用者に見えない、または変えると互換が切れる):

- 通知 ID `EffeTune.NowPlaying` (`plugin_processor.cpp:476`、mIV `now_playing_notify.cpp:33`)、エラー符号 `EFFETUNE-UI-*`、
  コードの関数名・ファイル名、localStorage と IndexedDB のキー、Visualizer のフォント ID (`visualizer-model.js:56-58`。レイアウトに保存される)。
- WebView の origin `https://effetune-mixwright.localhost/` (`webview_host.cpp:35-39`)。§5 の移行で保存領域をフォルダごと写すので、
  origin が同じでないと移したデータが見えない。
- プリセットとバックアップのファイル拡張子 `.effetune_preset` / `.effetune_backup`。DAW 用・デスクトップ版のプリセットを読み込む互換を保つ。
  既定のファイル名だけ `<NAME>` にする。
- mIV の設定の名前 (`effetune_*`、`ToolbarSectionId::EffeTune` の serde 名 `src/settings.rs:1820`)。変えると保存済みの設定が消える。
- mIV 内部の状態ファイル `data_dir/effetune/mixwright-state.json` (`src/effetune/mod.rs:1113-1117`, `:1378-1380`)。

### 2.3 クレジット (残す)

- 同梱の通知 (`THIRD-PARTY-NOTICES.txt` など) に、上流の二つの著作権表示と、MIT の許諾文・免責文をそのまま残す (About のクレジットだけでは代わりにならない):
  Mixwright `LICENSE` (Copyright (c) 2026, Yoshiyuki Kobayashi) と EffeTune `external/effetune/LICENSE` (Copyright (c) 2025-2026)。
  先頭行の製品名だけ `<NAME>` にし、直後に「EffeTune Mixwright (Frieve) を元にした改変版」と書く。
- プラグインの About と、画面下の「(C) Frieve 2025, 2026」(`effetune.html:344`、`electron-integration.js:417`) は残し、
  「Based on EffeTune / EffeTune Mixwright by Frieve」を添える。
- mIV の About、マニュアル、製品ページ、privacy.html に同じクレジットを書く。作者のサイト・GitHub への紹介は今のマニュアルの方針
  (`project_effetune_manual` の決定) を引き継ぐ。
- アイコン画像 (`images/icon_64x64.png`) は作者のものなので、mIV の音響調整のアイコン (`generate-effetune-icon.py` の音符) に差し替える。

## 3. クラス ID

- 出荷用の `kProcessorId` (`src/plugin/plugin_ids.h:14`、`72A0F755-…`) を新しい ID にする。SingleComponentEffect なので 1 つだけ。
  検証用の `A74D1C62-…` (`EFFETUNE_AUTOMATION_HOST_GATE_FIXTURE`) も、派生版用に別の ID にする。
- mIV はクラス ID を見ていない。host は工場の中の最初の `kVstAudioEffectClass` を選び (`plugin_loader.cpp:964-969`, `:1077-1080`)、
  保存済みの状態はファイルの中身をそのまま渡す (`src/effetune/mod.rs:1677-1769`)。状態の形式 (StateCodec、`formatVersion:1`) を
  変えなければ、出荷済みの `mixwright-state.json` はそのまま読める見込み。**派生版の最初の段階で、出荷済みの状態ファイルを
  派生版に読ませる自動テストを置いて確かめる**。
- Mac 用のバンドル ID (`src/plugin/CMakeLists.txt:106-123`) も派生版のものに変える (mIV は Windows 専用だが、上流のビルド定義を壊さないため)。

## 4. 保存先

### 4.1 新しい配置

```text
<data_dir>\effetune\
  mixwright-state.json          (既存。名前は変えない)
  plugin\                       (新設: 旧 %APPDATA%\Frieve\EffeTunePlugin と %APPDATA%\effetune の役)
    config.json  effetune_presets.json  effetune_plugin_presets.json  sfz-references.json (+ .lock / .tmp)
  webview\                      (新設: 旧 %APPDATA%\mimageviewer-vst3-host.exe の役。WebView2 の user data folder)
  migration-v1.done / migration-v1.not-applicable   (§5 の移行の判断の印)
```

`<data_dir>` は通常版が `%APPDATA%\mimageviewer`、ポータブル版が `<exe_dir>\data` (`src/data_dir.rs:321-323`)。
`--data-dir` で隔離したときもそこに入る。

### 4.2 受け渡し方法

- mIV は音響調整の bridge を起動するとき、**その bridge の環境変数に `MIV_SOUND_DATA_DIR=<data_dir>\effetune`** を設定する
  (ユーザー VST 用の bridge には設定しない)。名前は製品名の決定後に確定する。
- 派生版は次の 3 か所でこの値を使う。
  1. `ConfigStore::resolvePath()` (`config_store.cpp:26`): `<dir>\plugin\config.json`。SFZ の参照はこのフォルダに従う (`plugin_processor.h:556`)。
  2. `PresetStore::resolvePresetPath()` (`preset_store.cpp:40-66`): `<dir>\plugin\...`。デスクトップ版の `%APPDATA%\effetune` を使う分岐は消す。
  3. WebView2 の user data folder: CHOC の生成器 `tools/build-choc-webview-header.mjs` に置換を 1 つ足し、`Options` に
     `userDataFolder` を加えて `getUserDataFolder()` より優先させる。`webview_host.cpp:647-713` で `<dir>\webview` を渡す。
  設定・プリセット・SFZ のストアはプラグインのコンストラクタ (`plugin_processor.cpp:353-354`、`plugin_processor.h:554-556`) で作られ、
  host の context が届く `initialize(context)` より前に場所が決まる (SfzLibrary は作り直せない)。WebView は `initialize()` の中 (`:398-401`) で
  すぐに作られる。どちらにも間に合い、作り直しも要らないので、環境変数が一番単純で確実。
- **環境変数が無いときは起動に失敗させる** (エラーとして mIV に返す)。APPDATA へ黙って戻ると §1 の問題が戻る。
  派生版は DAW で使うことを想定しない。
- `WEBVIEW2_USER_DATA_FOLDER` (プロセス全体の環境変数) は使わない。同じ host の中の他の WebView にも効いてしまう。

### 4.3 検討して採らなかった案

- **プラグインの隣にある設定ファイルで場所を伝える**: 配布物の中身が利用者ごとに変わり、hash 検査と合わない。
- **IPC の新しい命令で場所を伝える**: 上の通りコンストラクタの時点で必要なので間に合わない。

## 5. 出荷済みのデータの移行

v4.3.0〜v4.8.0 の利用者が音響調整で作ったデータは、保存先を変えると派生版から見えなくなる。CLAUDE.md の
「利用者が作ったデータが失われる」場合に当たるので、**最初に一度だけ写す**。出荷済みの同梱版は v4.3〜v4.5 が Mixwright v0.12、
v4.6〜v4.7.1 が v0.13、v4.8 が v0.14 で、Windows の WebView の origin はどれも同じ (第 1 回レビューで確認)。
旧版のプラグインの設定・プリセット・WebView は、mIV のデータフォルダに関係なく APPDATA の共通の場所にあった (§1-1)。

### 5.1 対象と判断の印

判断は `<data_dir>\effetune\` の中の**印**で行い、フォルダが空かどうかでは判断しない
(既存の利用者は `mixwright-state.json` を持っているので「空」にならない。第 1 回レビュー P1)。

- `migration-v1.done`: 写し終えた。
- `migration-v1.not-applicable`: 対象外。
- どちらも無い: 未判断。

対象かどうか (第 2 回レビュー P2 を受けて改めた):

| データフォルダ | 判断 |
| --- | --- |
| 通常のデータフォルダ (`%APPDATA%\mimageviewer`) | 写す元 (§5.3) が 1 つでもあれば対象。無ければ対象外 |
| `--data-dir` の別のフォルダ | **`mixwright-state.json` があれば対象** (そのフォルダで音響調整を使っていた = 旧版の共通の場所のデータを使っていた)。無ければ対象外 (新しく作った検証用のフォルダへ普段のデータを流し込まない) |
| ポータブル版 | 対象外 (これまで音響調整を使えなかった) |

対象のフォルダが複数あれば、それぞれが同じ共通の場所から写す (旧版ではどれも同じデータを共有していたので、それぞれが同じ状態から始まる)。

印が無い限り、**派生版を起動しない**。新しい保存先 (`plugin\`、`webview\`) を移行の前に作らせないため
(先に作られると、次の起動で写す先に新しいデータが混ざる。第 1 回レビュー P1)。

### 5.2 いつ・どの順で

- mIV の起動時に、ユーザー VST の起動 (`src/app.rs:31590` 付近) と音響調整の起動の**両方より前**に、移行の判断と実行を 1 回だけ行う。
  worker で行い、UI スレッドでは行わない。移行が終わるまで、ユーザー VST と音響調整の起動はこの結果を待つ
  (旧 WebView 領域はユーザー VST 用の host も使うので、自分の host を起こす前に写す。第 1 回レビュー P2)。
- **写す間の書き込みを、書き込む側の境界で止める** (第 2〜4 回レビュー P2)。写す元は二種類あり、書き込む側が違う。
  - **プラグインの JSON ファイル** (§5.3 の上 2 行): 書き込むのは旧版のプラグイン (別のデータフォルダの旧版 mIV、DAW 用の Mixwright も含む)。
    どれも一時ファイルへ書いてから置き換える (上流 `config_store.cpp:72-93`、`preset_store.cpp:129-148` の MoveFileEx)。したがってファイル単位では
    「古い中身」か「新しい中身」のどちらかしか読めない。**ファイルごとに写し、写した先を JSON として読めることを確かめる**。読めなければ失敗 (§5.4)。
    ファイル同士の一致は求めない (旧版でも別々に保存されている)。DAW を止める必要はない。
  - **WebView の保存領域** (§5.3 の 3 行目): 書き込むのは、その領域を使う **WebView2 のブラウザプロセス** (`msedgewebview2.exe`) だけ。host が終了しても
    ブラウザプロセスが残って書き続けることがある (上流の破棄はブラウザの終了を待たない `choc_WebView.h:1548`。mIV でも親の終了後に残った実例がある
    `docs/effetune-integration-plan.md:826`)。プロセスの有無や更新時刻の比較では、書き込みが無いことの根拠にならない (第 4 回レビュー P2)。
    ブラウザプロセスは、使っている間ずっと保存領域の `EBWebView\lockfile` を開いている (Chromium の ProcessSingleton)。そこで、
    1. 写す前に、mIV が `%APPDATA%\mimageviewer-vst3-host.exe\EBWebView\lockfile` を**他から開けない形 (共有なし) で開く**。開けなければ、
       その保存領域を使っているブラウザプロセスがある (別の mIV、残っている WebView2) ので、写さない (§5.4)。
       ファイルが無ければ、閉じたときに消える形 (FILE_FLAG_DELETE_ON_CLOSE) で作って開く (原本に跡を残さない)。
    2. 開いたまま写す。**`EBWebView\lockfile` そのものは写さない** (共有なしで開いているので、写そうとすると自分でも開けない。
       写した先では WebView2 が作り直す。第 5 回レビュー P2)。この間に新しいブラウザプロセスがこの保存領域で起動しようとしても、lockfile を取れずに
       起動できない (別の mIV のプラグインの画面がその間だけ開けない。その mIV は開き直せば使える)。
    3. 写し終えたら閉じる。
    **これは Chromium の内部の仕組みに頼るので、F2 の最初に小さな試作で確かめる**。確かめること: ブラウザプロセスが生きている間は共有なしで開けない。
    開いている間は WebView2 の起動が失敗し、写す元のデータを壊さない。lockfile を除いて写した先で、派生版が WebView2 を起動でき、データを読めること。確かめられなければ実装に進まず、ClaudeCode に戻して利用者に相談する。
- **旧 WebView 領域 (`%APPDATA%\mimageviewer-vst3-host.exe`) が無い場合**は、WebView の部分を飛ばし、JSON ファイルの移行だけで完了 (`migration-v1.done`)
  にする。写す元に lockfile のためのフォルダを作らない (初めて mIV を使う DAW の利用者などで、毎回失敗し続けないように。第 5 回レビュー P2)。
- 自分以外の mIV の本体 (`mimageviewer-core.exe`、ポータブル版の `mimageviewer.exe`) と `mimageviewer-vst3-host.exe` が動いているかは、
  写すかどうかの根拠にはしない (上の lockfile が根拠)。通知の文で「すべて終了」を案内するためにだけ、ログに残す。
- プロセス間の待ち合わせ (mutex など) は作らない。旧版の mIV はそれに従わないが、lockfile には従う (WebView2 自身の仕組みなので)。

### 5.3 何をどこから

写す元は「旧版のプラグインが使っていた共通の場所」(`%APPDATA%` 配下)。どのデータフォルダの移行でも同じ場所から写す。

| 写す元 | 写す先 | 規則 |
| --- | --- | --- |
| `%APPDATA%\Frieve\EffeTunePlugin\config.json`、`sfz-references.json` | `<data_dir>\effetune\plugin\` | あるものだけ |
| プリセット `effetune_presets.json`、`effetune_plugin_presets.json` | 同上 | **旧コードと同じ規則で 1 つのフォルダを選ぶ**: `%APPDATA%\effetune` フォルダが存在すればそこ、無ければ `%APPDATA%\Frieve\EffeTunePlugin` (上流 `src/bridge/preset_store.cpp:40-66`)。2 つのフォルダを混ぜない (第 1 回レビュー P2) |
| `%APPDATA%\mimageviewer-vst3-host.exe\` の中身全体 (WebView2 の user data folder) | `<data_dir>\effetune\webview\` | origin を変えないので、Visualizer のプリセットと画像 (IndexedDB)、IR (OPFS)、測定 (IndexedDB)、UI の状態 (localStorage) がそのまま読める見込み。§5.5 で確かめる |

原本は消さない・変えない。ユーザー VST の他のプラグインのデータも WebView 領域に入っているが、まとめて写しても害はない。

### 5.4 写さなかったとき・失敗したとき

- 次のどれかの場合は、印を置かず、写しかけたものを消して、
  - 旧 WebView 領域があるのに、その `EBWebView\lockfile` を共有なしで開けなかった (使用中)。
  - 写す途中で失敗した (読めない・書けない)。
  - 写した先の JSON ファイルを JSON として読めなかった。
  **その起動では音響調整を使えない状態にする**。通知の文:「音響調整のデータを引き継げませんでした。この mImageViewer も含め、
  トレイに常駐しているものもすべて終了してから、1 つだけ起動してください」(第 3 回レビュー P2。すでに起動している mIV を残したまま起動し直すと、
  既存の窓が前面に出るだけで移行はやり直されない。`src/lib.rs:1297`、`src/tray.rs:3`)。ログに理由 (動いていたプロセス、失敗した段) を残す。ユーザー VST は旧 WebView 領域を使えるので起動してよい。
- 次の起動で再び試す。再試行の繰り返しや部分的な取り込みは作らない。まれな失敗は割り切り (CLAUDE.md)。原本が残るので、利用者のデータは失われない。

### 5.5 確かめること (F2 の受け入れ条件)

- **移行処理は、写す元の根と写す先を引数で受け取る関数にする**。通常の入口は `%APPDATA%` と `<data_dir>\effetune` を渡す。
  自動テストは、合成した旧データ (設定・2 つのフォルダのどちらかにあるプリセット・SFZ の参照・ダミーの WebView 領域) で、§5.1 の判断表・§5.3 の規則・
  §5.4 の後始末を確かめる。
- **フォルダを写して読めることの確認**: 写す元の根を仮の `%APPDATA%` にして検証用の入口 (test-script 用の引数。通常の利用者の入口とは分ける) で
  別の隔離した data-dir へ移行し、派生版でユーザープリセット・Visualizer の画像とプリセット・IR・測定を読み込み、保存し直し、mIV を再起動しても残っていることを
  確かめる。素材は次の項目で作る。エージェントは隔離した data-dir でだけ確認用ビルドを操作する。
- **旧版の保存データを読めることの確認** (第 4 回レビュー P2。保存先の変更と v0.15 への更新を同時に出すので、移行の受け入れに含める):
  - 旧版のプラグインは `--data-dir` を指定しても APPDATA に書く (上流 `config_store.cpp:26`、`preset_store.cpp:40`。第 2 回レビュー P1) ので、
    利用者の普段のデータを汚さない形で旧データを作る。この作業環境の Windows Sandbox では WebView2 が動かない記録がある
    (`docs/effetune-integration-plan.md:507`。第 3 回レビュー P2) ので Sandbox は使わない。
  - **旧版の素材を作る検証用ビルド** (第 5 回レビュー P1): 旧版のプラグインは、設定画面を操作しなくても、初回の案内を始めた時点で
    `onboardingTourShown` を `config.json` に保存する (上流 `external/effetune/js/ui/onboarding-tour.js:23`, `:93`、`ui-shim/vst-bootstrap.js:425`。
    v0.14 にもある)。host の名前を変えても、この保存先は分けられない。そこで、**上流 v0.14.0 のソースに、F1 と同じ保存先の差し替え (§4.2 の 3 か所) だけを入れた
    検証用のビルド**を作る (`G:\home\effetune-mixwright-trial` に v0.14.0 のタグから検証用のブランチを切る)。保存の形 (シリアライズ) は旧版のまま。
    このビルドを隔離した data-dir の確認用ビルドで動かし、設定・プリセット・SFZ の参照・Visualizer・IR・測定をすべて隔離した場所に作る。利用者の
    `%APPDATA%` には書かない。作ったものを旧版の配置 (`Frieve\EffeTunePlugin\`、`mimageviewer-vst3-host.exe\`) に並べて素材にする。配布はしない。
  - **JSON ファイル側の形の確認**: 上流 v0.12・v0.13・v0.14 のソースの保存の形 (ConfigStore・PresetStore・SfzLibrary と UI 側のプリセットの形) を v0.15 と比べ、
    違いがあれば、その版の形の fixture を合成して、派生版が読み込み・保存し直せることを自動テストで確かめる。
  - **v0.12・v0.13**: 保存を受け持つ上流のコード (`visualizer-preset-store.js`、`visualizer-model.js` の正規化、測定の `dataStorage.js`、
    IR の `opfs-ir-library-backend.js`、`user-data-backup`、上の JSON の 3 つ) を v0.14 と比べ、変わっていなければ v0.14 の素材で代える。変わっていれば、
    その版のタグに同じ差し替えを入れた検証用のビルドで素材を作る。各版で実機の操作を繰り返すことは求めない。
- **移行処理の自動テスト** (合成した旧データ) に、旧 WebView 領域が無い場合 (JSON だけで完了)、lockfile を除いて写すこと、§5.4 の各失敗で写しかけたものが消えることを含める。

### 5.6 DAW・デスクトップ版との共有がなくなること

- 移行の後に DAW 側やデスクトップ版で作ったプリセットは、mIV に自動では出てこない。行き来はプリセットの書き出し・読み込み
  (拡張子は互換のまま) で行う。これはリリース済みの挙動の変更 (日常の操作が増える)。
- 2026-10-24 に ClaudeCode が利用者へ「DAW やデスクトップ版とのプリセット共有はなくなる。リリース済みの挙動の変更なので、
  マニュアルと更新履歴に書く」と説明し、利用者は保存先を分ける方針で進めると回答した (§11 に記録)。更新履歴・マニュアル・privacy.html に書く。

## 6. ポータブル版での有効化

- 配布物: 署名済みの `mimageviewer-vst3-host.exe` と `<NAME>.vst3` をポータブル版の zip に入れる (`scripts/build-portable.ps1:430-450`、署名 `:514` 以降)。
  bundle は `resolve_bundle_at` の portable 分岐 (`src/effetune/mod.rs:686-690`) が読む平らな配置 `<exe_dir>\effetune\<NAME>.vst3`。
  host が入るとユーザー VST もポータブル版で使えるようになる (`src/video/dsp/mod.rs:42-52`)。
- 外す制限: `ToolbarSectionId::EffeTune.available_in_build()` (`src/settings.rs:1840-1846`)、設定画面と検索の cfg
  (`preferences/pages.rs:8299-8316`, `:8421-8485`, `:8627-8637`、`search_index.rs:1319-1351`)、About (`about.rs:102-126`, `:206-215`, `:319`)、
  `version_highlights.rs`、`try_auto_open` (`effetune/mod.rs:1208-1212`)、`app.rs` の自動表示の cfg、Now Playing の `available` (`effetune/mod.rs:1015`)。
  一覧は実装の最初に全部数え直す。
- ポータブル版は launcher を通らないので、世代の公開 (`runtime/<ver>/effetune/`) は無い。平らな配置のまま使う。
- WebView2 Runtime が必要な点は通常版と同じ。
- 音響調整は APPDATA を使わなくなるが、ポータブル版で使えるようになるユーザー VST (利用者が入れた任意のプラグイン) が APPDATA を使わないとは言えない。
  マニュアル・privacy.html ではそう書き分ける。
- privacy.html・製品ページ・マニュアルの「ポータブル版では使えない」を直す (`docs/portable-build-plan.md` も)。

## 7. 派生版のソースとビルド

### 7.1 ソースの置き場

- 上流の `effetune-mixwright` を元にした git リポジトリを `G:\home\miv-sound` (名前は製品名に合わせる) に作る。
  `upstream` に作者のリポジトリ、作業ブランチ `miv` に mIV の変更を積む。上流の新しいタグを取り込むときは merge する。
- 変更は小さく保つ。名前・クラス ID・保存先・Counter を外すこと (§8)・(後の段階で) 位置と歌詞の受け取り。上流の流儀
  (`tools/build-ui-assets.mjs` の置換と、外れたらビルドを止める検査) に合わせる。
- ソースを公開するか (例: GitHub に置く) は利用者が決める (§11)。MIT は公開を求めないが、改変版を配るなら公開しておくと
  問い合わせに答えやすい。

### 7.2 版の三つの表し方

| 用途 | 例 | 置き場所 |
| --- | --- | --- |
| 上流の版 | `0.15.0` (タグは `v0.15.0`) | `PROVENANCE.md` にタグの文字列のまま記録する。更新の検査 (`scripts/check-dependency-updates.ps1:170`, `:270` は文字列の完全一致) は、記録したタグと上流の最新タグを、どちらも先頭の `v` を取り除いてから比べる (第 2 回レビュー P3) |
| 派生版の表示用の版 | `0.15.0-miv.1` | `version.h` の文字列 (`EFFETUNE_PLUGIN_VERSION_STR`)、工場情報の版、画面の版、`third_party/miv-sound/<ver>/VERSION` |
| 数値の版 | `0.15.0.1` | `CMakeLists.txt:3` の `project(VERSION ...)` (数値しか受け付けない)、PE の版情報 |

状態に書く `appVersion` (`src/engine/pipeline_model.h:103`) はコンストラクタで上書きされ (上流 `plugin_processor.cpp:345`)、codec の互換判定には使われない
(上流 `state_codec.cpp:410`、第 1 回レビューで確認) ので、表示用の版でよい。

### 7.3 ビルド・確認・承認を分ける

第 1 回レビュー P2 を受け、「1 回のビルド成果物を確かめてから、その成果物だけを承認する」順にする。

1. **ビルド**: 派生版のリポジトリの ASCII の `build-miv.bat` (試作の `build-trial.bat` を元に、configure の `/wd4458` を含む) で configure → build → ctest。
   `scripts/build-miv-sound.ps1` (mIV 側、ASCII) がこれを呼び、候補ごとの置き場を作る (第 2・3 回レビュー P2):

   ```text
   target\miv-sound-candidates\<候補 ID>\
     artifact\          配布の入力そのもの: VERSION と <NAME>.vst3 の全体 (今の vendor/effetune-mixwright と同じ形)
     candidate.json     候補 ID、artifact の一覧 (相対パスと hash。manifest と同じ形)、出どころの記録
   ```

   候補 ID は `artifact\` の一覧の hash。`candidate.json` は `artifact\` の外に置くので自己参照にならない。`VERSION` はビルドの時に作って `artifact\` に入れる。
   置き場は候補ごとに別なので、後のビルドが前の候補を上書きしない。
   **未コミットの差分や submodule のずれがあれば止める**。出どころとして記録するのは、派生版の commit、上流のタグと commit、
   submodule の commit、CMake の構成 (preset と渡した `-D` の値)、道具の版 (MSVC・CMake・Node・vst3sdk の pin)。
2. **確認**: 候補 ID を指定して `build-dev.ps1` の確認用ビルドに `artifact\` を置き (候補 ID を受け取る引数を足す。置くときに一覧と中身が一致することを確かめる)、
   自動テストと実機確認をし、独立レビューを通す。確認の記録には候補 ID を書く。
3. **承認**: `scripts/build-miv-sound.ps1 -Approve -Candidate <候補 ID>` が、`artifact\` の中身を一覧と照合し (一致しなければ止める)、
   一覧から `third_party/miv-sound/<ver>/manifest.sha256` を作り、`PROVENANCE.md`・`VERSION`・通知 3 本を置き、**`artifact\` の中身だけ**を `vendor/miv-sound/` へ写す
   (今のファイル集合の検査が余分なファイルを拒むため、`candidate.json` は写さない)。承認物そのものを
   `C:\home\mimageviewer_vendor_backup\miv-sound-<ver>\` に保存する。承認はこの明示の操作のときだけ行う。
4. **配布ビルド**: `build-release.ps1` / `build-dist.ps1` は、今と同じく承認済みの manifest と照合するだけで、**manifest を作り直さない**
   (今の `scripts/effetune-distribution.ps1:46` が承認 manifest の作り直しを禁じているのと同じ考え)。

再現ビルドでバイトまで一致することは求めない (DLL に時刻が入り得る)。承認した成果物そのものを保存しておく。

### 7.4 配布の検査の作り替え

- `third_party/miv-sound/<ver>/`: `VERSION`、`manifest.sha256`、通知 3 本 (上流の著作権表示・許諾文・免責文をそのまま含む。§2.3)、`PROVENANCE.md`。
- 検査の仕組み (ファイル集合・hash・署名だけの変化・stage・世代の公開・パス長) はそのまま使い、固定値 (バンドル名・版・manifest の場所) を
  **一か所の定義から読む**ようにする。調査とレビューで見つかった固定値の場所:
  - `scripts/effetune-distribution.ps1:37`, `:41`, `:52`, `:94-95`, `:123`, `:129-135`, `:187`
  - `scripts/build-dev.ps1:137`、`scripts/build-release.ps1:105`, `:316`、`scripts/build-dist.ps1:220`
  - `crates/launcher/build_effetune.rs:12-17`, `:23`, `:37`, `:43`, `:61`、`build_effetune_source.rs:11`, `:75`, `:78`, `:173`、
    `crates/launcher/src/main.rs:922` 付近 (テスト)、`crates/launcher/src/effetune_bundle.rs` のテスト (`:386`, `:481`)
  - `src/effetune/mod.rs:23`、`src/runtime_cleanup.rs:166` (固定中のバンドルの認識条件)、`src/ui_dialogs/about.rs:103-126`, `:599-606`
  - `scripts/test-effetune-distribution.ps1`、`scripts/check-dependency-updates.ps1:268-273`、`scripts/gen-effetune-docs.py:29`, `:608-611`, `:762`
  - `installer/readme.txt`、`htdocs/mimageviewer/.htaccess:27`、テスト (`src/video/dsp/bridge.rs:1734`, `:2023`、`src/app/tests.rs:9320`、
    `crates/vst3-host/tests/path_boundaries.cpp:84-152`)
- **改行の保護**: `.gitattributes` は今、旧通知のパスと旧マニュアルのパスだけを守っている (`.gitattributes:26`, `:31`)。新しいパス
  (`third_party/miv-sound/**` の `-text`、新しいマニュアルの HTML の LF) を同時に足す。足さないと Windows の checkout で通知のバイト一致や
  生成物の検査が壊れる (第 1 回レビュー P2)。
- 古い世代の掃除: 削除の候補は世代の名前で列挙するので (`src/runtime_cleanup.rs:184`)、バンドル名ごとの掃除の経路を増やす必要はない。
  `:166` は「今固定しているバンドルを認識する」条件なので、新しい名前に合わせる (第 1 回レビューで補正)。

### 7.5 上流への追従

- 方針は「**常に派生版**」(利用者決定)。上流の新しい版は、価値がある変更のときに取り込む。mIV のリリース前に
  `scripts/check-dependency-updates.ps1` で上流の版 (`PROVENANCE.md` の上流の版と比べる) を確かめる。
- 取り込みの手順: 上流のタグを merge → 置換の検査が外れていないか確かめる → 7.3 の 1〜3 → マニュアルを生成し直す。

## 8. Counter (上流 v0.15.0 の新機能) の扱い

- 上流 v0.15.0 の Visualizer Counter は、ProcessContext の `projectTimeSamples` / `sampleRate` / `kPlaying` を表示に使う (§1-4)。
  mIV の host は処理サンプル数を入れているので、そのままでは曲の位置にならない。
- 初版の計画では、ブロックごとの曲の位置を共有メモリで渡して ProcessContext に入れる案を書いた。第 1 回レビューで、
  時間伸縮・ユーザー VST・入力リミッターの後の位置、1 回の処理要求の分割、予熱・リセット時の無音の押し出し・EOS の tail、
  一時停止中の先読み (処理位置と聞こえる位置が違う) で、どれも正しく扱うには定義が多いと指摘された (P2-5〜7)。
- **決定 (初版を改める)**: 派生版の最初のリリースでは、**Counter を外す**。上流がプラグイン版で Lyrics を外しているのと同じ方法で、
  Visualizer の追加メニューから Counter を、内蔵プリセットから Counter Gallery を外す。さらに、生成器が差し替える `counters()`
  (上流 `tools/build-ui-assets.mjs:374-389`) を**位置を渡さない形 (空の Map) にする**。メニューとプリセットを外すだけでは、取り込んだレイアウトや
  保存済みのレイアウトの Counter が ProcessContext の誤った位置を表示し続ける (第 2 回レビュー P2)。値が無いときは renderer が 0 を表示する
  (上流 `external/effetune/js/visualizer/visualizer-renderer.js:162`)。Counter の型と編集の機能は残す。生成物の検査
  (`tools/verify-ui-assets.mjs:387` 付近) と上流のテストも合わせて直す。ProcessContext と共有メモリには手を入れない。
- 曲の位置は §9 の段階で、歌詞の試作で作った「再生時計の位置を通知で送る仕組み」(方式 A) を入れ、**Counter と歌詞の両方に使う**。
  Counter が示すのは「聞こえている曲の位置」(再生表示の位置) とする。方式 A は一時停止・シーク・速度・曲の切り替えを時計の状態として
  1 か所で拾うので、ProcessContext に位置を入れる案より扱う組み合わせが少ない (CLAUDE.md「設計の簡素化」)。
- そのとき派生版の `build-ui-assets.mjs` の `counters()` の差し替え (上流 `:374-389`) を、ホストの位置を読む形に変える。

## 9. 位置と歌詞の受け取り (後の段階)

- 派生版の最初のリリースの後に行う。派生版なので、作者に頼まずに mIV が入れられる。
- 試作 (`G:\home\mimageviewer-lyrics` ブランチ `effetune-lyrics-trial`、2026-10-24 に master v4.8.0 を merge 済み。
  `G:\home\effetune-mixwright-trial` ブランチ `miv-lyrics-trial`) の方式 A を派生版のリポジトリへ移し、Counter (§8) と歌詞の両方に使う。
- 試作の測定 (`docs/effetune-lyrics-trial-plan.md` §13: オフセット 0 で歌詞が音より約 58 ms 早い、原因は cpal より後の出力側) から、
  出力側の遅れを位置に含める補正が論点になる。
- 歌詞を入れたら、上流の Lyrics 項目と Lyric Stage を派生版で表示する。

## 10. マニュアル・製品ページ・公開文書

### 10.1 プラグインのマニュアル (上流の日本語ドキュメントを元に生成)

- `scripts/gen-effetune-docs.py` を派生版向けに直す。
  - 表記: `chrome()` (`:550-595`) の「音響調整 (EffeTune)」「EffeTune 説明書」などを `<NAME>` / 「音響調整」に。本文の「EffeTune」は
    全体の置換の段 (`apply_overrides` の後) で `<NAME>` にする。上流へのクレジットの文 (冒頭と license ページ) は残す。
  - 外す章: mIV で使えない機能 (内蔵プレーヤー、音楽ライブラリ、LAN リモート操作、MCP、ブラウザ拡張、CUE、コントローラー割り当ての再生操作、
    デスクトップ版だけの設定)。ページ単位は `selection.json` の `include:false`、節単位は既存の `remove_section` の規則で書く。
    規則は一回だけ一致することを要求するので、上流が変わればビルドが止まって知らせる (今の仕組み)。
  - `SOURCE.md` の「submodule of EffeTune Mixwright <ver>」の照合 (`:608-611`) を派生版の版に合わせる。
- URL: 今の `manual/effetune/<ver>/` を `manual/sound/<ver>/` (名前に合わせる) へ移し、古い URL は `.htaccess` で 301 する
  (生成器の既存の redirect の仕組み `:716-753` を使う。134 ページが検索に載っているため)。
- テスト `scripts/test_gen_effetune_docs.py` を合わせて直す。

### 10.2 mIV のマニュアル・製品ページ・その他

- `manual/effetune.html` (音響調整のページ): 名前、保存先の節 (`:176-195` の APPDATA の記述)、ポータブル版の記述 (`:143`)、DAW との共有の変化、クレジット。
- 26 ページのサイドバーのリンク「音響調整 (EffeTune)」→「音響調整」。`index.html` (`:1200-1201`, `:1424-1431`)、`privacy.html`
  (「端末内に保存されるデータ」`:141-183`, `:325-364`。APPDATA に書かなくなる点、ポータブル版)、`manual/settings.html` (`:1160-1166`)、
  `manual/music.html`・`video.html`、`installer/readme.txt`。
- 更新履歴 (CHANGELOG・`manual/changelog.html`) は公開時に書く。過去の版の記述は書き換えない。
- 設計文書: `docs/effetune-integration-plan.md` (§0, §5.1, §7, §10, §10.1〜10.6, §17.4, §17.5)、`docs/effetune-now-playing-plan.md` §1、
  `docs/release-checklist.md`、`docs/release-operations.md`、`docs/native-dependencies.md`、`docs/portable-build-plan.md`、
  `docs/effetune-docs-overrides/README.md`。それぞれ本書を正本として参照させる。

## 11. 利用者に決めてもらうこと

決定済み (2026-10-24):

- 派生版にする。名称・クラス ID・保存先をすべて変え、保存先を mIV のデータフォルダへ移す。ポータブル版でも使えるようにする。
- **DAW・デスクトップ版とのプリセット共有がなくなること**: ClaudeCode が「プリセット共有はなくなる。リリース済みの挙動の変更なので、
  マニュアルと更新履歴に書く」と説明し、利用者は「その方針で進めたい」と回答した。行き来は書き出し・読み込みで行う (§5.6)。
- 利用者に見える EffeTune の語は消し、著作権表示とクレジットは残す。マニュアルを作り直す。作者へは一度だけ連絡する。

未決定 (設計レビューの後にまとめて聞く):

1. **製品名** (§2.1。推奨は mIV Sound)。
2. **派生版のソースを公開するか** (§7.1)。公開する場合の置き場 (GitHub の mIV のアカウントなど)。
3. **ポータブル版での有効化を同じリリースに入れるか** (§6)。入れない場合も保存先の切り替えは先に入れる。
4. プリセットのファイル拡張子を互換のまま残す方針でよいか (§2.2。推奨は残す)。
5. マニュアルの URL を名前に合わせて移すか (§10.1。推奨は移して 301)。

## 12. 段階

| 段階 | 内容 | 担当 |
| --- | --- | --- |
| F1 | 派生版リポジトリ (v0.15.0 元): 名前・クラス ID・3 種類の版・保存先の環境変数・WebView の user data folder・クレジットと通知・アイコン・Counter を外す (§8)、上流のテストの更新、出荷済みの状態ファイルを読むテスト、`build-miv.bat` | Codex (派生版リポジトリ) |
| F2 | mIV: ビルド・確認・承認の手順 (§7.3)、配布の検査の作り替えと固定値の一本化・`.gitattributes` (§7.4)、bridge の環境変数、移行 (§5、受け入れ条件 §5.5)、host の窓タイトル・FileDescription、About・設定画面の表記 | Codex (mIV の worktree) |
| F3 | マニュアルの生成器と公開文書 (§10) | Codex |
| F4 | ポータブル版での有効化 (§6) | Codex。§11 の未決定 3 による |
| F5 | 作者への連絡 (§13)。F1〜F3 が揃い、リリースの前 | 利用者 |
| 後 | 位置と歌詞の受け取り (§9)。Counter をそこで戻す | 別計画 |

F1〜F3 は同じリリースに入れる (名前が変わるのが一度で済むように)。各段階は独立レビューを通す。F1 と F2 は触るリポジトリが違うので並行できる。
F2 は F1 の候補の成果物 (§7.3 の 1) を確認用ビルドに置いてつなぐ。

## 13. 作者への連絡文の下書き (送るのは利用者。内容は事実の確認後に直す)

> Frieve 様
>
> mImageViewer の作者のみかげです。以前は EffeTune Mixwright の同梱についてご了承いただき、ありがとうございました。
>
> 今後、mImageViewer では Mixwright を元にした改変版を、別の名前 (`<NAME>`) で mImageViewer 側がビルドして同梱する形に変えます。
> 名前・識別子・データの保存先を分け、DAW やデスクトップ版の EffeTune とデータを共有しないようにします。
> 著作権表示と、EffeTune / EffeTune Mixwright を元にしていることのクレジットは、アプリ・マニュアル・同梱の通知に残します。
> 改変版についてのお問い合わせは mImageViewer 側で受け、Frieve 様にご負担をおかけしないようにします。
>
> 曲情報 (Now Playing) の受け渡しの仕組みは、引き続き使わせていただきます。今後ともよろしくお願いいたします。

## 14. 単純化の検討 (CLAUDE.md「設計の簡素化」)

1. **内部の名前を変えない**: 採用。通知 ID・キー・origin・ファイル拡張子・mIV の設定名・状態ファイル名をそのままにし、
   互換と上流への追従の手間を減らす (§2.2)。
2. **保存先は環境変数 1 つで渡す**: 採用。IPC の命令や設定ファイルの受け渡しを作らない (§4.2)。
3. **移行は印で判断し、終わるまで派生版を起動しない**: 採用 (第 1 回レビュー P1 を受けて改めた)。新旧のデータが混ざる組み合わせを作らない。
   失敗したらその起動では音響調整を使わず、次の起動でやり直す。DAW 側との同期や差分の取り込みは作らない (§5)。
4. **移行の対象は「音響調整を使った記録があるか」で決める**: 採用 (第 2 回レビュー P2 を受けて改めた)。新しい検証用のフォルダとポータブル版を外し、
   以前から別のデータフォルダを使っていた利用者は引き継ぐ (§5.1)。
5. **移行の間の排他は、書き込む側がもともと持っている境界を使う**: 採用 (第 2〜4 回レビュー P2)。JSON ファイルは旧版の一時ファイル置き換えで
   ファイル単位に整合するので、写した先の検査だけで足りる。WebView の保存領域はブラウザプロセスの lockfile を押さえる。mIV 独自のプロセス間の
   待ち合わせは作らない (旧版が従わない)。lockfile が使えることは F2 の最初の試作で確かめる (§5.2)。
6. **環境変数が無ければ失敗させる**: 採用。APPDATA への黙った戻り先を持たない (§4.2)。
7. **ビルド・確認・承認を分け、再現ビルドでの一致は求めない**: 採用。確認した成果物そのものを承認・保存する (§7.3)。
8. **Counter を最初のリリースでは外す**: 採用 (第 1 回レビュー P2-5〜7 を受けて改めた)。ProcessContext に位置を入れる案は、
   時間伸縮・分割・予熱・無音の押し出し・EOS・一時停止の先読みの組み合わせが多い。位置は後の段階で方式 A の 1 つの仕組みにまとめ、
   Counter と歌詞の両方に使う (§8, §9)。
9. **ポータブル版も平らな配置**: 採用。世代の公開をポータブル版に持ち込まない (§6)。

まれな失敗 (移行の書き込み失敗、承認物の破損など) は割り切り (ログ + 通知、移行は次回の起動で再試行)。

## 15. レビュー記録

### 15.1 第 1 回独立設計レビュー (2026-10-24、GPT-6.1 Sol / xhigh、read-only)

対象は初版 (a12b420c8)。Codex SID `01a16d0e-0324-71d3-95b8-eb48c03cab13`。判定 **REJECT**。保存先の分離と環境変数の採用は成立。
移行の条件と曲の位置の扱いに、通常の操作で破綻する経路があるという理由。ClaudeCode が根拠を読み、すべて妥当と判断して第 2 版に反映した。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P1 | 「新しい場所が空」の判断では既存の利用者が対象外になり、移行を先送りした後に新しい保存先が使われると移行できなくなる | §5.1 (印で判断、移行が終わるまで派生版を起動しない)、§14-3 |
| P2 | 移行をユーザー VST の起動より前に置く仕組みが無い。別の data-dir の mIV が旧 WebView 領域を使っている場合。隔離フォルダへの取り込み | §5.1 (通常のデータフォルダだけ)、§5.2 (両方の起動より前に 1 回、host のプロセスが動いていれば写さない) |
| P2 | プリセットの移行元の選び方が旧コード (フォルダがあればそちら) と違う | §5.3 |
| P2 | origin が同じというだけでは、IndexedDB・OPFS の移行が成り立つ証拠にならない | §5.5 (使い捨ての旧プロファイルでの受け入れ確認) |
| P2 | `raw.pts_secs` では音響調整の入力の位置にならない (時間伸縮・ユーザー VST・リミッター・分割) | §8 (Counter を外し、ProcessContext の案をやめる) |
| P2 | 予熱・リセット時の無音・EOS の tail も ProcessContext を作る | 同上 |
| P2 | 一時停止中も先読みで処理が進むので、最後の処理位置は聞こえる位置ではない | 同上、§8 (Counter は聞こえる位置。方式 A で後に入れる) |
| P2 | ビルドと承認の境界が曖昧。未コミットの差分や構成の記録 | §7.3 (ビルド・確認・承認を分ける、dirty なら止める、構成を記録) |
| P2 | 固定値の一覧の漏れと、`.gitattributes` の改行の保護 | §7.4 |
| P2 | 版を 1 種類にすると `project(VERSION)` と更新の検査が壊れる | §7.2 (3 種類に分ける) |
| P2 | DAW との共有がなくなることを、相談済みとして記録していない | §5.6、§11 (2026-10-24 の説明と回答を記録) |

ほかに確認された前提: 環境変数はコンストラクタと `initialize()` に間に合う (ただし `initialize(context)` には host の context が渡るので、
§4.2 の説明を補正した)。通常の同期転送は 1 組の値で対応できる。クラス ID を変えても状態の復元は妨げない。通常の永続の書き込みは 3 か所の
保存先の変更で捕まえられる (バックアップの一時退避は利用者が選んだ出力先の隣、automation trace は明示の環境変数の指定先)。MIT の通知は
著作権表示に加えて許諾文と免責文を残す必要がある。ポータブル版の平らな配置は既存の resolver と合う。ただし、利用者が入れた任意の
ユーザー VST が APPDATA を使わないとは言えない (§6 に追記)。

### 15.2 第 2 回独立設計レビュー (2026-10-24、同じ SID で継続)

対象は第 2 版 (0cf697485)。判定 **REJECT**。前回の空フォルダの判断・プリセットの移行元・固定値と改行の保護・共有がなくなる件の記録は解消。
ProcessContext の案を外したことで前回の P2-5〜7 は対象外になり、Counter を外す方法は上流の構造で成り立つと確認された。ビルド・確認・承認の分離と
版の 3 種類も適切とされた。ClaudeCode が根拠を読み、残りの 6 件をすべて妥当と判断して第 3 版に反映した。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P1 | 旧版のプラグインは `--data-dir` を指定しても APPDATA に書くので、§5.5 の旧プロファイルを隔離した mIV で作ることはできない | §5.5 (移行処理を引数で受ける関数にする、本物の旧データは Windows Sandbox で作る、検証用の入口を通常の入口と分ける) |
| P2 | 以前から別のデータフォルダを使っていた利用者を一律に対象外にすると、プリセットなどを失う | §5.1 (状態ファイルがあれば対象)、§14-4 |
| P2 | プロセスを確かめてから写し始めるまでの間に、別の mIV が host を起動できる。新しい host も同じ exe 名なので区別できない | §5.2 (写す前と後に確かめ、途中で起動したものがあれば捨てる。条件はほかの mIV をすべて終了すること)、§5.4 (通知の文)、§14-5 |
| P2 | メニューとプリセットから外すだけでは、保存済みのレイアウトの Counter が誤った位置を表示する | §8 (`counters()` を空にする) |
| P2 | 候補の置き場が固定パスなので、確認した候補と承認する候補が同じだと固定できない | §7.3 (候補 ID と候補ごとの置き場、承認時の照合) |
| P3 | 上流のタグの `v` の有無で、更新の検査が常に差分になる | §7.2 |

### 15.3 第 3 回独立設計レビュー (2026-10-24、同じ SID で継続)

対象は第 3 版 (8a0072878)。判定 **REJECT**。§5.1 の判断表 (状態ファイルによる判定は実装と合う)、Counter を空にすること、上流の版の比較、
候補 ID による同一性の考え方は解消と確認された。ClaudeCode が根拠を読み、残りの 4 件をすべて妥当と判断して第 4 版に反映した。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P2 | 写す前後 2 回のプロセスの確認では、写している間に起動して書き、終了したものを見逃す。ポータブル版の本体名 `mimageviewer.exe` が確認から漏れている | §5.2 (写す元の一覧・大きさ・更新時刻を前後で比べる。ポータブル版の本体も確認する)、§14-5 |
| P2 | 通知が「ほかの mIV」だけを挙げ、移行に失敗した mIV 自身やトレイ常駐を終了させないと、起動し直しても移行が始まらない | §5.4 (通知の文) |
| P2 | この作業環境の Windows Sandbox では WebView2 が動かない記録があり、旧データを作れない | §5.5 (素材は派生版を隔離して作り、旧版の配置に並べ替えて写す元にする) |
| P2 | 候補 ID の計算の対象と、管理ファイル・VERSION の境界が決まっていない | §7.3 (`artifact\` と `candidate.json` を分ける。vendor へは `artifact\` だけを写す) |

### 15.4 第 4 回独立設計レビュー (2026-10-24、同じ SID で継続)

対象は第 4 版 (cf8c021ba)。判定 **REJECT**。通知の文と候補の置き方は解消。§5.1 の判断表と Counter を外すことに新しい問題は無い。
ClaudeCode が根拠を読み、残りの 2 件を妥当と判断して第 5 版に反映した。同じ論点 (写す間の書き込み) が 3 回続いたので、
CLAUDE.md の「設計の簡素化」に従い、検出ではなく書き込む側の境界で止める形に改めた。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P2 | 一覧・大きさ・更新時刻の比較では、開いたまま書くファイルや同じ大きさの上書きを見逃す。host の終了後も WebView2 のブラウザプロセスが残って書くことがある | §5.2 (JSON は一時ファイル置き換えでファイル単位に整合する。WebView はブラウザプロセスの lockfile を押さえる。F2 の最初に試作で確かめる)、§14-5 |
| P2 | 派生版で作った素材では、旧版の保存データを読めるかは確かめられない。保存先の変更と v0.15 への更新を同時に出すので受け入れから外せない | §5.5 (フォルダを写す確認と旧データを読む確認を分ける。v0.14 のバンドルを名前を変えた host で動かして WebView の素材を作る。JSON は各版の形の fixture。v0.12・v0.13 は保存のコードの比較) |

### 15.5 第 5 回独立設計レビュー (2026-10-24、同じ SID で継続)

対象は第 5 版 (cb98efde9)。判定 **REJECT**。書き込む側の境界で止める方針、JSON をファイル単位で扱うこと、試作が成り立たなければ止めて相談すること、
フォルダを写す確認と旧データを読む確認を分けることは妥当と確認された。lockfile の振る舞いはレビューでは実測していない (F2 の試作で確かめる)。
ClaudeCode が根拠を読み、4 件を妥当と判断して第 6 版に反映した。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P1 | 旧版は設定画面を操作しなくても初回の案内で `config.json` を保存するので、host の名前を変えるだけでは利用者の APPDATA に書く | §5.5 (v0.14.0 のソースに保存先の差し替えだけを入れた検証用のビルドで素材を作る) |
| P2 | 共有なしで開いた lockfile を、フォルダを写すときに自分で開けない | §5.2 (lockfile は写さない。写した先の起動まで試作で確かめる) |
| P2 | 旧 WebView 領域が無い利用者は lockfile を作れず、毎回失敗する | §5.2 (WebView の部分を飛ばして JSON だけで完了)、§5.5 (自動テスト) |
| P3 | §5.4 に、やめたプロセスの検出の失敗条件が残っている | §5.4 |

### 15.6 第 6 回独立設計レビュー (2026-10-24、同じ SID で継続)

対象は第 6 版。**新しい指摘なし、判定 ACCEPT**。前回の 4 件は解消。v0.14 でも保存先は ConfigStore と PresetStore が決め、SFZ の参照は ConfigStore の
親フォルダに従うので、保存先だけを差し替えて旧版のシリアライズを保つ素材作りは成り立つ (v0.14 のタグでも照合)。lockfile の保持と起動の抑止は
静的レビューでは実証していないが、F2 の最初の試作で写した先の起動まで確かめ、成り立たなければ止めて相談する条件が明確なので、未解消の指摘ではなく
実装時に確かめる前提として扱える、との判断。F1 と「F2 の最初の lockfile の試作」から実装に進めてよい。移行の本実装は試作が受け入れ条件を満たしてから。
