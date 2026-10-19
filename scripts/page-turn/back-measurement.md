# 戻る方向 D3 の参考計測

§1.405 / page-turn計画 §27.17.20 の計測支援。製品起動は未実施。
実行前にシナリオ、所要時間（1 run 約1分＋初回build）、desktop/focus使用、
使い捨て `target/portable-smoke/data` の範囲について明示承認を得る。
通常profileのdev-runtime、利用者のportable、設定コピーを使わない。

1. `scripts/prepare-portable-smoke.ps1 -TestScript` で使い捨てportableを準備する（起動しない）。
2. 対象の03/04 fixture ZIPを指定し、各scriptを別runで実行する。
3. run直後にscript/overlay/sourceのSHA-256、viewport/ppp、GPU/driver、focus条件、ログ一式を証跡directoryへ保存する。
   runnerは次runでdataを削除するため、前runのログを上書きしない。
4. D3解析は自動実行されないので、別コマンドで行う。

```powershell
.\scripts\page-turn-smoke.ps1 `
  -Exe .\target\portable-smoke\mimageviewer.exe `
  -Script .\scripts\page-turn\back-f12-idle.rhai `
  -Folder .\target\repro\page-turn-budget-571-20261016\04_extreme_8000x8192.zip `
  -ScriptDataDir .\target\portable-smoke\data `
  -SettingsOverride .\scripts\page-turn\back-rtlcover-adjusted.settings.json `
  -RunTimeoutSeconds 180
python scripts/analyze_perf.py target/portable-smoke/data/logs/perf_events.jsonl page-turn-d3 --json-out target/back-measured.json
```

| script | 条件・操作 |
| --- | --- |
| `back-main.rhai` | 一覧idx12から直接本体open、2500ms後から後退4回 |
| `back-f12-immediate.rhai` | 一覧idx12→F12、window/context identityと通常描画成立の直後に後退、次は2500ms後 |
| `back-f12-idle.rhai` | 同じ直接open、2500ms後から後退4回 |
| `back-two-context-reversal.rhai` | F12で後退→前進、本体の別位置openと後退→前進、F12へ戻り反転。contextごとに別集計 |
| `back-boundary-wrap.rhai` | 先頭Previousと末尾Next。plainとwrap overlayを比較。wrap先は平らに着地、投機需要はBoundary |
| `back-forward-control.rhai` | idx0の直接本体open後に前進8回・2500ms間隔。03/04 × Single/RtlCover × plain/adjustedの八条件。16頁のRtlCoverでも8機会を確保する |

設定overlayはSingle/RtlCover/LtrCoverのplain/adjusted、OFF、wrapを用意した。
OFFはmain/F12、wrapはboundaryで比較する。OFF以外は演出200msで、既存source/AI範囲は変更しない。
LtrCoverは左右配置と実キーの別確認にも使う。新しい準備待ちAPIやD3完了待ちはない。
scriptの待ちは通常描画・入力成立だけで、2500msは比較条件の間隔である。
即時F12のmiss/skipを除外しない。Next優先によりcold直後の後退は同期missを残し得る。

解析の `status=reference` は正式性能gateの合格ではない。操作別の前進/後退、Next/Previous/unknown、
兄弟blocker、予約回収、Native除去回避、取消→実終了時間を確認する。
CPU→登録はrequest_id、登録→配送はrenderer/TextureId、Nativeはrenderer/exact keyでだけ結ぶ。
一致から補完するのは一意な準備起点のrequested_positionだけで、lookup/配送時のdemand_position欠落はunknownのままにする。
準備起点が複数位置にまたがる場合と相関できないlookup、旧ログはunknown。時刻が近い操作へ準備を推定帰属しない。
背景配送・静止中入力応答・Next到達時間、D2/D4/flat-first、正式3run×30機会は別途計測する。
