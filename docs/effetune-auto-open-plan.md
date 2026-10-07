# §1.337 動画再生時の EffeTune 自動表示 設計案

2026-10-07、ライン C。**設計レビュー待ち・未実装**。
正本: [バックログ](next-release-backlog.md) §1.337、
[EffeTune 統合](effetune-integration-plan.md) §0・§4・§10.1・§14、
[動画アーキテクチャ](video-architecture.md) と [detached 憲法](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項-最重要)。

## 決定済みとコード上の前提

- 利用者決定: **起動後の最初の適格なローカル動画再生で 1 回、全画面中は開かない、既定 OFF**。
- `VideoPlayer::is_playing()` は play intent と別。`src/video/mod.rs` の説明と
  `EngineActor::transition_to_playing` (`src/video/engine/actor.rs`) から、readiness 後の
  Playing が再生成功を示す。open 要求、autoplay=true、VideoInfo 到着、静止した先頭フレームは成功ではない。
- `App::poll_video` は通常・active viewer・ParkedLive の既存 mount 経路から呼ばれる。
  `FsCacheEntry::Video` には音声も入るので、entry 型だけでは動画判定できない。
- `App::poll_effetune` は Loading の `open_gui_when_ready: Option<ShowPermit>` を引き継ぎ、
  `request_show_gui_with_permit` へ送る。Idle のロード、Running の表示、host-control worker と
  hidden attach／ACK は既存構造を使える。**自動表示専用の再生成功通知は現在ない**。
- 現在の ShowPermit／GuiGate は最小化連番・Remote 取得連番を検査するが、**全画面を検査しない**。
  `DspBridge::show_slot_gui_checked` は `AllowSetForegroundWindow(host_pid)` を呼ぶ。
  既存表示をそのまま呼ぶだけでは、今回の全画面制約と前面化方針を満たせない。
  以上はソース調査。Visualizer の画面保持・Windows の実際のフォーカス挙動は未確認。

## 発火点と一度だけの所有者

**自動表示を決める場所は `App::poll_video` の再生結果集約後の 1 か所**にする。
各 open、キー／HUD の play、fast-swap、resume、EOF 連続再生には表示処理を足さない。

1. Engine の唯一の Playing 確定境界から、source／viewer identity を持つ
   **論理的な再生開始の成功事実**を VideoPlayer 経由で既存 poll に渡す設計を提案する。
   新規 source の初回と利用者の停止／pause 後の play を対象とし、seek・buffering 復帰・
   DSP handoff・音声トラック切替・周回による Playing 再入場は別の再生開始として数えない。
   単なる state の毎フレーム比較だと、全画面から出た継続再生を初回と誤認するため採らない。
   通知は source の既存寿命に従い、未採用の cache／破棄済み source の成功は表示へ流さない。
2. controller が持つ起動セッション内の単一 `AutoOpenSession` (Armed / Spent) で決定する。
   viewer ごとの「表示済み」や settings 内の「今回済み」は作らない。複数窓の成功も 1 回に集約する。
   設定 OFF や不適格な成功はその場で棄却し、復帰時の待ち行列に残さない。
3. 適格なら**ロード／表示要求を出す前に Spent**へ移し、二重発火を防ぐ。
   Idle は既存の実行中ロード経路、Running は既存の checked show、Loading は既存の
   ロード完了表示意図に合流する。手動表示意図を自動意図で上書きしない。
   Unavailable／Failed では自動ロード再試行をせず既存の理由表示を維持する。
   自動試行の失敗／途中取消でも Spent のまま。窓を閉じた後や次動画で開き直さない。

## 適格性・非同期表示の境界

| 条件 | 提案する扱い |
| --- | --- |
| 実ファイル動画の新しいローカル再生成功 | 設定 ON、表示可能な通常窓、EffeTune 利用可能なら候補 |
| 音声ファイル、動画→音声モード、RemoteHeadless／配信処理 | 対象外。音声ファイルへ拡張しない |
| F11 全画面 | この開始は棄却。解除しただけで後から開かない |
| 最小化・tray-only 非表示 | この開始は棄却。復元しただけで後から開かない |
| Remote の取得中・所有中・drain 中 | 正本 `remote_session_blocks_local_control()` で棄却。解除時は発火しない |
| portable | 実行しない。設定と設定検索の候補を表示しない |

不適格な開始は Armed を消費せず、**次の別の再生開始成功**を待つ。同じ再生の通常窓への復帰、
seek、設定を ON にしただけでは再生開始通知を生成しない。これは遅延 popup ではない。
再生と関係ない他窓の cache を走査して候補にしない。

ロード中／hidden attach 中／host 配送後に全画面・最小化・Remoteへ入った場合も、
**未表示の自動意図を取消し、解除時に復活させない**。ShowIntent を Manual / AutoVideo の
型付き意図にまとめ、既存 permit の検査に AutoVideo の表示条件と無効化世代を加える案を提案する。
全画面を一瞬往復しても古い auto permit が通らないよう、表示遷移の既存所有境界で世代を公開し、
host GUI thread が表示直前にも照合する。worker の一度の確認や時間窓だけでは済ませない。
既に表示済みの手動窓を全画面開始で新しく hide する機能には拡張しない。

**前面化に関する注意:** 自動開始は明示ボタンクリックと異なり、ロード完了時の foreground 権限を
保証できない。推奨は自動表示だけ**非アクティブ表示**とし、foreground 許可／activate を要求しない。
既存の manual show と非アクティブ復帰を区別して host に伝える必要があり、bridge／C++ の変更が
見込まれる。TOPMOST・owner 変更・フォーカス奪還 retry は追加しない (既存 owner=0 を維持)。

## 状態の組合せを減らす検討

起動直後に窓を開く案は再生成功の要望を満たさないため不採用。毎再生時の open、復帰待ち popup、
自動 retry を持たず、起動内の 1 owner と既存 Loading／show 意図だけに揃える。
再生や窓移動のモーダル化は通常操作を止めるため不採用。ロード・attach・表示 IPC は既存 worker、
UI は成功事実と gate の軽量更新だけにする。DSP 経路の起動後常時接続／保存契約は維持する。

## 利用者が決める質問 (回答まで実装しない)

- **C337-1:** 自動表示は非アクティブ表示でよいか。**推奨: はい**。前面権限を要求せずキー操作を奪わない。
- **C337-2:** 通常の F12 別窓／複数窓の動画も対象とし、いずれかローカル閲覧窓が全画面なら
  自動表示を抑止する案でよいか。**推奨: はい**。メインの `fullscreen_idx` の有無だけでは
  通常窓と全画面を区別できない。決定後、presentation owner の既存事実を使う。
- **C337-3:** ON の場合、未起動の EffeTune を開始して終了まで DSP を経由させてよいか。
  **推奨: はい**。既存手動開始と同じ。ロード待ちで再生を止める新処理は追加しない。
- **C337-4:** 成功した手動 open はこの起動の自動表示機会も消費するか。
  **推奨: はい**。利用者が手動で閉じた窓を初回動画で開き直さない。
  自動設定を途中で ON にした場合も、継続再生へ即表示せず次の再生開始だけを候補にする。

## 実装前後のレビュー・受け入れ

成功通知の logical start と seek／handoff の区別、全 viewer producer／close／swap／cancel、
Manual / AutoVideo の優先、fullscreen の世代公開と host 最終検査を**実装前に独立レビュー**する。
detached／presentation 経路に入る変更は設計 lead と独立 reviewer が構造的修正として合意し、
detached-rework-plan.md §11 に記録する。現時点ではその合意はない。
非起動の state／fake host テストは open失敗→成功、paused open→play、各抑止開始→復帰→
次開始、ロード中の抑止往復、手動意図優先、二窓同時、設定OFF→ON、portable欠落を対象とする。
実装時は既存 settings.db へ既定 false の加算設定と serde default を追加し、旧データを保持。
環境設定「動画・音声 → 動画 → 音響調整」、spec、EffeTune正本、manual/effetune.html、製品ページを更新する。
bridge変更時の最終確認は release launcher/core build が必要。今回は製品を起動せず、実機挙動は検証しない。
