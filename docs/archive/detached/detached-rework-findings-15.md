# 検収所見 #15: parked 窓の速いクリック棄却 (up_dragged) + × ボタンの間欠不発

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
実機 smoke (2026-07-07 22:1x、R1/R2 実施中) でユーザー報告 2 件。ログ解析済み
(scratchpad/smoke1_cur.log)。どちらも**リワーク中の新規退行ではなく** watcher /
deferred 入力の設計由来 (findings-8 世代)。ship 前に解消する。

## G1: 速いクリックが up_dragged で棄却され、復帰に 2 クリック必要になる

### 実測

- `deferred_activate_watcher_rejected reason=up_dragged` × **13 件** (R1 の高速切替
  308〜315s に集中、target_id=9/10/11)。
- 窓から窓へマウスを動かしながらクリックすると、down→up 間の移動が 8px を超え
  ドラッグ扱いで棄却。2 回目 (静止クリック) は成功 = 「追加でもう 1 クリック」。

### 修正要件 (G1)

ドラッグ判定を**押下位置で層別**する (閾値の調整 = チューニングは不可):

- **down がコンテンツ領域** (パッシブバー/タイトルバー以外): 移動量に関係なく
  release で activation (parked 窓のコンテンツはドラッグしても何も起きないので、
  移動を伴う release はすべてクリック意図とみなせる)。
- **down がタイトルバー領域**: 現行の移動判定を維持 (窓の再配置ドラッグで
  activation しない、という既存の意図を保存)。
- 判定は窓 rect とバー高さから決定的に計算 (時間窓・速度閾値は禁止)。
- テスト: コンテンツ down + 大移動 release → commit / タイトルバー down + 大移動 →
  棄却 / 既存のクリック活性テスト不変。

## G2: parked 窓の × ボタンが間欠でしか効かない (1 回目はアクティブ化になる)

### 実測と機構

- parked (deferred) 窓の × は deferred viewport の egui 入力 (`bar_close_requested`)
  頼み。findings-8 で確定済みのとおり **deferred へのポインタ配送は間欠的**。
- 届かないクリックは watcher が「窓への activation」として処理 → 窓が resume →
  2 回目の × は immediate viewport (入力確実) で close。ユーザー観察
  「× を押すと正常なアクティブになり、次で閉じれる」と一致。挙動は以前から存在。

### 修正要件 (G2)

- **watcher に × ボタンの hit 判定を追加する**: パッシブバーの × ボタン矩形は
  自前レイアウト (窓 rect から決定的に計算可能) なので、down/up とも × 矩形内なら
  activation ではなく **close 要求**をチャネルで送る (App 側は既存の close 経路
  `close_command_ids` に合流)。これで parked 窓の × は egui 配送に依存せず
  1 クリックで確実に閉じる。
- × 矩形の計算はバー描画側の定数と共有し、二重定義しない (描画とズレたら壊れるため、
  同一関数から導出)。
- ドラッグ判別: × down → × 外 release は破棄 (標準的なボタン挙動)。
- テスト: × 矩形内 down/up → close 送信・activation なし / × 外 → 従来どおり
  activation / ボタン rect が描画側と同一関数から導出されること。

## G3 (低優先・同時にやる): 誤帰属 drop のノイズ削減

- `deferred_activate_watcher_dropped reason=repair_failed` ×26 は、ParkedLive 動画窓・
  メインウィンドウへのクリックが rect 包含フォールバックで別の Parked 窓に誤帰属し、
  D2 ガードが正しく拒否したもの (**実害なし**だが診断ノイズ + 将来の誤解の元)。
- 修正: down フォールバック候補の形成条件に「cursor_root が**他の runtime に
  claimed されていない** かつ main hwnd でない」を追加 (repair 用の消去法の趣旨に
  合致。claimed 済み hwnd への クリックはその窓の担当経路に任せる)。

## 実装メモ (Codex 2026-07-07)

- G1: watcher の down 候補に `drag_sensitive` を持たせ、タイトルバー押下だけ既存の
  8px ドラッグ棄却を維持した。コンテンツ領域押下は passive content にドラッグ操作が
  無いため、大きく移動して release しても activation として扱う。
- G2: パッシブバーの × ボタン矩形を描画側と watcher で共有する
  `detached_image_window_bar_close_button_rect()` に切り出した。watcher は × 内
  down/up を activation ではなく close request として App に返し、既存の passive close
  副作用を共通ヘルパに合流させる。
- G3: rect 包含フォールバックは cursor_root が main HWND または既存 runtime の
  claimed HWND の場合は repair 候補にしない。これにより ParkedLive / main クリックの
  誤帰属は `repair_failed` まで進まず down 段階で棄却される。

## 完了条件

- [ ] G1 + G2 + G3、テスト付き。コミット `(detached-rework findings-15)`
      (fix3/fix4 と別コミット)
- [ ] full test 緑 / fmt / glyphs / build-release

## 実機確認

1. R1 再実施: 高速切替でもクリック 1 回で復帰 (up_dragged が出ない)
2. R2 再実施: parked 窓の × が常に 1 クリックで閉じる
3. parked 窓のタイトルバードラッグ (再配置) では activation しない
