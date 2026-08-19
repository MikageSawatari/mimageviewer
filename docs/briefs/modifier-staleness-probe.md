# 修飾キーずれの計装 (原因確定用、挙動は変えない)

着手前に [CLAUDE.md](../../CLAUDE.md) の「バグ修正の一般原則」と
[docs/keymap-spec.md](../keymap-spec.md) を読むこと。**これは計装だけの作業で、
production の挙動 (consume / dispatch / guard / 表示) は 1 行も変えない。**

## 1. 何が起きたか (2026-08-20、利用者環境で観測)

約 5 分間、**キー操作だけが何も起こさず、マウス操作は正常だった**。利用者は
マウスで HUD からフルスクリーンを閉じて凌いでいた。動画を再生した前後で解消した。

perf log (`perf_events.jsonl`) に時刻付きで残っている。以下は同一セッション:

| 記録 | 意味 | 時刻 |
| --- | --- | --- |
| `input/fs_close_key` | **キー**でフルスクリーンを閉じた | …1939.6 → **381 秒の空白** → 2320.9… |
| `input/fs_close_click` | **マウス**で閉じた | 2055.5〜2255.2 に **12 件** (全 17 件中) |
| `input/grid_key` | 一覧の矢印キーが選択を動かした | …1992.5 → **300 秒の空白** → 2292.2… |

利用者の追加証言が 3 つあり、いずれも**修飾キー (おそらく Ctrl) が押されっぱなし扱い**
だったとすると辻褄が合う:

1. <kbd>BS</kbd> を押すと「個別設定なし」のトーストが出た
   = <kbd>Ctrl</kbd>+<kbd>BS</kbd> の `FsClearAdjust`「現在の画像の補正を解除する」
2. <kbd>Esc</kbd> は無反応 = `consume_key` が `Modifiers::NONE` の**完全一致**を要求する
3. **ホイールでページ送りではなくズームした** = <kbd>Ctrl</kbd>+ホイール。
   キーとは別経路で修飾キーを読むので、独立した 3 つ目の証拠になる

## 2. なぜ今のログで確定できないか

- どの perf event にも**修飾キーの状態が入っていない**。
- `FsClearAdjust` は **event を 1 つも出さない**ので、利用者が見たトーストは記録に残らない。
- `input/fs_wheel` は「ページ送りが起きたとき」だけ出る。ズームした場合は**無音**なので、
  「ホイールが来なかった」のか「来たがズームになった」のか区別できない。

つまり、現在の計装は**症状の否定形しか記録していない**。肯定形を足す。

## 3. 入れるもの

### Event A — `input/modifier_probe`

egui が持つ修飾キー状態と、OS 直読みの状態を**並べて**記録する。
OS 側は既存の `keymap::modifier_held_via_os(permit, ModKind::{Ctrl,Shift,Alt})` を使う。
これは production では `GetAsyncKeyState` へ落ちる独立した probe である
([src/key_input.rs](../../src/key_input.rs) の `physical_key_down`)。

フィールド:

- `viewport`: `"main"` / `"fullscreen"`
- `focused`: このビューポートがフォーカスを持つか
- `egui_ctrl` / `egui_shift` / `egui_alt` / `egui_command`
- `os_ctrl` / `os_shift` / `os_alt` — permit が無くて読めない場合は**値を捏造せず**
  `permit=false` を記録してフィールドを省く
- `diverged`: egui と OS が食い違うか (導出値)
- `keys`: この frame で観測したキー名 (押下/離しとも、短い配列)
- `wheel`: この frame にホイール入力があったか
- `trigger`: `"key_event"` / `"wheel"` / `"heartbeat"`

### Event B — `input/modified_action`

「意図した操作の代わりに発火し得るアクション」が成立したとき、同じ修飾キー snapshot を
付けて記録する。最低限:

- `FsClearAdjust` (<kbd>Ctrl</kbd>+<kbd>BS</kbd> / <kbd>Q</kbd>) ← 利用者が見たトースト
- `FsBackToList` (<kbd>BS</kbd>)
- `FsClose` と Esc による close
- `GridParentFolder` (<kbd>BS</kbd> / <kbd>Alt</kbd>+<kbd>↑</kbd>)
- フルスクリーンの**ホイールによる倍率変更** (<kbd>Ctrl</kbd>+ホイール)

## 4. 発火条件 — ここが本題

**どの発火条件も、調査対象である修飾キーの値に依存させてはならない。**

この禁則は一般論ではない。同種の調査 (音声モードの Z) で、**3 回続けて**
「発火条件が調査対象の probe だけで決まっていたため、実ログに 1 行も出ない」計装を
出荷している。1 回目は peek gate、2 回目は 4-probe candidate gate、3 回目は
下流の summary が空。同じ失敗を 4 回目にしない。

Event A の発火条件は次の 3 つで、いずれも修飾キーの値と無関係:

1. この frame にキーイベントがある (押下・離しの別を問わない)
2. この frame にホイール入力がある
3. **無条件 heartbeat**: 上の 2 つが無くても 2 秒に 1 回出す

### heartbeat が repaint を要求してはいけない

`scripts/check-idle-health.ps1` は静止中の完全 sleep を要求する (毎リリース必須ゲート)。
heartbeat のために `request_repaint` / `request_repaint_after` を呼ぶと**このゲートが落ちる**。
**すでに起きている frame に相乗りするだけ**にすること。静止中に frame が来ないなら
event が出なくてよい (静止中は利用者もキーを押していない)。

## 5. 出力量

利用者の 1 セッションで perf log が 467MB に達している。追加分は小さく保つ:

- Event A は 1 frame あたり最大 1 件 (viewport ごと)
- heartbeat は 2 秒に 1 回 = 1 時間で約 1800 件
- **毎 frame 出す形にはしない**

`crate::perf::is_enabled()` が false のときは何も出さない。

## 6. これで何が分かるか

再発 1 回で 3 つに切り分かる。

| 観測 | 結論 |
| --- | --- |
| キー/ホイールは届いていて `egui_ctrl=true` / `os_ctrl=false` | **egui 側の修飾キー状態が stale**。前例あり (FS ビューポートで 2 回) |
| 両方 `true` | 物理的に Ctrl が押されている (キーの固着 / 他アプリのフック / RDP) |
| `input/modifier_probe` にキーが 1 件も現れない | 解釈ではなく**到達**の問題。入力所有権 / permit 側を見る |

いずれの場合も、次の修正方針が決まる。**この段階では直さない。**

## 7. テスト

- `next` 相当の純関数 (egui / OS の値のペア → `diverged`) の真理値表。
- 発火条件が修飾キーの値に依存しないことを固定する: 修飾キーが全て false / 全て true の
  どちらでも、キーイベントのある frame と heartbeat frame で event が出ること。
- heartbeat が repaint を要求しないこと (`request_repaint` を呼ばない)。

## 8. 再発時の読み方 (次の担当者向け)

`%APPDATA%\mimageviewer\logs\perf_events.jsonl` を開き、まず症状の窓を特定する。

```python
import json
P=r"...\perf_events.jsonl"
for l in open(P, encoding='utf-8', errors='replace'):
    if '"fs_close_key"' in l or '"fs_close_click"' in l or '"grid_key"' in l:
        e=json.loads(l); print(round(e['t'],1), e['kind'])
```

`fs_close_key` が途切れて `fs_close_click` だけが並ぶ区間が症状の窓。その窓の
`input/modifier_probe` を見る。**perf log は起動時にローテートする**ので、再起動して
しまった後は `perf_events.1.jsonl` 以降を見ること (5 世代)。
