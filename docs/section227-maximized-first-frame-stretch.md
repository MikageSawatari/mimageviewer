# §1.227 最大化した linked 別窓の初回フレーム引き伸ばし

## 状態と判断 (2026-09-12)

利用者がメイン窓と linked F12 画像別窓の両方をタイトルバーから最大化し、
画像を開いた直後だけ画像と HUD 全体が横に引き伸ばされる場合があると報告した。
これは先に許容された F12 切り替え時の単発のメイン窓前面化とは別の現象である。

録画 `C:\Users\mikag\Videos\2026-09-12 17-00-32.mkv`
(SHA-256 `D4F0DE89FB3BEB535CF989C2ACB41331EA7C0AFF21D28AA6556E5E800D671FAD`)
では、17:00:35.527 と 17:00:35.561 の 2 フレームで画像と HUD がともに横伸びし、
17:00:35.594 から正しい fit に戻る。30 fps の録画のため精密な実時間は断定せず、
録画上は約 67 ms の観測とする。利用者はその後「1 回だけ」とし、
修正リスクが高ければ保留して次リリース対象から外す判断を希望した。

## ログとコードの証拠

通常ログの対応操作はセッション経過 `3967.476s`、`window_id=123` である。
保存 placement は `1180×1140, maximized=true` だが、新しい host は
`visible=false, rect=(1136,107 1792×1766)` という復元サイズで登録された。
その時点は `display_ready=false, cache_state=none, thumb_ready=true, pending=true` であり、
次の UI frame まで `74.1ms` 空いた後に `DisplayReady(LiveCache)` となった。
必要行だけの固定スニペットは
`target/section227-maximized-first-frame-stretch-20260912/mimageviewer-window123.log`
(SHA-256 `5D7E349F47B2A03A28503C6136BBA03FA94487B3723D8C6B7C34F2C03E814CCB`) に保存した。

[`src/ui_fullscreen.rs`](../src/ui_fullscreen.rs) の `DetachedViewportBuilderVisibility::Hidden` は、
Windows で hidden builder に maximize も与えると `ShowWindow(SW_MAXIMIZE)` が host を表示するため、
builder には復元用 inner size だけを与えて maximize を外す。
その後、初回の child viewport を描画した後に
`send_detached_viewport_visible_commit` が `Maximized(true)` → `Visible(true)` を queue する。
vendored eframe の通常経路は `paint_and_update_textures` の後に viewport command を処理する。

したがって、正常に submit/present した場合でも、その frame は最大化前の復元サイズである。
続く maximize 自身が HWND を表示し client size を変え、resize 後の新しい描画が届くまで
古い backbuffer が拡大されるなら、録画の「画像だけでなく HUD 全体が 2 フレーム横伸び」を
説明できる。ログの 74.1 ms も録画の約 67 ms と整合する。

## 因果の限界と保留理由

この surface/client-size 不一致は有力な根因候補だが、GPU present から DWM composition までの
trace や、最大化後の各 surface generation / physical size を相関させた診断は取っていない。
よって DWM が伸長したと最終断定はしない。利用者の「メインでも」という観測も、
linked host の上記シーケンスだけでは原因を確定できない。

局所的な maximize の順番入れ替えは、hidden builder を早く表示して白い client や
最大化アニメーションを露出させる旧不具合を戻す。repaint、遅延、強制 fit は
最大化後の描画完了 ACK にならない。根本的に閉じるには、exact host incarnation、
maximize による nonzero surface generation / physical size、そのサイズでの successful present receipt を
typed transaction として所有し、DWM cloak 等の host-scoped 可視化と結ぶ必要がある。
close/cancel/error/stale ACK/`SurfaceAbsent` も含む backend plumbing が必要な中〜高リスク作業である。

今回は製品実装を行わず、旧 §1.115 の完了判断と分けて §1.227 として保留し、
次リリース対象から外す。
