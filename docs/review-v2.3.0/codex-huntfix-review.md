結論: 1 件あります。その他の 1-4 / 7 / 8 と、Drop の通常 close/drop 経路・4 atomic の通常 swap 経路はクリーンです。

[P2] src/app.rs:28107  
`park_current_viewer_context_as_live_media_inner(... preserve_main_context=true)` の legacy ParkedLive 経路で、main 復元 bundle が `ViewerContextBundle::empty()` ベースになります。その直後に `take_current_viewer_context_bundle()` が tx/rx/cancel/reload/heavy queue を parked media 側へ移すため、復元後の main context は `reload_queue=None` / `heavy_io_queue=None` になります。`update_keep_range_and_requests` は `reload_queue` が無いと `src/app.rs:21722` で return するので、ParkedLive 化後の main グリッドで Pending/Evicted サムネ再投入と idle upgrade が止まります。queue bundle 化前は App 側に worker runtime が残っていた経路なので、今回の per-context 化による退行です。復元 main 用に新しい worker pool を作る、またはこの経路だけ復元後に通常ロード初期化相当を通す必要があります。

補足: `FacetItemKind::Unknown` は v2.4 以降から v2.3 へ戻す前方防御としては整合しています。v2.2.0 への downgrade 非互換は docs の整理どおりリリースノート扱いが前提です。`git diff --check` は対象ファイルで通りましたが、read-only sandbox のため `cargo test` / build は未実行です。