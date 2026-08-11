# リモート閲覧: 音声ファイルの再生

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. 観測されている状態

音声ファイルは**一覧に出るのに開けない**。利用者から「一覧にも AUDIO ファイルは出ているので
再生できるようにしたい」との要望。

一覧に出るのは `GridItem::Audio` → `RemoteEntryKind::Audio` の写像があるため
([src/remote_ipc/container.rs](../src/remote_ipc/container.rs))。

開けないのは配信側の門で止まるため。
[src/video/stream/session.rs](../src/video/stream/session.rs) の `StreamingSession::start`:

    if !inputs.has_video || !inputs.has_audio {
        return Err("remote streaming requires both video and audio streams".to_owned());
    }

音声のみのファイルは `has_video == false` なので、開始時点で失敗する。
現在のクライアントは、これを「この端末では音声を再生できません」という案内へ落としている。

## 2. 方針 (利用者判断で確定)

**音声のみの HLS で配信する。動画と同じ経路に揃える。**

元ファイルをそのまま Range 配信する案は採らない。理由:

- **WMA は主要ブラウザが再生できない。** FLAC も端末差がある。素で流すと
  「対応形式は端末次第」という説明が要る
- 外出先の回線で FLAC や WAV を元のまま流すのは現実的でない
- 動画で fMP4 remux を採らず「PC 側で変換して配信」に一本化した判断
  ([web-remote-video-streaming-plan.md](web-remote-video-streaming-plan.md)) が、音声にも
  そのまま当てはまる

## 3. やること

- `StreamingSession::start` の門を「動画と音声の両方が要る」から
  **「音声が要る。映像は任意」**へ変える。映像が無い場合は映像側の encode / tap /
  segmenter を通さない
- 既存の [audio_encoder.rs](../src/video/stream/audio_encoder.rs) を使う。音声用の別実装を作らない
- **playlist / segment / seek / 生存管理 / session owner の扱いは動画と同じ経路にする。**
  音声専用の並行実装を作らない。分岐は「映像トラックがあるか」1 点に集約する
- クライアントは音声 entry を開いたとき、動画と同じ再生画面へ入る。映像の描画領域は持たず、
  シーク・再生位置・音量・前後ファイル移動は動画と同じコマンドを使う
- §1 の「この端末では音声を再生できません」案内は不要になるので削除する。
  ただし**本当に開けない種別 (変換前のアーカイブ等) の案内は残す**

## 4. 判断が要る点 (実装前に報告してよい)

- 音声のみのとき、既存の画質プリセット (`QualityPreset`) をどう解釈するか。
  音声ビットレートへ写像するのか、音声専用の段を持つのか。**決めたら根拠をコメントに残す**
- 音楽ビューの波形・スペクトラムはこの増分では**対象外**。まず「鳴ること」を成立させる

## 5. やってはいけないこと

- 音声用に playlist / segment / session を別実装すること
- 元ファイルをそのまま配信する経路を併設すること (形式ごとに挙動が変わる)
- session owner の排他、生存タイムアウト、放置タイムアウトの規則を音声だけ変えること
- 認証・秘密の扱いを変えること

## 6. テスト

- 音声のみのファイルで `StreamingSession::start` が成功すること
- 映像付きファイルが従来どおり動くこと (回帰)
- 映像も音声も無いファイルは従来どおり拒否されること
- playlist / segment の生成が動画と同じ経路を通ること
- session owner の切り替えで音声再生が止まること (動画と同じ規則)

## 7. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`crates/remote-ipc` / `crates/remote-web`、
  web テスト一式
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- `cargo check` の警告が増えていないこと
- ビルドとコミットは行わない。`htdocs/` は触らない
