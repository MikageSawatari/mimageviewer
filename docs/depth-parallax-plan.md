# 3D 写真 (AI 深度による視差表示と書き出し) — 設計書

状態: **設計確定 (2026-11-06)。実装前の独立レビュー 4 回で段 A〜C とも着手可 (§16)**。段 A は実装・指定自動テスト・独立レビュー・確認用 build 完了 (§10)。ClaudeCode の検収待ち。worktree `G:\home\mimageviewer-depth`、ブランチ
`depth-parallax`。調査とスパイクの記録は [depth-parallax-research.md](depth-parallax-research.md)。
試験実装の結果がよければ、**v5 リリースの次のリリース**に入れる (2026-11-06 利用者方針)。
バックログ番号は未採番。

## 0. 利用者の決定 (2026-11-06)

| 項目 | 決定 |
| --- | --- |
| モデル | **DA3 Mono-Large を fp16 化したものだけ**。本体に同梱せず、**追加ダウンロード** (約 0.67 GB) |
| 品質 | スパイクの比較動画で「十分良さそう」(利用者の目視) |
| 操作 | フルスクリーンで見るときに、**パネルで強さ等の項目を選び**、その設定でアニメーション表示と書き出しを行う |
| 動き | 自動の動き (左右など) に加え、**マウスに合わせて見るモード**を付ける |
| 書き出し | GIF / APNG / MP4 (調査時の要望) |

既定値はスパイクの基準 (強さ 2%、左右、周期 3 秒、中間の深さが止まる、奥行き整形あり) を使う。
利用者は比較動画でこの基準を含めて品質を了承しており、個別の値は試験実装で見て調整する。

## 1. 範囲

### v1 に含める

- 画像フルスクリーンの **3D 表示モード** (以下「3D モード」)。ON/OFF はキーとフルスクリーン上部の
  ボタン。入ると右側に **3D パネル**が出る。
- 動き: **マウス追従 / 左右 / 円 / 8 の字 / 上下**。強さ・周期・止まる深さ・奥行き整形をパネルで選ぶ。
  設定は画像ごとではなく全体の設定として保存する。
- 書き出し: パネルの「書き出し…」から、今の設定で **MP4 / GIF / APNG** の 1 周期ループを書き出す。
- 対象: 静止画 1 ページ表示。通常ファイル・ZIP 内画像・PDF ページ (表示されている画素から推定するので
  経路の違いは無い。§4)。
- モデルの追加ダウンロード (§3)。

### v1 に含めない (理由)

| 対象 | 理由 |
| --- | --- |
| 動画、アニメーション画像 (GIF / APNG / WebP) | フレームごとに深度が変わりちらつく。研究調査 §6 と同じ |
| 見開き・連結読み | 2 ページを 1 つの奥行きとして扱えない。360° と同じく入口で弾く |
| **別ウィンドウ (F12 の detached viewer)** | §6.4。状態の組み合わせを減らすため、v1 はメインのフルスクリーンだけ |
| 360° パノラマ表示との同時使用 | どちらも表示を占有するモード。3D に入ると 360° は終わり、3D 中は 360° に入れない (§6.2) |
| 深度の手修正、左右並び (SBS)、2 層化と背景補完 | 研究調査 §6 の v1.x / v2 |
| 先読み (次のページの深度を前もって計算) | dGPU では 30〜50 ms で不要。内蔵 GPU で 1 秒待つ点は試験実装で評価してから |
| mIV Remote | 表示はローカルだけ。Remote 側には何も出さない |

## 2. 全体の流れ

```
[D キー / 上部ボタン] → entry_allowed (§6.1) → モデル導入済み? ─no→ ダウンロード案内 (§3.3)、モードは始めない
                                                  │yes
                                                  ▼
                          3D モード ON (context 所有の DepthViewState, §6.2)
                                                  │ 毎フレーム
          表示中テクスチャ (resolve_fs_processed_payload) の TextureId が安定 (§4.1)
                                                  │ 新しい TextureId なら
          GPU で 504×504 の推論入力 + ガイド画像を描いて読み戻す (UI スレッドは非同期待ち, §4.2)
                                                  ▼
          深度 worker: 推論 (mutex 直列) → 前処理 (正規化・guided upsample・膨張・ぼかし・中央値)
                                                  ▼
          UI: 深度を R16Float テクスチャへ上げる (小さい) → 視差シェーダで描画 (§5)
```

結果待ちの間は通常の 2D 表示のまま、パネルに「奥行きを計算中」を出す。

## 3. モデルと追加ダウンロード

### 3.1 配布するファイル

- `da3mono_large_fp16_504.onnx` (約 0.67 GB)。**入力の形を固定済み**のもの:
  `image` [1,1,3,504,504]、`extrinsics_in` [1,1,4,4]、`intrinsics_in` [1,1,3,3]。
  入出力は fp32 のまま (fp16 は重みと内部計算だけ)。
  - 形を固定する理由: DirectML は動的な次元のままだと約 8 倍遅く、DA3 Mono-Large は動的なままだと
    Reshape で失敗する (研究調査 §8.1)。配布ファイル側で固定すれば、ランタイムでの
    dimension override も、サイズごとのセッション (セッションごとに重みを持ち VRAM が倍になる) も要らない。
  - 504×504 1 つに固定する理由: 画像の縦横比のまま推論した場合と比べ、**引き伸ばして 504×504 に
    入れた深度**は 8 枚で相関 0.987〜0.999、縁を複製して余白を足した場合は 0.858〜0.996 だった
    (`check_static.py`、研究調査 §8.6)。引き伸ばしを採る。極端な縦横比 (3:1 以上) では劣化が増える
    見込み (未計測)。
- `LICENSE` (Apache-2.0 の全文) と `NOTICE` (元のモデル名・入手元・**fp16 化と入力形の固定をした旨**)。
- 作り方はスクリプトにする (`tools/depth_model/build_depth_addon.py`: SpatialHub の ONNX → fp16 → 形の固定 →
  sha256 と index の生成)。スパイクの `to_fp16.py` と `check_static.py` を製品用にまとめ直したもの。
  公開手順は配布チャネルの作業として [release-checklist](release-checklist.md) に足す。

### 3.2 ダウンロードの仕組み

- 既存の **VLM アドオン** (`src/vlm_addon.rs` / `vlm_addon_download.rs`) の index 形式
  (`ModelIndex`: ファイルごとの sha256・bytes・license、分割、取り消し) と downloader を、
  **アドオンの種類を引数に取る形へ共通化して使う**。コピーして 2 つ持たない。
  - 実装担当は着手前に VLM 側のコードを読み、共通化で VLM の既存挙動 (保存先・index の URL・
    取り消し・失敗時の表示) が変わらないことを確かめる。変わるなら止めて設計担当へ返す。
  - 共通にするのは転送と配置 (ダウンロード・分割の結合・sha256・一時ファイルからの配置・取り消し)。
    VLM 固有の検証 (4 ファイル必須・GGUF・model ID、vlm_addon.rs:73) は VLM 側に残し、深度側は
    「ONNX 1 つ + LICENSE + NOTICE」の検証を持つ。
  - 編集アドオンパック (フォント + BiRefNet、約 550 MB) には入れない。入れると被写体マスクだけ
    使いたい人も 1.2 GB を落とすことになり、パックのビルダーも全 `*.onnx` を被写体マスクとして扱う。
- 保存先は `<data_dir>/addons/depth/` (VLM と同じ規則。ポータブルはその data_dir 配下)。
- 公開は GitHub Release のタグ `depth-model-v1` (VLM の `vlm-model-v1` と同じ扱い)。
- 導入状態の確認はファイル I/O を伴うので、App に結果を持ち、毎フレームは読まない (編集アドオンと同じ)。

### 3.3 未導入のとき

- 3D モードに入ろうとすると、**ダウンロード案内ダイアログ**を出してモードは始めない。
  ダイアログには大きさ (約 0.67 GB)、ライセンス (Apache-2.0)、入手元、保存先を出す。
- ダウンロード中・完了後に **自動で 3D モードへ入らない**。完了したら「もう一度 D キーで 3D 表示」と
  通知する。ダウンロード中にページを移る・フルスクリーンを閉じる、と 3D モードの組み合わせを作らないため。
- 設定画面のアドオン一覧 (VLM と同じ場所) からも導入・削除できる。

### 3.4 推論

- `ModelKind::DepthDa3MonoLarge` を足し、既存の `AiRuntime` のセッション管理 (`load_model` /
  `with_session`) で動かす。DirectML、失敗時は既存どおり CPU。
- 入力: 504×504 の sRGB RGBA (§4.2) → ImageNet の平均・分散で正規化 → [1,1,3,504,504]。
  カメラ入力は −1 埋め (SpatialHub の書き出しの約束)。
- 出力: `depth` と `sky` [1,1,504,504]。`sky > 0.5` は視差 0、それ以外は `1 / depth` (有限・正のときだけ)。
- **所要時間** (研究調査 §8.5): RTX 4090 で約 30〜45 ms、内蔵 GPU で約 1 秒、CPU のみで約 3.6 秒。
  初回はセッション作成と DirectML のコンパイルで 2〜3.5 秒。モデルは他の AI モデルと同じく
  一度読み込んだら保持する (VRAM 約 1.3 GB)。解放は既存のモデル再読み込みの経路だけ。
- **AI の mutex**: mIV の DirectML 推論はすべて `AiRuntime.sessions` の 1 つの mutex で直列になる。
  深度推論の間 (dGPU で数十 ms、内蔵 GPU で 1 秒、CPU で数秒) は AI 拡大のタイルも待つ。
  3D モード中は AI 拡大が後回しになることを受け入れる (優先度の仕組みは足さない)。
- 推論中は `local_ai_activity_lease()` を持つ (Remote の取得待ちが数える)。

## 4. 深度の入力と前処理

### 4.1 「表示しているもの」を 1 つの鍵にする

深度は **今画面に出している画素**から作る。補正・AI 拡大・消しゴム・ZIP / PDF のどれで作られた
画素かを 3D 側で選び直さない。表示側が毎フレーム選んでいるテクスチャをそのまま使う。

- 入口: `resolve_fs_processed_payload` (ui_fullscreen.rs、`resolve_fs_processed_texture` の中身) が返す
  `FullscreenPaintResource` の元テクスチャ。表示の優先順位 (display-pipeline §2.3) の正本はこの関数で、
  3D 側に同じ判断をもう 1 つ書かない。
- **鍵は `DepthSourceKey { texture_id: egui::TextureId }`**。egui の managed texture の id は
  プロセス内で再利用されない。画素が変われば (補正の確定、AI 拡大の到着、ページ移動) 表示側が
  別のテクスチャを作るので、鍵も自然に変わる。final / processed の経路で同じ id のまま画素を
  書き換える箇所は無い (独立レビューの確認、app.rs:89872 付近の新規生成)。実装担当は着手時に
  `TextureHandle::set` の呼び出し元を一覧にして確かめ直す。
- **素材の契約 (表示側が返す)**: 3D 側は表示側から次の 1 つを受け取り、自分で分岐を足さない。

  ```rust
  pub enum DepthSourceMaterial {
      Ready { texture: egui::TextureHandle },   // 表示側がこれ以上差し替える予定の無い、読める元テクスチャ
      NotYet(DepthSourceWait),                  // 2D のまま待つ (理由はパネル表示とテスト用)
  }
  ```

  `NotYet` になるのは: サムネイルの fallback、ページ送り中の通過表示、`complete=false` の final
  composite、元画像ホールド中、**注釈のベイク待ち** (comic が None を返し完成済みの下地へ fallback
  している間。下地の `complete=true` だけでは最終と言えない、app.rs:90704 / ui_fullscreen.rs:13396)、
  **元テクスチャを持たない Native の presentation** (保持済みの GPU 縮小結果など。`source_texture()` が
  意図的に `None`、ui_fullscreen.rs:11953 / gpu_lanczos.rs:938)。
  - Native だけが残っている場合、3D 側は既存の元素材の再準備の経路 (紙めくりから戻るときと同じ経路) を
    要求する。実装担当はその経路を特定し、無ければ設計担当へ返す (3D 用の再準備を新しく書かない)。
  - RAW の現像待ちで元画像の preview を出している間は、その preview を最終として扱う。現像が終われば
    表示側が別テクスチャへ替えるので、鍵が変わって深度を作り直す。
  - 判定は `resolve_fs_processed_payload` の分岐が知っている事実を返すだけにする。待ち時間の窓は使わない。
- 回転 (R / L の 90° 単位) はテクスチャに焼き込まれていないので、深度もテクスチャと同じ向きで作り、
  描画で両方に同じ回転をかける (§5.2)。

### 4.2 GPU から推論入力を作る (UI スレッドを止めない)

CPU の画素 (`ColorImage`) を探さず、**表示中のテクスチャを GPU で縮小して読み戻す**。

1. 表示中テクスチャの `wgpu::Texture` を `renderer.read().texture(&id)` で借りる (gpu_lanczos.rs と同じ。
   backing が `None` なら準備できていないとして 2D のまま)。
2. 3D 用の**色テクスチャ**を作る: egui の managed texture は `TEXTURE_BINDING | COPY_DST` だけで
   `COPY_SRC` が無い (vendor/egui-wgpu/src/renderer.rs:776) ので、`copy_texture_to_texture` は使えない。
   **元テクスチャをサンプルして level 0 へ描く blit** を行い、そのあと mipmap を生成する
   (`egui_wgpu::MipmapGenerator`、360° と同じ)。縮小表示でのちらつきを防ぐため、3D の描画と
   書き出しはこの mip 付きの色テクスチャから読む。CPU からの大きなアップロードは無い。
   - **色空間**: egui のテクスチャは sRGB で符号化した値を `Rgba8Unorm` に入れている
     (renderer.rs:782)。framebuffer も非 sRGB の形式が優先される (vendor/egui-wgpu/src/lib.rs:526)。
     3D の色テクスチャ・読み戻し・書き出しの描画先は**すべて `Rgba8Unorm`** にそろえ、シェーダは
     値をそのまま通す (sRGB の変換を挟まない)。推論入力もこの値 (= スパイクと同じ sRGB の 8 bit) になる。
3. 色テクスチャから 2 枚を描く: **推論入力** 504×504 (縦横比を無視して引き伸ばし) と
   **ガイド画像** (長辺 1536、元より大きくしない。縦横比はテクスチャのまま)。
4. 2 枚を `copy_texture_to_buffer` → `map_async`。**UI スレッドでは `poll(Wait)` しない**。
   map の callback は `Queue::submit` でも進む (wgpu-30 buffer.rs:363) ので、常時の別 poll は要らない。
5. **起こす責任**: egui は再描画の要求が無いと休む (ui-responsiveness.md:249)。マウス追従で
   ポインタが止まっていても進むよう、`ReadingBack` の間は毎フレーム非ブロッキングの
   `device.poll(PollType::Poll)` と `request_repaint` を行う。map が完了した frame で worker へ渡す。
   worker は結果を送ったあと、**要求元の viewport** に `request_repaint_of` を出す (要求に viewport id を持たせる)。
   - 回帰検査: ポインタを動かさずに 3D を ON にして `Ready` まで進むこと。

読み戻すのは 1 MB + 長辺 1536 の 9 MB 程度で、8K の画像でも CPU 側の大きなコピーは発生しない。

### 4.3 前処理 (深度 worker、スパイクの `parallax.prepare_depth` と同じ)

1. 視差を 1% / 99% パーセンタイルで 0〜1 に正規化
2. ガイド画像の大きさへ bicubic で拡大
3. ガイド画像の輝度で guided filter (半径 `max(2, round(長辺 / 256))`、eps 1e-3)
4. 0〜1 に clip、楕円で 3 px 膨張 (前景を少し太らせ、輪郭のハローを減らす)、σ 1.5 px のぼかし
   (px は長辺 1600 基準。長辺に比例させる)
5. **中央値**を求めて結果に添える (奥行き整形とシェーダの既定の焦点に使う)

奥行き整形 (中央値を 0.5 へ持っていくガンマ) と焦点は**パネルの設定なので worker では掛けない**。
シェーダで `pow(d, g)` を掛ける (§5.1)。設定を動かしても再計算は要らない。

結果は `DepthMap { key, width, height, values: Vec<f16>, median }`。UI スレッドで R16Float テクスチャに
上げる (長辺 1536 で約 3.5 MB、1 フレーム 1 枚。`fs_upload_backlog` の上限の考え方に合わせる)。

### 4.4 深度 worker

- プロセスに 1 本の常駐 thread `depth-estimation`。要求は **1 つの slot (最新が勝つ)** を
  `Mutex + Condvar` で渡す (coding-conventions の並行処理。try_lock の再試行ループは書かない)。
  v1 は 3D モードがメインの viewer だけ (§6.4) なので、同時に要る要求は 1 つ。
- 要求: `DepthRequest { context, request_id, viewport, key, input_504, guide, cancel: CancelToken, reply }`。
  - `request_id` は単調増加。**要求の寿命は `TextureId` ではなく context + `request_id` で識別する**。
    同じテクスチャで OFF → ON した場合に、取り消した前の要求の結果を受け取らないため。
  - `reply` は**要求ごとの受け口** (pending が持つ専用の receiver)。共有 channel にしない。
    pending を捨てれば受け口も消え、結果は届き先を失って捨てられる。
  - ランタイム初期化待ちは `AiRuntimeInitOwner::wait_terminal_while(|| !cancel)` (ui-responsiveness §2.1.2)。
  - 推論そのものは途中で止められない (数十 ms〜数秒)。終わったら cancel を見て、取り消し済みなら捨てる。
- UI は pending の受け口から受け取り、鍵が `DepthViewState` の今欲しい鍵と一致するときだけ採る。
  取り消しは: 3D モード OFF、フルスクリーン終了、フォルダ移動、context の terminal close
  (`ViewerContextBundle::cancel_all_context_work`)、アプリ終了。要求の `Drop` で cancel を立てる
  (BiRefNet の pending と同じ)。slot に新しい要求を置くと、前の要求は取り消し扱いになる。
- 深度の LRU: context 所有で 4 件 (GPU の R16Float テクスチャ + 中央値)。前後のページへ戻る・
  補正の ON/OFF で元のテクスチャに戻る、ときに計算し直さないため。ディスクには保存しない。

## 5. 描画

### 5.1 シェーダ (wgpu の paint callback)

- 360° と同じく `egui_wgpu::CallbackTrait`。**callback はテクスチャを持たない**。色テクスチャ・深度
  テクスチャ・bind group は context 所有の `DepthViewState` が `Arc` で持ち、毎フレーム
  `callback_resources` に入れる。paint 時に `DepthSourceKey` と context の一致を確かめてから描く
  (panorama_wgpu.rs の stale guard と同じ)。
- **slot の後始末**: `callback_resources` に入れた `Arc` は state を捨てただけでは消えない。
  360° と同じく (`clear_pano_upload`、app.rs:95685)、OFF・フルスクリーン終了・context の terminal close で
  **所有 context が一致する slot を明示的に取り除く**。
- **VRAM の会計**: 色テクスチャ (mip 込み。8K で約 340 MB) と深度テクスチャの LRU を、既存の VRAM
  accountant に登録する (async-architecture.md §4 の末尾「新しいテクスチャキャッシュを追加する時は」)。
  色テクスチャは今のページの 1 枚だけ持つ。
- アルゴリズムはスパイクの `parallax.render_view` (研究調査 §4)。画面上の点ごとに、手前から奥へ
  40 段で深度面を探し、二分探索 5 回で詰める。深さ `t` の面は `offset × (t − focus)` だけずれて見える。
- uniform: `offset` (画像幅に対する割合、x / y)、`focus`、`gamma`、`zoom`、uv の範囲、回転、段数。
  - `gamma`: 奥行き整形が ON なら `clamp(ln 0.5 / ln median, 0.3, 1.5)`、OFF なら 1。
    `median` が 0.02〜0.98 の外なら 1。
  - `zoom = 1 + 2.2 × 強さ`。画像の外が見えないよう少し拡大する。
- サンプラは clamp-to-edge + trilinear。uv は表示している範囲 (切り取り後の範囲) の中に clamp する。

### 5.2 置き場所と変換

- 画像の矩形 (fit した位置と大きさ) と uv の範囲・90° 回転は、**通常の 2D 描画が計算したもの**を使う。
  3D 側で fit を計算し直さない。
- 3D 中は常に**画面に合わせた表示**にする。ズーム・パン・自由回転は描画に反映しない
  (状態は保持し、3D を抜けると元に戻る)。Ctrl+ホイールのズームとドラッグのパンは 3D 中は受け付けない。
- 画像の外の背景・上部バー・ステータスは通常どおり。

### 5.3 動きと時刻

- 自動の動きは時刻だけで決まる関数 `motion_offset(kind, strength, phase)`。
  `phase = (経過時間 / 周期) mod 1`。左右 = (sin, 0)、円 = (cos, 0.6 sin)、8 の字 = (sin, 0.5 sin 2θ)、
  上下 = (0, 0.7 sin)。書き出しは同じ関数に `phase = i / N` を渡す (継ぎ目が出ない)。
- **マウス追従**: 画像矩形の中心からのポインタ位置を −1〜1 に正規化し、`offset = −強さ × (nx, 0.6 ny)`
  を目標にして時定数 0.15 秒で近づける。ポインタが 3D パネルの上にあるときは目標を動かさない
  (パネルを操作すると絵が揺れるのを防ぐ)。ポインタが窓の外なら中心へ戻す。
  基準の逆引き shader で手前がポインタから離れる向きとする (右 pointer → 手前は左)。
  段 B の純粋追従・GPU sample 方向のテストで固定し、実機の操作感は別に確認する。
- 再描画: 自動の動きの間は毎フレーム `request_repaint`。マウス追従は目標に近づき切るまで。
  フルスクリーンの viewport が最小化・非表示のときは要求しない。
  - 3D の自動の動きは常に再描画するので、アイドル時の負荷検査 (`check-idle-health.ps1`) の対象外になる
    ことを development-build-and-test-native に書く。

## 6. モードの状態と寿命

### 6.1 入口条件 (1 つの関数)

`depth_parallax::entry_allowed(...) -> Result<(), DepthEntryBlock>` を 1 つだけ作り、キー・上部ボタン・
パネルの表示判定が同じ答えを使う (360° の `panorama::entry_allowed` と同じ考え方)。弾く条件:

- 画像フルスクリーンでない、動画、アニメーション画像
- 見開きの 2 ページ表示中、連結読み
- メインの viewer の、メイン窓のフルスクリーン表示でない (§6.4)
- wgpu の描画状態が無い
- 編集系のモード (消しゴム・隠蔽加工・補正レイヤー・テキスト注釈・切り取り・SNS 分割) の最中

モデル未導入は入口条件ではなく、通過したあとに案内ダイアログを出す分岐 (§3.3)。
弾いたときは理由をトーストで出す (既存の `FsNavNoOpReason` の流儀)。
360° が ON のときに 3D へ入るのは弾かず、§6.2 の 1 つの持ち主が 360° を終わらせてから 3D にする。

**同じ条件を「続けてよいか」にも使う**: 入口条件のうちページと表示の状態に関わるもの
(静止画・1 ページ・連結読みでない・メインの表示) は、3D 中も毎フレーム同じ関数で照合する (§6.3)。

### 6.2 状態 — 表示を占有するモードの持ち主を 1 つにする

360° の `panorama_state` と 3D の状態を別々の `Option` で持つと、両方が `Some` になり得る。
現行の 360° には V キーと `panorama_intent` の自動復帰という 2 つの入口があり、それぞれ 360° の
state を作る (ui_fullscreen.rs:36651、app.rs:94965)。さらに 360° は「state を持ったまま非アクティブ」
(360° でないページ) を許すので、「3D の入口で 360° が ON か」を見るだけでは、後で 360° のページへ移った
ときに 2 つが同時に有効になる。

```rust
// ViewerContextBundle の swap 対象 (viewer ごと。App 全体に置くと別 context に漏れる)
// 既存の panorama_state を置き換える
pub(crate) fs_exclusive_view: FsExclusiveView,

pub enum FsExclusiveView {
    None,
    Panorama(PanoramaState),   // 今の panorama_state の中身 (非アクティブを含む)
    Depth(DepthViewState),
}
```

- 遷移は **1 つの持ち主** `set_fs_exclusive_view(target, cause)` だけが行う。360° の V キー・
  上部ボタン・intent の自動復帰、3D の D キー・上部ボタン・継続条件の OFF (§6.3)、フルスクリーン終了の
  すべてがここを通る。別の値へ移るときは、前の値の OFF の後始末 (360° なら `clear_pano_upload` 等、
  3D なら §5.1 の slot 除去と要求の取り消し) を先に行う。
- 360° の既存コードは `panorama_state()` / `panorama_state_mut()` の accessor で読む形にし、
  中身の扱いは変えない。
- **3D に入るときは 360° の state を捨て、360° の intent (`panorama_intent.on`) も落とす**。利用者が
  明示的に 3D を選んだので、3D を抜けたあと 360° のページで自動的に 360° に戻らないようにする。
  360° の自動復帰は `panorama::entry_allowed` を通るので、3D 中は復帰しない (3D 中であることを
  入口条件に入れる)。

```rust
pub struct DepthViewState {
    pub panel_visible: bool,
    pub started_at: Instant,                 // 自動の動きの時刻原点
    pub mouse: MouseFollow,                  // 目標と現在の offset
    pub source: DepthSourcePhase,            // 下
    pub lru: DepthLru,                       // §4.4
}

pub enum DepthSourcePhase {
    WaitingForStableTexture,                 // 2D 表示
    ReadingBack { key, readback },           // GPU → CPU (§4.2)
    Estimating { key, pending: DepthPending }, // worker (Drop で取り消し)
    Ready { key },                           // 3D 描画
    Failed { key, reason },                  // 2D 表示 + パネルに失敗を表示
}
```

- 相互に排他な段階を bool / Option の組で持たず、1 つの enum にする (CLAUDE.md「バグ修正の一般原則」)。
- パネルの設定 (動き・強さ・周期・焦点・整形) は `Settings` に置く。**画像ごと・context ごとには持たない**。
  `Settings` への項目追加は既存の serde 既定値で読めるので移行は要らない (機能は未リリース)。
- 3D の ON/OFF をフルスクリーンの外へ持ち越さない。360° の `panorama_intent` に当たるものは作らない
  (§6.3 の「閉じたら終わり」で済ませる)。

### 6.3 遷移

| きっかけ | 動作 |
| --- | --- |
| D キー / 上部の 3D ボタン | 入口条件を通れば ON。ON なら OFF |
| 上部バーの × | 3D を OFF (フルスクリーンは閉じない。360° と同じ) |
| Esc / 一覧へ戻る / フォルダ移動 | フルスクリーンごと閉じ、3D も終わる |
| 静止画の次 / 前ページへ移動 | ON のまま。新しいページの表示が安定したら深度を作る |
| **続ける条件を満たさなくなった** (動画・アニメーション画像のページへ移った、同じページでアニメーションと後から分かった、見開き・連結読みに切り替わった、F12 で別ウィンドウ表示になった) | **3D を OFF にする** (§6.2 の持ち主を通る)。トーストで知らせる |
| 3D パネルの「書き出し…」 | モーダルの書き出しダイアログ (§7) |

- 「ON のまま続ける条件を満たさない」状態を作らない。360° は「state を保ったまま非アクティブ」
  にしていて、戻ったときに入場時の後始末を通らない既知の穴がある (360° 設計書 §5.1.1、backlog §1.169)。
  3D は満たさなくなった時点で OFF にし、この組み合わせを最初から無くす。
- **照合は 1 つの関数、呼ぶのは 3D を描く直前**: 各ページ移動の関数 (open_fullscreen の各種、
  スライドショー再開、フォルダをまたぐ移動、動画タイル、ページ一覧) や見開き・連結読みの切替に
  それぞれ OFF を足さない。継続の照合は `depth_view_may_continue(fs_idx)` (§6.1 と同じ条件) 1 つにし、
  `FsExclusiveView::Depth` を持つ viewer の pass で **2 か所**から呼ぶ。満たさなければ §6.2 の持ち主で OFF にする。
  1. **viewer の pass の先頭**: 動画・見開き・連結表示などの媒体別の early return と描画分岐より前
     (ui_fullscreen.rs:27309、28482 の分岐より前)。3D の描画分岐へ届かない frame (動画へ移った、
     見開きになった) でも `Depth` が残らないようにする。残すと静止画へ戻ったときに入場の後始末なしで
     3D が復活する (独立レビュー第 3 回 P1)。
  2. **3D の描画分岐に入る直前**: 1 のあとにその pass の中で条件が変わった場合。満たさなければ
     その frame は 2D で描く。
  - どちらも、深度の状態の借用と renderer の lock を取る前に行う。GPU の `CallbackTrait::paint` の中では
    照合も遷移もしない。
  - root の frame の固定位置 (たとえば 360° の intent の照合の位置) に 1 回置くだけでは足りない。
    フルスクリーンの準備は root の入力より前に走り (app.rs:99412)、子 viewport の中でも結果の採用と
    キー処理がある (ui_fullscreen.rs:34209、28206) ので、固定位置のあとで条件が変わって同じ frame に
    3D を描き得る (独立レビュー第 2 回 P1)。描画の直前なら、その frame の入力と採用がすべて済んでいる。
  - これは時間窓や再試行ではなく、描く時点の事実 (今のページ・表示の状態) での状態の照合である。
  - open_fullscreen の途中では照合しない (360° と同じ理由。呼び出し元がその後でモードを戻すことがある)。
    描画の直前に呼ぶので、open_fullscreen の途中に当たることは無い。
- ON にするときの後始末は 360° と同じ: スライドショー停止、補正パネル・補正レイヤー・切り取り・SNS 分割・
  ルーペ・分析・消しゴム・比較・メタデータパネルを閉じる。

### 6.4 別ウィンドウ (detached viewer) と 3D モード

- **v1 はメインの viewer のフルスクリーンだけ**で使える。理由: 3D モードは毎フレームの再描画・
  GPU 資源・深度 worker を持つ。複数の viewer で同時に動かすと、`callback_resources` の slot、
  worker の要求の取り合い、park / fork 時の移動先、と組み合わせが増える。v1 では入口で弾いて
  組み合わせを作らない。
- それでも状態は `ViewerContextBundle` に置く。メインの bundle が mount を外れる間 (active detached を
  mount している間) に App の field として見えてしまわないため。
- **「メイン」の定義**: context の識別子 (メインの bundle か) と、**今の presentation** (メイン窓の
  フルスクリーンか、別ウィンドウか) の両方で決める。メインの context 自身も F12 で別ウィンドウ表示に
  なる (viewer_context_registry.rs:4936、app.rs:63053) ので、識別子の比較だけでは足りない。
- **presentation の切替を確定する所で 3D を終える**: 通常の F12 は
  `apply_egui_viewer_presentation_for_current_item` (app.rs:91819 → 91624) で presentation を変え、
  `reset_detached_pause_foreground_modes` は通らない。F12 のキー処理に OFF を足すのではなく、
  **viewer の presentation を書き換える共通の確定点** (実装担当が特定する。F12 の ON / OFF・その他の
  presentation の切替が共通に通る所) で、切替先がメイン窓のフルスクリーンでなければ §6.2 の持ち主で
  `Depth` を終える。360° は別ウィンドウでも使えるので、ここでは `Depth` だけを終える。
  - 加えて §6.3 の描画直前の照合が、確定点を通らない経路があっても別ウィンドウの viewport で 3D を
    描かないことを保証する (2 つは役割が違う: 確定点は状態の持ち主の後始末、照合は描画の安全)。
  - 実装担当は、F12 の直後の frame に別ウィンドウで 3D を描かないことをテストで確かめる。
- park / fork の経路で 3D の状態を相手側へ渡す処理は作らない。fork・park は常に `FsExclusiveView::Depth`
  を OFF にしてから行う (`reset_detached_pause_foreground_modes` は 360° と同じく占有モードを終える)。
- detached の述語に触れる箇所 (入口条件の「メインの viewer か」と park 時の OFF) は、
  CLAUDE.md「Detached viewer リワーク中のルール」に従い、**ClaudeCode と Codex の双方のレビューで
  症状パッチではない構造的な変更だと合意**し、detached-rework-plan.md §11 に記録する。

### 6.5 他の機能との関係 — 「表示を占有するモード」を 1 つの述語にする

360° が他の機能を止めている箇所 (調査では 30 か所余り: キー・編集ツールの入口・補正パネル・
ズーム / パン / ホイール・見開き・ルーペ・メタデータパネル・ナビゲータ・ページ一覧・
トリミング・動画タイル) に、3D 用の同じ判定を 1 つずつ書き足さない。

- §6.2 の `FsExclusiveView` を読む述語 `fs_exclusive_view_active(fs_idx) -> Option<ExclusiveViewKind>`
  (`Panorama` / `Depth`) を 1 つ作る。これは判定の集約で、状態の持ち主は §6.2 の 1 つ。
- 360° の判定箇所を 2 種類に分ける: **「表示を占有するモード中だから止める」**箇所は新しい述語に置き換え、
  **360° の意味に依存する**箇所 (ホイール = 画角、ドラッグ = 視線、ナビゲータの球面表示など) は
  `is_panorama_mode_active` のまま残す。
- 単純に置き換えられない箇所 (独立レビューの指摘): ズームの持ち主 `FsZoomOwner` (ui_fullscreen.rs:15104)、
  タッチのピンチ・パン、紙めくりの可否 (page_turn.rs:1809)、連結表示の判定。3D 中の挙動は次のとおり
  決めておき、実装担当は表に書く:
  - ズーム・パン (マウス・タッチ): 止める (3D は fit 固定、§5.2)。
  - **紙めくり**: 単ページの静止画は紙めくりの対象 (`page_turn_view_eligible`) なので、入口条件では
    除外されない。**3D 中は紙めくりの対象外**にする (`page_turn_view_eligible` の 360° の判定を占有モードの
    述語に置き換える)。3D 中のページ移動は紙めくり無しの通常の切替になる。
  - **紙めくりの最中に D を押した**: 入口条件で弾く (理由付き)。紙めくりは短いので待ってもらう。
    紙めくりの表示 (Native) を 3D の素材にしないので、表示の持ち主の切替を 3D 側で扱わない。
    紙めくりが終わると表示は既存の通常合成 → 素材の準備の経路 (app.rs:90001、86998) に戻るので、
    次に D を押せば元テクスチャがある。
  - 連結表示の判定: 連結読みは入口条件で除外済み。3D 中に連結へ切り替わったら §6.3 で OFF。
- 実装担当は全箇所を表にして (箇所・どちらに分類したか・理由)、本書の §10 に記録する。
  置き換えで 360° の挙動が変わらないことを、既存の 360° のテストで確かめる。
- 3D 中のホイール: **通常どおりページ送り**。3D 中に固有の意味は持たせない (360° と違う点)。
  Ctrl+ホイールのズームだけ止める (§5.2)。
- 3D 中の左右パネルの表示切替 (I / Tab): **3D パネルの表示・非表示を切り替える**。他のパネルは
  3D 中は出ない。

## 7. 書き出し

### 7.1 流れ

1. 3D パネルの「書き出し…」→ **モーダルの書き出しダイアログ** (`App::common_modal_dialog_open` に登録)。
   - 形式: MP4 / GIF / APNG (APNG には「ファイルが大きくなります」の注記)
   - 長辺: 720 / 1080 / 1600 / 2160 px (元のテクスチャより大きいものは出さない)
   - 動き: 自動の動き 4 種から (パネルがマウス追従なら左右を初期値にする)
   - MP4 だけ: 長さ (ループ回数 1〜10、既定 4)
   - 強さ・周期・焦点・整形はパネルの設定をそのまま使う (ダイアログで二重に持たない)
2. 「保存先を選ぶ」→ 既存の core と同じ `rfd::FileDialog::save_file` (既定名 `<元の名前>_3d.mp4` 等)。
3. **モーダルの進行ダイアログ** (フレーム数 / 全体、キャンセル)。書き出し中は他の操作を受け付けない。
4. 終わったらトーストで保存先を知らせる。

書き出しをモーダルにするので、書き出し中のページ移動・3D OFF・設定変更の組み合わせは起きない
(CLAUDE.md「長い処理はモーダルにして割り込みを消せないか」)。ただし `common_modal_dialog_open` への
登録だけでは、外から来る「開く」要求と mIV Remote の取得は止まらない (独立レビューの指摘):

- **Remote**: 書き出しダイアログを開く時点で既存の `try_local_modal_lease` (remote_ipc/session.rs:1448)
  を取り、取れなければ書き出しを始めない (既存の通知付きの版を使う)。lease は書き出しの進行ダイアログが
  閉じるまで持つ。
- **開く要求** (関連付け・D&D・単一起動への引数など): `document_open_modal_admission_blocked`
  (app.rs:42299) に書き出し中を足し、既存のモーダルと同じく受付を止める。
- **凍結**: 書き出しの要求を作る時点で、パネルの設定・回転・uv の範囲・画像の寸法・色テクスチャと
  深度テクスチャの `Arc`・**透過部分の下地** (§7.2) を `DepthExportRequest` に写す。worker はこれだけを見る。

### 7.2 フレームの描画

- **画面と同じシェーダ**でオフスクリーンのテクスチャ (書き出しの大きさ、**`Rgba8Unorm`**。§4.2 の
  色空間と同じ) に描き、読み戻す。描き方 (WGSL) を画面用と書き出し用で 2 つ持たない。
  - pipeline・uniform buffer・bind group は**書き出し用に別に作る**。画面用の prepare は毎フレーム
    uniform を書き換える (panorama_wgpu.rs:526 と同じ作り) ので、共有すると書き出しのフレームに
    画面の値が混ざる。
  - **透過**: 書き出しはすべて不透明にする (MP4 は透過を持てず、GIF の 1 bit 透過は縁が汚れるため。
    形式で挙動を分けない)。下地は**画面で透過部分に敷いている下地と同じもの** (黒・白・市松。
    画像の外の余白色とは別、ui_fullscreen.rs:10742、10819) を使い、その種類と市松の升目の大きさ
    (画面の升目を書き出しの寸法の比で拡大縮小したもの、画像の左上を原点) を要求に凍結する。
    egui のテクスチャは premultiplied alpha なので (renderer.rs:456)、合成は gamma 空間で
    `rgb + 下地 × (1 − a)`、合成後の alpha は 1。
  - 回帰検査: 中間の灰色 (sRGB 128) と半透明の画像で、黒・白・市松の各下地について画面の描画と
    書き出しの画素が一致すること。
- 描画・読み戻し・エンコードは **書き出し worker** で行う。wgpu 30 の `Device` / `Queue` は
  `Clone + Send + Sync` なので、worker が encode → submit → `poll(Wait)` してよい (UI スレッドは待たない)。
  色テクスチャ・深度テクスチャは凍結した要求の `Arc` で持つ (モーダル中は差し替わらない)。
- フレーム数 `N = round(周期 × fps)`。fps は形式で固定: MP4 30、GIF 15、APNG 15 (スパイクの大きさの計測に合わせる)。
  `phase = i / N`。

### 7.3 形式ごと

| 形式 | 使うもの | 要点 |
| --- | --- | --- |
| GIF | `image` 0.25 の `GifEncoder` | `Repeat::Infinite`。256 色化は gif crate の NeuQuant (フレームごと)。**遅延は 10 ms 単位**で、`image` は ms の整数へ切り捨ててから 10 ms 単位にする (image-0.25.10 codecs/gif.rs:586)。`1000 / 15` ms をそのまま渡すと各 60 ms になり 1 周期が約 10% 短くなるので、累積時刻から各フレームに 60 / 70 ms を配分し、1 周期の合計を周期に一致させる |
| APNG | `png` 0.18 の `Encoder::set_animated` / `set_frame_delay` | 可逆なので大きい (720 px・2 秒で 14〜18 MB、研究調査 §8.3) |
| MP4 | `video::mvexport` の部品 | H.264 の encoder 選択 (`open_h264_encoder`)、BT.709 の YUV 変換、`PartialOutput` (一時ファイル → 完了時に rename)、faststart。**映像だけの `Mp4Muxer`** を足す (今の `Mp4Muxer::new` は音声 stream を要求する)。寸法は偶数に丸める |

- MP4 の部品は `pub(super)` のものがあるので `pub(crate)` に広げる。音楽ビデオの書き出しの挙動は変えない
  (既存のテストで確かめる)。
- 取り消し: 進行ダイアログのキャンセルと、窓を閉じる・アプリ終了 (`Drop` で cancel)。途中のファイルは残さない
  (MP4 は `PartialOutput`、GIF / APNG も一時ファイル → rename にそろえる)。

## 8. パネルの UI

- 右側の固定パネル (Area + Frame::popup + ScrollArea。coding-conventions の「固定パネル」と
  `tests/support/scroll_layout.rs` の高さ検査に従う)。見出し「3D 写真」は ScrollArea の外。
- 項目:
  - 動き: マウス追従 / 左右 / 円 / 8 の字 / 上下
  - 強さ: 0.5〜5%、既定 2%
  - 周期: 1〜10 秒、既定 3 秒 (マウス追従のときは無効表示)
  - 止まる深さ: 奥 ↔ 手前 (0〜1)、既定 0.5 (整形 ON なら中央値の深さ)
  - 奥行きを整える: ON / OFF、既定 ON (説明: 手前の床などが奥行きを占めて平らに見えるのを防ぐ)
  - 「書き出し…」
  - 状態の 1 行: 「モデルを準備中」「奥行きを計算中」「失敗しました (理由)」
- 配色は `os_theme` の helper、文字色は `ui.visuals()`。固定文言に絵文字を使わない
  (`scripts/check_ui_glyphs.py`)。
- 上部ホバーバーに 3D ボタンを足す (360° ボタンの隣)。入口条件で弾かれる画像では無効表示。
- UI スナップショット (`tests/ui_snapshot.rs`) にパネルを足す。

## 9. キー

- `KeyAction::FsDepthParallax`、既定 **D**、文脈は画像フルスクリーン (`FsImage`)。
  D は消しゴム・隠蔽加工・補正レイヤーの描画モードでも使うが、それらは 3D と同時に入れない
  モード内のキーなので衝突しない (360° の V と消しゴムの V と同じ関係)。
- 実装担当は `keymap.rs` の `default_chords()` と `ALL_ACTIONS` で D が画像フルスクリーンと共通の文脈で
  空いていることを確かめる。`ini_name()` / `context()` / `trigger()` / `docs/keymap.ini.default` /
  keymap-spec をそろえる。

## 10. 実装の段階 (Codex への委任単位)

| 段 | 内容 | 受入れ |
| --- | --- | --- |
| A | **実装済み (2026-11-07)**。配布用モデルの作成スクリプト、アドオン (VLM の共通化)、`ModelKind`、深度 worker、前処理の Rust 版。詳細・検証記録は直下 | Python 固定 fixture 比較を含む対象 82/82、VLM アドオン既存テスト・snapshot 成功。実モデル 1/1、Python stretch 相関 1.00000000 |
| B | 3D モード: 状態・入口・遷移・占有モードの述語 (§6.5 の表)、GPU の色テクスチャと読み戻し、シェーダ、マウス追従、パネル、キー | 状態遷移テスト、入口条件のテスト、360° の既存テスト、シェーダの不変条件テスト (§11) |
| C | 書き出し (GIF / APNG / MP4、映像だけの muxer、進行ダイアログ) | 書き出したファイルを読み戻してフレーム数・遅延・寸法を確かめるテスト。音楽ビデオの書き出しのテストが通る |
| D | 文書 (§12) と製品ページ・マニュアル | 公開文書の検査スクリプト |

各段のあと独立レビュー (Sol / xhigh)。実機確認は利用者 (`build-dev.ps1` のバイナリ)。

### 段 A の実装記録 (2026-11-07)

- `tools/depth_model/build_depth_addon.py` でローカルの SpatialHub 原本を fp16 化・504 固定・単一 ONNX 化。
  Apache-2.0 全文は既存の `vendor/egui/LICENSE-APACHE` を流用し、変更内容入り NOTICE と
  `depth-model-index.json` を生成する。実変換先は git 管理外の `target/depth-addon/`。
- `addon.rs` に転送・検証・連結・配置・管理 worker を集約し、`AddonKind` で VLM / 深度を選ぶ。
  VLM 固有の GGUF / model ID / 4 ファイル検証は `vlm_addon.rs`、深度の固定 ONNX / LICENSE /
  NOTICE 検証は `depth_addon.rs`。保存先・配布タグは §3.2 のとおり。既存 downloader の
  Range 再開・取消時 staging 保持・転送再試行を共用する (§13 に新たな回復機構は足さない)。
- App の `DepthSetup` は導入状態を cache し、設定の「アドオン」に導入・削除・取消・進行を追加。
  VLM とボタン・進捗描画を共用する。`DepthSetup::installed_model_path` (cached status の判定) と
  `App::request_depth_addon_setup` が段 B の入口用 API。3D モードの状態・案内からの入場は未実装。
- `ModelKind::DepthDa3MonoLarge` と `depth_estimation.rs` を追加。共通 runtime owner を待ち、
  `load_model` / `with_session` で固定形の fp32 I/O を実行する。モデルの完全検証は cold load 時だけで、
  保持済み session があればページごとのファイル hash は行わない。TRT pack / 拡大モデル一覧には含めない。
- worker は process に 1 本、`Mutex + Condvar` の待機 slot は最新 1 件。同 context の active 要求を
  supersede し、別 context の active 要求は取り消さない。slot の差替は旧 queued 要求を取消・解放する。
  `DepthPending` は要求ごとの receiver と cancel を持ち、Drop と採用時にも取消を照合する。
  `context + request_id + key` を照合し、完了後に要求元 viewport を repaint。lease は実処理終端まで保持する。
- `depth_preprocess.rs` に percentile / cubic / guided filter / scaled 膨張・Gaussian / median を実装。
  `DepthMap.values` は `Vec<half::f16>`、median は量子化前の f32。ガンマ・焦点は掛けない。
  非有限の disparity は前処理入口で拒否する (モデル出力の非正・非有限 depth は先に視差 0 へ変換)。
  Python / OpenCV の段別 fixture は 16,861 bytes。大サイズ case は全処理後の境界・中央 sample と全量 median を記録する。

設計の機能変更はない。膨張半径の整数化は Python / OpenCV に合わせた round-to-even、ぼかしは
長辺比例の実数 sigma とする。取り消し token は既存の viewer / BiRefNet と同じ `Arc<AtomicBool>`。
要求は `App::local_ai_activity_lease()` を受け取り、待機中も含めて実処理終了まで保持する。
ViewerContextBundle / GPU / キー・3D パネルは段 B、公開文書は段 D に残す。
段 B で導入・削除と 3D 要求を接続するときは、§3.4 の session 保持／既存 reload 契約と
表示側の取消を整合させる。UI から `unload_model` を直接呼んで推論 mutex を待つ経路は作らない。
要求を出す側は既存 AI と同じ `App::ensure_ai_runtime()` で初期化を開始してから共有 owner を渡す。
初回の導入確認は lazy scan の完了を待って判定し、未確認を未導入として再取得させない。

#### 段 A の検証記録

対象は `depth-parallax` の本段の未コミット差分。全 Cargo コマンドで
`MSBUILDDISABLENODEREUSE=1`。native 依存の初回 24 並列 build で FFmpeg bindgen の
access violation と TurboJPEG の MSBuild 失敗が発生したため、TurboJPEG を `/m:1 /nr:false`
で構築し、その後は `CARGO_BUILD_JOBS=1` で検証した。製品テストの失敗とは区別する。

| 実行 | 結果・証拠 |
| --- | --- |
| `python -m unittest discover -s tools/depth_model -p test_build_depth_addon.py` (spike venv) | 2/2 成功。fp32 I/O、fp16 initializer、固定入力、外部 tensor なし、index の hash / bytes、別 export の拒否。`target/depth-python-tests.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/^addon::/) \| test(/depth\|vlm_addon/) \| test(=ai::trt_worker_lifecycle::tests::models_outside_the_pack_never_start_the_worker)'` | 最終 82/82 成功、exit 0。UPDATE 指定なし。前処理 4 件、worker の取消・相関・別 context、VLM 共通化の既存回帰・snapshot、深度 pack の欠落 / 同長 hash 破損 / size 不一致、UI 小画面・busy を含む。`target/depth-tests-final.log` |
| 新規 UI snapshot だけ `UPDATE_SNAPSHOTS=1` で明示生成 | 2/2 成功。その後上の通常比較も成功。`depth_addon_confirm_dark.png` 64,353 bytes、`depth_addon_preferences_dark.png` 49,555 bytes。実装者・独立 reviewer が日本語、ライセンス、取得元、保存先、ボタンの欠け・重なりが無いことを目視。`target/depth-snapshot-update.log` |
| `MIV_DEPTH_MODEL_DIR=target/depth-addon`、nextest `--run-ignored only -E 'test(=depth_estimation::tests::depth_real_model_matches_python_static_stretch)' --success-output immediate` | 1/1 成功 (2.69 秒)。実写真 `target/depth-spike/samples/IMG_0001.JPG` (EXIF 反映後 720×960) を同じ `INTER_CUBIC` stretch で入れ、504×504 出力・Python 相関 **1.00000000**。共有 `AiRuntimeInitOwner` / DirectML で実行。通常 profile は使わず test data dir に隔離。`target/depth-real-model.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit 0。`target/depth-check-final.log` |
| `cargo fmt` (引数なし) → `cargo fmt --check` | 成功、exit 0。`target/depth-fmt.log` |
| `python scripts/check_ui_glyphs.py` | 危険グリフ 0、exit 0。`target/depth-glyphs.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` (`CARGO_NET_OFFLINE=true`) | 成功、exit 0。通常 feature / 非 portable の core と Remote / EPUB / MCP / music-video editor の 5 exe、FFmpeg 6 DLL と CRT 4 DLL を配置。PE 依存検査 runtime=4 / pe=5 成功 (`target/vcrt-pe-reports/dev-runtime.json`)。core は `target/dev-runtime/mimageviewer-core.exe`、391,114,240 bytes。実行していない |

実変換したモデルは 671,032,016 bytes、SHA-256
`0f5ff3be208d04bc34937946546d75b6d12d8aba4aa2cf1cc26a6bde2c26ed95`。
`tools/depth_model/README.md` に生成・fixture・実写真 oracle の再生成コマンドを残した。
モデル・写真 oracle・build/test log はすべて git 管理外。ネットからモデルや大きなファイルは取得していない。
全体 gate / 実アプリの対話試験は未実施 (今回の指定は範囲を絞った検証)。製品起動・実機検収の代用とはしない。
公開配信先での実取得も未確認 (転送・配置はローカル HTTP サーバで検証)。利用者の確認対象は、設定の
「アドオン」から「3D 写真の追加モデル」の表示と取得案内の開閉。3D 表示の入口自体は段 B。
通常版／トレイ常駐を終了してから、利用者が次のコマンドで起動する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

これは高速ビルド用の通常 profile で、既定の `%APPDATA%\mimageviewer` の実設定・データを更新し得る。
single-instance mutex を通常版と共有する。エージェントによる起動は行っていない。

変更ファイル:

- 依存・module: `Cargo.toml`、`Cargo.lock`、`src/lib.rs`。
- 転送・導入: `src/addon.rs`、`src/depth_addon.rs`、`src/vlm_addon.rs`、`src/vlm_addon_download.rs`。
- worker・推論: `src/depth_estimation.rs`、`src/depth_preprocess.rs`、`src/ai/mod.rs`、
  `src/ai/model_manager.rs`、`src/ai/tensorrt_builder.rs`、`src/ai/trt_worker_lifecycle.rs`、
  `src/previous_run_diagnostics.rs`、`src/previous_run_diagnostics/checkpoint.rs`。
- 設定 UI・App: `src/app.rs`、`src/ui_dialogs/mod.rs`、`src/ui_dialogs/depth_addon.rs`、
  `src/ui_dialogs/model_addon.rs`、`src/ui_dialogs/vlm_addon.rs`、`src/ui_dialogs/preferences.rs`、
  `src/ui_dialogs/preferences/pages.rs`。
- 生成・fixture: `tools/depth_model/build_depth_addon.py`、`generate_fixtures.py`、
  `test_build_depth_addon.py`、`requirements.txt`、`README.md` (いずれも同ディレクトリ)、
  `tests/fixtures/depth/preprocess.json`、`tests/snapshots/depth_addon_confirm_dark.png`、
  `tests/snapshots/depth_addon_preferences_dark.png`。
- 設計文書: 本書、`docs/async-architecture.md`、`docs/architecture-overview.md`。

独立レビュー (GPT-6.1 Sol / xhigh) は事前の所有境界と完成差分・追加テスト・新 PNG・検証ログを確認し、
P1/P2 指摘なし、段 A の実装は受入可と判断。最終検収と commit は ClaudeCode が行う。
この worktree では stage / commit を行っていない。

#### Codex P1〜P3 対応 (2026-11-07、基点 `6e20dfa43`)

独立レビュー指摘を修正した。ここからは上記の初回実装記録とは別の未コミット差分。

- **P1**: `AddonKind` の scan / install / remove 名 helper を実際の spawn に渡す。
  VLM は従来の `vlm-model-{scan,install,remove}` を復元し、深度は
  `depth-model-{scan,install,remove}`。6 名の完全一致を回帰テストで固定した。
- **P3**: エラー本文全体の `VLM` → `Depth` 置換を廃止した。HTTP・検証・配置の生成点で
  `kind.label()` を埋め込み、パスと下位エラー文字列はそのまま保持する。取消 sentinel も
  種別ごとの producer / consumer を揃えた。VLM の既存文字列と回帰テストは維持。
  `VLM` を含むパス・JSON 下位エラー・HTTP 下位エラーの保持など、P1/P3 合計 7 テストを追加。
- **P2**: 504×504 の二軸 ramp・矩形段差・整数 seed noise を両言語で再生成し、
  1536×1024 の二次元ガイドへ `parallax.prepare_depth` と同じ前処理を行う検査を追加。
  境界・段差の両側・全体を分散して **797 画素**、および全量の中央値を比較する。
  fixture 全体は **38,053 bytes**。許容差は f16 丸めの最大 1/4096 と float32 の
  OpenCV SIMD / Rust 加算差を合わせて画素 3e-4、量子化前中央値 2e-5。
  実測の最大差 **0.00024390**、中央値の差 **0.00000003** (debug / 最適化版とも成功)。
- 実モデル ignore テストを推論から前処理まで `depth-estimation` thread 上で通す形に広げた。
  同じ実写真 (720×960、ガイドは拡大しない) の **691,200 画素すべて**を Python の f32 結果と比較。
  推論相関 **1.00000000**、前処理後の最大差 **0.00024426**、平均差 **0.00005896**、
  中央値の差 **0.00000003**。1/1 成功。画像・全量 oracle は `target/depth-addon/` だけに保存。
- 200 ms の目安を超えたため、percentile / median の全件 sort を隣接順位の選択へ変更。
  box filter の縦 rolling sum を行順に処理し、Gaussian は横の reflect を padded row に事前計算、
  縦を連続行の加算にした。各画素の f64 加減算・Gaussian の tap 加算順は維持し、並列化は追加していない。
  percentile は旧 sort とビット一致する回帰も追加した。

**前処理の計測条件**: Intel Core Ultra 9 285、rustc 1.99.1、Windows x64。
`tools/depth_model/preprocess_probe/` は製品 `src/depth_preprocess.rs` を `#[path]` で直接コンパイルする
独立したテスト用 workspace。アルゴリズムのコピーを持たず、アプリと同じ MiMalloc 0.1.52
(`default-features=false`、同じ vendored libmimalloc-sys)、root の linker / `+crt-static` を使う。
最適化は root と同じ `dev-runtime` (opt-level=2、lto=off、codegen-units=64 など)。
repo root から `--offline --test-threads=1`、他の Cargo / テスト実行と重ねずに計測した。
同じ `depth-estimation` thread で 1 回 warm-up 後に 3 回測定し、その中央値。
入力生成・thread 作成・返却 map の drop は除外し、`prepare_depth` 内の確保・scratch 解放・全フィルター・
中央値・f16 化は含める。入力と完成結果を `black_box` で観測する。3D 表示全体の応答時間の計測ではない。

| profile | 修正前 3 回 (ms) | 修正前中央値 | 修正後 3 回 (ms) | 修正後中央値 |
| --- | --- | ---: | --- | ---: |
| `dev-runtime` (最適化) | 214.360 / 214.752 / 216.848 | **214.752 ms** | 119.596 / 121.536 / 123.314 | **121.536 ms** |
| debug (`test`) | 2206.732 / 2193.411 / 2193.933 | **2193.933 ms** | 1317.034 / 1310.109 / 1309.065 | **1310.109 ms** |

最適化版は 200 ms の目安を下回った。debug は診断用であり、200 ms の判定には用いない。
計測ログは `target/depth-perf-mimalloc-{before,after}-{optimized,debug}.log`。
修正前は `git show 6e20dfa43:src/depth_preprocess.rs` の製品コード部分を改変せず、現行テスト部分だけを
組み合わせた一時 host (`target/depth-preprocess-baseline/`) で計測した。
元製品コード部分の SHA-256 は `354fe1e3243fc1ceb7ba47eefea25df19284b48286c11990f7a8a072bffd03e6`。
同じ条件で修正前後の全 1,572,864 画素の f16 と中央値を保存し、**ビット単位で全量一致**も確認した
(3,145,732 bytes、両方の SHA-256 は `ad7c934e1664a3133bd5bfce0ad71c7a615b3e8afc9e17e96b2e183c082eac02`)。
一時 host と比較データは git 管理外。単離用 host の再実行方法は `tools/depth_model/README.md` に記載。

**検証** (`MSBUILDDISABLENODEREUSE=1`、ネット取得なし):

- 本体の前回と同じ nextest 絞り込み: **91/91 成功**、exit 0 (`target/depth-review-tests.log`)。
  VLM の既存テスト・snapshot と、新規実サイズ・順位選択・診断回帰を含む。
- 実モデル ignore テスト: **1/1 成功**、exit 0 (`target/depth-review-real-model.log`)。
- 単離 host の通常テスト: debug / dev-runtime **各 6/6 成功** (同じ製品テストを再利用、重複合算しない)。
  timing テストは上の 4 条件で各 1/1、全量ビット比較 host は前後各 1/1 成功。
- `cargo check --offline -p mimageviewer --bin mimageviewer-core`: 成功、exit 0 (`target/depth-review-check.log`)。
- 最後に standalone host とワークスペース全体へ `cargo fmt` を実行。`cargo fmt --check` も成功。
  UI グリフ検査は危険グリフ 0、exit 0 (`target/depth-review-{probe-fmt,fmt,glyphs}.log`)。
- `build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0`: 成功、exit 0 (通常 feature、非 portable)。
  core と 4 コンパニオンを更新し、PE 依存検査 runtime=4 / pe=5 成功。
  `target/dev-runtime/mimageviewer-core.exe` を起動せず引き渡す。通常 profile の起動手順・
  実設定への注意は上記のとおり。全体 gate / 実 UI / 公開配信先の実取得は今回も未実施。
- 独立 reviewer (Sol / xhigh) は P1/P3 の生成点・取消分類と数値同値、上のログ・基点ソースを確認し、
  新規指摘なし、P1〜P3 修正は受入可と判断した。

変更ファイル: `src/addon.rs`、`src/vlm_addon_download.rs`、`src/depth_addon.rs`、
`src/depth_preprocess.rs`、`src/depth_estimation.rs`、`tools/depth_model/generate_fixtures.py`、
`tools/depth_model/README.md`、`tools/depth_model/preprocess_probe/{Cargo.toml,Cargo.lock,src/lib.rs}`、
`tests/fixtures/depth/preprocess.json`、本書。stage / commit は行わない。

### 段 B の実装記録 (2026-11-07、実装・独立コードレビュー済み)

段 A の上に状態・表示・入力を接続する未コミット差分。独立コードレビューは ACCEPT、
P1/P2 残件なし。ClaudeCode の検収・利用者の実機確認は未実施であり、全件 gate の失敗と
補修後の限定検査は下記の実行記録で区別する。stage / commit、製品バイナリの起動は行わない。

- `fs_exclusive_view.rs` の `FsExclusiveView::{None, Panorama, Depth}` を App と
  `ViewerContextBundle` の一つの owner とする。非アクティブな panorama state も同じ enum に残す。
  生きた状態の入替えは `set_fs_exclusive_view` が旧 owner を退役してから公開する。
  `app/depth_view.rs` が入口、継続照合、presentation の commit、パネル操作、worker 接続を担う。
- 型の本体 (`MotionKind`、`DepthParallaxSettings`、`MouseFollow`、`DepthEntryInput` / block、
  `DepthSourceMaterial` / wait、`DepthSourcePhase`、`DepthLru`、`DepthViewState`) は
  `depth_parallax.rs` に置く。設定本体を `settings.rs` に分散させず、`Settings.depth_parallax` が参照する。
  全体設定として保存し、欠落時の既定・未知の動き・非有限値・範囲外値は既定または範囲内へ正規化する。
  設定の転送は既存の「環境設定にある項目だけ」方針に従い、3D パネルで操作するこの値を対象外とする。
  import に同名値があっても転送先の値を維持する。全体保存と転送の対象範囲を混同しない。
- 表示の正本を複製せず、`resolve_fs_processed_payload_and_depth` が通常 selector の選択した
  payload と readiness を一緒に返す。元画像ホールド、通過表示、注釈ベイク、不完全な final、
  thumbnail fallback、元 source のない Native は `NotYet`。RAW preview は既存契約のまま扱う。
  Native の再準備は §15 の既存の通常 final/edit 準備を使う。
- `depth_parallax_wgpu.rs` は managed source を sample して sRGB 符号値の `Rgba8Unorm`
  色テクスチャへ blit し、mipmap を作る。504×504 と長辺最大1536のガイドを別に描いて
  bounded buffer へ読み戻す。UI は `map_async` と非 blocking の `PollType::Poll` / receiver poll だけを使う。
  深度は `R16Float`、現在色だけを保持し深度 LRU は4件、callback は context / key / resource ID を照合する。
  `DepthViewState` の Drop は要求を取消し、自分の context の callback slot だけを除く。
  色・深度・readback は `VramSubsystem::DepthParallax` と既存の context 会計に含める。
- shader は基準の逆引き式を維持する。マウス target はポインタの正規化変位の **負** とし、
  `source = pixel - offset * (depth - focus)` で手前がポインタから離れる。
  右へ動かすと手前は左、下へ動かすと手前は上へ動く。純粋な追従テストと GPU の色 ramp テストで
  target の符号と実際の sample 方向を検査する。これは実機での見え方の検収済みを意味しない。
- 新規 managed texture の GPU 登録は UI の後であり、`load_texture` 自体は次の frame を要求しない。
  `Ready` 素材の backing が未登録の場合だけ要求元 viewport を再描画し、登録後に読み戻しを始める。
  一般の `NotYet` は既存 producer の完了通知を使い、ReadingBack は Poll + repaint、推論結果は worker の
  viewport repaint、同 frame の upload 繰越は次 frame repaint が起床を所有する。
  実際の texture delta の登録を挟む App 回帰と、ポインタ入力なしの state → Ready 回帰で確認した。
- 固定パネルは明示的な位置・本文高の制約を正本とし、Area の推定サイズによる二重の位置補正を無効にする。
  1093×614 と 546.5×307 の contains / scroll end / header固定 / close の検査、PNG 目視を行う。
- 独立実装レビュー (GPT-6.1 Sol / xhigh) は最終コードと新規回帰を **ACCEPT、P1/P2 なし**とした。
  ClaudeCode の検収、利用者による実機確認とは分けたコードレビューの結論である。
- `ui_depth_parallax.rs` は固定ヘッダと高さを制限した scroll body を持つ右パネル。
  動き・強さ・周期・焦点・奥行き整形を保存し、マウス追従では周期を無効表示する。
  段 C 前の「書き出し…」は準備中の無効表示。3D 中は fit 固定、通常 wheel はページ送り、
  Ctrl+wheel / mouse・touch の zoom・pan は停止し、I / Tab は3Dパネルを切り替える設計に接続する。
- detached との境界は [detached-rework-plan §11](detached-rework-plan.md#11-リワーク外からの変更記録)
  に記録した。main context ID と `ViewerPresentation::Fullscreen` を両方要求し、
  `commit_viewer_presentation` が別 presentation の公開前に Depth を終える。
  native 側の静止画 window mode 切替・動画 presentation・rating 後の遷移もこの commit 境界へ通す。
  継続照合は owning pass の先頭と描画直前の2箇所とし、不適格な表示へ持ち越さない。
  読み取り専用の兄弟 context の open / close から別 owner を失効させない。

#### §6.5 の全箇所分類表

追補の入口監査では、キー以外の既存リング dispatch も調べた。段 B で追加する `Depth` のときだけ、
`apply_ring_action` が slideshow / zoom / pixel grid / 背景切替 / 比較 pin を抑止し、metadata は
3D パネルの表示切替へ送る。キー・リング共通の `cycle_fullscreen_fit_mode` も Depth 中は変更しない。
これにより非表示の 2D zoom / pan / 自由回転が fit 切替で初期化されない。
既存 Panorama のリング挙動は今回の変更対象に広げていない。

素材の追加監査では、local edit の materialization が未完了で final が `None` の間に、
通常表示が raw 下地を返す経路を確認した。2D の fallback は維持し、深度素材だけ
`IncompleteComposite` とする。`RawPreview` / `PreviewShown` の現像待ち preview は正本 §4.1 の
明示例外として `Ready` にできる。両方を実 resolver の回帰で検査した。
失敗済みの key は元画像 hold 等の `NotYet` でも保持し、同じ key で推論を再試行しない。

| 追加監査箇所 | 分類 | 理由 |
| --- | --- | --- |
| `app/gamepad_input.rs::apply_ring_action` | Depth の入力規則 | キー以外の direct dispatch でも表示の占有と 2D 状態保持を守る。既存 360° は変更しない |
| `cycle_fullscreen_fit_mode` | Depth の fit 固定 | フィット切替は描画に反映しないだけでなく、保存している 2D transform の reset も行わない |

**基点は実装前 HEAD `8adc67829`**。以下の行番号はこの commit の source を指す。
段 B の編集で行番号や field 名が変わるため、関数名と併記して追跡する。
「占有」は `fs_exclusive_view_active` へ集約する制限、「360」は panorama 固有の意味を維持する箇所、
「分岐」は Depth の仕様を別に決める必要のある箇所。兄弟をまとめた行も対象の出現位置を列挙した。

| 基点の箇所 | 分類 | 判断・段 B の接続方針 |
| --- | --- | --- |
| `app.rs:52806,52809` 補正・分析 panel | 占有 | 他パネルの停止 |
| `app.rs:79046,79147` edit continuation / target switch | 占有 | 非同期再開も同じ編集入口条件 |
| `ui_fullscreen.rs:14656` zoom mode eligibility | 占有 | fit 固定の3Dへ zoom mode を開始しない |
| `ui_fullscreen.rs:22124` similar preview | 占有 | 通常画像を置換する preview の入場停止 |
| `ui_fullscreen.rs:25027` info lock、`28007` overflow、`28041` PDF analysis | 占有 | panel / 通常表示の領域・入力所有 |
| `ui_fullscreen.rs:28382,28387` analysis draw | 占有 | 通常分析 renderer を出さない |
| `ui_fullscreen.rs:29179,29184,29215,29223` local adjust / SNS / crop / trim | 占有 | 各ツール panel の停止 |
| `ui_fullscreen.rs:29281,29677,39757` callouts / overflow request / side chrome | 占有 | パネルの表示と入力受付を揃える |
| `ui_fullscreen.rs:30576` seek menu blocker | 占有 | 通常静止画 menu の停止 |
| `ui_fullscreen.rs:41225,41273` ordinary wheel zoom / canvas click | 占有 | 2D transform を変更しない |
| `ui_fullscreen.rs:43016,49713` edit admission / loupe | 占有 | 共通入口と描画の両方を停止 |
| `app/page_overview.rs:230` overview | 占有＋360 | 占有中は入場停止。既存の `panorama_intent.on` による停止は別に維持 |
| `ui_video_tile.rs:69`、`ui_view_trim.rs:993` | 占有 | tile の入口、trim の準備完了後の再開も停止 |
| `ui_fullscreen/page_turn.rs:1813` | 占有 | Depth 中は紙めくりなしで通常 navigation |
| `ui_fullscreen.rs:15104,15110` `FsZoomOwner` | 分岐 | Depth owner を明示し、Image へ落として不可視の zoom 値を動かさない |
| `ui_fullscreen.rs:15127,15192,15225,15233,15245,15281,15292,15374,15405,15430,15655,40331` | 分岐 | pivot・キー/ピンチ・pan・drag・PDF rerender の owner dispatch 全体で Depth は無操作 |
| `ui_fullscreen.rs:15140,15163` panorama touch helper | 360 | FOV と yaw/pitch の専用操作を維持 |
| `ui_fullscreen.rs:40728` wheel navigation 判定 | 360＋分岐 | 360 は全 wheel を FOV に所有。Depth の通常 wheel は navigation、Ctrl+wheel だけ停止 |
| `ui_fullscreen.rs:37999` touch tap zones | 分岐 | 360 の視線 drag と3Dの fit 固定を区別。zoom/pan を止めても3Dの通常ページ移動は維持 |
| `ui_fullscreen.rs:501,24970,24971,29393,29396,29401,29413,29510,29511,51223` topbar inputs | 分岐 | 占有制限と、360 の active / projection 値を分離 |
| `ui_fullscreen.rs:51336,51420,51455,51481,51593` topbar restriction | 占有 | その他・編集・VST・slideshow 等の通常ボタンを抑止 |
| `ui_fullscreen.rs:51315,51322` close、`51784,51815,51821` 360 button | 分岐＋360 | × の終了先と tooltip はモード別、360 の選択表示・投影操作は360固有 |
| `ui_fullscreen.rs:34359,34531` Escape / early × | 分岐 | Depth の終了を panorama toggle へ渡さない |
| `ui_fullscreen.rs:35908` と派生 `35915,35916,35918,35919,35920,35921,35922,35923,35924,35925` | 分岐 | Z/S/M/bg/grid/compare は占有制限、I は3D panel 切替。編集は共通入口へ通す |
| `ui_fullscreen.rs:36011,36104` | 整理 | 前者は診断値、後者は未使用の `_pano_active_now`。機能 gate ではない |
| `ui_fullscreen.rs:36647` Shift+V | 360 | 投影方式だけの操作。V の入場判定は Depth 中の入場を拒否 |
| `ui_fullscreen.rs:28512` | 360 | panorama shader の paint。Depth shader は同じ通常描画境界で別に選ぶ |
| `ui_fullscreen.rs:29246` confirmation banner | 分岐＋360 | banner は360専用だが、その分岐が metadata への fallthrough も防ぐ。Depth panel を別枝にする |
| `ui_fullscreen.rs:38028,38058,38300,38351,38360,38389,38418,38497,39149` | 360 | 球面 navigator の layout / pose / input / paint。Depth の通常 navigator は停止 |
| `ui_fullscreen.rs:37258,37328` navigator admission（Depth のため条件追加） | 分岐 | Depth だけ navigator の描画・入力を止め、保存済み2D pan/zoom を維持する。従来の通常 flat / 360 navigator の可否は維持 |
| `ui_fullscreen.rs:45254,45788` non-paged cleanup / continuous | 分岐 | 連結へ切替で Depth を終了。継続照合は reading-flow の事実を使い、表示 predicate で変化を隠さない |
| `ui_fullscreen.rs:48511,48515,48566,48569,48615` | 360 | drag pose、wheel FOV、panorama renderer |
| `ui_fullscreen.rs:7100,7121,7136,7147,7257,7271` pure chrome / touch helpers | 占有＋分岐 | 呼出元から占有制限を渡す。canvas の意味を持つ touch 判定は360とDepthを区別 |
| `app.rs:94171,94172` | 360 | panorama state と検出結果の意味を維持 |
| `app.rs:94481,94488,94600,95546` | 360 | refinement / GPU preparation の source と pose |
| `app.rs:94969,95037,95067` | 360 | auto-reentry intent / hint / 未使用 current helper。Depth 中の360復帰は共通 owner が拒否 |
| `app.rs:95364,95375,95394,95408,95439` | 360＋owner | projection / intent の意味は維持し、状態の作成・破棄は共通 setter へ移す |
| `app.rs:64157,81507` park / fullscreen close | 分岐 | Depth を終了。park の360 intent は既存どおり保持、close は既存の終了規則 |
| `app/native_video.rs:6137,6145,6154,6162,9097,9102,9104,9114,9136,9140,9157,9161,9173,9176,9418,12915` | 360 | native video の panorama 同期・FOV・pose。Depth へ置換しない |
| `app.rs:17444,21367`、`app/viewer_context_registry.rs:1695,2974,3404,3736,4117,4380` | owner | 保存・初期化・bundle swap を enum へ移す。独立した gate ではない |

テスト側の `panorama_state` 直接設定も同じ enum owner に追従させる。
360 の既存テストは変更前の意図を維持し、Depth の入口・遷移・別 context・GPU の stale resource・
マウス方向・設定転送・パネルの snapshot に回帰を追加する。

#### 段 B の変更ファイル (39 件)

- 新規の型・GPU・パネル: `src/fs_exclusive_view.rs`, `src/depth_parallax.rs`,
  `src/depth_parallax_wgpu.rs`, `src/ui_depth_parallax.rs`, `src/app/depth_view.rs`。
- App の接続・会計: `src/app.rs`, `src/app/gamepad_input.rs`, `src/app/native_video.rs`,
  `src/app/page_overview.rs`, `src/app/rating_membership_ops.rs`,
  `src/app/viewer_context_registry.rs`, `src/app/vram_accounting.rs`。
- 表示・入力・設定・導入: `src/lib.rs`, `src/keymap.rs`, `src/settings.rs`,
  `src/settings_transfer.rs`, `src/ui_dialogs/depth_addon.rs`,
  `src/ui_dialogs/preferences/search_index.rs`, `src/ui_fullscreen.rs`,
  `src/ui_fullscreen/page_turn.rs`, `src/ui_video_tile.rs`, `src/ui_view_trim.rs`,
  `src/vram_budget.rs`。
- 回帰・fixture・snapshot: `src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`,
  `src/app/tests.rs`, `src/ui_fullscreen/front_popup_input_tests.rs`,
  `src/depth_estimation.rs` (test 用完了 receiver fixture のみ), `tests/ui_snapshot.rs`,
  `tests/snapshots/depth_parallax_panel_dark.png`。
- 文書: `docs/depth-parallax-plan.md`, `docs/detached-rework-plan.md`,
  `docs/panorama-360-view-plan.md`, `docs/display-pipeline.md`, `docs/async-architecture.md`,
  `docs/ui-responsiveness.md`, `docs/keymap-spec.md`, `docs/keymap.ini.default`,
  `docs/development-build-and-test-native.md`。

#### 設計との差分と検証の限界

段 B の機能範囲・入口条件・継続条件を変える仕様変更はない。小さな配置・型の置き場所と
設定転送の既存方針への接続は上記に記録した。WGSL は画面 uniform / pipeline から分離し、
段 C の書き出しは実装せず、ボタンを無効表示する。段 D の製品ページ・マニュアル・privacy は未変更。

全件検査で、追加設定の分類済み件数の期待値更新漏れと、段 A のモデル管理ページの検索索引未登録を検出した。
前者は総数489・対象155・除外334へ更新し、後者は実 anchor / page / 見出しで索引登録した。
索引の既存検査は描画 helper の実呼出と `depth_addon.rs` 内の見出しを照合し、検査を省略しない。
この補修と実描画 source の検査追従も独立コードレビューで ACCEPT、指摘なし。

実モデルの品質・所要時間・実 HWND / GPU での操作感は今回の自動検査では検収していない。
右 pointer で手前が左へ動く方向、透過下地、90度回転、ページ移動後の再準備、V / D 相互排他、
F11 / F12 / 見開き・連結・動画への切替での終了を、利用者の実機確認へ引き継ぐ。

#### 段 B の実行記録

- 3D / panorama / 占有モード / keymap の限定検査: 403/403 成功
  (`target/depth-b-focused-final.log`)。その後の最終 3D 検査は88/88成功
  (`target/depth-b-depth-final2.log`)。GPU offscreen の不変条件・逆引き方向・slot / LRU、
  App の repaint 所有・要求相関・一 frame 一枚・兄弟 context 会計を含む。
- 素材 resolver の追補85/85、リング入力の追補1/1成功
  (`target/depth-b-material-tests.log`, `target/depth-b-ring-test.log`)。
- UI snapshot は全240/240成功 (`target/depth-b-ui-snapshot-final.log`)。
  同じ headless test target を直接実行したもので、製品 exe は起動していない。
  1093×614 の新規 PNG を実装担当と独立 reviewer が目視し、見出し・全項目・状態・無効の書き出しボタンが
  読め、重なりや切れがないことを確認した。546.5×307 の scroll end / header固定も検査した。
- 指示どおり `scripts/test-lib-full.ps1` は一度だけ実行。
  16490件中16485成功、5失敗、73skip、796.553秒 (10 slow, 2 leaky)、exit100。
  正本ログは `target/test-lib-full/runs/20261107T033036728Z-50edeaff/` の stderr / JUnit。
  失敗は上記の分類件数・検索索引各1件、既存 IPC 2件、既存 audio EOF 1件。
- 既存3失敗を同じ built lib で serial 単独実行
  (`target/depth-b-isolated-failures.log`): audio EOF は成功、IPC 2件は再び Windows の
  Os code5 / PermissionDenied で失敗。audio は全件負荷時の失敗を観測しただけで原因を断定しない。
  IPC は一意な名前の `CreateNamedPipe` を作る低レベル検査と fixture server 準備の両方で失敗し、
  Depth state を作る前の権限エラーである。関連コードや検査を緩めず、全件 gate 成功とは扱わない。
- 補修後の設定転送・検索索引は63/63成功 (`target/depth-b-settings-final2.log`)。
  全件での失敗5件を最終 source の全件成功へ読み替えず、補修した2箇所の限定再検査として残す。
- 最終 production source の `cargo check --offline -p mimageviewer --bin mimageviewer-core` は成功
  (`target/depth-b-check-final.log`)。workspace 全体へ引数なしの `cargo fmt` を実行し、
  `cargo fmt --check` と `python scripts/check_ui_glyphs.py` (危険な文字0) も成功。
- `scripts/build-dev.ps1 -PreserveRuntime` を一度実行し exit0。
  core (3分51秒) と remote / EPUB PDF / MCP / music-video の4 companion を normal feature set の
  `dev-runtime` へ用意した。portable feature は追加していない。VCRT PE 検査も runtime4 / PE5 成功。
  `target/depth-b-build-dev.log` に完了記録を残し、成果物は起動していない。

確認用の起動は、インストール済み・トレイ常駐の mImageViewer を終了してから、利用者自身で実行する。
通常の `%APPDATA%\mimageviewer` プロファイルを使うため、実際の設定・データが更新され得る。
単一起動 mutex は通常版と共通である。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

静止画で D / 上部3Dボタン、未導入案内と導入後の手動入場、素材準備中の2Dから3Dへの切替、
パネル全項目と I / Tab、ポインタ側から覗き込む方向とパネル上の追従停止を確認する。
通常 wheel のページ移動・Ctrl+wheel の zoom停止・90度回転での色と深度の一致・OFF後の2D変換保持、
360から3Dへの切替とV拒否、動画・見開き・連結・F11/F12・全画面終了での終了と非復活も確認対象。
自動の動き中は idle health の対象外。実モデルの品質・速度と操作感の検収は利用者・ClaudeCodeへ引き継ぐ。

#### 段 B 独立レビュー P1〜P2 の対応 (2026-11-07、基点 `dee2fd1ec`)

ClaudeCode が正当と判断した別 session の独立指摘3件に対応する未コミット差分。
先の実装記録・コードレビュー ACCEPT はこの追加指摘の解消証拠とは分ける。

- P1: `fs_zoom_mode_context_ok` は Z / 非キー操作の入場判定のまま使い、保存済み2Dズームの
  寿命判定とは分けた。二つの blocked caller は共通の `fs_zoom_reset_for_blocked_context` を通り、
  Depth だけは既存の transient reset で照準・release待ち・edgeを畳む。確定 active / factor は残す。
  Z edge の消費と held level の追跡は維持し、Depth から新しい照準やズーム解除を始めない。
  Panorama 等の対象外 context は従来どおり full reset とする。
- P2: `activate_mounted_as_independent_detached` を `commit_viewer_presentation` に接続した。
  live / materialized の両 fork が通る `split_current_context_preserving_main_grid` の入口で、
  context を移す前に Depth だけを共通 setter で退役する。要求取消と自 context の callback slot 除去を
  Drop の既存契約で完了し、Panorama の state / intent 移譲は変えない。
- P2 の配線検査: 実 Z press / release → D → idle / 拒否される Z → D を
  `handle_fs_key_input` へ流す。継続検査は video / spread / continuous / 後判明 animation の各ケースを
  実 `render_fullscreen_viewport` から通し、still 帰還後の pass でも復活しないことを検査する。
  headless immediate viewport callback は専用 thread、資源 slot は各ケース専有 renderer に隔離する。
  実 independent activation と両 fork では、推論中は cancel token、Ready は slot 除去と LRU 破棄、
  逆の phase に置いた兄弟の要求・資源不変を相補的に検査する。
- 素材は実 resolver で、complete な2D下地があっても `AnnotationBake` の間は `NotYet` とする検査を追加。
  source-free Native しか残らない fixture では、元 managed allocation が消えていることを検査し、
  既存の `ensure_original_material_for_entry(MaterialOnly)` → `fs_pending` → `poll_prefetch` → final 完成を通す。
  Native の実 payload が選択されたとき、下層の generic `IncompleteComposite` より `NativeOnly` の理由を
  優先するよう素材契約を揃えた。OriginalHold / PageTransition / AnnotationBake の優先と2D選択は維持。
  新しい素材 producer・再ロード経路は作らない。
- GPU は製品 callback / WGSL を使い、非一様な二つの深度面、90度回転、非中央の切り取り uv、
  逆向きの変位と、両端を越える大きな変位での crop UV clamp を固定画素の期待値で検査する。
  白 / 市松の透明・半透明・不透明部分と余白は、
  実際の `paint_image_underlay` と通常2D texture mesh の描画結果に照合する。

**2D 状態の兄弟経路監査**:

| 箇所 | 結論 |
| --- | --- |
| 毎 frame の Z handler、非キー Z action | 今回の admission / 保存寿命分離の対象。Depth の確定状態だけ保持する |
| draw の fit / zoom / pan / 自由回転 | Depth が先に選ばれ、描画引数だけ fit / zero にする。保存値を書き換えない |
| key / touch / gamepad / middle-drag の zoom owner | `FsZoomOwner::Depth` は無操作。保存状態を reset しない |
| wheel | Depth の Ctrl+wheel は Z 分岐より前に消費。通常 wheel はページ移動の既存契約 |
| navigator / drag / double-click | Depth は navigator 不許可、canvas入力は専用の無操作分岐。drag origin の破棄は一時状態だけ |
| fit cycle | Depth は入口で拒否し、保存 fit や2D transformを変更しない |
| `reset_fullscreen_fit_transform` / `reset_continuous_reading_transform` | 明示 fit / reading-mode 切替の既存境界。Depth中の毎 frame reset ではない |
| `open_fullscreen` の変換初期化 | ページ変更の既存境界。同一ページ D 往復の per-frame 処理ではない |

**production presentation 代入の全件監査** (テスト・scenario fixture は除外):

| 箇所 | 扱いと理由 |
| --- | --- |
| `app/depth_view.rs::commit_viewer_presentation` | 生きた presentation を変更する唯一の直接代入。旧 Depth を先に退役する |
| `viewer_context_registry.rs::activate_mounted_as_independent_detached` | 今回 bypass を除き共通 commit へ接続。ほかの live transition も同じ commit を使用 |
| `app.rs` の App initializer / `viewer_session.rs::ViewerSession::default` | 新しい空 context の初期値で、退役対象が無い。commit は通さない |
| `ViewerSession::swap_with_mounted` (registry の mount / fork) | context 所有の既存値を exclusive owner とともに運ぶ操作。別 owner の transition として再 commit しない。fork の Depth は搬送前に退役する |
| `cfg(test)` 内と `test_script.rs` の scenario fixture | 実行中の製品 presentation の変更ではないので監査対象外 |

detached §11 に BA-7 の退役順序と BA-4 の既存 identity / commit 境界を記録する。
追加 modal・rollback state・bool は不要で、表示時 guard ではなく公開・移譲元で不適格な組合せを消す。
既存 IPC 2件は利用者から「sandbox 外では成功」との連絡を受けた。本対応で IPC の変更・再検査はしない。
全件 gate は依頼どおり実行せず ClaudeCode に引き継ぐ。

今回の変更は11ファイル: `src/ui_fullscreen.rs`, `src/app/viewer_context_registry.rs`,
`src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`, `src/ui_fullscreen/page_turn.rs`,
`src/depth_parallax_wgpu.rs`, `src/depth_estimation.rs`, `src/addon.rs`,
`src/ui_dialogs/depth_addon.rs`, `docs/depth-parallax-plan.md`, `docs/detached-rework-plan.md`。
新規 test は9件（実キー1・実viewer pass1に4条件・所有境界3・素材2・GPU2）。
`depth_estimation.rs` の cancel token accessor、addon / DepthSetup の status 注入は `cfg(test)` だけで、
実モデルや通常 profile を用いず実 request / entry の契約を検査するための fixture とする。

実行結果（全 cargo / build / test に `MSBUILDDISABLENODEREUSE=1`）:

- 深度関連97/97成功、12.545秒 (`target/depth-b-review-depth2.log`)。
  初回は test fixture API の公開範囲2箇所で compile error を検出し、test 専用委譲と公開 enum の照合へ修正した。
- 既存の panorama / pano / exclusive / keymap / fs_zoom / viewer_context_registry は、成功した depth 選択を
  除いて430/430成功、19.392秒 (`target/depth-b-review-related.log`)。
  この二つの選択の計527件を成功とし、重複する後続のGPU限定実行は件数へ加算しない。
- production core の `cargo check --offline -p mimageviewer --bin mimageviewer-core` は成功、42.94秒
  (`target/depth-b-review-check.log`)。
- workspace 全体へ引数なしの `cargo fmt` を実行。UI glyph checker は危険な文字0
  (`target/depth-b-review-glyphs.log`)、`cargo fmt --check` と `git diff --check` も成功。
- 補強した crop overshoot は該当 GPU test だけ再実行し1/1成功、1.122秒
  (`target/depth-b-review-crop-clamp.log`)。uv端の半 texel 補間値を固定し、全画像端への誤った clamp では通らない。
- 独立完成レビュー（GPT-6.1 Sol / xhigh）は11ファイルと上記の実行ログ・文書を照合し ACCEPT、
  P1/P2 残件なし。BA-7 / BA-4 の構造変更であることも再確認した。全件 gate / 実機検収は対象外。
- `scripts/build-dev.ps1 -PreserveRuntime` は exit0。core 2分22秒、付属4バイナリも成功し、
  VCRT PE 検査は runtime4 / PE5 成功 (`target/depth-b-review-build-dev.log`)。
  normal feature set で確認用 binary を更新し、製品バイナリ・実UIは起動していない。
  利用者の実機確認は Z確定 → D → D の拡大表示復帰、Depth中のZ拒否、D中のF12 / 独立窓への切替と
  帰還後の非復活、360表示の従来のZ解除とfork移譲を対象とする。起動方法と実profileの注意は上記を使う。
  HEAD `dee2fd1ec` を維持し、stage / commit は行わない。

## 11. テスト

- **状態遷移**: OFF → ON → (ページ移動) → ON、× / D で OFF、フルスクリーン終了・フォルダ移動で取り消し。
- **続ける条件の照合** (§6.3): open_fullscreen の各種、スライドショー再開、フォルダをまたぐ移動、
  動画タイル、ページ一覧からの移動、同じページでの見開き・連結読みの切替、アニメーションと後から
  分かる場合、F12 での別ウィンドウ化、の各経路で 3D が OFF になり、別ウィンドウで 3D を描く frame が無い。
- **占有モードの持ち主** (§6.2): 360° ON から 3D に入ると `Depth` に切り替わり、360° の state と
  `intent.on` が消え、`intent.projection` は残る、3D 中に
  360° のページへ移っても 360° が復帰しない、3D 中の V キーは弾かれる、360° の非アクティブ state が
  残っていても 3D と同時に有効にならない。
- **動画を挟む**: 3D 中に動画のページへ移ると `Depth` が消え、静止画へ戻っても 3D は復活しない
  (見開き・連結表示でも同じ)。
- **要求の相関**: 古い鍵の結果を捨てる、取り消した要求の結果を捨てる、同じテクスチャで OFF → ON したとき
  前の要求の結果を受け取らない、slot の置き換えで前の要求が取り消される、`Drop` で cancel が立つ、
  別の context の状態が変わらない。
- **起こす責任**: ポインタを動かさずに 3D を ON にして `Ready` まで進む。
- **GPU 資源**: OFF・終了・terminal close で `callback_resources` の slot が消え、VRAM の会計から外れる。
- **入口条件**: 動画・アニメーション・見開き・連結読み・detached・編集モード・紙めくりの最中で弾き、
  理由が正しい。360° ON では弾かずに切り替わる (上)。3D 中は紙めくりが起きない。
- **占有モードの述語**: 3D 中に止めるキー・入口が 360° 中と同じであること (分類表の「止める」側)。
  360° 固有の挙動 (ホイール = 画角) が 3D 中に出ないこと。
- **シェーダの不変条件** (オフスクリーン描画のテスト。`egui037_stage4_gpu_tests.rs` と同じ仕組み):
  深度が一様で焦点と等しいとき出力は入力と同じ、深度が一様なとき全体が同じだけずれる、offset 0 のとき
  入力と同じ (zoom を除く)。
- **動きの関数**: phase 0 と 1 で同じ値 (継ぎ目が無い)、各動きの範囲。
- **前処理**: 正規化・guided filter・中央値をスパイクの出力と固定値で比べる。
- **書き出し**: 各形式でフレーム数・遅延・**1 周期の合計時間**・寸法・ループ指定を読み戻して確かめる。
  取り消しで途中ファイルが残らない。画面と書き出しの色が一致する (中間の灰色・半透明)。
  書き出し中は開く要求と Remote の取得が受け付けられない。
- **パネル**: UI スナップショット、小さい画面 (1093×614) で全項目に届くこと。

## 12. 文書の同時更新

- 製品: `htdocs/mimageviewer/manual/` (3D 写真の節)、`index.html` (機能カード)、`privacy.html` と
  製品ページの「安心して使えます」(**追加ダウンロードが 1 つ増える**: 通信先と保存されるデータ)、
  ライセンス一覧 (DA3 Mono-Large、Apache-2.0、fp16 化した旨)。
- 設計: architecture-overview (新しい worker とモジュール)、display-pipeline (3D の描画層)、
  async-architecture (深度 worker と読み戻し)、ui-responsiveness (読み戻し・アップロードの上限)、
  virtual-folders (ZIP / PDF は表示中の画素から作るので差が無いこと)、keymap-spec、
  360° の設計書 (占有モードの述語へ移したこと)、detached-rework-plan §11 (§6.4)。
- 公開手順: release-checklist にモデルの公開 (タグ `depth-model-v1`) を足す。

## 13. まれな失敗の扱い

まれな失敗は割り切り (ログ + 通知)。ダウンロードの失敗・推論の失敗・GPU の読み戻しの失敗・
書き出し中のディスクの失敗は、ログと通知を出してその操作を終える。再試行や途中状態の保持は足さない。
推論が失敗した画像はパネルに失敗を出し、2D のまま表示する (同じ鍵では再試行しない。3D を入れ直すと再計算)。

## 14. 決めたこと・採らなかった案

| 論点 | 採った案 | 採らなかった案と理由 |
| --- | --- | --- |
| 深度の入力 | 表示中テクスチャを GPU で縮小して読み戻す | CPU の画素を 360° の `resolve_pano_source` のように選び直す: 表示側と別の選択をもう 1 つ持つことになり、補正・AI・編集の組み合わせで表示とずれ得る。8K では CPU 側のコピーも大きい |
| 色の元 | 表示中テクスチャの GPU コピー + mipmap | 360° のように CPU から上げ直す: UI スレッドで 40〜110 ms 止まる (360° 設計書 §4.1.1) |
| 推論サイズ | 504×504 固定・引き伸ばし | 縦横比ごとのセッション: セッションごとに重み (VRAM 約 1.3 GB) を持つ。余白で埋める: 深度が崩れた (§3.1) |
| 形の固定 | 配布ファイルで固定 | ランタイムの dimension override: 固定ファイルなら不要で、読み込みの経路が他のモデルと同じになる |
| モデル | DA3 Mono-L fp16 の 1 つ | DAv2-Small を既定に足す: 利用者がイラストで Mono-L を最も自然と判断。fp16 で内蔵 GPU でも約 1 秒 |
| 配布 | 独立したアドオン (VLM の仕組みを共通化) | 本体に同梱: 0.67 GB 増える。編集パックに同梱: 被写体マスクだけの人まで落とす |
| 3D の ON を持ち越す | 持ち越さない | 360° の intent のように持ち越す: 復帰の契機・入口条件・park の扱いが増える |
| 360° との排他 | 1 つの enum (`FsExclusiveView`) と 1 つの遷移の持ち主 | 別々の `Option` + 判定の集約だけ: 360° の非アクティブ state や自動復帰で両方が有効になり得る (独立レビュー P1) |
| 満たさなくなったとき | OFF にする | 360° のように保ったまま非アクティブ: 戻ったときに入場の後始末を通らない穴 (§6.3) |
| OFF にする場所 | 毎フレーム 1 か所で照合 | 各ページ移動・見開き切替・F12 の関数に OFF を足す: 経路が多く網羅できない (独立レビュー P2) |
| 別ウィンドウ | v1 は対象外 | 対応する: 再描画・GPU 資源・worker の取り合いの組み合わせが増える (§6.4) |
| 書き出し | モーダル、画面と同じシェーダ | 非モーダル: 書き出し中の操作との組み合わせ。CPU で描く: 描き方が 2 つになる |
| AI の優先度 | 足さない (既存の mutex のまま) | 深度推論を後回しにする優先度: 3D は利用者が今待っている前面の処理 |

## 15. 調査で解消した点と未確認の点

段 B 着手時の source 監査 (基点 `8adc67829`) で次を確認した。

- `TextureHandle::set` / `set_partial` の production 呼出元は
  `ui_adjustment_panel.rs:8731` (local-adjust mask preview)、`ui_conceal.rs:1189`
  (conceal mask の部分更新)、`ui_erase.rs:1364` (erase mask の部分更新)、および別アプリの
  `crates/musicvideo-editor/src/app.rs:1905` (動画編集 preview)。前三者は編集 UI の mask overlay、
  後者は fullscreen の producer ではない。final / processed の元画素には到達しない。
  final の生成は `app.rs:89372,89875,90162,90820` の新しい `ctx.load_texture`、
  edit material の再生成は `86889,86929` の upload helper を使う。
- Native だけが残っている間も `resolve_fs_processed_payload` は held payload を選ぶ前に
  通常 material selector を呼ぶ。通常の final 準備 (`ensure_final_composite_texture_impl`)
  → `ensure_edit_result_pixels` → `assemble_edit_result_pixels` →
  `ensure_original_material_for_entry(idx, OriginalMaterialEntry::MaterialOnly)`
  (`app.rs:89960` 以降、`86864,86998,87001,78804`) が既存 pending / upload owner へ再準備を要求する。
  CPU edit pixels が残る場合は既存の paced upload を使う。紙めくり中もこの通常準備は続き、
  終了後は同じ selector へ戻る。Depth 中は `page_turn_view_eligible` が false となり、
  `page_turn_held_resource` が None を返すため、通常 selector の素材がそのまま選択される。
  専用の「紙めくり再準備 API」は不要で、3D 専用 producer は追加しない。
  source-less payload は `NotYet(NativeOnly)` として既存の準備を待つ。
- マウス方向は §10 の負の target に決めた。基準の shader 式を変えず、右 pointer で手前が左へ動く。
  符号と実際の GPU sample 方向の回帰を追加した。実機での操作感の確認は残る。

以下は未確認。

- 極端な縦横比 (3:1 以上) での深度の劣化 (§3.1)。
- 一般的な GPU・遅い CPU での所要時間 (研究調査 §8.5)。
- 名前 (「3D 写真」で仮置き)。

## 16. 独立レビューの記録

### 第 1 回 (2026-11-06、GPT-6.1 Sol / xhigh、read-only、実装前)

P1 4 件・P2 6 件。ClaudeCode が主要な根拠 (egui のテクスチャの usage と形式、F12 の経路、
`image` の GIF 遅延の切り捨て) をコードで確かめ、すべて採って本書を直した。

| 指摘 | 対応 |
| --- | --- |
| P1 egui のテクスチャに `COPY_SRC` が無く GPU コピーできない | blit で level 0 を描いてから mipmap (§4.2) |
| P1 通常の F12 は `reset_detached_pause_foreground_modes` を通らない。メインの context も別ウィンドウになる | 「メイン」を識別子 + presentation で定義。F12 は毎フレームの照合で OFF (§6.4) |
| P1 判定の集約は排他状態の持ち主にならない。360° の 2 つの入口と非アクティブ state で両方が有効になり得る | `FsExclusiveView` の 1 つの enum と遷移の持ち主。3D に入ると 360° の intent も落とす (§6.2) |
| P1 書き出しを `Rgba8UnormSrgb` にすると sRGB が二重にかかる | 3D の描画先をすべて `Rgba8Unorm`。透過は背景色へ合成して不透明に (§4.2・§7.2) |
| P2 OFF の境界がページ移動以外 (後から分かるアニメーション、見開き・連結の切替) を覆わない | 毎フレーム 1 か所で照合 (§6.3) |
| P2 payload が元テクスチャを持たない場合 (Native、注釈のベイク待ち) | 素材の契約 `DepthSourceMaterial` (§4.1) |
| P2 マウス追従で止まっている間に非同期処理が進まない | `ReadingBack` 中の poll + 再描画、結果は要求元 viewport へ repaint (§4.2) |
| P2 `callback_resources` の slot が残る、VRAM の会計、要求の寿命を `TextureId` で識別している | slot の明示除去、accountant 登録、context + `request_id` と要求ごとの受け口 (§4.4・§5.1) |
| P2 モーダル登録だけでは開く要求と Remote の取得が止まらない、uniform が画面と混ざる | `try_local_modal_lease`、`document_open_modal_admission_blocked`、要求の凍結、書き出し用の pipeline (§7.1・§7.2) |
| P2 GIF の遅延が 60 ms に切り捨てられ周期が約 10% 短くなる | 累積時刻から 60 / 70 ms を配分し、合計時間をテストする (§7.3) |

レビュー担当は、bundle 所有と既存の識別子による対象制限 (§6.4) を「症状パッチではなく構造的な変更」
と認めた (上の P1 2 件を反映する条件付き)。ClaudeCode も同じ判断。detached-rework-plan.md §11 への
記録は段 B の実装時に行う。

### 第 2 回 (2026-11-06、同じセッションの継続)

第 1 回の 10 件は、解決 7・一部 3 (F12 の終了の時点、書き出しの下地、照合の位置)。新しい指摘 4 件を採った。

| 指摘 | 対応 |
| --- | --- |
| P1 照合の位置が実際のフレームの順序と合わない (準備が root の入力より前、子 viewport 内にも採用とキー処理) | 照合は 1 つの関数で、3D の描画分岐の直前に呼ぶ。加えて presentation の切替の確定点で `Depth` を終える (§6.3・§6.4) |
| P2 紙めくりとの関係が未決定。単ページの静止画は紙めくりの対象 | 3D 中は紙めくりの対象外。紙めくりの最中の D は弾く (§6.5) |
| P2 書き出しの下地が凍結されていない (黒・白・市松、余白色とは別) | 下地の種類と市松の升目を凍結、合成後 alpha 1、各下地で一致を検査 (§7.1・§7.2) |
| P3 360° からの入場テストが旧仕様 | 切替・`intent.on` 解除・projection 保持の検査に変更 (§11) |

レビュー担当の判断: 段 A (モデル・アドオン・推論・前処理) は段 B / C の確定を待たずに着手してよい。
注: `codex exec resume` は sandbox の指定を引き継がず workspace-write で動いた。作業ツリーに変更が無いことを確かめた。

### 第 3 回 (2026-11-06、同じセッションの継続、read-only)

段 A・C は着手可。段 B は 1 件 (P1) を確定してから。

| 指摘 | 対応 |
| --- | --- |
| P1 照合が 3D の描画分岐へ届く経路だけで、動画の early return や見開き・連結で飛ばされ、静止画へ戻ると復活する | viewer の pass の先頭 (媒体別の分岐より前) と 3D の描画分岐の直前の 2 か所で同じ関数を呼ぶ。借用・lock の前、paint callback の中では行わない。動画を挟むテストを追加 (§6.3・§11) |

F12 の確定点で終える変更と、viewer の pass の中で持ち主を呼ぶこと (既存の描画も `&mut self` で状態を
更新している、ui_fullscreen.rs:48594) は妥当と確認された。

### 第 4 回 (2026-11-06、同じセッションの継続、read-only)

第 3 回の P1 は解消。新しい P1 / P2 は無し。段 B も着手可。
