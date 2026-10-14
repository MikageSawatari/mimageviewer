# mIV Remote の不具合調査

> この文書は 2026-10-15 に CLAUDE.md から移した節をそのまま収めたもの。本文中に残る「本書」は移動前の CLAUDE.md を指す。移した節の行き先一覧は CLAUDE.md の「移した節の行き先」表を参照。

## mIV Remote の不具合調査は、まずサーバ側ログを見る ⚠️

**端末 (スマホ / タブレット) 側の挙動は、既にサーバへ送られて記録されている。**
リモートの不具合を調べるとき、**推測する前にこのログを開くこと。**

```
%APPDATA%\mimageviewer-remote\remote-web-log.jsonl
```

**data_dir の兄弟ディレクトリ** `<data_dir>-remote` に出る
(`data_dir::remote_service_log_dir`)。既定の data_dir なら上記のパス。
ポータブル版なら `<exe_dir>\data-remote\`。`--log` は **remote 側で必須**引数で、
パスは core が決めて渡す (`remote_ipc::service`)。JSON Lines 形式。

> 以前ここには「既定は相対パスでリポジトリルート直下」と書いてあったが**誤り**。
> 2026-08-23 の調査でリポジトリ root と `%APPDATA%\mimageviewer` を探しても
> 見つからず時間を使った。`--log` 必須化で本体がパスを決める形に変わっている。

16 MiB を超えるとローテーションし、現在ファイルと
`remote-web-log.1.jsonl`〜`.4.jsonl` の計 5 世代を保持する (合計およそ 80 MiB)。
本体の perf log と違い**起動時にはローテーションしない** — PIN 変更・serve 設定・
全端末ログアウトで core が child を再起動するため、起動契機にすると数回の操作で履歴が消える。

### 何が入っているか

`/api/telemetry` に届いたクライアント側のイベントが、HTTP 応答ログの
`details.telemetry.events[]` として入る。種類は `remote_session` / `error` /
`command` / `page_prefetch` / `image` / `thumbnail_grid` / `folder_list` / `video_health` など。

`type: "error"` のイベントは `category` で分かれる
(`fetch_non_2xx` / `fetch_error` / `image_load_error` / `window_error` /
`spread_load_error` など)。**`window_error` と `unhandled_rejection` は
クライアントの未捕捉例外**で、`window.addEventListener` から拾っている。

### 読み方の例

```python
import json, io, os, collections
c = collections.Counter()
p = os.path.join(os.environ['APPDATA'], 'mimageviewer-remote', 'remote-web-log.jsonl')
for line in io.open(p, encoding='utf-8'):
    if not line.strip():
        continue
    events = ((json.loads(line).get('details') or {}).get('telemetry') or {}).get('events') or []
    for e in events:
        if e.get('type') == 'error':
            c[e.get('category', '?')] += 1
print(c.most_common())
```

### 重要な性質

- **`/api/telemetry` はセッション取得を要求しない** (`route_requires_remote_session`
  の除外)。**本体が切断した後でも端末のログは届く。** 切断まわりの調査に使える
- 通常の telemetry は間隔でまとめて送る。一方、`window_error` /
  `unhandled_rejection`、terminal な動画再生層 error (`video_stream_hls_fatal` /
  `video_stream_media_element_error` / `video_stream_playback_stalled`) と
  `remote_session` の `control_transition` は、その JS turn で `sendBeacon`
  (非対応時は keepalive fetch) へ渡す。各 event の
  `client_event_sequence` で、別リクエストになった transition と例外の前後を復元できる
- `video_health` は play intent 中に10秒周期で、current time / 全体位置、buffered ahead、ready state、
  frame drop、hls.js bandwidth / fragment load、play Promise の attempt / success / rejection / pending、
  利用可能なら回線種別 / RTT を残す。current time が停止しても周期は止まらない。
  `waiting_threshold` / `stall_terminal` / `hls_fatal` / `media_error` / `play_rejected` は即時
- runtime waiting は 0.25 秒以上の実再生進行を `videoPlaybackStallDecision` の `resolved` として
  単一 watch owner から解除する。3 秒警告も同じ判定を通し、意図的 pause の `stalled` は watch を作らない
- telemetry は enqueue 前に `normal` (既定) / `debug` へ段階化される。normal は数値・固定列挙を残し、
  path/address/resource、message/stack、client/session 識別子を除く。debug は端末設定の明示 ON 時だけ
  remote address、message、client ID、SHA-256 短縮 session 相関値を残す。ON 中は HUD の
  「詳細記録 ON」が常時見える。remote session 生値、PIN、Bearer token はどちらにも出さず、server の
  `permanent_log_secrets` / `redact_serialized_secret` を最終境界として維持する
- セッション transition は `observer` (`ping` / `video_poll` / `acquire` など)、
  `observed_status` / `http_status`、遷移前後の status / phase /
  `blocks_interaction` を持つ。server message、URL、client/session ID は載せない
- `window.onerror` はスクリプトが cross-origin 扱いだと内容が落ち、
  `"Script error."` になる。ただし製品の `app.js` と同梱 `hls.min.js` は同一 origin
  配信で、`app.js` は module script でもある。`"Script error."` が残る場合は、まず
  ブラウザや拡張機能が注入した外部 script も疑う

### 経緯

2026-08、切断まわりの不具合調査で、私 (ClaudeCode) は 3 回続けて原因を推測し
3 回とも外した。その間ずっとこのログが手元にあり、開いていなかった。
実際の原因 (静止画専用メソッドを動画ビューアに対して呼んだ TypeError) は、
未捕捉例外として記録され得るものだった。**推測の前にログを見る。**
