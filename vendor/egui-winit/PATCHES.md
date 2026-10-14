# egui-winit 0.33.3 のローカル変更

I2 段 1（2026-10-14）: IME が処理したキーの到着を調べる診断用 trace。
段 1 はキー・IME の変換、egui イベントの生成・消費、EventResponse を変更しない。
I2 段 2a（同日）: 実機記録に基づき、IME 処理済みキーの press を遮断する挙動変更を追加。

## 現在の修正：本家 0.34.0 #7967 と同じ（2026-10-14）

出典は [PR #7967](https://github.com/emilk/egui/pull/7967) と
[egui-winit 0.34.0 の実装](https://raw.githubusercontent.com/emilk/egui/0.34.0/crates/egui-winit/src/lib.rs)
（`try_on_ime_processed_keyboard_input`）。**egui 更新時はこの差分ごと消す**：
`vendor/egui-winit`、workspace exclude と path patch を削除し、本家へ戻す。

Windows の `allow_ime` が true の場合だけ、logical Process を遮断して
`State::pressed_processed_physical_keys: HashSet<PhysicalKey>` に記録する。
対応する release は logical が元のキーに戻るため、同集合から physical を remove して遮断する。
allow_ime=false の入力は集合を変更せず渡し、Process press のなかった physical の release も渡す。
本家と同じ条件順序で、追加の focus-reset / 期限は設けない。状態は State（viewport）ごとに所有する。
遮断時の EventResponse も本家同様 repaint=false / consumed=wants_keyboard_input とする。
Ime 変換と通常 Key / Text / clipboard producer、秘匿 trace は保持する。

KeyEvent は private field を持つため、その公開部品を受ける同名の状態遷移 helper に判定を置く。
`append_keyboard_input` はこの判定を全 producer の前で呼び、State と配送テストが同じ経路を通る。
旧純関数・配送テストを対応 release の遮断へ更新し、allow_ime=false、非対応 release、repeat、
複数 physical と sibling 状態の分離を追加検証する。移植全体の記録は
[IME 計画](../../docs/ime-process-keys-plan.md)。

## 出典・取り込み

- crates.io package: `egui-winit-0.33.3.crate`
- package SHA-256: `ec6687e5bb551702f4ad10ac428bab12acf9d53047ebb1082d4a0ed8c6251a29`
- upstream repository: `https://github.com/emilk/egui`
- `.cargo_vcs_info.json` の commit: `44cdd653e2317d300fb8a6c9c36b03f23991e803`
- upstream package path: `crates/egui-winit`
- 検証日: 2026-10-14

Cargo registry の archive の SHA-256 は、取り込み前の root `Cargo.lock` の checksum と一致。
archive 内の全 9 ファイルと展開済み registry source の bytes 一致を確認し、archive から取り込んだ。
registry 管理用 `.cargo-ok` と生成された `target/` は取り込まない。
root workspace の exclude と `[patch.crates-io]` に追加し、本体・ラボ・workspace メンバーの
推移的依存も同じ path の egui-winit を使う。package の version / dependencies / features は不変。

段 1 の原本からの変更は `src/lib.rs` の観測 hook。段 2a は同ファイルに純粋な
キー配送判定と単体テストを追加する。追加ファイルは
`src/ime_key_trace.rs`（診断実装と単体テスト）、本書、ライセンス本文 2 ファイル。
`Cargo.toml` / `Cargo.toml.orig` / 同梱 `Cargo.lock` も原本のまま保持する。
root `Cargo.lock` の変更は egui-winit の source / checksum の除去だけ。

## ライセンス

`MIT OR Apache-2.0` 宣言を保持する。原本 archive はライセンス本文を含まないため、
既存 `vendor/egui-wgpu` の本文を bytes 不変で同梱する（既存 egui の取り込みと同じ）。

| コピー元 | SHA-256 |
| --- | --- |
| `../egui-wgpu/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../egui-wgpu/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |

## trace の境界・出力形式

- `MIV_IME_KEY_TRACE=<絶対パス>` を最初の `State::new`（winit 統合の起動）で process 全体につき
  1 回だけ読む。未設定・空・相対パスなら完全に無効で、worker・ファイル I/O・イベントの走査・
文字列生成・時計読み取りは行わない。無効時も段 2a のキー配送規則は適用する。
- 全 State / viewport は同じ worker と単調時計を共有する。`ms` は最初の State 初期化からの
  経過 ms。最初の State 構築より前の区間は観測できない。`viewport` は対象 State の `ViewportId` を
  省略なしの 64-bit hex で出す（egui の Debug は 16-bit の省略形なので使わない）。
- `WindowEvent::KeyboardInput` / `Ime` の処理直前に時刻とイベント列の末尾位置を取る。
  入力処理後、新たに追加された実際のイベントだけを読み取る。変換結果の再計算はしない。
  synthetic press の無視、Cut/Copy/Paste の early return、IME Enabled の重複抑止も観測できる。
- 1 行 1 到着事象の UTF-8 テキスト。`egui=[...]` は追加順で、空なら `[None]`。
  Key の `repeat` は egui-winit が出した値（原本どおり false）であり、winit の repeat と区別する。
- text / Preedit / Commit / Paste は `bytes`（UTF-8 byte 数）、`empty`、`newline`（CR または LF の有無）
  だけを出す。Preedit の cursor は有無だけ。文字内容、cursor の位置、clipboard 内容は出さない。
  logical Character / Dead / Unidentified の内容、文字に対応する physical / egui Key、
  unknown な physical code もマスクする。
  Space は logical の NamedKey::Space も含めて文字キーと同じように秘匿する。
  logical NamedKey::Space は Character の長さ等だけの表記、physical / egui Key は文字キーの汎用表記へ
  そろえ、Space 固有のマスク名を残さない。logical NamedKey::Process は Process のまま残す。
  Enter / Escape / Tab / Process など非文字キーの種類は残す。
- UI thread はメモリ上で秘匿した行を生成して非同期 channel へ送るだけ。
  worker がファイルを create / append し、1 行ごとに write / flush する。open / write / flush / send /
  worker 起動の失敗は黙って以後の記録を停止する。再試行・復旧・終了時の完了待ちは設けない。
  強制終了や即時終了では未配送の末尾が残らない可能性があるため、再現後は少し待ってから終了する。

## ソースで確認した見立ての範囲

winit 0.30.13 `src/platform_impl/windows/keyboard_layout.rs:946` は `VK_PROCESSKEY` を
`NamedKey::Process` に変換する。同ファイル `:210` の `get_key` はこの非文字キーを優先して返す。
`src/platform_impl/windows/keyboard.rs:502-513` は lParam の scan code から physical key を作り、
scan code が 0 なら virtual key から補う。したがって physical が必ず元の Enter とはソースだけで
断定できない。本 trace が観測するのは winit の logical Key と変換済みの physical KeyCode、
egui への出力とイベントの到着順であり、raw VK / scan code は記録しない。
実機ログでは Process と physical KeyCode・egui Key の対応を確認する（文字キーと Space の種類は秘匿）。

egui-winit 原本 `src/lib.rs:1117` / `:1125` の変換には Process が無く、`:815` の
`logical_key.or(physical_key)` が physical Enter へ fallback する見立ては一致する。
これは発生条件のソース確認であり、実機で確認した範囲は次節に区別して記録する。

## 設計 lead の実機記録（2026-10-14、Google 日本語入力・この PC）

設計 lead が隔離データの本体とscratchpadのラボで操作した、修正前の段 1 trace。
本体の注釈本文欄とラボの歌詞欄の両方で、
変換確定の Enter・変換中の Escape・候補選択中の Tab・変換の Space の press は
logical=Named(Process)、physical は対応する KeyCode で到着し、egui-winit は対応する Key の
press を出していた。文字キーの press も logical=Named(Process) だった。
上記の非文字キーと Space の release は対応する NamedKey として到着し、Process ではなかった。
Process の press は、同じ ms の空 Preedit → Commit → Disabled より前に到着した。
IME が開いていても変換していないときの Enter は logical=Named(Enter) だった。
raw VK / scan code の観測結果は含まない。他の IME・PC まで一般化しない。
P2 修正後の trace は Space の種類を全経路で秘匿するが、logical=Named(Process) は保持する。

## 段 2a の旧キー配送規則（2026-10-14、移植元の履歴）

以下の release 通過・集合なしの判断は、冒頭の本家整合によって置き換えた。

`on_keyboard_input` が呼ぶ配送関数の入口で、`logical=NamedKey::Process` かつ press を
`should_forward_keyboard_input` の純関数で拒否する。repeat も同じ入口を通るため、
physical fallback、Text、Cut / Copy / Paste の全 producer より前に遮断される。
release は Process であっても渡し、追跡集合や新しい入力状態は持たない。
`EventResponse` と Ime イベント変換は変更しない。通常キー・非ラテン配列の physical
fallback と clipboard shortcut は維持する。

根拠は設計 lead が隔離データの本体注釈本文とラボ歌詞欄で記録した段 1 trace
（Google 日本語入力・この PC）。Process press の fallback が確定 Enter を改行にし、
本体の300ms除去が直後の通常Enterまで落としていた。native presenter の既存
`VK_PROCESSKEY` に egui key を割り当てない規則と一致させる。他IME・PCの実機確認は未実施。
trace は既定無効のまま残し、Process press の出力は実際の追加イベントが空なので
`egui=[None]` となる。段 1 の記録と取り込み履歴は上記に保持する。

単体テストは Process press 拒否、release 通過、通常 Enter / Esc / Tab / Space、
非ラテン配列の physical fallback と clipboard 判定を固定する。
workspace 外のため `scripts/test-full.ps1` に無 filter の lib test を追加する。
同梱 upstream lock の旧 image / tiff 依存は offline cache に無いため、manifest 単独ではなく
root graph の `cargo test -p egui-winit --lib` で、本体と同じ依存を使って全件実行する。

### 段 2a レビュー P3: 配送経路の回帰（2026-10-14）

`KeyEvent` は private field のため合成できない。`on_keyboard_input` は部品を取り出して
`append_keyboard_input` を呼ぶだけにし、遮断・キー変換・Key / Text / clipboard event の
追加をこの関数へ挙動不変で移した。テストは軽量な `RawInput` の実イベント列を検査する。
clipboard getter は遅延 closure とし、Process press / repeat と release では呼ばれないことも
検証する。winit の repeat は従来どおり egui の判定に委ね、出力 Key の repeat は false。
trace の begin / finish hook、出力と `EventResponse` は変更しない。

追加5件は Process press / repeat（Enter / Esc / Tab / Space / 文字・clipboardキー）、
Process release、通常キーの press / release・文字・非ラテンfallback、通常 Cut / Copy /
Paste（CRLF正規化、release）、空・取得不能clipboardを実配送経路で固定する。
全21件成功。配送関数内の `should_forward_keyboard_input` 呼び出しを一時的に削除すると
新しい Process 配送テストが失敗（20 passed / 1 failed、exit 101）し、戻すと全21件成功。
ログは `target/i2a-p3-{mutation,restored-tests}.log`。コミット・アプリ起動・build は行わない。

## 段 1 の検証履歴

root graph を使う `cargo test -p egui-winit --lib` で診断単体テストを実行する。
KeyEvent は winit 0.30.13 `src/event.rs:655` の private `platform_specific` field により
外部から安全に合成できないため、Process/Enter の元変換 helper と、キー metadata の射影を検証する。
IME は合成でき、追加イベントだけの観測・複数 viewport・非改変・秘匿を検証する。
writer の行ごとの flush、write / flush / channel 失敗時の停止も検証する。
実 KeyboardInput 到着と実 IME の順序の利用者報告は前節を参照。独立レビュー完了は主張しない。

Space 秘匿の P2 回帰は、press / release・text の有無・logical Process の有無と、
egui Key の key / physical_key の各経路で、Space の出力が他の文字キーの出力と一致することを確認する。
両回帰が修正前に失敗することを確認済み。変更は trace の射影だけで、入力イベント生成は変更しない。
P2 修正後の `cargo test -p egui-winit --lib` は全 12 件成功、`cargo fmt --check` と
vendor の rustfmt check も成功。ログは `target/i2-review-space-{before,tests,fmt}.log`。

2026-10-14 の検証: 上記診断単体 10 件、brief 指定の本体 core check / 共有 UI・ラボ・egui-ime /
本体 ime_focus・egui_focus_policy、root fmt と vendor rustfmt が成功。依存 graph は本体・全ラボで
本 path の一系統。release ラボのビルドも成功した。詳細ログは root `target/i2-*`。
追加全体 gate は App のサイズ制限失敗と capture_region テストの stack overflow で失敗し、
本体 lib は全件完走していない。本体 dev-runtime 更新は既存 core プロセスが出力 exe を保持するため
利用者の終了待ち。これらを本 trace の変更に混ぜて修正せず、全体 gate / 本体 build 成功は主張しない。
