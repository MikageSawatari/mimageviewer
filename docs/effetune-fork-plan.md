# 音響調整の派生版 (Mixwright の mIV 用別名ビルド) 計画

状態: **設計初版。独立設計レビュー前、実装未着手**。2026-10-24。
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
| 版 | `src/plugin/version.h`、`CMakeLists.txt:3`、`src/engine/pipeline_model.h:103` (状態に書く appVersion) | 上流の版 + mIV の連番 (例 `0.15.0-miv.1`)。状態の appVersion は上流の版のままでよいか §4 で確かめる |
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

- 同梱の通知 (`THIRD-PARTY-NOTICES.txt` など) に、上流の二つの著作権表示をそのまま残す:
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
  migration-v1.done             (§5 の移行が成功した印)
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
  これらはプラグインのコンストラクタ (`plugin_processor.cpp:353-354`) と `initialize()` (`:398-401`) で使われ、IHostApplication より先なので、
  環境変数が一番単純で確実 (`IHostApplication` 経由は間に合わない)。
- **環境変数が無いときは起動に失敗させる** (エラーとして mIV に返す)。APPDATA へ黙って戻ると §1 の問題が戻る。
  派生版は DAW で使うことを想定しない。
- `WEBVIEW2_USER_DATA_FOLDER` (プロセス全体の環境変数) は使わない。同じ host の中の他の WebView にも効いてしまう。

### 4.3 検討して採らなかった案

- **プラグインの隣にある設定ファイルで場所を伝える**: 配布物の中身が利用者ごとに変わり、hash 検査と合わない。
- **IPC の新しい命令で場所を伝える**: 上の通りコンストラクタの時点で必要なので間に合わない。

## 5. 出荷済みのデータの移行

v4.3.0〜v4.8.0 の利用者が音響調整で作ったデータは、保存先を変えると派生版から見えなくなる。CLAUDE.md の
「利用者が作ったデータが失われる」場合に当たるので、**最初に一度だけ写す**。

- **何を**: (a) `%APPDATA%\Frieve\EffeTunePlugin` の 4 ファイル (と、デスクトップ版のフォルダ `%APPDATA%\effetune` にプリセットがあればそちらの 2 ファイル)、
  (b) `%APPDATA%\mimageviewer-vst3-host.exe` (WebView2 の user data folder) の中身全体。
- **いつ・誰が**: mIV の core が、音響調整の bridge を初めて起動する前に、worker で写す (UI スレッドでは行わない)。
  ユーザー VST の bridge より前に行う (WebView のフォルダを使っている最中に写さない)。起動中の bridge があれば移行は次回に回す。
- **どう**: 新しい場所が空のときだけ写す。原本は消さない・変えない。成功したら `migration-v1.done` を置く。
  失敗したら印を置かず、ログと通知を出して空のまま起動する。次回の起動で再び試す (まれな失敗は割り切り。原本が残るので失われない)。
- **ポータブル版**: 移行しない (これまで使えなかったので移すデータが無い)。
- **DAW・デスクトップ版との共有はなくなる**。移行の後に DAW 側で作ったプリセットは mIV に出てこない。これはリリース済みの挙動の変更なので、
  更新履歴・マニュアル・privacy.html に書く。必要なら、プリセットの書き出し・読み込み (拡張子は互換のまま) で行き来できると案内する。
- WebView のフォルダには、ユーザー VST の他のプラグインのデータも入っている。まとめて写しても害はない (使わないものが残るだけ)。

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
- privacy.html・製品ページ・マニュアルの「ポータブル版では使えない」を直す (`docs/portable-build-plan.md` も)。

## 7. 派生版のソースとビルド

### 7.1 ソースの置き場

- 上流の `effetune-mixwright` を元にした git リポジトリを `G:\home\miv-sound` (名前は製品名に合わせる) に作る。
  `upstream` に作者のリポジトリ、作業ブランチ `miv` に mIV の変更を積む。上流の新しいタグを取り込むときは merge する。
- 変更は小さく保つ。名前・クラス ID・保存先・(後の段階で) 歌詞と位置の受け取り。上流の流儀
  (`tools/build-ui-assets.mjs` の置換と、外れたらビルドを止める検査) に合わせる。
- ソースを公開するか (例: GitHub に置く) は利用者が決める (§11)。MIT は公開を求めないが、改変版を配るなら公開しておくと
  問い合わせに答えやすい。

### 7.2 ビルド

- `G:\home\effetune-mixwright-trial` の試作で、VS 2026 BuildTools + 同梱 Ninja + CMake 4.3 + Node 22 でビルドと ctest が通ることを確かめ済み
  (`docs/effetune-lyrics-trial-plan.md` §0.1)。MSVC 14.50 の C4458 は configure の `/wd4458` で避ける。v0.15.0 でビルド要件は変わっていない。
- 派生版のリポジトリに ASCII の `build-miv.bat` (試作の `build-trial.bat` を元に) を置き、configure → build → ctest まで行う。
- mIV 側に `scripts/build-miv-sound.ps1` (ASCII) を足し、派生版のビルドを呼んで、成果物を `vendor/miv-sound/` へ置き、
  承認用の manifest と出どころの記録を作る (§7.3)。

### 7.3 配布の検査の作り替え

今は「承認した公式 ZIP の各ファイルの hash」と照合している (`scripts/effetune-distribution.ps1`、
`third_party/effetune-mixwright/v0.14.0/manifest.sha256`、`crates/launcher/build_effetune*.rs`)。これを次に変える。

- `third_party/miv-sound/<ver>/`: `VERSION` (`0.15.0-miv.1` など)、`manifest.sha256` (承認した**派生版のビルド成果物**の全ファイル)、
  通知 3 本、`PROVENANCE.md` (派生版リポジトリの commit、上流のタグと commit、submodule の commit、ビルドに使った道具の版)。
- 承認するのは「ある 1 回のビルド成果物」。ビルドの再現でバイトまで一致することは求めない (DLL に時刻が入り得る)。
  承認した成果物そのものを `C:\home\mimageviewer_vendor_backup\miv-sound-<ver>\` に保存する。
- 検査の仕組み (ファイル集合・hash・署名だけの変化・stage・世代の公開・パス長) はそのまま使い、固定値 (バンドル名・版・manifest の場所) を
  一か所の定義から読むようにする。調査で見つかった固定値の場所: `effetune-distribution.ps1:37`, `:41`, `:52`, `:94-95`, `:123`, `:129-135`, `:187`、
  `build_effetune.rs:12-17`, `:23`, `:37`, `:43`, `:61`、`build_effetune_source.rs:11`, `:75`, `:78`, `:173`、`src/effetune/mod.rs:23`、
  `src/runtime_cleanup.rs:166`、`about.rs:103-126`, `:599-606`、`test-effetune-distribution.ps1`、`check-dependency-updates.ps1:268-273`、
  `gen-effetune-docs.py:29`, `:608-611`, `:762`、`installer/readme.txt`。
- 世代の名前は manifest から決まる (`effetune_bundle.rs:198-209`) ので、名前が変われば新しい世代になる。古い世代の掃除は
  `runtime_cleanup.rs:166` がバンドル名を固定で見ているので、新旧どちらの名前も掃除できるように直す。

### 7.4 上流への追従

- 方針は「**常に派生版**」(利用者決定)。上流の新しい版は、価値がある変更のときに取り込む。mIV のリリース前に
  `scripts/check-dependency-updates.ps1` で上流の版を確かめる (今もある仕組み)。
- 取り込みの手順: 上流のタグを merge → 置換の検査が外れていないか確かめる → ビルドと ctest → 独立レビュー → `build-miv-sound.ps1` で
  承認 → マニュアルを生成し直す。

## 8. ProcessContext に曲の位置を入れる

v0.15.0 の Counter (§1-4) のために必要。派生版の初版を v0.15.0 元にするので、同じリリースに入れる。

- host が音響調整の bridge に渡す ProcessContext を、**そのブロックの先頭の曲の位置** (host のサンプルレートのサンプル数) と、
  **再生中かどうか**にする。`sampleRate` は今も設定している。
- 位置は Rust 側がブロックごとに知っている (`src/video/audio.rs` の `raw.pts_secs` と `seek_serial`、EffeTune 段 `:2686-2760`)。
  音声の受け渡しは 480 フレームずつの同期往復 (`src/video/dsp/bridge.rs:1501`, `:1554`) なので、**送る前に共有メモリの header へ
  そのブロックの位置と再生状態を書き、C++ がその往復の処理で ProcessContext に入れる**。リングの形式はそのまま、header の欄を足すだけ
  (protocol の版を上げる)。
- 一時停止中はブロックが届かない (`audio.rs:3838-3850`) ので、Counter は最後の位置で止まる (表示として正しい)。kPlaying を落とす機会は
  無いが、Counter は再生状態をアニメーションの判断にしか使わない (上流 `visualizer-counter.js:54-68`)。
- 再生速度を変えているときは、プラグインに届く音は出力の時間で進むが、位置は曲の位置を入れる。Mixwright はこの不連続を見て自動化の時刻を
  合わせ直す (`automationBlockStart`) が、mIV は自動化を使わないので影響しない見込み (実装時に確かめる)。
- ユーザー VST の bridge は今のまま (この段階では対象外。利用者の VST が位置を使う例が出たら別に検討する)。
- Remote 配信は端末より先を処理するので、位置は端末の再生位置にならない。Remote 中は音響調整の窓が開けない
  (`src/effetune/mod.rs:914`) ので、表示上の問題は出ない。
- 歌詞の試作で比べた「方式 B」(`docs/effetune-lyrics-trial-plan.md` §3) と同じ経路になる。歌詞の同期の精度は §9 で別に扱う。

## 9. 歌詞連携の位置付け

- 派生版の後の段階にする。派生版なので、作者に頼まずに mIV が歌詞と位置の受け取りを入れられる。
- 試作 (`G:\home\mimageviewer-lyrics` ブランチ `effetune-lyrics-trial`、`G:\home\effetune-mixwright-trial` ブランチ `miv-lyrics-trial`) の
  方式 A (再生時計の位置を通知で送る) をそのまま派生版へ移すか、§8 の ProcessContext の位置 (方式 B) を使うかは、その段階で決める。
  試作の測定 (§13: オフセット 0 で歌詞が音より約 58 ms 早い、原因は出力側) から、どちらでも出力側の遅れの補正が論点になる。
- 歌詞を入れたら、作者の Lyrics 項目と Lyric Stage を派生版で表示する (上流は「ホストに歌詞が無い」ため外している)。

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

1. **製品名** (§2.1。推奨は mIV Sound)。
2. **派生版のソースを公開するか** (§7.1)。公開する場合の置き場 (GitHub の mIV のアカウントなど)。
3. **ポータブル版での有効化を同じリリースに入れるか** (§6)。入れない場合も保存先の切り替えは先に入れる。
4. プリセットのファイル拡張子を互換のまま残す方針でよいか (§2.2。推奨は残す)。
5. マニュアルの URL を名前に合わせて移すか (§10.1。推奨は移して 301)。

## 12. 段階

| 段階 | 内容 | 担当 |
| --- | --- | --- |
| F1 | 派生版リポジトリ (v0.15.0 元): 名前・クラス ID・保存先の環境変数・WebView の user data folder・クレジット・アイコン、上流のテストの更新、出荷済みの状態ファイルを読むテスト、`build-miv.bat` | Codex (派生版リポジトリ) |
| F2 | mIV: 配布の検査の作り替え (§7.3)、`build-miv-sound.ps1`、bridge の環境変数、§5 の移行、名前の固定値、host の窓タイトル・FileDescription、About・設定画面の表記 | Codex (mIV の worktree) |
| F3 | ProcessContext に曲の位置 (§8)。host の C++ と Rust、protocol の版 | Codex (mIV、F2 の後) |
| F4 | マニュアルの生成器と公開文書 (§10) | Codex |
| F5 | ポータブル版での有効化 (§6) | Codex。§11-3 の決定による |
| F6 | 作者への連絡 (§13)。F1〜F4 が揃い、リリースの前 | 利用者 |
| 後 | 歌詞連携 (§9) | 別計画 |

F1〜F4 は同じリリースに入れる (名前が変わるのが一度で済むように)。各段階は独立レビューを通す。F1 と F2 は触るリポジトリが違うので並行できる。
F2 の前に、F1 で作った試作の成果物で mIV 側をつなぐ (`build-dev.ps1` に試作のバンドルを置く仕組みは歌詞の試作で作った
`-EffetuneBundle` の考えを使う)。

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
3. **移行は一度だけ写す**: 採用。DAW 側との同期や差分の取り込みは作らない。原本を残すので失敗しても失われない (§5)。
4. **環境変数が無ければ失敗させる**: 採用。APPDATA への黙った戻り先を持たない (§4.2)。
5. **再現ビルドでの一致を求めない**: 採用。承認した成果物そのものを保存し、その hash で検査する (§7.3)。
6. **ポータブル版も平らな配置**: 採用。世代の公開をポータブル版に持ち込まない (§6)。

まれな失敗 (移行の書き込み失敗、承認物の破損など) は割り切り (ログ + 通知、移行は次回の起動で再試行)。
