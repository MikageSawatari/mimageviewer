# Stage R2b 検収所見 #1: ParkedLive 復帰経路の欠陥 (差し戻し)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md) / 指示書:
[stage-r2b](detached-rework-stage-r2b.md)

検収結果: **Part 2 (live-park) の復帰経路が NG**。実機で「動画表示 → ホイール切替 →
『メタデータ読み込み中』のまま固まる」を再現し、ログ (2026-07-06 07:5x 採取、
Fable 解析) で以下を確認した。park 自体 (`park_legacy_live_media` → ParkedLive、
47.135s) と再生継続は動作している。**同じ Codex セッションを resume して以下を
修正すること。**

## 実機ログの証拠 (時系列)

```
47.135 state_transition id=3 Closing→ParkedLive reason=park_legacy_live_media   ← park 成功 (idx=45 再生中)
49.768 video-resume-thumb: ScaleContext::get ...                                ← 復帰系の何かが起動
49.794 VideoPlayer::open done path=...Ultimate taste....mp4 (idx=46)            ← ★新しい VideoPlayer が open された
49.794 prepare_viewer_presentation_open_begin idx=46 entering_video=true
49.794 state_transition id=3 ParkedLive→Active reason=session_begin
49.794 session_begin window_id=3 source=Book                                    ← ★動画窓なのに source=Book
49.794 video cache hit idx=46 → resume playback
49.798 presenter switched source epoch=3
49.939 active_context_state window_id=Some(1) fs_idx=Some(13)                   ← ★mount 中の bundle は静止画窓 id=1、
       session=Some({window_id:3, source:Book})                                    session は id=3。文脈と session が乖離
49.803〜 passive_event id=3 が毎フレーム (~12ms 間隔) 出続ける                   ← ★id=3 の passive snapshot が残存
50.290〜120.967 [audio-decode] audio_tx send blocked ... engine_state=Loading
       clock_playing=false (最終 71,177ms ブロック)                              ← ★新 player のエンジンが Loading のまま
                                                                                   71 秒間デッドロック相当。UI は
                                                                                   「メタデータ読み込み中」表示で固着
```

## 所見 (修正必須)

### F1: 復帰で新しい VideoPlayer が open され、エンジンが Loading で永久停止

ParkedLive の復帰は **park された bundle 内の再生中 player をそのまま使う**のが
仕様 (指示書 §2.2-4「復帰で映像・音声が途切れない」)。実際は
`VideoPlayer::open` が走って 2 本目の player が生まれ、その engine が `Loading`
から進まず、audio-decode スレッドが 71 秒以上 send blocked (スレッド持ち逃げ +
UI は読み込み中表示のまま)。復帰経路では **新規 open を構造的に不可能**にすること
(テストで固定: 「ParkedLive → Active の遷移中に VideoPlayer::open が呼ばれない」)。

### F2: 復帰後の identity 乖離 (session=id3/Book、mount された bundle=id1)

`session_begin window_id=3 source=Book` — 動画窓の復帰なのに source が Book。
さらに直後の `active_context_state` は mount 中 bundle が window_id=1 (静止画ピン窓)
を指しており、**session と mount 中文脈が別の窓**になっている。復帰は
「ParkedLive 専用経路」として実装し、汎用の paused_bundle 復帰 (Book/Image 用) に
相乗りさせないこと。source は `Video` で復帰する。

### F3: 復帰後も passive snapshot が残る (二重人格)

ParkedLive→Active 後も id=3 の passive_event が毎フレーム出続けている =
passive リストから snapshot が除去されていない。窓 1 本につき描画経路は常に 1 つ
(active か passive のどちらか)。遷移と snapshot 除去を同一フレームで atomic に行う。

### F4 (仕様確認): ParkedLive / passive 窓上のホイールは不活性にする

今回の連鎖はホイール操作が引き金の可能性が高い (49.768 の resume-thumb 起動)。
仕様は「クリック = 復帰のみ」なので、**passive / ParkedLive 窓上のホイール・
キー入力は何も起こさない** (復帰もしない、ナビもしない)。クリック以外の入力で
open / nav / resume が起動する経路が残っていないかを passive 入力処理全体で確認し、
見つけた経路を列挙して塞ぐこと。

## 修正後の完了条件 (追加)

- [ ] F1〜F4 の回帰テスト (最低: resume 中 open 禁止 / resume 後 snapshot 除去 /
      resume の source=Video / passive 窓ホイール no-op)
- [ ] 既存 R2b テスト + full test 緑
- [ ] 修正コミットは Part 2 の続きとして別コミット (`R2b fix1` を含める)
- [ ] `.\scripts\build-release.ps1` で実機バイナリ再準備

実機再検証 (ユーザー) は前回の 5 項目に加えて:
「動画 detached → 他窓クリックで live-park → **ParkedLive 窓の上でホイール** →
何も起きない → クリックで復帰 → 再生継続のままシーク可能 → ホイールで次の動画へ
切替できる」を通しで確認する。
