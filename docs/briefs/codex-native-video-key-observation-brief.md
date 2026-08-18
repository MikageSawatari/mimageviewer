# backlog §1.92 — native video のキー経路に無条件の記録を入れる (観測のみ)

対象: backlog §1.92「別ウィンドウの動画再生中に外部アプリから戻ると <kbd>Z</kbd> だけ効かない」。

**これは計装のみの作業。原因修正・挙動変更は一切しない。** detached viewer はリワーク凍結中で、
症状パッチを入れない。今回やるのは「どこで消えているか」を切り分ける記録を足すことだけ。

## 0. 症状 (利用者報告 2026-08-18、以前のバージョンでも再現)

- 別ウィンドウで**動画を再生している**状態で、他アプリからそのウィンドウをクリックして戻すと
  <kbd>Z</kbd> だけ無反応。**P やカーソルキーは効く**。
- 上部 HUD からマウスで音声モードへ切り替えると、以降は <kbd>Z</kbd> も効く。
- **音声モード表示中に戻した場合は起きない。**

## 1. 今のログでは切り分けられない

<kbd>Z</kbd> が消え得る場所は 3 段ある。

1. `handle_native_video_key_event` ([native_video.rs:6558](../../src/app/native_video.rs:6558)) に
   そもそも届いていない
2. 届いたが、match 到達前の gate のどれかで `return` した
   (music_vst_shell / video_audio_vst / normalize modal / detached Enter・Esc /
   rating / slot / side panel …)
3. match して `enter_video_audio_mode` ([native_video.rs:7479](../../src/app/native_video.rs:7479))
   を呼んだが、その中の早期 return で弾かれた

**現在ログに出るのは 3 の入口の 1 行だけ** (`log_video_audio_enter_request("native_key", …)`)。
それも match した場合にしか出ないので、1 と 2 を区別できない。そして
`enter_video_audio_mode` の早期 return は**全て無言**で、少なくとも 6 経路ある
(既に入場済み / item と fullscreen_idx の不一致 / mode switch・source swap・detached host switch
進行中 / presenter HWND 未確定 / 音声トラック無し / detached の entry_target 捕捉失敗)。

## 2. やること

### 2.1 入口に無条件の 1 行 (P1)

`handle_native_video_key_event` の**最初の gate より前**で、`!key.repeat` のキーイベントを
**毎回**記録する。記録内容:

- `virtual_key` / `scan_code` / `extended` / `ctrl` `shift` `alt` / `repeat`
- `fs_idx`、presentation の種別 (フルスクリーン / ウィンドウ内 / 別ウィンドウ)
- `video_audio_mode`、`music_vst_shell` の有無
- **前面ウィンドウの HWND と presenter の HWND** (外部アプリから戻った直後かを見たいので必須)
- 連番 (`seq`)

repeat は 1 秒あたり 1 行に集約した件数だけ出す (auto-repeat で溢れさせない)。

### 2.2 outcome を型で表す (P2)

同じ関数の**各 `return` に型付きの理由を対応付け**、**イベント 1 件につき必ず 1 行**、
outcome 付きで出す。P1 と 2 行に分けても、P1 の行へ outcome を後から載せてもよいが、
**「どの return も必ずどれかの outcome として出る」**ことを型 (enum + exhaustive match)
で保証すること。match に到達して action が決まった場合は、その action 名を outcome に載せる。
どの arm にも当たらなかった場合も `no_match` として出す。

### 2.3 `enter_video_audio_mode` の早期 return に理由を付ける (P3)

各早期 return を型付きの理由に置き換え、**呼び出し元 (source) と一緒に 1 行出す**。
`log_video_audio_enter_request` が既に入口を記録しているので、**その対になる結果行**を作る。
成功時も 1 行出す (現在の「entered audio mode」で足りるならそれでよい)。

呼び出し元は native key 以外にもある (HUD の ♪ ボタン等)。**source を引数で受け取り**、
HUD 経由と key 経由を後から区別できるようにする。

## 3. 抑制条件の禁止事項 (重要)

**記録の抑制条件を、調査対象の信号に依存させない。** 具体的には:

- 「<kbd>Z</kbd> のときだけ出す」にしない。**全てのキーを出す** (P とカーソルが効くという報告が
  正しいかも同じログで検証できる必要がある)
- 「音声モードのときだけ」「detached のときだけ」にしない
- rate limit は**時間**だけを条件にする (repeat の集約のみ)

この案件では同じ罠を 2026-08-17〜18 に 3 回踏んでいる ([keymap-spec.md](../keymap-spec.md) の
「診断計装」節、および §4.2 の経緯)。

## 4. やらないこと

- 入力の consume / dispatch / gate の判定を変えない。**読み取り専用**。
- `enter_video_audio_mode` の早期 return の**条件を変えない**。理由を付けるだけ。
- detached 周りの述語・viewport 経路の挙動を変えない。
- 時間窓による判定を足さない。

## 5. テスト

- 行のフォーマット関数に unit test (既存の `format_video_audio_pre_handle_fs_key_diagnostic`
  のテストと同じ形)。全フィールドが出ることを固定する。
- outcome enum が**全ての早期 return を覆う**ことを、exhaustive match で保証する構造にする
  (どこかに `_ =>` を置いて逃がさない)。
- mutation: 各 outcome の生成を削ると対応テストが落ちることを確認して報告する。

## 6. ビルドの制約 (今日のリリース進行中につき厳守)

- **`cargo build --release` と `.\scripts\build-dist.ps1` を実行しないこと。**
  `target\release\mimageviewer.exe` は今夜公開する署名済み配布物そのもので、
  release build で上書きされる (退避コピーは `dist\v3.1.1\`、
  経緯は [RELEASE-v3.1.1-pending.md](RELEASE-v3.1.1-pending.md))。
- 検証ビルドは `.\scripts\build-dev.ps1` (`target\dev-runtime`) を使う。
- `.\scripts\test-full.ps1` は可 (release 成果物を読むだけで上書きしない)。
- commit / stage はしない。ブランチは `master`。**別セッションが同じ作業ツリーで動いている**ので、
  自分が触っていないファイルの変更を戻したり stage したりしない。

## 7. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

報告には、追加した outcome の一覧 (= 無言だった経路の数) と mutation 結果を含める。
