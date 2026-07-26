静的レビューのみです。read-only 環境のためビルド/テストは未実行です。

[P2] KeyHold 割当で一部の物理キーが効かない / 別キーとして効く
- 場所: src/keymap.rs:6267
- シナリオ: `FsZoomMode` や各 `*SpacePan` を `NumpadEnter` / `NumpadAdd` / `Yen` などに割り当てると、保存は通るが hold が成立しない。逆に `Numpad1` などは `egui::Key::Num1` に畳まれるため、テンキー割当なのに上段数字キーの押下/高速タップで反応しうる。
- 根拠: `validate_for_trigger(KeyHold)` は「修飾なし通常キー」なら許可するが、`key_held_chord` は `KeyName::to_egui()` が `None` のキーを即 false にする。`take_key_hold_edges` も `to_egui()` だけで対象キーを作るため、同フレーム押下+離し救済も働かない。一方 Press 系は `matches_win32` で scan/extended を見ており、ここだけ物理キー契約が崩れている。
- 確度: 高

[P2] X リング中にモーダル/IMEでブロックされると、保持中スティックが解除後に通常操作へ漏れる
- 場所: src/gamepad.rs:324
- シナリオ: X を押してリングを出し、左スティックを倒したまま IME/モーダル/FS コンテキストメニュー等で gamepad dispatch がブロックされる。その間に X を離す、またはブロック解除後もスティックを倒したままにすると、次の許可フレームでページ移動/動画シーク/グリッド移動が発生しうる。
- 根拠: ブロック中は `handle_gamepad_input` が action dispatch を捨てて `suppress_pending_actions()` だけ呼ぶ。ここでは `west_tap_suppressed` と方向クリアのみで `require_directional_neutral()` を立てない。さらに release がブロック中に処理されると `finish_gamepad_west_release()` 自体が呼ばれず、ブロック解除後は `dispatch_gamepad_analog()` が `west_ring_active == false` として通常アナログ操作へ進む。release が許可後に来ても `WestReleaseOutcome::Suppressed` は neutral gate を立てない。
- 確度: 高

問題なしとして見た観点:
- `KeyAction` の `ini_name` / parse / `ALL_ACTIONS` / default chord validation は既存テストで網羅され、新規アクションの明らかな抜けは見つかりません。
- Press 系の `consume_action` / `consume_action_no_repeat` / native `matches_vk_action` は Win32 edge と `ctx.wants_keyboard_input()` を通しており、KeyHold 以外の物理キー分離は概ね保たれています。
- `ring_shortcut.rs` の設定 sanitize、旧 `gamepad_ring_enabled=false` の正規化、右ドラッグ short tap の既存メニュー復元、picker の context/anchor stale close は問題なしに見えます。
- ゲームパッド切断時は `gamepad_state.clear()` で保持/リピート/アナログ状態を落としており、切断由来の stale hold は見つかりません。