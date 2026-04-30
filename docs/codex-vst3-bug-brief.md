# VST3 機能の残課題 4 件 — Codex への調査依頼

このファイルを Codex GUI でそのまま投入してください。
**回答も Markdown で返してもらう** ように依頼しています。
回答先: `docs/codex-vst3-bug-answer.md` (Codex がここに書き出す)。

---

## 依頼内容

mIV の VST3 機能で、私 (Claude) がここ数日修正を試みたが直し切れない不具合が
4 件あります。Rust 本体 + 別プロセスの C++ bridge `mimageviewer-vst3-host.exe`
構成で、bridge にプラグインをロードし mIV プロセスの host window に plugin の
child window を attach する設計です。

各不具合について「**バグの場所と直し方**」を **コード読解ベースで** 提示して
ください。`docs/codex-vst3-bug-answer.md` を新規作成し、優先度 (P1/P2/P3) と
ファイル + 行番号 + 修正案を箇条書きで書いてください。

リポジトリは `C:/home/mimageviewer/` です。

---

## 設計の要点 (前提)

- **host window**: mIV プロセス側で `src/video/dsp/gui.rs::run_gui_thread` が
  spawn する専用スレッドで `CreateWindowExW` する。HWND を bridge にクロス
  プロセスで渡す。
- **plugin child**: bridge プロセス内で plugin の `IPlugView::attached(host_hwnd)`
  によって、host window の child として作られる。
- **永続 GuiHost**: ユーザー要望で「DAW 並みの高速トグル」のため、show/hide
  ごとに createView/removed を呼ぶのではなく、**初回 show でだけ** create + attach し、
  以降は ShowWindow(SW_HIDE / SW_SHOWNA) で可視性だけ切替える設計。
- **WS_EX_TOPMOST**: 通常時は OFF。フルスクリーン動画再生中だけ動的に
  `SetWindowPos(HWND_TOPMOST)` で持ち上げる (= 通常時は popup menu 等の
  フォーカス問題を避けるため)。`set_all_guis_topmost(bool)` がそれを行う。

---

## 不具合 1 (P1): プラグイン GUI の z-order が再表示で保たれない

ユーザーは複数 (3-4 個) のプラグイン GUI を開いて手で並べる。VST 管理パネルの
toggle で **一斉 hide → 再 show** すると、ユーザーが手で配置した z-order が
保たれず、初期順 (= slot 順) に戻ってしまう。

**試した対策**: `show_slot_gui` の再表示パスから `bring_to_front`
(= `SetWindowPos(HWND_TOPMOST)`) を撤去し、`set_window_visible(SW_SHOWNA)`
だけにした。SW_SHOWNA は仕様上 z-order を変えないはず。

**疑い**:
- フルスクリーン中は `set_all_guis_topmost(true)` が slot 順で
  `SetWindowPos(HWND_TOPMOST)` を呼ぶので、そこで z-order が壊れる
- それとも別の場所で何か z-order を変えている?

**関連コード**:
- `src/video/dsp/mod.rs::DspBridge::show_slot_gui` (再表示パス、~340-400 行目)
- `src/video/dsp/mod.rs::DspBridge::set_all_guis_visible`
- `src/video/dsp/mod.rs::DspBridge::set_all_guis_topmost`
- `src/video/dsp/gui.rs::set_window_visible`
- `src/app.rs` のフルスクリーン遷移ハンドラ (`vst3_was_fullscreen` で検索)

**質問**:
1. 既存コードのどこで z-order が壊れている?
2. 修正案: 既存 z-order を `GetWindow(GW_HWNDNEXT)` 等で snapshot し、
   topmost 切替後に **bottom-to-top の順** で `DeferWindowPos` する方針で
   問題ないか?

---

## 不具合 2 (P1): SSL Meter Pro 等の右クリックメニューが即閉じる

プラグイン GUI 内で右クリックすると、ポップアップメニューが一瞬出てすぐ消える
(= 視認できないレベル)。SSL Meter Pro で確実に再現。

**試した対策**:
- `WS_EX_TOPMOST` は通常時は OFF にした (= popup の owner-foreground 要件を
  満たすため)
- `WM_PARENTNOTIFY` を host wndproc で受けて、`WM_LBUTTONDOWN` /
  `WM_RBUTTONDOWN` 時に `SetForegroundWindow(host_hwnd)` を呼ぶ handler を追加

→ どちらも改善せず。

**疑い**:
- plugin の child window が `WS_EX_NOPARENTNOTIFY` を持っており、
  `WM_PARENTNOTIFY` がそもそも親に届いていない?
- それとも SetForegroundWindow が cross-process restriction で reject されて
  いる? (mIV は host を作ったプロセスなので資格はあるはずだが)
- あるいは popup の owner が plugin's child window で、その owner が
  top-level window でないと `TrackPopupMenu` がうまく動かない仕様?

**関連コード**:
- `src/video/dsp/gui.rs::wndproc` の `WM_PARENTNOTIFY` ハンドラ (= ~170 行目)
- `src/video/dsp/gui.rs::create_window` のスタイル設定
- `crates/vst3-host/src/plugin_loader.cpp::PluginLoader::show_gui` の
  `view->setFrame(plug_frame_)` + `view->attached(hwnd, kPlatformTypeHWND)`

**質問**:
1. 右クリックメニュー即閉じの **構造的な原因** は何か?
2. 修正案: bridge 側で host window を subclass して `WM_RBUTTONDOWN` を
   intercept する? それとも mIV 側 host で `WM_MOUSEACTIVATE` を返す?
   あるいは plugin's child window 自体を subclass する手段は?

---

## 不具合 3 (P2): フルスクリーンで VST ボタン 2 回目押下時に plugin GUI が表示されない

シナリオ:
1. mIV を動画フルスクリーン再生中
2. VST ボタン #1 押下: panel + 全 plugin GUI が **正しく**表示
3. VST ボタン #2 押下: panel + 全 plugin GUI が hide
4. VST ボタン #3 押下: **panel は出るが plugin GUI が出ない**

**永続 GuiHost 設計**: GuiHost は drop されない、window は CreateWindowExW で
作成済み、hide は SW_HIDE のみ、show は `gui_hwnd != 0` の早期 return パスで
SW_SHOWNA。

**疑い**:
- フルスクリーン遷移ハンドラ (`set_all_guis_topmost(true)`) が WS_EX_TOPMOST
  を変えると window state が壊れる?
- WM_DESTROY が想定外のタイミングで来て window が破棄されている?
- 何らかの理由で `gui_hwnd` フィールドがリセットされている?

**関連コード**:
- `src/video/dsp/mod.rs::DspBridge::show_slot_gui` 再表示パス
- `src/video/dsp/mod.rs::DspBridge::hide_slot_gui`
- `src/video/dsp/gui.rs::run_gui_thread` のメッセージループ
- `src/app.rs` の `vst3_was_fullscreen` 遷移ハンドラ
- `src/ui_fullscreen.rs` の VST ボタン押下処理

**質問**:
1. 2 回目以降の表示で plugin GUI が出ない原因は何か?
2. 修正案を具体的に。

---

## 不具合 4 (P2): Insight2 リサイズで内容が暴れる (改善はしたが完全には消えない)

ホストウィンドウをドラッグでリサイズすると、plugin (Insight2) のレンダ内容が
遅れて追従し、ドラッグ停止と内部リサイズの間にラグがあって視覚的に「暴れる」。

**試した対策**:
- `PlugFrame::resizeView` でフィードバックループ抑止: `notify_host_resize`
  で `view->onSize` を呼ぶ前に `last_user_resize_tick_` を立て、その後 250ms
  以内の `resizeView` コールバックは `SetWindowPos` スキップ (= タイムスタンプ
  式抑止) を実装。
- `WS_CLIPCHILDREN` + `hbrBackground = NULL` で flicker は減ったが、内容追従の
  ラグは残る。

**疑い**:
- 250ms の窓では Insight2 の非同期 resizeView を取りこぼす
- もしくは `WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE` ベースに切替て、
  drag 完了まで `view->onSize` を遅延すべき?
- それとも sync invalidate (= 子 window の即時再描画要求) が必要?

**関連コード**:
- `crates/vst3-host/src/host_app.cpp::PlugFrame::resizeView`
- `crates/vst3-host/src/plugin_loader.cpp::PluginLoader::notify_host_resize`
- `src/video/dsp/gui.rs::wndproc` の `WM_SIZE` ハンドラ (Rust 側でドラッグ
  サイズを bridge に転送)

**質問**:
1. 完全に「暴れない」ようにする最善のアプローチは?
2. WM_ENTERSIZEMOVE/WM_EXITSIZEMOVE 切替 vs タイムスタンプ式の延長 vs 別のもの?

---

## 回答フォーマット

以下のテンプレートで `docs/codex-vst3-bug-answer.md` を作成してください:

```markdown
# Codex 回答: VST3 不具合 4 件の調査結果

## 不具合 1 (P1): z-order
- 場所: ファイル名:行番号
- 原因: ...
- 修正案: ...

## 不具合 2 (P1): 右クリックメニュー
- 場所: ...
- 原因: ...
- 修正案: ...

## 不具合 3 (P2): 2 回目 show fail
- 場所: ...
- 原因: ...
- 修正案: ...

## 不具合 4 (P2): Insight2 振動
- 場所: ...
- 原因: ...
- 修正案: ...

## 全体構造への提案 (任意)
- ...
```
