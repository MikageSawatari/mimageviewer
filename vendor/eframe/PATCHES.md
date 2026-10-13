# Native入力のproducer観測API（2026-10-13）

本書は今回追加したAPIの台帳であり、以前からあるmIV fork全差分の一覧ではない。

`App::native_input_events(viewport_id, NativeInputSource, &[egui::Event])`は、
既存egui-winit変換後の「今回のWindowEventが生成したsuffix」を読み取る補助APIである。
変換は一回だけ実行し、元EventResponseをそのまま返す。空suffixでも通知する。
MouseInput Left Released / Touch End(contact ID＋physical client位置f64 bits) / Otherを区別し、既存viewport IDを渡す。
位置情報がなく省略されたmouse releaseを、後続の有効なクリックへずらして対応付けないために必要。

既定callbackはno-op。callbackはviewport stateが借用中のためdata-only/nonreentrantとし、
描画・viewport生成/操作・入力変換/注入をしない。mIVはROOTの単一native-input ownerへ注釈を渡す。
raw_input_hookは完成注釈と最終RawInputのrelease序数を確認してからctx.runへ渡す。
wgpu/glowの両経路で同じAPIを使い、既存predicate/描画scheduler/入力消費を変更しない。
web backendには追加しない。eguiのclicked_primary_release_indexの台帳は隣のegui/PATCHES.mdにある。

5件のheadless回帰でsource/contact ID・位置bits（符号/精度を保持）、suffix、空suffix、既定callbackと入力保持を検査する。
filterは`native_input_observer_tests`。ライセンス/上流版数/既存fork機能は変更しない。

coreのnative pendingはこのcallbackでreadyへ移譲し、ROOT raw_input_hookはreadyだけを射影する。
callback間のinline frameで後続inputのpendingを退役しない。observer自体は保存状態を追加しない。
