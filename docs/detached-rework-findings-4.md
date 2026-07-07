# 検収所見 #4: 連動 (linked) PDF 窓が再アクティブ化で機能喪失する + session_begin ストーム

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
ゲート C の smoke チェックリスト実施中 (2026-07-06) にユーザーが発見。

⚠ ログはアプリ再起動で上書きされ、以下は消失前に Fable が抽出した部分証拠。
再現手順は確立しているので、headless テストでの再現を正とする。

## F8: 連動 PDF 窓の再アクティブ化が「裸の凍結静止画」に落ちる

### 再現手順 (ユーザー実機、S1 設定)

1. PDF ページを detached 表示してピン留め (窓 A = independent)
2. 別の PDF ファイルを開く → 新しい連動窓 B (ピンなし)
3. 窓 A をクリックして active 化 (→ B は passive 化)
4. 窓 B をクリック

### 症状

- B は前後移動 (ページ nav) が効かない
- メインウィンドウとの連動が切れたまま
- 上部バーが「ページ数・サイズ表示 + ピン + ×」だけの特殊表示
- この状態でピンを押しても「画像単独」のまま回復しない

期待仕様 (implementation-plan §3 表⑦): 連動 passive をクリック → **連動 active として
復帰** (メイン同期・ページ nav とも回復)。

### 消失前に取れた証拠

```
(周期ダンプ、214〜229s) passive_window_state id=2 pinned=true can_activate=true
    has_bundle=false has_descriptor=true has_stamp=true frozen_pages=0
(171.115s) runtime_flags window_id=2 pinned=false->true linked=true->false
    reason=passive_toggle_pin
```

- **id=2 (窓 B) は `has_bundle=false`** = park 時に paused_bundle が保持されていない。
  descriptor と凍結テクスチャのみ。
- 171.115 の pin 押下は passive のまま処理され (`passive_toggle_pin`)、
  linked=true→false に変わっただけで復帰していない。
- ユーザーが見た「特殊な上部バー」は passive 窓のバー = **クリックしても実際には
  active 化が完了していない**か、descriptor fallback が「連動 book context」ではなく
  「裸の凍結静止画 context」を作っている疑い。

### 調査・修正要件

1. 手順 3 の時点 (A の activate に伴う B の park) で、B (連動 book context) の
   paused_bundle がどの経路で失われるかを特定する。
   - 連動窓は設計上 bundle を持たない (メイン追従で再構成する) のか、
     持つべきなのに落ちているのか、をまず仕様として確定する
     (implementation-plan 表⑦ と突き合わせ)。
2. `has_bundle=false + has_descriptor=true` の passive をクリックしたときの
   復帰経路 (descriptor fallback) が、**連動 book context として** 再構成される
   ことを保証する (ページ nav / メイン同期 / 通常の上部バー)。
3. passive 状態でのピン押下 (`passive_toggle_pin`) の仕様確認: 表⑦ の属性モデル
   に従うか、復帰後のみ許可にするか。現状は linked フラグだけ書き換わり、
   ユーザーには何も起きたように見えない。
4. headless 再現テスト: 「pinned A + 連動 book B → A activate → B activate →
   B が Active / linked / nav 可能 / メイン同期あり」を assert。

## F9: `session_begin` ストーム (毎秒数十回)

消失前のログ 72.3〜73.1s に、window_id=1 (ピン済み independent) に対する

```
state_transition window_id=1 from=Active to=Active reason=session_begin
runtime_flags ... reason=prepare_viewer_presentation_open
```

が **20〜50ms 間隔で数十回連続**する区間があった。過去のセッション (2026-07-06 08時台)
でも同種の Active→Active session_begin 連発 (0.3〜0.6s 間隔 × 30 回) を観測している。

- `prepare_viewer_presentation_open` が毎フレーム相当で再実行されるループが
  存在する疑い。トリガ (PDF enumerate リトライ / 連動追従 / 他) を特定する。
- 実害の有無を評価し、ループなら塞ぐ。正当な再試行なら理由をログとコメントに
  明文化する (完了報告に判定を書く)。

## 完了条件

- [ ] F8 の headless 再現テスト + 修正、表⑦ との整合を完了報告に明記
- [ ] F9 の原因特定と対処 (修正 or 正当性の明文化)
- [ ] 既存 detached / parked_live / deferred テスト + full test 緑
- [ ] コミットに `detached-rework findings-4` を含める
- [ ] `.\scripts\build-release.ps1` で実機バイナリ準備

## F10 (小・同梱可): デバッグログのスパムがローテーション世代を食い潰す

ログには `.prev` (起動時 1 世代) と `.log.bak` (サイズローテーション 1 世代) の
保全があるが、`MIV_DETACHED_WINDOW_DEBUG=1` 時に `parked_live_poll_begin` が
毎フレーム (~6ms 間隔)、`passive_window_state` が定期ダンプで出るため **~5MB/分**
となり、ローテーションが数分で 1 周して証拠が消える (2026-07-06 に実害:
F8 発生セッションのログが `.bak` 上書きで消失。核心行は本書に転記済み)。

- `parked_live_poll_begin` は「状態変化時 + 数秒に 1 回」程度に間引く
  (時間窓での挙動制御ではなくログの間引きなので憲法 5 の対象外)。
- `passive_window_state` の周期ダンプも同様に間隔を広げてよい。
- 判定に使う情報量は落とさない (state_transition 等のイベントログは全量維持)。

## 運用メモ (再発防止)

実機でバグを踏んだら、できるだけ早くログを退避する (ローテーションで数分後に
消え得るため):
`Copy-Item $env:APPDATA\mimageviewer\logs\mimageviewer.log $env:APPDATA\mimageviewer\logs\bug-YYYYMMDD-HHMM.log`
(`.log.bak` / `.log.prev` があればそれも)。smoke チェックリストにもこの注意を含める。
