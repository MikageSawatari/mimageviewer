結論: クリーン。今回対象の 4 件について、P1/P2/P3 の指摘はありません。

確認メモ:
- `src/audio_decode.rs:543`: flush は `delay().output + SWR_OUTPUT_SAFETY_SAMPLES` で確保し、`Vec` 追加前に `try_reserve` 済み。`delay==0` / `samples==0` / flush error で抜けるため無限ループ化は見当たりません。
- `src/app.rs:1099`: fallback 採用だけ `visible && !iconic && rect_ok` に絞られており、`NoChange` のまま次フレーム再試行される経路もあります。created 側を無条件に残す意図とも整合しています。
- `src/keymap.rs:543`, `src/keymap.rs:6291`: `to_vk` は `KeyName` 全体に対して exhaustive で、Windows KeyHold の OS 直読み専用化は fullscreen viewport の stale egui 状態対策と整合します。`NumpadEnter` と主 Enter の VK 衝突は KeyHold 限定の既知制約として許容範囲で、press/edge 系の物理判定は維持されています。
- `src/gamepad.rs:319`, `src/app/gamepad_input.rs:1946`: neutral gate は West 保持中の suppress / suppressed release に限定され、West 非保持の通常 suppress では立たないため、既存の「ポップアップ後にアナログ操作が継続できる」設計は壊していません。

検証は read-only sandbox のため `cargo test` 未実行です。今回は差分と周辺コードのレビューのみです。