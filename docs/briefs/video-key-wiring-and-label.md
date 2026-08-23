# 動画の新設キーが egui 経路に配線されていない / コマンド名が途中で切れる

利用者報告 (2026-08-23、v3.2.0 出荷前の実機確認)。**2 件。どちらも動画アップスケールで
新設された操作まわり。**

## 1. T キー (`VideoScaleFilterNext`) が効かないことがある

### 観測

再現手順: フル機能設定で動画を別ウィンドウで開く → メインウィンドウで別の PDF を開く →
動画ウィンドウをクリックしてアクティブにする → **T が効かない**。開いた直後は効く。

`MIV_KEY_DEBUG=1` の実ログで経路が確定している:

```
効く:   raw native-video  vk=0x54 → consume VideoScaleFilterNext [FsVideo] via=native_match
効かない: raw main        vk=0x54 → [fs-key] source=fullscreen ... T:down → (consume 行なし)
同状況の Space: raw main  vk=0x20 → consume VideoPlayPause [FsVideo] via=consume_action  → 動作
```

**コンテキストは両方とも `[FsVideo]` で正しい。**違うのは解決経路で、
キーが egui 側 (main) に届く状況では `consume_action` 経路が使われる。

### 原因

`VideoScaleFilterNext` は**リポジトリ全体で native 経路の 1 箇所にしか存在しない**:

- [native_video.rs:7935](../../src/app/native_video.rs:7935) `matches_vk_action(...)` のみ

一方、効く操作は egui 側にも配線されている:

- [ui_fullscreen.rs:18651](../../src/ui_fullscreen.rs:18651) `consume_action(ctx, KeyAction::VideoPlayPause)`
- 同 18678 `VideoBookmark` / 18691 `VideoLoop` / 18737 `VideoVolumeUp` ほか

**キー所有権 (§1.111) の問題ではない。**新設操作の配線漏れ。凍結ルールにも触れない。

### やること

- `VideoScaleFilterNext` を `consume_action` 経路にも配線する。
  既存の動画操作と**同じ場所・同じ形**に置く (新しい仕組みを作らない)。
- **`VideoAnime4kRemeasure` など、動画アップスケールで新設された動画操作すべてを棚卸しする。**
  同じ穴が他にもあるはず。1 件だけ直して終わりにしない。
- 動作は既存と同じ (拡大方法を順送り + トースト)。挙動を変えない。

### ⚠️ テスト — 同型の漏れを機械的に捕まえる

**個別に 1 件ずつ assert するのではなく、「native 経路にしかない動画操作」を列挙して
空であることを検査するテストを書く。** これが無いと次の新設操作で同じ漏れが再発する。

- 例: `FsVideo` context の `KeyAction` を `ALL_ACTIONS` から取り、
  native 側 / egui 側の配線有無を突き合わせる。
- 意図的に片方だけの操作がある場合は、**理由付きの allowlist** にして
  「なぜ片側だけでよいか」をコメントに残す (`ime_focus` の raw TextEdit 検査と同じ形)。
- 実装が難しければ、代替案と理由を報告に書く。**テスト無しで済ませない。**

## 2. コマンド名が「〜を順」で切れて表示される

### 観測

環境設定のキー割り当て一覧で「**動画の拡大方法を順**」と表示される
(正しくは「動画の拡大方法を順に切り替える」)。

### 原因

[pages.rs:1552](../../src/ui_dialogs/preferences/pages.rs:1552) 付近の
`compact_operation_label` が一覧用に語尾を機械的に削っており、
`"に切り替える"` に一致して削ると「順」が宙に浮く。「順に」は副詞で「切り替える」に係るため。

**全 `KeyAction` を機械検査した結果、明確に壊れているのは 2 件だけ:**

| action | 元 | 現在の表示 |
| --- | --- | --- |
| `VideoScaleFilterNext` | 動画の拡大方法を順に切り替える | 動画の拡大方法を順 |
| `VideoLoop` | 動画のループ方式を順に切り替える | 動画のループ方式を順 |

他に 60 件が `"〜にする"` → `"〜に"` になるが (例「サムネイル列数を1列に」)、
これは簡潔なラベルとして読めるので**意図された動作**。**触らないこと。**

### やること

- 上記 2 件が一覧で意味の通る日本語になるようにする。
  **説明文 (`KeyAction::description`) を直すか、削る語尾の扱いを直すかは選んでよい。**
  ただし他の 60 件の表示を変えないこと。
- 選んだ方法と、なぜ他へ波及しないかを報告に書く。
- **表示文字列だけの修正。**キー割り当てや動作を変えない。

## 3. 制約

- **時間窓・sleep・retry で吸収しない。**
- detached / viewport 述語には触らない (触る必要が出たら止めて報告)。
- 既定のキー割り当てを変えない。

## 4. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo test --test ui_snapshot` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `python scripts/check_ui_glyphs.py` が 0 件
- **報告に、棚卸しで見つかった他の漏れ / ラベル修正の方法と波及範囲**を書く
