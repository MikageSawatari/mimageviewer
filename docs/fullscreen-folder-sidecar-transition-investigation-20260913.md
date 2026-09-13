# Ctrl+上下のフォルダ移動とサイドカー復元待機の表示退行

2026-09-13。公開準備中の利用者報告による読み取り専用調査。製品コードの変更・実アプリ起動や入力は行っていない。

## 観測

画像フルスクリーンのCtrl+上下で激しくちらつく。利用者はv3.9.0でも発生すると報告。
通常プロファイルのログを `target/fullscreen-folder-transition-20260913/observed.log` に保存した。
SHA-256: `EAE15DC2C5E8582E75DA167211B288EC430151FE066C2954EE9414E69729ABC5`。

同ログの一例（プロセス相対時刻）:

- 67.432s: fullscreen-eguiからctrl_nav_forwardを受付。
- 67.449s: presentation close、load_folder、旧worker取消。
- 67.451s: sidecar restore request=8200、context=0、generation=146開始。
- 67.452s: SLOW FRAMEにgrid=0.7msを記録。
- 67.465s: open_fullscreen idx=0。
- 67.468s: sidecarによる入力保持を解除。

周辺の移動でも復元ownerは約17〜35ms。この例は100ms後のmodal表示が原因という説明には合わず、待機中に一覧描画へ戻った記録がある。ログは描画内容の動画証拠ではないため、見え方そのものの確認は利用者報告に基づく。

## コードの因果

`9c9df532c`（v3.9.0に含まれる）で、フォルダのitems差替え後のサイドカー取り込みを非同期化した。
`defer_sidecar_restore_fullscreen` は再open要求をsidecar owner内の `deferred_fullscreen` に保持し、terminalで再開する。
しかし `ui_fullscreen.rs::fs_nav_deferred_reopen_wait_active` はPDF列挙待ちと書庫変換待ちのみを判定し、サイドカーによる再open待ちを含まない。v3.9.0と現行の両方に同じ欠落がある。
`poll_fs_nav_lock` はitems generationが進んだのにfullscreen_idxがNoneで、この待機判定がfalseなら、navigation lockと保持画像を解除する。
同じ待機判定はviewport維持などの表示継続経路にも使われる。

旧同期取り込みでは一フレームの中で通過していたclose〜reopenの区間が、非同期化後はフレームをまたぐ。その新しい待機ownerと既存の表示継続判定が整合していない。
旧XMP取り込み撤去で長い操作待ちは減ったが、この表示継続の欠落は残っている。

## 修正に必要な条件

- サイドカー待機を一律で表示保持するのではなく、再open intentを持つ現在context/generation/itemのownerから待機を導出する。
- 表示保持・viewport維持・一覧抑止が同じ遷移の事実を参照する。
- 新しい補正適用済み画像が表示できるまで前の表示unitを保持し、キャンセル・切替・失敗時は永続保持しない。
- 通常一覧の復元や別contextの復元で無関係の窓を保持しない。PDF/ZIP・検索スコープ・兄弟移動など共通入口を確認する。
- 新しい待機フラグ、固定delay、modal抑止による症状回避にはしない。47秒停止の修正とデータ保護を維持する。
- 非同期待機をまたぐnavigation状態テストを追加し、実機検証は別途了承を得た枠で行う。

最大化時の一回の画像引伸ばし（§1.227）とは別の問題として扱う。公開前修正を推奨する。

## 独立照合

Sol/xhighの独立担当も、導入前後のコードとログを照合し、非同期sidecar再open待機をnavigation保持側が認識しない互換欠落を確認した。修正時はcontext・generation・item identityを照合し、単なるApp-globalなsidecar_active判定を採用しないことに設計担当と合意した。
