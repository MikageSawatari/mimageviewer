# 音響調整の歌詞連携 試作計画 (EffeTune Mixwright Lyrics)

状態: **設計初版。独立設計レビュー前、実装未着手**。2026-10-17。
目的は製品化ではなく、**作者 (Frieve-A) へ相談する前に、ホストから歌詞と再生位置を渡す方式が
実際に噛み合うかを、動く試作で確かめること**。試作は配布しない。
Coordinator は ClaudeCode。実装は Codex (mIV 側・Mixwright 側とも)。

## 0. 背景と前提

- EffeTune 2.14.0 (2026-10-16) で Visualizer に Lyrics 項目と Lyric Stage などのプリセットが入った。
  デスクトップ版は内蔵プレーヤーの曲から歌詞を読む。
- 同日公開の **Mixwright v0.14.0 (プラグイン版) は Lyrics を意図的に外している**。
  上流 README:56-58 は、追加メニューから Lyrics を、内蔵プリセットから Lyric Stage を除き、
  「音声は DAW から来るので再生中の歌詞は使えない」とする。テストは
  `tests/esm/vst-upstream-ui.test.mjs:96`, `:126`。除外の実体は `tools/build-ui-assets.mjs:82`, `:389`, `:404`。
- ホスト契約 `docs/host-now-playing.md` (v0.13.0〜) は title / album / artist / artwork だけを受ける。
  **契約外の属性は無視される** (`同:36`) ので、属性を足しても旧版プラグインは壊れない。
- mIV に同梱しているのは v0.13.0。試作は v0.14.0 を元にする。
- 本書の上流ファイルの参照は、次の 2 つの取得物の行番号:
  `G:\home\effetune-mixwright-trial` (Mixwright v0.14.0、commit e4147644305f、submodule
  `external/effetune` = 01c96f00f8e9 / dsp-v0.14.0)、
  およびそこに含まれる EffeTune 本体のソース。

### 0.1 ビルド環境 (2026-10-17 に ClaudeCode が実行して確認)

- VS 2026 BuildTools (MSVC 14.50) + 同梱 Ninja + CMake 4.3.1 + Node 22.16 で、
  `cmake --preset windows-release` → `cmake --build` → `ctest` が通った (108/108 passed)。
  vst3sdk は CMake の FetchContent が取得する。
- 上流は VS 2022 前提。MSVC 14.50 では `plugin_processor.h:112` などで **C4458 が `/WX` によりエラー**になる。
  試作では configure 時に `-DCMAKE_CXX_FLAGS="/DWIN32 /D_WINDOWS /EHsc /wd4458"` を渡して回避した。
  上流のソースは変更していない。作者への相談時に報告候補として添える。
- 生成されたバンドルのファイル数は 462 で、公式 v0.14.0 の ZIP と同じ。
- 使ったビルドスクリプトは ClaudeCode の scratchpad にあり、実装時に
  `G:\home\effetune-mixwright-trial\build-trial.bat` (試作ブランチ側) へ置き直す。

## 1. EffeTune の歌詞の仕組み (上流ソースの要約)

| 項目 | 内容 | 参照 |
| --- | --- | --- |
| 取得元 | 曲ファイル内のタグ (ID3 USLT / SYLT、Vorbis `LYRICS` / `UNSYNCEDLYRICS`、MP4 `©lyr`、WMA `WM/Lyrics`) と、同じフォルダの同名 `.lrc` (大文字小文字を区別しない、1 MiB まで) | `js/library/metadata/lyrics.js:4`, `text-encoding.js:793` |
| 優先順位 | 時刻付き `.lrc` → 時刻付きタグ → 時刻なし `.lrc` → 時刻なしタグ | `lyrics.js` `chooseLyrics` |
| LRC 解析 | `[mm:ss.xx]` の行時刻、`<mm:ss.xx>` の単語時刻 (Enhanced LRC)、`[offset:]`。上限は 2048 行・32768 文字・単語 2048 | `lyrics.js` `parseLrc` / `boundLyrics` |
| 文字コード | UTF-8 以外に Shift_JIS などを自動判定 | `text-encoding.js` |
| 描画への入力 | `{ data, time, duration, isStopped }`。`time` は出力遅延を引いた**聞こえている位置** | `js/visualizer/visualizer-view.js:343-352` |
| 描画 | Offset (−10〜+10 秒)、曲頭のフェードイン / 曲末のフェードアウト、時刻なし歌詞は曲の進み具合でスクロール | `js/visualizer/visualizer-lyrics.js` |

プラグイン版では `lyrics()` が内蔵プレーヤーを見るため常に `null` になる。
`lyrics.js` と `text-encoding.js` は、保存済みレイアウトを描くためにプラグイン版にも残っている
(`tools/build-ui-assets.mjs:929`, `tools/verify-ui-assets.mjs:831`)。
したがって、**解析と描画は上流のコードをそのまま使え、足りないのは「歌詞の本文」「再生位置」「曲の長さ」を外から入れる口だけ**。

## 2. mIV の音声経路で分かっている時刻 (2026-10-17 のコード読み)

- デコードした各ブロックはメディアの PTS を持つ (`src/video/decoder.rs:1592` `AudioFrame.pts_secs`)。
- audio-pump は 1 ブロックずつ、時間伸縮 → 正規化 → ユーザー VST → 音響調整 (EffeTune) →
  リミッターの順に処理する (`src/video/audio.rs:2497-3092`)。時間伸縮は**音響調整より前**に入る (`:2566`)。
- 処理後のチャンクは、プラグインなどの合計遅延を引いた聞こえる位置
  `audible_pts_secs` を持つ (`audio.rs:1586` `audible_pts_after_latency`, `:764` `ProcessedChunk`)。
- 出力コールバック `fill_output` がチャンクを取り出すたびに、`AvClock::set_audio_pts` で再生時計を合わせる
  (`audio.rs:3980`, `src/video/clock.rs:441`)。`AvClock::now_secs()` は atomics だけで読め、
  一時停止中は止まり、シーク中は移動先を返す (`clock.rs:611-630`)。`Arc<AvClock>` は複数のスレッドで共有される。
  **これは映像と音声を合わせるのに使っている時計そのもの**。ただし WASAPI のバッファと
  デバイスの遅延は引いていない (cpal の `OutputCallbackInfo` は使っていない、`audio.rs:1768`)。
- 処理はこの時計より先に進む。処理済みキューの目標は 0.10 秒で、1 チャンク分を足して最大 0.15 秒ほど
  (`audio.rs:1995`, `:3048`)。これにデバイス側のバッファが加わる。
- **メディアの位置はブリッジに渡っていない**。共有メモリのリングは PCM だけを運ぶ (`src/video/dsp/bridge.rs:58-78`)。
  プラグインへの ProcessContext は、リセットからの処理フレーム数を時刻として入れ、常に kPlaying を立てる
  (`crates/vst3-host/src/plugin_loader.cpp:1289-1303`)。シーク時は 0 に戻る (`:2409`)。
- 一時停止中は処理済みキューが満杯のまま止まるので、**プラグインへはブロックが 1 つも届かない** (`audio.rs:3838-3850`)。
- Remote 配信は同じ音響調整を共有するが、処理は端末の再生位置より 60 秒ほど先を進む
  (2 秒セグメント × 既定 30 本、`src/video/clockless_transcode.rs:1012-1062`)。
  PC 側は端末の位置を最大 5 秒おきの再開用の進捗でしか知らない (`crates/remote-ipc/src/lib.rs:653`)。
- 再生速度は 0.25〜4.0 倍 (`clock.rs:52-63`)。Remote は 1.0 倍に固定される (`src/remote_ipc/ui.rs:1345`)。
- 曲情報の送信は Now Playing の所有者 1 つが行う (`src/effetune/now_playing.rs:31`)。
  `Cmd::SetNowPlaying` (`bridge.rs:91`) → C++ が GUI スレッドで `EffeTune.NowPlaying` を notify
  (`crates/vst3-host/src/now_playing_notify.cpp:28-61`)。
- 歌詞のタグはどこでも読んでいない。`read_metadata_value` (`decoder.rs:8739`) は指定したキーだけを探す。

## 3. 同期方式の比較

歌詞の本文と曲の長さは、どの方式でも Now Playing の通知に属性を足せば済む (§4.1)。
方式の違いは**再生位置をどう渡すか**だけ。

### 方式 A: 再生時計の位置を通知で送る (推奨)

mIV が `AvClock` の位置・再生中かどうか・速度に、送った時点の時刻を添えて通知し、
プラグイン側がそこから外挿する。

- 送るもの: `positionSeconds` (メディア時刻の秒)、`playing`、`rate` (再生速度)、
  `hostTimeNanoseconds` (位置を読んだ瞬間の単調時計)。
- 時計の共通化: 試作は Windows 専用とし、`std::chrono::steady_clock` (MSVC では QueryPerformanceCounter) を
  両プロセスで使う。mIV の Rust 側は `QueryPerformanceCounter` を直接読む。
  macOS を含む契約の書き方は作者と相談する点として残す (§8)。
- 外挿: プラグインの native 側が telemetry 応答の時点で
  `position + (now − hostTime) × rate` を計算して返す。telemetry は 60 Hz で読まれる
  (`ui-shim/vst-audio-manager.js:981`)。JS は受け取った時点からの `performance.now()` の差で
  フレームごとに補間する。プロセス間の時計の対応を JS に持ち込まない。
- 合うもの: 映像と同じ時計なので、**動画では映像と歌詞が同じずれ方になる**。プラグインの遅延は
  `audible_pts_secs` で引き済みなので、プラグイン側で自分の遅延を**引き直してはいけない**。
- ずれるもの: デバイス側のバッファ分 (共有モードで通常は数十 ms、Bluetooth では百数十 ms 以上と言われる。
  mIV では未計測) だけ歌詞が早く出る。映像も同じだけ早い。Visualizer の Offset で手動補正できる。
- Visualizer のグラフ (スペクトルやメーター) は処理した時点の音で描かれる見込み
  (プラグインの処理中に解析し telemetry で渡す作り)。その場合、**歌詞はグラフより 0.1〜0.15 秒 + デバイス分だけ遅れて見える**。音に対しては歌詞のほうが正しい。
- 一時停止・シーク・速度変更・曲の切り替えは、すべて時計の状態の変化として 1 か所で拾える (§4.3)。
- Remote は「位置なし」を送るだけで済む (§4.4)。
- 費用: 音声経路と IPC のリングに手を入れない。通知を 1 種類と、時計を読む小さな仕事を足すだけ。

### 方式 B: ブロックごとにメディア位置を ProcessContext で渡す

ブリッジへ渡すブロックに PTS を付け、C++ が `projectTimeSamples` に入れる。
一時停止中は kPlaying を落とす。VST3 の本来の時刻の渡し方。

- 良い点: 音そのものと同じ流れで位置が届く。DAW でも同じ受け取り方になる。
- 問題点:
  1. 共有メモリのリングは PCM だけを運び、C++ は届いた分を最大 480 フレームずつ読む
     (`crates/vst3-host/src/main.cpp:1807`)。サンプルと PTS の対応を運ぶには、リングの形式変更
     (protocol v8) と、ブロック分割をまたぐ対応表が要る。
  2. 処理した時点の位置なので、プラグイン側で「処理から聞こえるまで」の 0.1〜0.15 秒 + デバイス分を
     知る手段が無い。結局、聞こえる位置を知るには mIV から遅れの値を別に渡す必要がある。
  3. 時間伸縮が前段にあるので、プラグインに届く音は出力時間で進む。`projectTimeSamples` を
     メディア時刻で進めると 1 ブロックあたり `numSamples` 以外の量で進み、Mixwright は毎ブロック
     自動化の時刻を合わせ直す (`src/plugin/plugin_processor.cpp:2456-2475` `automationBlockStart`)。
  4. 一時停止中はブロック自体が届かないので、kPlaying を落とす機会が無い。「一定時間届かなければ停止」と
     推定する仕組みが要る。
  5. Remote は処理が 60 秒先なので、位置として使えない。
- 判断: 試作では採らない。方式 A で不足が分かった場合に限り再検討する。

### 方式 C: プラグインが処理した量から自分で数える

Now Playing の受信からの処理量で位置を推定する。シーク・一時停止・Remote で破綻するので**検討から外す**。

### 結論

**試作は方式 A で作る**。音声経路を触らず、映像と同じ時計に揃えられる。
状態の組み合わせ (一時停止・シーク・速度・曲の切り替え・Remote) を「時計を読む 1 か所」に集められる
(CLAUDE.md「設計の簡素化」)。方式 B と比べて作者側の変更も小さい。
確認では、ずれの実測 (§6.3 の Offset 合わせ) で、方式 A のずれが許容範囲かを見る。

## 4. 試作の設計

### 4.1 歌詞の本文と曲の長さ (Now Playing の拡張)

`EffeTune.NowPlaying` に次の属性を足す。契約 v1 は未知の属性を無視するので、ID は変えない。

| 属性 | VST3 型 | 内容 |
| --- | --- | --- |
| `lyrics` | `setString` | LRC 形式または時刻なしの歌詞本文。試作の上限は 262,144 UTF-16 単位 (終端を除く) |
| `durationSeconds` | `setFloat` | 曲の長さ。不明なら属性を付けない |

- 本文は mIV が 1 つだけ選んで送る。選び方は上流の `chooseLyrics` と同じにする。
  時刻付きの判定は「行頭に `[mm:ss` 形式の時刻があるか」だけを見る。
  解析はプラグイン側の上流 `parseLrc` に任せ、mIV は LRC を解析しない。
- 空の `lyrics` は「歌詞なし」。snapshot の差し替えという既存の意味をそのまま守る。
- mIV の取得元 (試作の範囲):
  - 同名 `.lrc`: 同じフォルダ・同じ stem・大文字小文字を区別しない。1 MiB まで。
    文字コードは BOM (UTF-8 / UTF-16) → 正しい UTF-8 → Shift_JIS の順に判定する (`encoding_rs` は依存済み)。
    読み込みは Now Playing の画像用 worker の同じ仕事の中で行い、UI スレッドで `read_dir` も `stat` もしない。
  - 埋め込み: 音声のデコード worker が `VideoInfo` を作るときに、フォーマットのメタデータから
    `lyrics` で始まるキー (ID3 USLT は `lyrics-eng` のような名前になる見込み) と
    `UNSYNCEDLYRICS` / `LYRICS` を読む。実際のキー名は試作の最初に FFmpeg で確かめる (§6.1)。
  - SYLT (ID3 の時刻付き歌詞) は FFmpeg のメタデータに出ない見込みなので、試作では扱わない。
  - CUE シートの区間は扱わない。
- 曲の長さは、既にある再生情報の duration を使う。

### 4.2 再生位置 (新しい通知)

新しい通知 ID **`EffeTune.PlaybackPosition`** を使う (試作上の名前)。

| 属性 | VST3 型 | 内容 |
| --- | --- | --- |
| `available` | `setInt` | 1 = 位置あり、0 = 位置なし (Remote・停止中・曲なし)。0 のとき他の属性は無視 |
| `playing` | `setInt` | 1 = 進行中、0 = 一時停止 |
| `positionSeconds` | `setFloat` | メディア時刻 (秒)。プラグインの遅延は引き済み |
| `rate` | `setFloat` | 再生速度 (一時停止中も現在の値) |
| `hostTimeNanoseconds` | `setInt` | 位置を読んだ瞬間の `steady_clock` (Windows では QPC 由来の ns) |

- 旧版のプラグインは未知の ID を SDK の通常の扱いにするだけなので、mIV は結果を気にしない (ログだけ残す)。
- プラグインは最新の 1 件だけを持つ。保存しない。プラグインの再初期化のたびに mIV が再送する
  (Now Playing と同じ規則)。

### 4.3 mIV 側: 位置を送る仕組み

```text
Now Playing の所有者 (Local のメディア 1 つ)
  └ 所有者に対応する player の Arc<AvClock> と識別子を、位置の送り手に登録
位置の送り手 (effetune-host-control worker 内の小さな状態)
  ├ 200 ms ごとに clock.now_secs() / is_playing() / playback_speed() と QPC を読む
  ├ 予測 (前回の送信からの外挿) との差が 20 ms を超えた、再生状態か速度が変わった、
  │   または前回の送信から 1 秒たったときだけ送る
  └ 所有者が変わった・消えた → すぐに available=0 を送る
```

- **時計を読む場所を 1 つにする**: 一時停止・シーク・速度変更・ループ・曲の切り替えを、
  それぞれの入口に呼び出しを足して拾うことはしない。時計の状態の変化として送り手が拾う。
  入口を列挙して漏れを塞ぐ必要が無くなる (CLAUDE.md「設計の簡素化」)。
- 200 ms / 20 ms / 1 秒は試作の初期値で、§6.3 の結果を見て調整する。シークの反映は最大 200 ms 遅れる。
- 所有者と時計の対応は Now Playing の所有者の識別子をそのまま使う。DSP の所有トークンを代わりにしない
  (now-playing 計画 §3.1 と同じ)。所有者が切り替わるときは、旧 player の時計で新しい曲の位置を送らないよう、
  識別子が一致するときだけ送る。
- 送り手は worker 上で動き、UI スレッドは識別子と `Arc<AvClock>` を渡すだけ。毎フレームの処理や再描画の要求は足さない
  (アイドル時の健全性を保つ)。所有者がいないときは 200 ms の起床もしない (channel の `recv` で待つ)。
- RT スレッド (`fill_output`) には何も足さない。
- 単一 player が鳴る利用者前提 (now-playing 計画 §0) を引き継ぐ。
- detached viewer の述語や viewport の経路には触れない。所有者の登録は Now Playing と同じ境界で行う。

### 4.4 Remote・ポータブル版・歌詞のない曲

- Remote 配信の所有者のときは、歌詞を空にして `available=0` を送る。端末の位置が分からないため、
  試作では表示しない。将来の案は §8。
- ポータブル版は音響調整が無いので何もしない (既存どおり)。
- 歌詞のない曲は `lyrics` を空にする。位置は送ってよい (Lyrics 以外の項目は位置を使わない)。

### 4.5 IPC と C++ ブリッジ

- `Cmd::SetNowPlaying` に `lyrics` と `duration_seconds` (wire 名は `durationSeconds`) を足す。
- `Cmd::SetPlaybackPosition { slot_id, available, playing, position_seconds, rate, host_time_ns }` を足す。
  応答は返さない (SetNowPlaying と同じ)。
- `PROTOCOL_VERSION` を 7 → 8 に上げる (`crates/vst3-host/include/protocol.h:102`, `src/video/dsp/bridge.rs:42`)。
  host の exe は CMake で作り直す。
- C++ は Now Playing と同じ mailbox と GUI スレッドへの post で notify する。位置の通知は最新値で上書きし、
  途中の値は捨ててよい。音声スレッドでは notify しない。
- 歌詞の UTF-8 → UTF-16 変換は既存の strict 変換を使い、上限でスカラー値の途中を切らない。

### 4.6 Mixwright 側 (試作ブランチ)

`G:\home\effetune-mixwright-trial` に試作ブランチ `miv-lyrics-trial` を作って変更する。
上流の EffeTune 本体 (submodule) には手を入れず、既存の流儀どおり `tools/build-ui-assets.mjs` の差し替えで済ませる。

1. `src/plugin/plugin_processor.cpp` の `notify` (`:476` 付近):
   - NowPlaying で `lyrics` と `durationSeconds` を任意属性として読む。上限超過・不正な UTF-16 は
     既存の項目と同じく `kInvalidArgument` とし、前の snapshot を保つ。
   - `EffeTune.PlaybackPosition` を受け、最新値を 1 件持つ。
2. `appendNowPlayingSnapshot` (`:531`) で `lyrics` と `durationSeconds` を JSON に含める。
   revision の扱いは今のまま (歌詞の本文は revision が変わったときだけ運ぶ)。
3. telemetry 応答 (`:4300` 付近) に `playback: { available, playing, positionSeconds, rate }` を足す。
   `positionSeconds` は応答を作る時点で外挿した値にする。
4. `ui-shim/vst-audio-manager.js`: 位置を受け取った時点の `performance.now()` と一緒に持ち、
   `getHostLyrics()` が `{ data, time, duration, isStopped }` を返す。`data` は Now Playing の revision ごとに
   `parseLrc` → `boundLyrics` を 1 回だけ行って保持する。
5. `tools/build-ui-assets.mjs`: `visualizer-view.js` の `lyrics()` を `getHostLyrics()` に差し替える
   (`metadata()` の差し替え `:360-371` と同じ方式)。Lyrics の追加メニューと Lyric Stage の除外
   (`:82`, `:404`) は試作では外す。製品でどうするか (ホストが歌詞を送るときだけ出す等) は作者の判断に委ねる。
6. `tools/verify-ui-assets.mjs` と `tests/esm/vst-upstream-ui.test.mjs` の除外前提を試作に合わせて直す。
7. `docs/host-now-playing.md` に試作の契約 (§4.1, §4.2) を追記し、相談用の下書きにする。

## 5. 作業の分け方とブランチ

| 区切り | 場所 | 内容 |
| --- | --- | --- |
| T1 | `G:\home\effetune-mixwright-trial` (`miv-lyrics-trial`) | §4.6 の変更、上流テストの更新と追加、ビルドと ctest |
| T2 | 新しい worktree `G:\home\mimageviewer-lyrics` (`effetune-lyrics-trial`、master から) | §4.1〜4.5 の mIV 側。T1 と同時に進めてよい (触るファイルが重ならない) |
| T3 | T2 の worktree | 試作バンドルを差し込んだ確認用ビルド、試験素材、利用者の実機確認 (§6) |
| T4 | ClaudeCode | 結果をまとめ、作者への相談文と変更の要点 (patch) を用意する。送るのは利用者 |

- Codex を同じ作業ツリーで 2 本同時に動かさない。T1 と T2 は別ツリーなので並行できる。
- 試作バンドルは `vendor/effetune-mixwright` (公式 v0.13.0) を上書きしない。
  `scripts/build-dev.ps1` は `vendor\effetune-mixwright\EffeTune Mixwright.vst3` を
  `target\dev-runtime\effetune\` へコピーする (`build-dev.ps1:136-184`)。試作では `-EffetuneBundle <path>` のような
  ASCII の引数を足し、dev-runtime にだけ試作バンドルを置く。開発版の core がバンドルの hash を検査するかは
  T2 の最初にコードで確かめる。
- 配布用のビルドは、承認済みの公式バンドルの hash を検査する (`scripts/effetune-distribution.ps1`) ので、
  試作バンドルが配布物に混ざることは無い。この検査は緩めない。
- 永続データは増やさない (歌詞・位置は保存しない)。未出荷の試作なので移行は不要。

## 6. 確認の手順

エージェントは製品バイナリを起動しない (CLAUDE.md)。表示とずれの確認は利用者の実機で行い、
エージェントは自動テストと素材の準備を担う。

### 6.1 自動テスト (Codex / ClaudeCode)

- Mixwright: `ctest --preset windows-release` 全件。追加するテスト:
  - native: NowPlaying の `lyrics` / `durationSeconds` の受理・上限・不正値で前の snapshot が残ること、
    PlaybackPosition の受理と外挿 (時刻を差し替えられる形で)、`available=0` で位置が消えること。
  - esm: `getHostLyrics()` が revision ごとに 1 回だけ解析すること、外挿と一時停止、Offset とフェードが上流と同じ入力で動くこと。
- mIV: `.\scripts\test-focused.ps1` に加えて、次を対象を絞って実行する。
  - `Cmd` の直列化 (キーの順、`cmd` が先頭、利用者の文字列で誤認しないこと)。
  - `.lrc` の選び方 (大文字小文字、時刻付きの優先、1 MiB 超の拒否、UTF-8 / UTF-16 / Shift_JIS)。
  - 埋め込みのキー: FFmpeg で USLT 付き MP3・LYRICS 付き FLAC・`©lyr` 付き M4A を作り、実際のキー名を確かめるテスト。
  - 位置の送り手: 偽の時計で、一時停止・シーク・速度変更・所有者の交代・所有者の消滅で何が送られるか、
    同じ状態が続くときは 1 秒おきの再送だけになること、所有者がいないときは起床しないこと。
  - protocol v8 の handshake と、v7 host を拒否すること。
- `cargo check -p mimageviewer --bin mimageviewer-core`、C++ host の CMake ビルド、`.\scripts\build-dev.ps1`。

### 6.2 試験素材 (エージェントが作る、リポジトリ外の `target\lyrics-trial\`)

- `beep-60s.flac`: 2 秒ごとに 50 ms の短いビープ (0 秒から 58 秒まで 30 回)。
  `beep-60s.lrc`: 各ビープと同じ時刻に行 `01`〜`30`。1 行は Enhanced LRC で 0.5 秒ごとの単語時刻を付ける。
- `beep-60s-sjis.lrc` (Shift_JIS で同じ内容)、`BEEP-60S.LRC` (大文字の名前)。
- 埋め込みの確認用: USLT 付き MP3、LYRICS 付き FLAC、`©lyr` 付き M4A (時刻付きと時刻なしの両方)。
- 歌詞なしの曲、時刻なしの `.lrc` だけの曲、ビープ入りの短い動画 (映像にも秒数を焼き込む)。
- 素材は生成スクリプトで作り、市販曲の歌詞は使わない。

### 6.3 利用者の実機確認 (試作の確認用ビルド)

準備: 起動中の mIV を終了 → T3 の確認用ビルドを起動 → 音響調整の窓を開いて Visualizer にし、
Lyric Stage (16:9) を選ぶ。隔離した data-dir で起動する手順を併記する。

| # | 操作 | 見るところ |
| --- | --- | --- |
| 1 | `beep-60s.flac` を再生 | ビープと行の切り替わりが合うか。早い / 遅いと感じるか |
| 2 | Lyrics 項目の Offset を ±0.05 秒ずつ動かし、いちばん合う値を探す | **合った Offset の値** (方式 A のずれの実測値)。スピーカーとヘッドホン (使っていれば Bluetooth も) で |
| 3 | 一時停止して 3 秒待ち、再開 | 停止中に行が進まないか、再開後すぐ合うか |
| 4 | シークバーで前後に飛ぶ、矢印キーで飛ぶ | 飛んだ先の行に切り替わるまでの遅れ (最大 0.2 秒の想定) |
| 5 | 速度 0.5 倍 / 2 倍 | 行の切り替わりがビープに付いてくるか |
| 6 | ループ再生で曲末から曲頭へ | 曲頭の行に戻るか、フェードイン / フェードアウト |
| 7 | 別の曲 → 元の曲 | 前の曲の歌詞が残らないか |
| 8 | 再生中に音響調整の窓を閉じて開き直す | 開き直した直後から正しい行か |
| 9 | ビープ入りの動画 | 映像の秒数・ビープ・歌詞の行がそろうか |
| 10 | Shift_JIS の `.lrc`、大文字の名前、埋め込みの各形式、歌詞なし | 表示されるか、文字化けしないか、歌詞なしで前の歌詞が残らないか |
| 11 | Remote で再生 | 歌詞が出ないこと (試作の仕様) |
| 12 | 手元の実際の曲と LRC | 実用上ずれが気になるか (主観) |

結果は「# ごとに OK / 気になる点」と、#2 の Offset の値を返してもらう。
#2 の値が数十 ms 以内なら方式 A のまま相談する。大きい場合は、デバイスの遅延を時計に含める
(cpal のタイムスタンプを使う) 案を mIV 側の課題として検討する。方式 B へは、それでも不足するときだけ戻る。

## 7. 単純化の検討 (CLAUDE.md「設計の簡素化」)

1. **位置の変化を 1 か所で拾う**: 採用。入口ごとの通知は足さない (§4.3)。
2. **歌詞の選択と解析をどちらか一方に寄せる**: 解析はプラグイン (上流のコード)、選択は mIV。
   2 本の候補を送ってプラグインに選ばせる案は、契約が複雑になるので採らない。
3. **Remote を試作から外す**: 採用。位置の出どころが無いので「表示しない」に固定する。
4. **配送の保証・再試行**: 作らない。最新値の再送 (1 秒おきと再初期化時) だけ。
   まれな失敗は割り切り (ログのみ)。
5. **モーダル化**: 該当しない (長い処理が無い)。

## 8. 作者への相談で決めること (試作の結果と一緒に出す)

- 歌詞と位置の契約を `EffeTune.NowPlaying` の拡張 + 新 ID にするか、別の形にするか。ID の名前。
- 時計の共通化を契約にどう書くか (Windows の QPC、macOS の `mach_absolute_time`、
  あるいはプラグインが受信時刻で記録する簡易版)。
- 歌詞の上限 (試作は 262,144 UTF-16 単位)。
- ホストが歌詞を送るときだけ Lyrics 項目と Lyric Stage を出すか、常に出すか。
- 解析をプラグインで行う前提でよいか (SYLT のような形式をホストが LRC に直して送る扱い)。
- Remote のように「位置が分からない」ホスト状態の扱い。
- 試作で見つかった上流側の小さな問題 (MSVC 14.50 の C4458 など)。

## 9. 範囲外

- 製品版への組み込み (作者の対応後に、Now Playing と同じ手順で別の計画を立てる)。
- mIV 自身による歌詞表示。
- SYLT・CUE・ストリームの歌詞、Remote 端末での歌詞表示。
- 試作バンドルの配布、公開文書・マニュアルの更新 (試作なので行わない)。
