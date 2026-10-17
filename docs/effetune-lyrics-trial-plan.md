# 音響調整の歌詞連携 試作計画 (EffeTune Mixwright Lyrics)

状態: **第 4 版。第 1〜3 回独立設計レビュー (いずれも REJECT、§10) の指摘を反映、再レビュー待ち。実装未着手**。2026-10-17。
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
  (`audio.rs:3980`, `src/video/clock.rs:441`)。`AvClock::now_secs()` は一時停止中は止まり、
  シーク中は移動先を返す (`clock.rs:611-630`)。内部の MasterClock は短い Mutex を取る
  (`src/video/engine/clock.rs:185`)。`Arc<AvClock>` は複数のスレッドで共有される。
  **これは映像を合わせるための表示用の時計**で、聞こえているサンプルの位置そのものではない。
  シーク先の override、外挿、wall-rate cap、monotonic guard を含む。例えば速度変更の直後は、
  処理済みのチャンクが旧速度のまま残る一方で cap は新速度を使うので、可聴 PTS と差が残る可能性が
  コードから読み取れる (実機では未確認)。可聴 PTS と時計の差は既存の診断値
  `audio_audible_pts_bits` / `audio_lead_ms_bits` に出る (`audio.rs:3999-4011`)。
  また WASAPI のバッファとデバイスの遅延は引いていない (cpal の `OutputCallbackInfo` は使っていない、`audio.rs:1768`)。
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
- **「映像の時計に追従すること」と「実際の音に一致すること」は別の性質**として扱う。
  - 映像の時計への追従: 映像と同じ時計なので、動画では映像と歌詞が同じずれ方になる。
    プラグインの遅延は `audible_pts_secs` で引き済み (速度込み) なので、プラグイン側で自分の遅延を**引き直してはいけない**。
  - 実際の音とのずれは、少なくとも次の 2 つの和になる。
    (1) 時計と可聴 PTS の差 (速度変更・遅延の変化・シーク・DSP の引き継ぎ・EOF やループの直後に出得る。§2)。
    (2) デバイス側のバッファ (共有モードで通常は数十 ms、Bluetooth では百数十 ms 以上と言われる。mIV では未計測)。
    (2) は映像も同じだけ早い。どちらも試作で測って切り分ける (§6.3)。Visualizer の Offset で手動補正できる。
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
確認では、記録による時計の誤差の測定 (§6.1 の D 記録) と、利用者の総合的な見え方 (§6.3) を分けて、
方式 A のずれが許容範囲かを見る。

## 4. 試作の設計

### 4.0 曲の世代で、歌詞と位置を同じ曲に結び付ける

曲情報と位置は別の通知で届くので、受け取る側で組み合わせを確かめないと、
**前の曲の歌詞を新しい曲の位置で描く**ことが起こり得る。既存の C++ の mailbox は、notify の実行中に届いた
最新版を保留し、実行が終わってから配送する (`crates/vst3-host/include/now_playing.h:187-216`)。
曲情報と位置を別々の mailbox に入れると、曲 A の曲情報を notify している間に届いた曲 B の位置が、
曲 B の曲情報より先にプラグインへ届く。送る前に識別子を照合するだけでは防げない。

- **曲の世代 `mediaGeneration` (u64)** を Now Playing の所有者が持つ。メディアの採用 (Local / Remote)・
  clear のたびに 1 つ進める。A→B→A も毎回別の世代になる。
- 既存の snapshot の `revision` (画像の完成でも進む) とは別物。歌詞や画像が後から届いても世代は変わらない。
- 曲情報と位置の**両方の通知に世代を載せる**。プラグインは、最新の曲情報と最新の位置の世代が一致し、
  かつ位置が `available=1` のときだけ歌詞を描く。一致しない間は歌詞を描かない (Lyrics 項目は空)。
- これにより C++ の mailbox は今の 1 件保留の形のまま 2 つ並べてよく、配送の順序を揃える仕組みは足さない。

### 4.1 歌詞の本文と曲の長さ (Now Playing の拡張)

`EffeTune.NowPlaying` に次の属性を足す。契約 v1 は未知の属性を無視するので、ID は変えない。

| 属性 | VST3 型 | 内容 |
| --- | --- | --- |
| `mediaGeneration` | `setInt` | §4.0 の曲の世代 |
| `lyricsSidecar` | `setString` | 同名 `.lrc` の本文 (文字コードを直したもの)。無ければ付けない。上限 262,144 UTF-16 単位 |
| `lyricsEmbedded` | `setString` | 曲ファイルのタグにある歌詞の本文。無ければ付けない。上限は同じ |
| `durationSeconds` | `setFloat` | 曲の長さ。不明なら付けない |

- **どちらの歌詞を使うかはプラグインが上流のコードで決める**。プラグインはそれぞれを上流の `parseLrc` で解析し、
  `selectLyrics({ sidecar, embedded })` に渡す (`js/library/metadata/lyrics.js`)。
  これで「時刻付きの判定」「メタデータ行だけの `.lrc` は歌詞なし扱いでタグへ戻る」「`[ar:A][00:01.00]本文` のような行」
  「`[00:99]` は無効な時刻」などが上流と同じになる。mIV は LRC を解析も分類もしない
  (同じ規則を 2 か所に書かない)。
- 上流との既知の違い: 上流は曲ファイルの複数の歌詞タグを順に見て最初の時刻付きを選ぶ (`lyricsFromMetadata`)。
  試作の mIV はタグを 1 つしか送らない (FFmpeg のメタデータで最初に見つかったもの)。
  複数の歌詞タグを持つファイルでは選ばれるものが違い得る。SYLT と CUE は扱わない (§9)。
- 取得元 (試作の範囲):
  - 同名 `.lrc`: 同じフォルダ・同じ stem・大文字小文字を区別しない。1 MiB まで。
    文字コードは BOM (UTF-8 / UTF-16) → 正しい UTF-8 → Shift_JIS の順に判定する (`encoding_rs` は依存済み)。
    上流は Shift_JIS 以外も自動判定するので、その差は試作の既知の違いとする。
  - 埋め込み: 音声のデコード worker が `VideoInfo` を作るときに、フォーマットのメタデータから
    `lyrics` で始まるキー (ID3 USLT は `lyrics-eng` のような名前になる見込み) と
    `UNSYNCEDLYRICS` / `LYRICS` を読む。実際のキー名は試作の最初に FFmpeg で確かめる (§6.1)。
- **画像用 worker の結果を「曲情報の補完結果」に広げる**。今の完了処理は画像が取れたときだけ snapshot を更新し、
  画像が無いと `Absent` にするだけ (`src/effetune/now_playing.rs:304-315`、`now_playing/artwork.rs:201`)。
  代表画像の無い FLAC + `.lrc` は主な試験素材なので、このままでは歌詞が公開されない。
  - worker の 1 件の仕事で、画像と `.lrc` を**独立に**取りに行く。片方の失敗や欠落はもう片方に影響しない。
  - 完了時は、画像 (Ready / Absent) と sidecar 歌詞 (Ready / Absent) を合わせて、送る内容が変わるときだけ
    新しい revision の snapshot を公開する。画像なし・歌詞ありでも公開する。
  - 既存の取消・revision の照合・最新 1 件の保留はそのまま使う。
  - 埋め込みの歌詞は `VideoInfo` と一緒に届くので、title などと同じく最初の text snapshot から含める。
- UI スレッドでは `read_dir` も `stat` もファイル読み込みもしない (既存の Now Playing と同じ)。
- 曲の長さは、既にある再生情報の duration を使う。

### 4.2 再生位置 (新しい通知)

新しい通知 ID **`EffeTune.PlaybackPosition`** を使う (試作上の名前)。

| 属性 | VST3 型 | 内容 |
| --- | --- | --- |
| `mediaGeneration` | `setInt` | §4.0 の曲の世代 |
| `available` | `setInt` | 1 = 位置あり、0 = 位置なし (Remote・停止中・曲なし)。0 のとき他の属性は無視 |
| `playing` | `setInt` | 1 = 進行中、0 = 一時停止 |
| `positionSeconds` | `setFloat` | メディア時刻 (秒)。プラグインの遅延は引き済み |
| `rate` | `setFloat` | 再生速度 (一時停止中も現在の値) |
| `hostTimeNanoseconds` | `setInt` | 位置を読んだ瞬間の単調時計 (Windows では QPC を ns に換算した値) |

- 時計: MSVC の `std::chrono::steady_clock` は QPC を ns に換算した値で、プロセス固有の起点を引かない
  (第 1 回レビューで確認)。mIV の Rust 側も QPC を同じ換算で読めば共通の時計になる。
- 旧版のプラグインは未知の ID を SDK の通常の扱いにするだけなので、mIV は結果を気にしない (ログだけ残す)。
- プラグインは最新の 1 件だけを持つ。保存しない。プラグインの再初期化のたびに mIV が再送する
  (Now Playing と同じ規則)。

### 4.3 mIV 側: 位置を送る仕組み

```text
Now Playing の所有者 (唯一の正本)
  Empty
  Media { identity, generation, revision, text, artwork, lyrics,
          position_source: Local { clock: Arc<AvClock>, diagnostics: Arc<AudioDiagnostics> }
                         | Unavailable (Remote) }
位置の送り手 (effetune-host-control worker 内の小さな状態)
  ├ 所有者の状態から (generation, position_source) を写し取る
  ├ Local のとき 200 ms ごとに clock.now_secs() / is_playing() / playback_speed() と QPC を読む
  ├ 予測 (前回の送信からの外挿) との差が 20 ms を超えた、再生状態か速度が変わった、
  │   または前回の送信から 1 秒たったときだけ送る
  └ 世代が変わった・Unavailable・Empty → その世代で available=0 をすぐ送る
```

- **時計は所有者の状態に持たせる**。送り手に別の登録を作らない。Local の player を採用するときに
  その player の `Arc<AvClock>` と、同じ player の `Arc<AudioDiagnostics>` (`src/video/mod.rs:451`) を、
  Remote の採用では `Unavailable` を、同じ所有者の更新で入れる。
  旧 player の時計で新しい曲の位置を送ることは、世代と時計が同じ状態から一緒に読まれるので起こらない。
- **時計を読む場所を 1 つにする**: 一時停止・シーク・速度変更・ループ・曲の切り替えを、
  それぞれの入口に呼び出しを足して拾うことはしない。時計の状態の変化として送り手が拾う
  (CLAUDE.md「設計の簡素化」)。
- 200 ms / 20 ms / 1 秒は試作の初期値で、§6 の結果を見て調整する。200 ms は**読み取りの間隔**であって、
  表示に反映されるまでの上限ではない。GUI スレッドへの配送、実行中の telemetry 要求、描画の待ちが加わる。
- `now_secs()` は短い Mutex を取る (§2) が、5 Hz の読み取りなので試作では問題にしない。
- DSP の所有トークンを所有者の代わりにしない (now-playing 計画 §3.1 と同じ)。
- 送り手は worker 上で動き、UI スレッドは所有者の更新を今までどおり渡すだけ。毎フレームの処理や再描画の要求は足さない
  (アイドル時の健全性を保つ)。Local の位置が無いときは 200 ms の起床もしない (channel の `recv` で待つ)。
- RT スレッド (`fill_output`) には、下の D 記録用の atomics の書き込みだけを足す (待ち・ロック・ログ出力はしない)。
- 単一 player が鳴る利用者前提 (now-playing 計画 §0) を引き継ぐ。
- detached viewer の述語や viewport の経路には触れない。所有者の更新は Now Playing と同じ境界で行う。
- 確認用に、送るたびに perf ログへ 1 行残す (§6.1 の D 記録)。試作だけの記録で、製品化の際に見直す。
  載せるのは、送った位置と QPC、再生状態と速度、送った時点の時計の seek serial、
  そして **lead の採取値** (下記) とその採取の連番。
  送った位置と `audio_audible_pts_bits` の単純な差は、コールバックから読み取りまでの経過時間が
  混ざるので指標にしない (第 2 回レビュー P2-1)。
- **lead の採取値を 1 組として公開する** (試作だけの診断): lead (`audio_lead_ms_bits`) は、出力コールバックが
  実際にサンプルを消費して時計を更新した回にだけ書かれる (`audio.rs:3996-4011`)。一方、既存の
  `callback_seek_serial` / `callback_engine_state` / `callback_latest_wall_ns` はコールバックの入口で毎回書かれ
  (`audio.rs:1774`)、無音で早く戻る回 (`audio.rs:3838-3850`) でも新しくなる。`audio_audible_pts_valid` も
  「今回更新した」という印ではない。既存の値を組み合わせると、古い lead を新しい場面の値として数え得る
  (第 3 回レビュー P2-1)。そこで lead を書くのと同じ場所で、
  `{ lead, そのときの時計の seek serial, そのときの時刻 }` を 1 組として連番付きで書く
  (連番を奇数にしてから 3 値を書き、偶数に戻す。読む側は前後の連番が同じ偶数のときだけ採用する)。
  RT 側は atomics の書き込みだけで、待たない。

### 4.4 Remote・ポータブル版・歌詞のない曲

- Remote 配信の所有者のときは、歌詞の属性を付けず、`available=0` を送る。端末の位置が分からないため、
  試作では表示しない。Remote 中は音響調整の窓自体が開けない (`src/effetune/mod.rs:914`、
  `crates/vst3-host/include/gui_visibility.h:5`) ので、この契約は自動テストで確かめる (§6.1)。将来の案は §8。
- ポータブル版は音響調整が無いので何もしない (既存どおり)。
- 歌詞のない曲は歌詞の属性を付けない。位置は送ってよい (Lyrics 以外の項目は位置を使わない)。

### 4.5 IPC と C++ ブリッジ

- `Cmd::SetNowPlaying` に `media_generation`、`lyrics_sidecar`、`lyrics_embedded`、`duration_seconds` を足す
  (wire 名は §4.1 の属性名)。
- `Cmd::SetPlaybackPosition { slot_id, media_generation, available, playing, position_seconds, rate, host_time_ns }`
  を足す。応答は返さない (SetNowPlaying と同じ)。
- `PROTOCOL_VERSION` を 7 → 8 に上げる (`crates/vst3-host/include/protocol.h:102`, `src/video/dsp/bridge.rs:42`)。
  host の exe は CMake で作り直す。
- C++ の Now Playing の解析は、決まったキーの集合を前提にしている (`now_playing.h:181` の `seen != 255`)。
  新しいキーを加えた形に直し、キーが欠けたときの扱い (任意属性) を決める。
  最大の画像と最大の歌詞 2 本を同時に載せた frame が 32 MiB の上限に収まることを境界テストで確かめる
  (第 1 回レビューで計算上は収まると確認済み)。
- 位置の通知は、Now Playing と同じ形の 1 件保留の mailbox をもう 1 つ持ち、GUI スレッドへの post で notify する。
  最新値で上書きし、途中の値は捨ててよい。順序の保証は §4.0 の世代で受け取る側が確かめる。音声スレッドでは notify しない。
- 歌詞の UTF-8 → UTF-16 変換は既存の strict 変換を使い、上限でスカラー値の途中を切らない。

### 4.6 Mixwright 側 (試作ブランチ)

`G:\home\effetune-mixwright-trial` に試作ブランチ `miv-lyrics-trial` を作って変更する。
上流の EffeTune 本体 (submodule) には手を入れず、既存の流儀どおり `tools/build-ui-assets.mjs` の差し替えで済ませる。

1. `src/plugin/plugin_processor.cpp` の `notify` (`:476` 付近):
   - NowPlaying で `mediaGeneration`、`lyricsSidecar`、`lyricsEmbedded`、`durationSeconds` を任意属性として読む。
     上限超過・不正な UTF-16 は既存の項目と同じく `kInvalidArgument` とし、前の snapshot を保つ。
   - `EffeTune.PlaybackPosition` を受け、最新値を 1 件持つ。
2. `appendNowPlayingSnapshot` (`:531`) で上の 4 つを JSON に含める。
   revision の扱いは今のまま (歌詞の本文は revision が変わったときだけ運ぶ)。
3. **`telemetry/read` と `host/getInfo` の両方の応答**に、同じ位置の snapshot
   `playback: { mediaGeneration, available, playing, positionSeconds, rate }` を足す
   (`:4144` と `:4300` 付近)。`positionSeconds` は応答を作る時点で外挿した値にする。
   非表示中は `host/getInfo` しか読まれず (`ui-shim/vst-audio-manager.js:1022-1038`)、起動時も `host/getInfo` から始まる。
4. `ui-shim/vst-audio-manager.js`:
   - 位置は、非表示時の早期 return より**前**で、受け取った時点の `performance.now()` と一緒に採用する。
   - 非表示から表示に戻った最初の回は `telemetry/discard` だけで終わり、位置を取りに行かない
     (`ui-shim/vst-audio-manager.js:1025-1031`)。最後の非表示の読み取りの後に一時停止・シークが起きると、
     採用済みの位置は古い。そこで **表示の世代** (表示に戻るたびに 1 進む整数) を持つ。
     - 表示に戻ったことを `visibilitychange` で受けた時点、つまり最初の描画より前に、表示の世代を進めて位置を「古い」とする。
     - 位置を取りに行く要求ごとに、発行した時点の表示の世代を覚える。
     - 「古い」を解除できるのは、**今の表示の世代で発行した要求の応答**だけ。非表示中に出した要求の応答が
       表示に戻った後に届いても、その位置は採用しない (第 3 回レビュー P3)。
     - 「古い」の間は `getHostLyrics()` が `null` を返す (次の読み取りは 1/60 秒後なので、空になるのは数フレーム)。
       discard の分岐自体は変えない。
   - `getHostLyrics()` が `{ data, time, duration, isStopped }` を返す。曲情報と位置の世代が一致し `available=1` のときだけ
     `data` を返し、それ以外は `null`。`data` は Now Playing の revision ごとに、§4.1 の `parseLrc` と
     `selectLyrics` を 1 回だけ行って保持する。
5. `tools/build-ui-assets.mjs`: `visualizer-view.js` の `lyrics()` を `getHostLyrics()` に差し替える
   (`metadata()` の差し替え `:360-371` と同じ方式)。Lyrics の追加メニューと Lyric Stage の除外
   (`:82`, `:404`) は試作では外す。製品でどうするか (ホストが歌詞を送るときだけ出す等) は作者の判断に委ねる。
   `selectLyrics` などの関数が配布物に残っているかを確かめ、足りなければ verify の許可リストを直す。
6. `tools/verify-ui-assets.mjs` と `tests/esm/vst-upstream-ui.test.mjs` の除外前提を試作に合わせて直す。
7. `docs/host-now-playing.md` に試作の契約 (§4.0〜4.2) を追記し、相談用の下書きにする。

## 5. 作業の分け方とブランチ

| 区切り | 場所 | 内容 |
| --- | --- | --- |
| T1 | `G:\home\effetune-mixwright-trial` (`miv-lyrics-trial`) | §4.6 の変更、上流テストの更新と追加、ビルドと ctest |
| T2 | 新しい worktree `G:\home\mimageviewer-lyrics` (`effetune-lyrics-trial`、master から) | §4.0〜4.5 の mIV 側。T1 と同時に進めてよい (触るファイルが重ならない) |
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
- **プラグインの保存領域を分ける**: Visualizer のレイアウトは自動で保存され
  (`external/effetune/js/visualizer/visualizer-view.js:210`)、WebView の保存先はホスト exe のファイル名から決まる
  (`choc_WebView.h:1853` `getUserDataFolder`。mIV では `%APPDATA%\mimageviewer-vst3-host.exe\`)。
  mIV の `--data-dir` ではこれを分けられない。試作ブランチでは host の exe 名を
  `mimageviewer-vst3-host-lyrics-trial.exe` に変え、実利用中の Visualizer のレイアウトを汚さない。
  実際に起動される名前は、埋め込んだ host を展開するときの名前で決まる (`src/video/dsp/extract.rs:115`)。
  CMake の出力名・埋め込み元・identity の検査は標準の名前のまま変えず、**試作ブランチで展開する basename だけを変える**。
- それでも分けられないもの: Mixwright の設定 `%APPDATA%\Frieve\EffeTunePlugin\config.json`
  (`src/bridge/config_store.cpp:26`) とプリセット (`%APPDATA%\effetune\` または同フォルダ) は DAW と共有される。
  確認中は Mixwright の設定画面を変えず、プリセットを保存しない (§6.3 の注意に書く)。
- 永続データは増やさない (歌詞・位置は保存しない)。未出荷の試作なので移行は不要。

## 6. 確認の手順

エージェントは製品バイナリを起動しない (CLAUDE.md)。表示とずれの確認は利用者の実機で行い、
エージェントは自動テスト・素材・記録の分析を担う。

### 6.1 自動テストと記録 (Codex / ClaudeCode)

- Mixwright: `ctest --preset windows-release` 全件。追加するテスト:
  - native: NowPlaying の新しい属性の受理・上限・不正値で前の snapshot が残ること、
    PlaybackPosition の受理と外挿 (時刻を差し替えられる形で)、`available=0` で位置が消えること、
    `telemetry/read` と `host/getInfo` の両方に同じ位置が載ること。
  - esm: `getHostLyrics()` が revision ごとに 1 回だけ解析すること、**世代が一致しない間は `null`**、
    外挿と一時停止、起動時 (`host/getInfo` から開始)、非表示中の一時停止・シーク → 再表示で古い位置を使わないこと、
    **非表示中の最後の読み取りの後に位置が変わり、再表示の最初の描画 (discard の回) で `null` になること**、
    **非表示中に出した要求が終わらないまま表示に戻り、その応答が後から届いても「古い」が解除されないこと**、
    E 記録の集計 (中央値・95 パーセンタイル) が 5 秒おきに native へ送られること、
    Offset とフェードが上流と同じ入力で動くこと。
  - 選択: `[ar:A][00:01.00]本文`、`[00:99]`、メタデータ行だけの `.lrc` (タグへ戻る)、時刻なし `.lrc` と時刻付きタグ。
- mIV: `.\scripts\test-focused.ps1` に加えて、次を対象を絞って実行する。
  - `Cmd` の直列化 (キーの順、`cmd` が先頭、利用者の文字列で誤認しないこと)。C++ の解析で新しいキーを読むこと、
    最大の画像と最大の歌詞 2 本を載せた frame の境界。
  - 所有者と世代: A→B→A、旧所有者の clear、Remote への切り替え、ブリッジの再登録での再送、
    **曲 A の notify 中に曲 B の曲情報と位置が届く順序** (位置が先に届いても世代が違うので描かれないこと)。
  - Remote: 歌詞の属性が付かず `available=0` になること、Remote 終了後に旧い歌詞・位置が戻らないこと。
  - `.lrc` の探し方 (大文字小文字、1 MiB 超の拒否、UTF-8 / UTF-16 / Shift_JIS)。
  - **画像なし + 歌詞あり**で歌詞が公開されること、画像の失敗が歌詞に影響しないこと、その逆。
  - 埋め込みのキー: FFmpeg で USLT 付き MP3・LYRICS 付き FLAC・`©lyr` 付き M4A を作り、実際のキー名を確かめるテスト。
  - 位置の送り手: 偽の時計で、一時停止・シーク・速度変更・所有者の交代・所有者の消滅で何が送られるか、
    同じ状態が続くときは 1 秒おきの再送だけになること、Local の位置が無いときは起床しないこと。
  - lead の採取値: 時計を更新しなかったコールバック (一時停止・停止中の早期 return) で連番が進まないこと、
    旧い seek serial の採取値を集計から外すこと、読み取りの途中で書き換わった組を捨てること。
  - protocol v8 の handshake と、v7 host を拒否すること。
- `cargo check -p mimageviewer --bin mimageviewer-core`、C++ host の CMake ビルド、`.\scripts\build-dev.ps1`。
- **D 記録 (時計の誤差の測定)**: §4.3 の perf ログの行から、コールバック内の lead を場面ごとに集計する
  スクリプトを用意する (Python、リポジトリ内 `scripts/`)。場面は、通常再生・速度変更の直後・遅延の変化
  (EffeTune の読み込み直後の 0→128 サンプル)・シーク・DSP の引き継ぎ (曲の切り替え)・EOF とループ。
  集計に入れるのは、(a) lead の採取値の連番が前の行から進んでいて (その間にコールバックが時計を更新した)、
  (b) 採取値の seek serial が送った時点の時計の seek serial と一致し、(c) 再生中の行だけ。
  連番が進んでいない行・旧い serial の行・読み取りの途中で書き換わった行は除き、除いた件数を理由別に出す。
  これで「時計と可聴 PTS の差」(§3 方式 A の (1)) を、利用者の見え方とは別に数字で確かめる。
  デバイス側の遅延 (2) と、プラグイン内の配送・描画の遅れ (下の E 記録) はこの記録には出ない。
- **E 記録 (プラグイン内の配送と描画の遅れ)**: native は応答を作る時点まで外挿するが、JS は受け取った時点を
  補間の起点にするので、**応答を作ってから JS が受け取るまでの時間は補われず、その分だけ歌詞が遅れる**
  (`ui-shim/vst-bootstrap.js:183-190` の往復)。描画の待ちも加わる。試作の JS は、telemetry の要求から応答までの時間と、
  Visualizer の描画間隔を集計し、5 秒おきに中央値と 95 パーセンタイルを native へ送る。native は 1 行を stderr に書き、
  mIV の既存の bridge stderr の受け口 (`src/video/dsp/bridge.rs:686-732`) でログに残る。残る場所は T2 の最初に確かめる。
  要求から応答までの時間は、補われない遅れの**上限**であって量ではない (待ちの大半が応答を作る前にあることもある)。
  描画間隔も、画面に実際に出るまでの遅れそのものではない。E は原因の候補を絞るための上限と傾向として使い、
  Offset の値から差し引く補正量には使わない (第 3 回レビュー P2-2)。

### 6.2 試験素材と測定用レイアウト (エージェントが作る、リポジトリ外の `target\lyrics-trial\`)

- `beep-60s.flac`: 2 秒ごとに 50 ms の短いビープ (0 秒から 58 秒まで 30 回)。代表画像なし。
  `beep-60s.lrc`: 各ビープと同じ時刻に行 `01`〜`30`。1 行は Enhanced LRC で 0.5 秒ごとの単語時刻を付ける。
- `beep-60s-sjis.lrc` (Shift_JIS で同じ内容)、`BEEP-60S.LRC` (大文字の名前)。
- 埋め込みの確認用: USLT 付き MP3、LYRICS 付き FLAC、`©lyr` 付き M4A (時刻付きと時刻なしの両方)。
- 歌詞なしの曲、時刻なしの `.lrc` だけの曲、ビープ入りの短い動画 (映像にも秒数を焼き込む)。
- 素材は生成スクリプトで作り、市販曲の歌詞は使わない。
- **測定用レイアウト**: Lyric Stage は行の移動に 0.4 秒の遷移を使う
  (`external/effetune/presets/visualizer/16x9.json:4595` `lyricsStepDuration`) ので、行の切り替わりのどこを
  ビープに合わせるかで演出が測定値に混ざる。Single line も固定の 0.18 秒で旧行から新行へ動き、
  Step transition time の項目自体が出ない (`visualizer-editor.js:745-752`、`visualizer-lyrics.js:7`, `:33`)。
  測定は次の設定の Lyrics 項目 1 つだけのレイアウトで行う。
  View = List、Scroll = Step (vertical)、Step transition time = 0、Current line position = 50%、
  Fade-in / Fade-out time = 0、Offset = 0、Word highlight = オフ。
  比べる瞬間は「**中央の現在行の数字が次の数字に変わった瞬間**」とビープの鳴り始め。
  手順書には各設定項目の画面上の名前を書く。

### 6.3 利用者の実機確認 (試作の確認用ビルド)

準備: 起動中の mIV を終了 → T3 の確認用ビルドを隔離した data-dir と **`--perf-log`** を付けて起動
(perf 記録は引数が無いと有効にならない、`src/lib.rs:1315`) → 隔離 data-dir の `logs` に `perf_*.jsonl` ができたことを確かめる
→ 音響調整の窓を開いて Visualizer にする。起動コマンドは手順書に PowerShell で書く。

隔離の範囲 (手順書の冒頭に書く):
- mIV の設定・キャッシュ・ログは `--data-dir` で分かれる。
- Visualizer のレイアウトは、試作の host 名を変えたことで実利用中のものと分かれる (§5)。
- Mixwright の設定とプリセットは DAW と共有なので、確認中は設定画面を変えず、プリセットを保存しない。

| # | 操作 | 見るところ |
| --- | --- | --- |
| 1 | 測定用レイアウト (§6.2) で `beep-60s.flac` を 1.0 倍で再生 | 数字の切り替わりとビープが合うか。早い / 遅いと感じるか |
| 2 | 同じ条件で Offset を ±0.05 秒ずつ動かし、いちばん合う値を探す | **1.0 倍での総合的な補正値** (主観)。スピーカーとヘッドホン (使っていれば Bluetooth も) で |
| 3 | 一時停止して 3 秒待ち、再開 | 停止中に数字が進まないか、再開後すぐ合うか |
| 4 | シークバーで前後に飛ぶ、矢印キーで飛ぶ | 飛んだ先の数字に切り替わるまでの遅れ |
| 5 | Offset を 0 に戻し、速度 0.5 倍 / 2 倍 | 数字の切り替わりがビープに付いてくるか (Offset は速度に比例しないので 0 で見る) |
| 6 | ループ再生で曲末から曲頭へ | 曲頭の数字に戻るか |
| 7 | 別の曲 → 元の曲 | 前の曲の歌詞が残らないか、切り替え直後に別の曲の歌詞が一瞬出ないか |
| 8 | 再生中に音響調整の窓を閉じて開き直す | 開き直した直後から正しい数字か |
| 9 | ビープ入りの動画 | 映像の秒数・ビープ・数字がそろうか |
| 10 | Shift_JIS の `.lrc`、大文字の名前、埋め込みの各形式、歌詞なし | 表示されるか、文字化けしないか、歌詞なしで前の歌詞が残らないか |
| 11 | Remote で再生してから終了し、ローカルで別の曲を再生 | 旧い歌詞・位置が戻らず、新しい曲の歌詞に切り替わるか |
| 12 | Lyric Stage (16:9) で手元の実際の曲と LRC | 実用上ずれが気になるか (主観。演出込みの見え方) |

結果は「# ごとに OK / 気になる点」と、#2 の Offset の値 (符号も) を返してもらう。確認後に、隔離 data-dir の `logs` にある
**`perf_*.jsonl` (D 記録) と `mimageviewer.log` (E 記録。bridge の stderr はここに入る、`src/video/dsp/mod.rs:1209`)** の
2 つを受け取り、§6.1 の D・E 記録と合わせて判断する。

判断:
- D 記録の差が小さく (数十 ms 以内)、#2 の値も小さい → 方式 A のまま相談する。
- D 記録の差が場面によって大きい → 時計と可聴 PTS の差が原因。位置の出どころを可聴 PTS 側に寄せる案を mIV 側で検討する。
- D 記録の差は小さいのに #2 の値が大きい → まず「配送・描画・デバイスのどれかで、未判定」とし、
  **Offset の符号**で候補を絞る。Offset は正の値で歌詞を遅らせる。
  - 合う Offset が正 (歌詞が音より早い) → デバイス側の遅延 (音が遅れて出る) と合う向き。
    スピーカーとヘッドホン・Bluetooth で値が変わるかを見る。変わるなら、cpal のタイムスタンプを使って
    デバイスの遅延を含める案を mIV 側で検討する。
  - 合う Offset が負 (歌詞が音より遅い) → プラグイン内の配送・描画の遅れと合う向き。E の上限がその大きさを
    超えていなければ候補から外す。超えていれば、JS の補間の起点を直す案 (往復の半分を足す等) を、
    往路と帰路が対称という**未確認の仮説**として Mixwright 側で試す (試して値が縮むかで確かめる)。
  - どちらの向きでも説明が付かない、または両方が打ち消し合っていそうなときは未判定のまま報告する。
- 方式 B へは、これらでも不足するときだけ戻る。

## 7. 単純化の検討 (CLAUDE.md「設計の簡素化」)

1. **位置の変化を 1 か所で拾う**: 採用。入口ごとの通知は足さない (§4.3)。
2. **歌詞の選択規則を 1 か所に置く**: 第 2 版で変更。初版は mIV が 1 本選んで送る案だったが、
   行頭の時刻だけを見る簡易判定では上流と同じ選択にならない (第 1 回レビュー P2-5)。同じ規則を Rust に写すと
   2 か所で保守することになるので、候補 2 本を送り、選択と解析は上流のコードに任せる。
3. **配送の順序を揃える仕組みを作らない**: 採用。曲の世代を両方の通知に載せ、受け取る側で一致を確かめるだけにする (§4.0)。
4. **Remote を試作から外す**: 採用。位置の出どころが無いので「表示しない」に固定する。
5. **配送の保証・再試行**: 作らない。最新値の再送 (1 秒おきと再初期化時) だけ。
   まれな失敗は割り切り (ログのみ)。
6. **モーダル化**: 該当しない (長い処理が無い)。

## 8. 作者への相談で決めること (試作の結果と一緒に出す)

- 歌詞と位置の契約を `EffeTune.NowPlaying` の拡張 + 新 ID にするか、別の形にするか。ID の名前。
- 曲の世代で歌詞と位置を結び付ける形でよいか。
- 時計の共通化を契約にどう書くか (Windows の QPC、macOS の `mach_absolute_time`、
  あるいはプラグインが受信時刻で記録する簡易版)。
- 歌詞の上限 (試作は 262,144 UTF-16 単位を 2 本)。候補 2 本を送ってプラグインが選ぶ形でよいか。
- ホストが歌詞を送るときだけ Lyrics 項目と Lyric Stage を出すか、常に出すか。
- SYLT のような形式をホストが LRC に直して送る扱い。
- Remote のように「位置が分からない」ホスト状態の扱い。
- 試作で見つかった上流側の小さな問題 (MSVC 14.50 の C4458 など)。

## 9. 範囲外

- 製品版への組み込み (作者の対応後に、Now Playing と同じ手順で別の計画を立てる)。
- mIV 自身による歌詞表示。
- SYLT・CUE・ストリームの歌詞、Remote 端末での歌詞表示。
- 試作バンドルの配布、公開文書・マニュアルの更新 (試作なので行わない)。

## 10. レビュー記録

### 10.1 第 1 回独立設計レビュー (2026-10-17、GPT-6.1 Sol / xhigh、read-only)

対象は初版 (6fe048b9c)。Codex SID `01a14811-72b4-7600-9500-ff93bb2cfe0b`。判定 **REJECT**。
方式 A と試作の範囲には賛成。曲情報と位置の整合と、表示の演出と時計の誤差の切り分けが未確定という理由。
ClaudeCode が各指摘の根拠のコードを読み、8 件すべてを正しいと判断して第 2 版に反映した。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P1 | 別々の mailbox では、曲 A の歌詞を曲 B の位置で描く組み合わせを防げない | §4.0 (曲の世代)、§4.3 (時計を所有者の状態に持たせる)、§4.5、§6.1 |
| P2 | `now_secs()` は表示用の時計で、ずれはデバイス分だけではない。atomics だけで読めるという記述も誤り (Mutex を取る) | §2、§3 方式 A、§6.1 の D 記録、§6.3 の判断 |
| P2 | Lyric Stage は 0.4 秒の遷移を使うので Offset 合わせに演出が混ざる。Offset は速度に比例しない | §6.2 の測定用レイアウト、§6.3 #2・#5 |
| P2 | 今の worker は画像が取れたときだけ公開するので、画像なし + 歌詞ありが公開されない | §4.1 (曲情報の補完結果)、§6.1 |
| P2 | 行頭の時刻だけの判定では上流と同じ選択にならない | §4.1 (候補 2 本 + 上流の `selectLyrics`)、§7.2 |
| P3 | 非表示時と起動時は `host/getInfo` を読むので、telemetry だけでは古い位置で描き得る | §4.6.3〜4、§6.1 |
| P3 | Remote 中は音響調整の窓が開けないので、「歌詞が出ない」を目視できない | §4.4、§6.1、§6.3 #11 |
| P3 | mIV の隔離 data-dir ではプラグインの保存領域が分かれない | §5 (host の exe 名を変える)、§6.3 の隔離の範囲 |

レビューで確認できた前提: `audible_pts_secs` が速度込みで DSP の遅延を引いていること、リングは PCM だけで
ProcessContext は処理フレーム数と常時 kPlaying であること、MSVC の `steady_clock` が QPC 由来で共通の時計に使えること、
音楽の矢印キーのシーク・速度変更・ループが実装されていること、プラグイン版でも Lyrics の Offset を編集できること、
最大の画像と歌詞を同時に載せても 32 MiB に収まること。FFmpeg の歌詞のキー名は未確認のまま (§6.1 で確かめる)。

### 10.2 第 2 回独立設計レビュー (2026-10-17、同じ SID で継続)

対象は第 2 版 (0af8c32d7)。判定 **REJECT**。前回の判定理由のうち、曲情報と位置の整合は解消
(世代と時計を同じ状態から読み、両通知の世代を照合する構造で配送順の逆転に対応できる)。
画像なし + 歌詞あり、候補 2 本の選択、Remote の確認方法も解消。`selectLyrics` は生成済みのプラグイン版の配布物にも残っており、
許可リストの変更は不要と確認された。残りは測定方法と再表示時の位置で、ClaudeCode が根拠のコードを読み、5 件とも正しいと判断して第 3 版に反映した。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P2 | D 記録が、時刻の違う 2 つの位置を引き算している (経過時間 × 速度が混ざる)。診断値は `AvClock` からは読めない | §4.3 (コールバック内で計算済みの lead を使う、`AudioDiagnostics` も所有者の状態に持たせる)、§6.1 の D 記録 (valid・再生中・同じ seek serial の行だけ集計) |
| P2 | Single line では Step transition time を設定できず、固定 0.18 秒の遷移が残る | §6.2 (List + Step (vertical) + Step transition time = 0、中央の現在行で比べる) |
| P2 | D が小さく Offset が大きいとき、残りをすぐデバイスの遅延とみなすのは早い (プラグイン内の配送・描画の遅れが D に出ない) | §6.1 の E 記録、§6.3 の判断 |
| P3 | 再表示の最初の回は discard だけで、最新の位置を取りに行かない | §4.6.4 (表示に戻ったら位置を古いと印を付け、新しい応答まで `null`)、§6.1 |
| P3 | 実機手順に perf 記録の有効化が無い | §6.3 の準備 (`--perf-log` とログファイルの確認) |

前回の P3 (保存領域の分離) について、実際に起動される名前は展開時の名前で決まる (`src/video/dsp/extract.rs:115`) との補足を受け、
§5 を「展開する basename だけを変える」に改めた。

### 10.3 第 3 回独立設計レビュー (2026-10-17、同じ SID で継続)

対象は第 3 版 (dc6bf46a9)。判定 **REJECT** (新しい P1 は無し)。測定用レイアウト (List / Step / 遷移 0 で現在行が中央へすぐ切り替わる)、
lead の定義、`--perf-log`、`AudioDiagnostics` を所有者に持たせる寿命、E 記録の stderr 経路 (`mimageviewer.log` に入る)、
展開する basename だけを変える案は成立と確認された。ClaudeCode が根拠のコードを読み、3 件とも正しいと判断して第 4 版に反映した。

| 指摘 | 内容 | 反映先 |
| --- | --- | --- |
| P2 | lead とコールバックの serial・時刻は同じ採取の値という保証が無い (入口の値は早期 return の回でも新しくなる) | §4.3 (lead を書く場所で lead・serial・時刻を連番付きの 1 組として公開)、§6.1 の D 記録の絞り込みとテスト |
| P2 | E の上限や分位点は遅れの量ではなく、差し引けない。ソフトウェアの遅れとデバイスの遅れは向きが逆 | §6.1 の E 記録 (上限と傾向として使う)、§6.3 の判断 (Offset の符号で候補を絞る、半往復の補正は仮説として試す) |
| P3 | 表示に戻る前に出した要求の応答でも「古い」を解除できてしまう | §4.6.4 (表示の世代と、今の世代で出した要求の応答だけが解除できる規則)、§6.1 |
