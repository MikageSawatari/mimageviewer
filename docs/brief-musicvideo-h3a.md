# 実装 brief: H3a 解析の契約（音楽ビデオ）

担当: Codex GPT-6.1 Sol / high（実装・テスト）。設計・検収: ClaudeCode Opus 5.5。独立レビューは別担当が行う。
作業場所: `C:\home\mimageviewer-musicvideo`（ブランチ `musicvideo`）。報告は日本語。

## 1. 正本

`docs/music-video-export-plan.md` の「#### D3 の改訂 — H3a 解析の契約」節が正本。実装前の独立レビューを
3 回通して ACCEPT 済み。本 brief は作業範囲と確認方法だけを書く。契約の内容は正本を読むこと。
前提がコードと食い違ったら、実装で吸収せず止めて報告する。

## 2. 作業範囲

1. `music-core`: FFT 窓の中心を更新間隔の格子に揃える解析方式を、選べる構成として追加する（契約 3）。
   既定の方式（本体の音楽ビュー）の挙動は変えない。既存の music-core テストはそのまま通ること。
   論理長 L を持つ PCM の view（`samples` と `logical_frames`）を入力にできるようにする（契約 1）。
2. `musicvideo-core`:
   - 出力フレームの格子型（有理数 fps、`frames = round(duration·fps)`、1 未満の拒否、`time(n)`、`index_at(p)`）
     を置く（契約 2・5）。書き出し driver の `rational_frame_rate` とフレーム数の計算はここへ移す。
   - 解析タイムライン（契約 4）: 型で区別した「未処理／フレーム c まで処理済み」状態、step 関数 1 つ、
     1 秒ごとのチェックポイント、型付きの key（ビット比較）、区間の区切りでの取消。
     `SpectrumSource` の時刻ベースの API（後退・250ms 超の飛びでのリセット、同時刻キャッシュの旗）は、
     タイムラインへ置き換えるか内部に閉じ込め、音楽ビデオの経路から時刻ベースの呼出しを無くす。
   - 較正（契約 6）: 要求出力尺と元音源長を明示的に受け取り、`k/60 < min(...)` を実際に比べて標本を採る。
     空集合・短い尺の結果を固定する。
3. 書き出し driver（`src/video/mvexport/driver.rs`）: 格子型とタイムラインを使い、較正へ要求尺と
   長さを揃える前の元音源長を渡す。AAC 側の長さ揃え（`fit_audio_to_duration`）の規則は変えない。
4. ラボ（`tools/musicvideo_lab`）:
   - プレビュー: 編集セッションが `Arc<Mutex<タイムライン>>` を 1 つ持つ。worker は同時に 1 つだけ（現行の受付を維持）。
     lock は解析の間だけ、取得直後に取消を確認、`SpectrumFrame` は lock 内で複製して描画前に放す。UI スレッドは
     lock / try_lock / join をしない。表示するのは `index_at(位置)` のフレームで、解析もシーン評価も `time(n)` で行う。
     既存の `previous_spectrum` の受け渡しは無くす。
   - 較正: key に出力尺と PCM の識別（既存の `ResourceRevision`）を含める。出力尺の変更で較正し直す。
   - PNG 書き出し・`benchmark_once` などのベンチマーク・その他 `SpectrumSource` の呼出しを、同じ格子型・view・
     タイムラインへ揃える。PNG 書き出しの `ceil` のフレーム数規則は格子型の規則に置き換える。
   - `mvexport_cli` も同じ入口を使うこと。
5. 正本の該当節に実装記録（変更したファイル、テスト件数、計測値、変更前後の書き出しハッシュの扱い）を追記する。
   計画表 §7 の H3a 行の状態も更新する。

範囲外: P4b（クレート移動、FFmpeg デコードへの統一、スナップショット入力の書き出し、再生まわり）。
symphonia と FFmpeg のデコード差は扱わない。

## 3. テスト（正本の A1〜A6）

- A1 乱数の要求列を含む参照定義とのビット一致（チェックポイントちょうどの番号・同番号の反復・設定変更・取消後の再開を含む）。
- A1b ラボのホストの handler テスト（取消済み worker が終わるまで次を起動しない、lock 取得直後の取消確認、
  描画エラー・revision 不一致の破棄・プロジェクト切替・音声差し替え・Undo の後も状態が失われず、古い key の状態が使われない）。
- A2 生の FFT 値の純粋性と、view と実体化した PCM の一致（先頭・終端付近、samples が L より短い・長い）。
- A2b 格子の境界（`index_at(time(n)) = n`、境界の直前直後、round 規則、1 未満の拒否）。
- A3 較正: 出力尺が音源より短い・長い・同じで、プレビュー側と書き出し側の入力から同じ結果。無音延長が標本に入らない。
- A4 格子の重複実装が無いこと（driver・PNG・プレビュー・ベンチマーク・CLI）。
- A5 無変調ハッシュ（`selection_unmodulated_render_hash_is_unchanged` など）が変わらない。
- 変更の前に、各テストが変更前のコードまたは意図的に壊した実装で失敗することを確かめ、記録する。
  （挙動不変の部分は、新コードを変異させて落ちることを確かめれば足りる。）

## 4. 計測（A6）

release ビルドで、変更前（HEAD）と変更後を同じ条件で測る。
- 1 フレームの解析時間（ラボのベンチマーク）。
- 5 分の合成素材・60fps で、4 分の位置へ冷えた状態からの移動と、後段だけの編集（手動範囲・表示ゲイン）後の再計算時間。
  逐次で 0.5 秒を超える場合だけ区間の並列化を追加し、それでも超える場合は報告して止める（生の値のキャッシュは
  設計担当が判断する）。
- 書き出し速度（既存の書き出し計測と同じ手順）。

## 5. 確認コマンドと注意

```
cargo test -p music-core
cargo test -p musicvideo-core
cargo test -p musicvideo-ui
cargo test -p musicvideo_lab
cargo test -p mimageviewer --lib mvexport
cargo fmt --check
python scripts/check_ui_glyphs.py
```

- `cargo test -p mimageviewer` 系の前に `vendor/ffmpeg/bin/*.dll` を `target/debug/` と `target/debug/deps/` へ
  コピーする（無いとテストが 1 件も走らない）。
- `turbojpeg-sys` が MSB3191 で落ちたら `MSBUILDDISABLENODEREUSE=1` を付ける。
- 本体アプリもラボも起動しない（計測はヘッドレスのベンチマーク・テストで行う）。コミットしない
  （このサンドボックスは git dir に書けない。差分を残して報告する）。`cargo fmt` はかける。
- まれな失敗（worker の panic で lock が汚れる等）は「ログ + 通知 + タイムラインを作り直す」で割り切る。
  回復の仕組みを足さない。
- 緑の区切りで止めて報告しなくてよい。止めるのは、設計判断が要るときと前提の矛盾を見つけたときだけ。

## 6. 報告

変更ファイル、追加テストと失敗確認、全テスト件数、計測値（変更前後）、書き出しハッシュが変わった検査とその理由、
正本との相違（あれば）を日本語で。
