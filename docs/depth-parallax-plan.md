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

- メイン context のウィンドウ表示 / 全画面の **3D 表示モード** (以下「3D モード」)。ON/OFF はキーと閲覧上部の
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
| **別ウィンドウ (F12 の DetachedWindow)、独立 context** | §6.4。v1 はメイン context の MainWindow / Fullscreen だけ。F11 のウィンドウ表示は対象に含める |
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

結果待ちの間は通常の2D画像を保ち、パネルの状態行と、150ms後に画像上へ出す待機の箱で知らせる。
パネルを隠していても箱を出す。初期化中は「3D 写真のモデルを準備中…」、それ以外は
「奥行きを計算中…」とし、実効キーがある場合だけ「<キー> で取り消し」を小さく添える (§5.4)。

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

- lazy scan の未確認は未導入扱いにせず、確認中のモーダルを出す。D／上部3Dボタン由来で
  導入済みと確認できた場合は窓を閉じ、その時点の `entry_allowed` と受付時の画像の同一性
  （context・items世代・位置・item識別子）を再照合して、そのまま3Dへ入る。
  環境設定由来の確認は自動入場しない。確認中に画像が変わっていれば入場要求は終了する。
  ローカル閲覧入力はモーダルで止まるので、再開・rollbackの状態機械は足さない。
  関連付けのopenやRemoteなどモーダル外の変更には、上記の照合と操作権の確認で応答する。
- 未導入なら **ダウンロード案内ダイアログ**を出してモードは始めない。
  ダイアログには大きさ (約 0.67 GB)、ライセンス (Apache-2.0)、入手元、保存先を出す。
- ダウンロード中・完了後に **自動で 3D モードへ入らない**。完了したら現在のキー割当を使って再入場を
  案内する。ダウンロード中にページを移る・フルスクリーンを閉じる、と 3D モードの組み合わせを作らないため。
  導入済み確認だけを継続するのは、起動ごとにDを2回押す手間を除き、短いモーダル確認の既存境界を使うため。
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

### 5.4 結果待ちの表示

`FsExclusiveView::Depth` で source phase が `Ready` / `Failed` 以外の間は、素材 `NotYet` も含めて
画像上に非モーダルの状態箱を出す。状態の正本は `depth_wait_status` 一つとし、パネルの状態行と共有する。
runtime が Dormant / Initializing なら準備中、それ以外は計算中。Failed のパネル文言と既存通知は維持し、箱は出さない。

見た目は既存のアニメーション展開中表示と同じ角丸・半透明の暗い箱、17ptの白文字、下の細い不定進捗バー。
初回入場と terminal から再び待つ時だけ提示時計を開始し、NotYet → ReadingBack → Estimating や待ち中の素材変更では
一続きのbusy表示を維持する。150msはちらつき防止だけの条件で、仕事の生存・完了・取消を決めない。
上部chromeと下のページ情報、現在表示中の右パネルを除いた画像領域に置き、1093×614 logicalで収める。
取消の2行目は keymap の現在の実効ラベルを使い、未割当なら省く。

gateに達するための遅延wakeを一度予約し、箱が見えている間だけ33ms周期でバーを再描画する。
hidden / minimized は予約せず、Ready / Failed / OFFでは箱と箱由来の再描画を止める。
結果採用の起床と自動の3D運動は既存ownerが引き続き担当する。

**設計の簡素化の検討**: modalも検討したが、待ちは初回2〜3.5秒、以後はページごとの短い推論であり、
毎ページmodalにすると通常の閲覧に操作待ちを加える。待ち中もDでの取消とページ送りを受け付ける必要がある。
割込み・要求取消・新しい画像での再推論は段Bの要求ownerが既に扱っており、新しい状態の組合せは増えないため、
非モーダルの提示だけを足す。

## 6. モードの状態と寿命

### 6.1 入口条件 (1 つの関数)

`depth_parallax::entry_allowed(...) -> Result<(), DepthEntryBlock>` を 1 つだけ作り、キー・上部ボタン・
パネルの表示判定が同じ答えを使う (360° の `panorama::entry_allowed` と同じ考え方)。弾く条件:

- 画像の閲覧中でない、動画、アニメーション画像
- 見開きの 2 ページ表示中、連結読み
- メイン context の MainWindow / Fullscreen でない (§6.4)。共通の `App::depth_presentation_eligible` で context と許可集合の両方を照合する
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
- 3D の ON/OFF を閲覧終了や presentation の実切替の後へ持ち越さない。360° の `panorama_intent` に当たるものは作らない
  (§6.3 の「閉じたら終わり」で済ませる)。

### 6.3 遷移

| きっかけ | 動作 |
| --- | --- |
| D キー / 上部の 3D ボタン | 入口条件を通れば ON。ON なら OFF |
| 上部バーの × | 3D を OFF (閲覧表示は閉じない。360° と同じ) |
| Esc / 一覧へ戻る / フォルダ移動 | 既存の閲覧終了で 3D も終わる |
| MainWindow ⇔ Fullscreen (F11) の実切替 | 共通の `commit_viewer_presentation` で 3D を終了し、D で再入場できるとトーストで知らせる。同じ presentation の再確定では継続 |
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

- **v1 はメイン context の MainWindow / Fullscreen だけ**で使える。理由: 3D モードは毎フレームの再描画・
  GPU 資源・深度 worker を持つ。複数の viewer で同時に動かすと、`callback_resources` の slot、
  worker の要求の取り合い、park / fork 時の移動先、と組み合わせが増える。v1 では入口で弾いて
  組み合わせを作らない。
- それでも状態は `ViewerContextBundle` に置く。メインの bundle が mount を外れる間 (active detached を
  mount している間) に App の field として見えてしまわないため。
- **「メイン」の定義**: context の識別子 (メインの bundle か) と、**今の presentation** (メイン窓の
  MainWindow / Fullscreen か、別ウィンドウか) の両方で決める。メインの context 自身も F12 で別ウィンドウ表示に
  なる (viewer_context_registry.rs:4936、app.rs:63053) ので、識別子の比較だけでは足りない。
- **presentation の切替を確定する所で 3D を終える**: 通常の F12 は
  `apply_egui_viewer_presentation_for_current_item` (app.rs:91819 → 91624) で presentation を変え、
  `reset_detached_pause_foreground_modes` は通らない。F12 のキー処理に OFF を足すのではなく、
  **viewer の presentation を書き換える共通の確定点** (実装担当が特定する。F12 の ON / OFF・その他の
  presentation の切替が共通に通る `commit_viewer_presentation`) で、実際の表示切替または切替先が共通適格性を満たさなければ §6.2 の持ち主で
  `Depth` を終える。360° は別ウィンドウでも使えるので、ここでは `Depth` だけを終える。
  - 加えて §6.3 の描画直前の照合が、確定点を通らない経路があっても別ウィンドウの viewport で 3D を
    描かないことを保証する (2 つは役割が違う: 確定点は状態の持ち主の後始末、照合は描画の安全)。
  - 実装担当は、F12 の直後の frame に別ウィンドウで 3D を描かないことをテストで確かめる。
- park / fork の経路で 3D の状態を相手側へ渡す処理は作らない。fork・park は常に `FsExclusiveView::Depth`
  を OFF にしてから行う (`reset_detached_pause_foreground_modes` は 360° と同じく占有モードを終える)。
- detached の述語に触れる箇所 (入口条件の「メインの viewer か」と park 時の OFF) は、
  CLAUDE.md「Detached viewer リワーク中のルール」に従い、**ClaudeCode と Codex の双方のレビューで
  症状パッチではない構造的な変更だと合意**し、detached-rework-plan.md §11 に記録する。

#### 6.4.1 MainWindow 対応 (2026-11-08、案1を採択・実装済み)

対象は **メイン context の `MainWindow` と `Fullscreen`**。`DetachedWindow` と独立 context は
引き続き対象外で、§6.4 の複数 viewer・GPU slot・worker・park/fork の理由を変更しない。
設計担当 ClaudeCode が案1を採択し、独立 Sol / xhigh と症状パッチではない構造変更であることに合意した。
入口と確定点の適格性を共有し、実切替で既存 owner を終える。完了通知の宛先移管や追加 repaint は作らない。
以下の経路表の参照行は調査基点 `58816f030`（関数名を追跡の正本とする）。実装基点は `29bb7f54b`、証拠は§10。

**描画までの経路と依存先**

| 対象 | MainWindow で通る経路／参照 | 全画面への依存・設計上の扱い |
| --- | --- | --- |
| 入口と継続 | `app/depth_view.rs::depth_entry_allowed`、`depth_view_may_continue`、`depth_presentation_eligible` | メイン context かつ MainWindow / Fullscreen の共通許可条件へ拡張済み。単ページ・静止画・編集・wgpu 等の他条件は維持 |
| F11 / F12 の表示確定 | F11: `ui_fullscreen.rs`:36329 → `native_video.rs::toggle_egui_viewer_window_mode_for_input`:5229 → `toggle_still_window_mode`:5170 → `depth_view.rs::commit_viewer_presentation`:171。F12: `app.rs::apply_egui_viewer_presentation_for_current_item`:91808 → 同じ確定点 | `viewer_presentation` と `native_video_in_window_active` が既存経路で更新される。Depth 用の別 viewport や表示切替 owner は作らない |
| 静止画 canvas | `ui_fullscreen.rs::fullscreen_embedded_still_active`:32568、`render_fullscreen_viewport_with_root_input`:27376 / `render_fs_body` 呼出し:30172 | MainWindow は root の CentralPanel、Fullscreen は `show_viewport_immediate(fs_id, ...)`。**同じ render_fs_body** を通る。静止画は native fullscreen video presenter / D3D11 HWND を使わない |
| 表示素材・Depth 描画 | `ui_fullscreen.rs::resolve_fs_processed_payload_and_depth`:13268 → `advance_depth_source`:28777 → `draw_fs_image_with_receipt` → `try_paint_depth`:44604、`depth_view.rs`:252 | 素材選択・fit transform・UV・回転・下地は共通。Depth は表示画素の texture ID を使い、fullscreen の面積を素材 key に含めない。各 pass 先頭:27387 と描画直前:44603 の継続照合は両方残す |
| GPU と callback_resources | `depth_view.rs::enter_depth_parallax`:234 / `try_paint_depth`:323、`depth_parallax.rs::attach_renderer` / `publish` / `Drop` | `App.wgpu_render_state.renderer` が所有し、OS 窓ごとの renderer ではない。slot は context と resource ID で照合。サイズ変更だけなら色・深度資源は共用可能。別 context への譲渡はしない |
| worker の wake | `depth_view.rs::advance_depth_source`:411、`depth_estimation.rs` の要求 viewport、`depth_parallax.rs::advance`:395 | `ctx.viewport_id()` を要求に捕捉して完了時にその viewport を起こす。MainWindow で新規開始すれば ROOT。継続案では、旧 viewport 宛ての実行中要求を切替後どう起こすかの設計が必要 |
| パネルと入力除外 | `depth_view.rs::draw_depth_panel`:362、`ui_depth_parallax.rs::draw_depth_parallax_panel_with_export`:32、`ui_fullscreen.rs`:28356 / 39988 | 現在の `ctx.content_rect()` 内に右パネルを配置し、`depth_panel_rect` を同じ ctx の一時データへ記録。MainWindow も同じ関数。移行時は前 viewport の rect を移植せず、新しい pass で描いてから canvas 入力を判定 |
| 待機表示 | `ui_fullscreen.rs::draw_depth_wait_indicator`、`app/depth_view.rs::depth_wait_status` | 現在の page transform / content rect から領域を求め、上部160pt・下部48pt・表示中パネルを避ける。専用 viewport ID 固定ではない。小窓の実描画でパネルとの非交差・取消表示を検証 |
| キー・リング・pointer | `ui_fullscreen.rs::handle_fullscreen_root_key_input`:33766、`handle_fs_key_input_with_operation`、`gamepad_input.rs::apply_ring_action` | embedded では root 側 shortcut probe は処理を委譲し、viewer 本体が同じ ROOT のキーを処理する。Grid 用入力へ落とさない。Viewer の surface / owner gate を維持し、F11・D・I/Tab・Shift+B・ページ送りを各1回だけ受理 |
| マウス追従 | `depth_view.rs::try_paint_depth`:284、`depth_parallax.rs::MouseFollow::update`:116 | 現在 ctx の `hover_pos` と `transform.paint_rect` は同じ論理座標。OS screen 座標や専用 fullscreen rect に変換しない。パネル上で凍結、窓外では中央へ戻す。リサイズ後は新しい fit rect を使う |
| カーソル自動非表示 | `ui_fullscreen.rs::fs_ui_is_clean`:51284、`update_fullscreen_cursor_visibility`:33615、呼出し:30079 | 現行 MainWindow 閲覧も同じ仕組みで現在の `ctx.viewport_id()` を対象にする。modalはcleanを偽にするが、`DepthViewState.panel_visible` はこの関数で直接参照していない。パネル表示だけで非表示を止めるとは断定せず、実効挙動を検証する。今回の案では独自の非表示規則を加えず、既存入力境界を維持 |
| 書き出しの表示と前面 | `viewer_chrome.rs::draw_viewer_shared_dialogs`:396 → `depth_export.rs::draw_depth_export_dialog`:793、`ui_fullscreen.rs`:29868、`ui_depth_export.rs::draw`:66 | canvas 描画後に capture・open・dialog 描画までを同じ viewer ctx で実行。MainWindow も ROOT 内でここを通る。root tail は embedded 時 early return (`app.rs`:99967) するため、tail に追加してはいけない。backdrop と Foreground Window の `set_sublayer` / `move_to_top` を維持し、パネルより前面にする |
| modal の止め方 | `app.rs::common_modal_dialog_open`:23988 / `modal_dialog_block_reason` の depth_export、`document_open_modal_admission_blocked`:42338、`depth_export.rs::admit_depth_export`:670 | Settings〜Finished の operation がある間は共通 modal。キー・wheel・canvas・リングの既存 modal gate を使う。Remote の local modal lease は窓によらず取得〜dismiss まで保持。関連付け・D&D・単一起動等の開く要求も受付停止。外クリックは backdrop が所有し、背面へ渡さない |

**表示切替の組合せを先に減らす**

| 案 | 資源・状態と利点 | 失われる使い勝手／費用 |
| --- | --- | --- |
| 1: MainWindow ⇔ Fullscreen の実際の切替で Depth を終了し、D で再入場 | 共通確定点で既存 setter を通して終了。推論取消・slot 破棄・旧 viewport の wake の移管を扱わずに済む。終了後は通常2Dの既存 F11 経路だけになる | 切替後に D がもう1回必要。**深度 LRU は DepthViewState 所有で Drop される** (`depth_parallax.rs`:283 / 294 / 488)。新しい D は `new` で空 LRU、readback と推論をやり直す。表示側の色テクスチャが残っていても「キャッシュで即復帰」とは言えない。動きの時刻・mouse offset・panel の開閉も新規状態になる（Settings の値は残る） |
| 2: 同じ Depth を継続 | context が同じで renderer も共通なので、Ready の色・深度・LRU、Settings、panel_visible、動きの時刻を保持できる。fit / UV / aspect は次 pass の transform から再計算。切替のための再推論と D の手間が無い | ReadingBack / Estimating での切替も普通に起きる。worker 完了の repaint 宛先が旧 viewport に固定されており、現在は移管が無い。パネル rect・pointer・表示 snapshot は新 viewport で描き直す必要がある。Ready だけを扱って継続できたとする実装にはできない |

**採択したのは案1**。表示切替中の pending 推論の宛先移管を新設せず、既存の終了経路で組合せを減らせる。
現行 Fullscreen → MainWindow も Depth を終了するので、その方向の挙動を悪化させない。
逆方向にも同じ規則を適用する。ただし新たに使える MainWindow から全画面へ広げる際にも D と再推論が要る。
設計担当がこの再操作・再推論の負担を了承した（2026-11-08）。案2は不採択。
同じ presentation への再確定は切替でなく、Depth を終了しない。
終了の案内は `FsDepthParallax` の実効キー割り当て（既定 D）を既存helperで表示する。
割り当てなしの場合は上部の3Dボタンを案内し、解除したDキーを案内しない。

**MainWindow 特有の組合せ（採択仕様）**

| 操作・状態 | 扱い |
| --- | --- |
| サイドバー・グリッド | 同時表示を新設しない。現行 embedded 閲覧は main の menubar / toolbar / grid を描かず early return (`app.rs`:99954)。Depth を許すために一覧や通常全画面の機能を削る必要はない。閲覧内の補正・metadata・navigator等は§6.5どおり、I/Tabは3Dパネルのみ |
| リサイズ・最大化 | そのまま。色・深度の key はサイズ非依存。新しい content rect / fit transform / aspect を毎描画使用。PDF の既存再レンダリング等で**素材**が変わった場合のみ§4.1で新規推論 |
| 最小化・非表示 | モードはそのまま、既存 `advance_depth_source` の非表示／minimized gate で poll・採用を休止。自動モーションの追加 repaint も抑止。復帰の通常 pass で再開。推論結果保持の新しい中間状態は作らない |
| 別アプリへ切替 | 終了させない。非focusは最小化とは違い、現状は自動モーションが継続し得る。既存 focus / keyboard permit で他アプリの入力を受けないことを確認し、今回の拡張で休止規則は変更しない |
| Esc・Enter・Backspace・一覧へ戻る | 既存の viewer close / return-to-list を通して Depth 終了 (`app.rs::close_fullscreen`:81688付近)。単に panel を隠して state を残さない。フォルダ移動も既存の終了・継続規則を維持 |
| ページ一覧（O）、見開き・連結、編集、360° | ページ一覧は `page_overview_available` の占有中入場禁止を維持 (`app/page_overview.rs`:230付近)。Depth中はOを止める。既にページ一覧ならその実行可能キー集合にDは含めない。見開き・連結等へ状態が変わった場合は共通継続照合で終了。3Dから360°への切替を新しく許さない |
| 書き出し Settings / Running / Finished | modal のまま。F11/F12・D・ページ移動・Shift+B・panel操作を止め、開く要求とRemote取得も拒否する。ROOT上のbackdrop＋dialogでpanel/canvasより前面を保証。native保存先選択もそのmodalの内側。外クリックでcanvasやpanelが動くならNG（依頼19 NG1と同じ受入条件） |

**detached 述語に触れる範囲と構造合意**

- 入口 `DepthEntryInput.main_viewer_presentation` と `commit_viewer_presentation` の終了規則を、
  同じ「メイン context かつ MainWindow / Fullscreen」の適格性へ接続する。許可集合を明記するので
  `DetachedWindow` は引き続き除外される。context 比較を落としたり `!= DetachedWindow` だけにしない。
  `App::depth_presentation_eligible` へ接続した。適格性に加え「実切替で終了」の規則を確定点に置く。
- `fullscreen_embedded_still_active`、`render_fs_body` の root/child routing、
  `handle_fullscreen_root_key_input`、`apply_egui_viewer_presentation_for_current_item` は読み取り監査対象。
  それらの変更が必要になれば detached viewport 経路に触れる変更として扱う。
  `reset_detached_pause_foreground_modes`、fork/park、bundle swap、window_id/HWND/placement は今回拡張しない。
- CLAUDE.md「Detached viewer リワーク中のルール」と detached-rework-plan §2 の対象。
  実装着手前に ClaudeCode と独立 Sol / xhigh レビューで、適格な単一contextの表示許可を広げる構造変更であり、
  detached の症状を guard 等で隠す変更ではないと合意済み（2026-11-08）。
- detached-rework-plan §11 へ記録: 日付・基点・合意者、変更した共通適格性と確定点、
  MainWindow/Fullscreen許可・DetachedWindow/独立context拒否、案1の切替時終了、fork/park不変、
  ROOT/child双方の照合・slot破棄・モーダルテストの証拠。実装と併せて§11へ記録する。

**受入テスト計画（実行結果は§10）**

1. 採択した案1の入口・確定点の共通適格性を一塊で検査する。
   状態遷移テストは main × MainWindow/Fullscreen/DetachedWindow、独立contextの全表示、
   同じpresentationへの再確定、F11両方向、F12両方向、close/fork/parkを含める。
   pending取消・slot退役・D再入場で空LRUを確認。ReadingBack / Estimating 中にF11で切替し、
   D再入場後に旧結果が届いても新ownerとslotが保たれることを確認する。
2. ROOT embedded の実viewer headless passで、入口直後と描画直前の照合、同pass内の動画・見開き等への変更、
   不一致素材のcallback不送出、fit固定・下地・回転・リサイズ・panel rectを検査。
   既存Fullscreenの入力・ページ送り・2D transform保存・推論/描画テストを再利用する。
3. ROOTの本物のパネル/canvasを背面に、書き出し全phase・初回/後続pass・再open・形式変更を検査。
   modalの外クリック・wheel・キー・リングで背面が変化せず、dialog内の操作だけが届くこと、
   lease/開く要求拒否、UI snapshotと§7の凍結素材を検査。tailへ依存した二重描画を許さない。
4. 1093×614 logical points と通常サイズ、UI倍率100%/200%で、右panelの×・書き出しボタン、
   dialogの見出し・全固定操作行が収まり、必要時だけ本文scrollで末尾へ届くことをgeometryとsnapshotで確認。
   リサイズ・最小化復帰・Alt+Tab・OS保存先dialogのowner/focusは、実装後の承認済みportable実機枠で確認する。
   今回は製品起動・実機操作を行わない。

まれな失敗は既定の割り切り（ログ＋通知）で閉じ、失敗別の回復設計は加えない。
公開文書は段Dでまとめて更新する。現時点の `htdocs/mimageviewer/manual/` のgrepでは「3D 写真」の節自体が
未追加だった。更新予定は `manual/fullscreen.html` の3D写真節（使用できる表示、F11、F12対象外、Dで再入場の有無、
パネル・背景・書き出し）、`manual/shortcuts.html`（DとF11）、`manual/faq.html`（対応範囲）、
`index.html` の3D紹介・対応表示説明。既存Creative 3D LUTの説明は別機能なので対象にしない。
段Dで公開説明を更新する。案1の再操作・再推論は採択済みであり、wake所有の移管は対象外。

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
- 3D 中のマウス左クリック／タップ: **通常どおりページ送り**。360°がcanvasを視線操作に使うための
  クリック停止をDepthへ持ち込まない。判定は表示中のfit transformを使い、保存2D transformを変更しない。
- 3D 中の左右パネルの表示切替 (I / Tab): **3D パネルの表示・非表示を切り替える**。他のパネルは
  3D 中は出ない。
- **透過部分の背景切替 (Shift+B / ImageBackgroundCycle)**: Depth中も許可する（利用者決定2026-11-08）。
  表示占有による制限ではなく、キーの抑止は360°固有に分類する。Panoramaの既存キー抑止は維持。
  リングは基点でもPanoramaを抑止していないため、その既存挙動を維持する。AIアップスケール中は従来の黒/白2段と
  表示キャッシュ失効を共用し、素材が変われば§4.1どおり深度も作り直す。書き出しの凍結は§7のまま。

## 7. 書き出し

### 7.1 流れ

1. 3D パネルの「書き出し…」→ **モーダルの書き出しダイアログ** (`App::common_modal_dialog_open` に登録)。
   - 形式: MP4 / GIF / APNG (APNG には「ファイルが大きくなります」の注記)
   - 長辺: 720 / 1080 / 1280 / 1600 / 2160 px (元のテクスチャより大きいものは出さない。
     720 px 未満なら元の長辺を1つ表示)
   - 長辺の既定: MP4 1280 / GIF 720 / APNG 1080 px。形式を切り替えるたびにその形式の既定へ戻す。
     選択肢にないときは既定以下で最大の候補、なければ最小の候補。設定は保存せず、開くたびに既定から始める
   - 動き: 自動の動き 4 種から (パネルがマウス追従なら左右を初期値にする)
   - MP4 だけ: 長さ (ループ回数 1〜10、既定 4)
   - 強さ・周期・焦点・整形はパネルの設定をそのまま使う (ダイアログで二重に持たない)
2. 「保存先を選ぶ」→ 既存の core と同じ `rfd::FileDialog::save_file` (既定名 `<元の名前>_3d.mp4` 等)。
3. **モーダルの進行ダイアログ** (フレーム数 / 全体、キャンセル)。書き出し中は他の操作を受け付けない。
4. 終わったらトーストで保存先とファイルの大きさを知らせる。worker が rename 後のファイルから大きさを取り、
   完了事象に載せる。取得できなければ大きさだけ省く。UI スレッドで metadata を読まない。

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
  - 状態の1行: 「モデルを準備中」「奥行きを計算中」「失敗しました (理由)」。待機中は画像上にも
    §5.4の状態箱を出し、パネルを隠していても知らせる。準備／計算中の判定はパネルと箱で共有する。
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
| C | **実装済み (2026-11-07、ClaudeCode検収・利用者実機確認待ち)**。書き出し (GIF / APNG / MP4、映像だけの muxer、進行ダイアログ)。直下に実装・検証記録 | 限定lib286/286、UI関連6/6成功。全件gateはClaudeCode担当 |
| D | **文書更新済み (2026-11-08)**。製品ページ・マニュアル・日英 privacy・ライセンス案内・モデル公開手順。更新一覧とコード照合は §12 | 公開文書の検査、Rust はライセンス文言のみの check・関連テスト（結果は §12） |

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
`apply_ring_action` が slideshow / zoom / pixel grid / 比較 pin を抑止し、metadata は
3D パネルの表示切替へ送る。キー・リング共通の `cycle_fullscreen_fit_mode` も Depth 中は変更しない。
これにより非表示の 2D zoom / pan / 自由回転が fit 切替で初期化されない。
既存 Panorama のリング挙動は今回の変更対象に広げていない。
背景切替は2026-11-08利用者決定でDepthの抑止対象から外した。Panoramaのキー抑止と、
元から抑止されていないリングの挙動は維持する。

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
| `ui_fullscreen.rs` ordinary wheel zoom / canvas click | 分岐（依頼18で訂正） | wheel zoom は占有。canvas click は360°の意味に依存し、Depthの通常ページ送りを許可。保存2D transform は不変 |
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
| `ui_fullscreen.rs:35908` と派生 `35915,35916,35918,35919,35920,35921,35922,35923,35924,35925` | 分岐 | Z/S/M/grid/compare は占有制限、I は3D panel 切替。編集は共通入口へ通す |
| `ui_fullscreen.rs::handle_fs_key_input_with_operation` の `FsBgCycle`、`gamepad_input.rs::apply_ring_action` の `ImageBackgroundCycle` | 360（キー抑止）／通常操作（Depth） | 2026-11-08決定。Depthは下地を毎描画反映するため背景切替を許可。キーは `is_panorama_mode_active` だけで止める。Panoramaリングは基点でも許可されており変更しない |
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
| navigator / drag / double-click | Depth は navigator 不許可、transform変更は無操作。依頼18でclick採用を分離し、保存transformを参照せずページ送りを許可。drag origin の破棄は一時状態だけ |
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

#### 段 B 再レビュー P1: 休止 Z の消費側 (2026-11-07、基点 `551040002`)

Z確定 → D では2Dの保存状態を残すが、その値をタッチ認識へ直接渡すと Depth の通常ページ送りまで停止する。
保存と実効の区別を全 consumer の境界で揃える。`fs_zoom_mode_engaged()` は保存 active / aiming の述語として残し、
`fs_zoom_mode_effective()` は `fs_zoom_owner() == FsZoomOwner::ZoomMode` を返す。
active / aiming の runtime consumer もそれぞれ owner を通す helper を使う。
owner の優先順（Depth → 保存 Z → Panorama → Analysis → Image）は変更しない。
新しい状態・bool・復元用snapshotは追加せず、Depth の入口で保存ズームを捨てる対処もしない。
通常2Dと360では旧 raw 判定と実効判定が一致する既存の順序を維持する。

以下は **基点 `551040002` の行番号**。`src/` の全 `fs_zoom_active` / `fs_zoom_mode_engaged()` と
照準の兄弟 consumer を検索し、テスト・コメント・宣言・書込みを分けて全てを分類した。

| 基点の消費箇所 | 分類・対応 |
| --- | --- |
| `ui_fullscreen.rs:14704-14705` 保存述語 | raw維持。保存 state 自体を表す |
| 同 `14760` toggle action | raw維持。admission を通った後の既存 Z state の切替であり、Depth は到達しない |
| 同 `14862,14872,14882` key state machine | raw維持。admission後の確定／照準・held repaint。対象外の transient reset と level latch は既存どおり |
| 同 `15186` owner解決 | raw維持。Depth を先に選び、Zの保存値から実効 owner を一度だけ決める。helper と循環しない |
| `app.rs:18247-18249,21656-21657` field/default | 保存値の宣言・初期化。変更なし |
| `viewer_context_registry.rs:1787,3047,3477,3810,4199,4429` field/default/extract/swap/move | context所有の保存・搬送。active/aiming を保持し、描画や入力の可否を決めないので変更なし |
| `ui_fullscreen.rs:14967` 単ページZ描画 | 実効 active。保存中の Depth からZ描画を導かない |
| 同 `15159` pan clamp | 実効 owner。Z中は通常pan clampを適用しない既存の分岐 |
| 同 `15648,15670` gamepad zoom/pan | 実効 owner。占有モード拒否は既存どおり、その後のZ判定も統一 |
| 同 `22224` similar preview | 実効 owner。既存の占有モード拒否は維持 |
| 同 `25131` info panel lock適格性 | 実効 owner。表示幅の確保と実描画に同じ値を渡す |
| 同 `28651` 単ページのZ描画分岐 | 実効 owner。重複するDepth専用guardは除く |
| 同 `29343` view trim panel | 実効 owner。保存Zをパネル抑止へ使わない。Depth固有の適格性は既存 owner が守る |
| 同 `29376` metadata panel | 実効 owner。パネルの既存占有規則は維持 |
| 同 `29404` callout/touch handles | 実効 owner。共通 chrome 入力へ渡す |
| 同 `30699` page-turn context blocker | 実効 owner。Depth は保存Zでなく既存の占有モード判定で除外 |
| 同 `32171` PDF display demand | 実効 owner。休止Zを単ページZの解像度需要と誤認しない |
| 同 `33618` cursor visibility | 実効 aiming。休止した照準状態で ZoomAiming のcursor非表示を作らない |
| 同 `38222` navigator current zoom | 実効 owner。Zの倍率から通常倍率へ換算するかの判定 |
| 同 `38301` navigator adopt zoom | 実効 owner。実際のZ操作だけが保存値を通常panへ採用しresetする |
| 同 `38884` navigator gesture | 実効 owner。Depthのnavigator入場拒否は維持 |
| 同 `39915` side panel chrome | 実効 owner。表示・hit判定の共通入力 |
| 同 `39988` panel hit領域 | 実効 owner。保存Zでpanel上のpointer判定を変えない |
| 同 `40092` adjustment hover | 実効 owner。Depth等の別の抑止条件は維持 |
| 同 `40317` touch recognition | 実効 owner。本件の修正境界。tap/pinch driver を保存Zで停止しない |
| 同 `40786-40787` wheel診断 | 実効 active/aiming。実際のwheel処理で使う値を記録する |
| 同 `40891-40904` wheel分類 | 実効 active/aiming。Depthの通常wheel=ページ移動、Ctrl+wheel拒否は維持 |
| 同 `41082,41088` wheel適用 | 実効 active/aiming。重複するDepth専用guardを除き、保存倍率を動かさない |
| 同 `41396` wheel後のclick用transform | 実効 owner。通常描画のズーム投影だけを計算する |
| 同 `50546` spread clip | 実効 owner。Zの表示クリップかの判定 |
| 同 `50592` spread Z入口 | 実効 owner。保存Zから専用solveを開始しない |
| 同 `50615,50632,50647` spread solve/placement/gap | 実効 active。確定状態を描画へ反映するかの判定 |
| `app.rs:52810` adjustment panel reachability | 実効 owner。描画側との適格性を揃える |
| `ui_fullscreen/page_turn.rs:1817` animation適格性 | 実効 owner。Depth自体の紙めくり除外は既存の占有判定で維持 |
| raw flag の reset/代入 | state machine/lifecycleの書込みでありruntime consumerではない。変更なし |
| `cfg(test)` の直接設定・assert | fixtureと保存値の検査。runtime 判定へ置換しない |

**派生する入力・表示の確認（依頼18で訂正）**: 当初マウス canvas click を占有 owner 判定で無操作にしていたが、
Depthでも通常ページ送りを許可するのが正しい。タップ領域も Depth の通常ページ送りを許可し、360/Analysis を除く既存規則のまま。
初回タッチヘルプの開始／終了と上部バーのtouch target登録は `40317` の認識可否から派生するため、同じ実効値で揃う。
上部バーの表示自体には保存Zの直接consumerは無い。navigatorのDepth入場拒否とパネル固有の占有規則も変更しない。

回帰は実 Z press/release → D → Iでパネルを隠す → child viewer の egui-winit と同じ完全な
Touch/Pointer signature（Start・移動・primary press、End・primary release・PointerGone）→ 実 PageSide 移動 → D を通す。
Z未使用のDepthと移動先が一致し、移動中は実効ownerがDepth、解除後は保存active/factorを持つZoomModeへ戻ることを検査する。
別の owner matrix / cursor 回帰で、保存値と実効値の違い、および2D/360の既存優先順を固定する。
全件gateは実行せず、ClaudeCodeが使用中の `target/test-lib-full/` は参照・変更しない。
変更は7ファイル: `src/ui_fullscreen.rs`, `src/app.rs`, `src/ui_fullscreen/page_turn.rs`,
`src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`, `docs/depth-parallax-plan.md`,
`docs/display-pipeline.md`。状態field・keymap・通常2D/360の仕様は変更しない。

検証記録（全cargo/build/testに `MSBUILDDISABLENODEREUSE=1`）:

- 新規回帰2件を含む限定670/670成功、45.305秒、exit0 (`target/depth-b-touch-related.log`)。
  選択は `depth|panorama|pano|fs_zoom|touch|keymap|fs_cursor`。先の狭い選択で通ったmatrixは重複加算しない。
  初回はfixtureの借用順を修正し、次の狭い選択はmatrix成功・touch baseline失敗だった。
  touch fixtureの不完全なイベント列を、既存correlationが受理する完全signatureへ直し、
  時刻・App frame番号も進めた後に上記の全選択が成功した。製品の相関処理を緩めていない。
- `cargo check --offline -p mimageviewer --bin mimageviewer-core` 成功、45.59秒
  (`target/depth-b-touch-check.log`)。追加のsource変更はその後もtest fixtureだけである。
- workspace全体へ引数なしの `cargo fmt` を実行し、最終 `cargo fmt --check` 成功
  (`target/depth-b-touch-fmt.log`)。UI glyph checkerは危険な文字0 (`target/depth-b-touch-glyphs.log`)。
- 独立完成レビュー（GPT-6.1 Sol / xhigh）は最終fixture・670件の実行ログ・本分類表まで照合して
  ACCEPT、P1/P2残件なし。
- `scripts/build-dev.ps1 -PreserveRuntime` は exit0。normal feature set の core は2分08秒、
  付属4バイナリも成功。VCRT PE検査は runtime4 / PE5 成功 (`target/depth-b-touch-build-dev.log`)。
  確認用binaryは起動していない。実機では Z確定 → D → タッチのページ送り → D の拡大表示復帰と、
  通常2D / 360の従来操作を利用者に確認してもらう。起動方法・実profileの注意は上記を使う。
  HEAD `551040002` を維持し、stage/commit・製品起動はしていない。
- 作業制約の逸脱: reviewerがログ名探索で `rg --files target -g 'depth-b-touch*' -g '!test-lib-full/**'`
  を一度実行した。禁止領域のディレクトリ走査を完全には除外できないため「一切アクセスなし」とは扱わない。
  `target/test-lib-full/` のファイル内容は未読・未変更。利用者へ報告し、以後は指定済みログの直接参照だけに限定した。

#### 段 B 実機フィードバック: 画像上の待機表示 (2026-11-07、基点 `acc937311`)

利用者から「Dを押してから動き始めるまで反応がない」との実機報告を受けた追加。
§5.4の非モーダル箱を実装し、既存の取消・ページ送り・要求ownerは維持する。

- `app/depth_view.rs::depth_wait_status` が source phase と runtime snapshot から準備／計算中を一度だけ決め、
  パネルと箱が共有する。Ready / Failed / Depth以外はNone。既存の失敗文字列と通知は変更しない。
- `DepthViewState.wait_started_at` は常に存在する提示時計であり、新しいpending状態や生存sentinelではない。
  Ready / Failed後の新しい待ちだけをresetし、NotYet → 読み戻し → 推論、待ち中の別keyは継続する。
  既存 `animation_expansion_progress_visible` と同じ150ms gateを使う。
- `draw_feedback_toast` のviewer共通経路で、source採用とcurrent panel矩形の公開後に描く。
  パネルを隠しても表示し、表示中のpanelだけを避ける。上160pt／下48ptと現在の画像領域を使ってchromeを避ける。
  gate前は到達用wake、描画した箱だけ33ms再描画とし、hidden/minimized/terminalでは箱由来の予約を止める。
- `ui_depth_wait.rs` は状態・I/O・repaintを持たない純描画。アニメーション展開中表示に揃えた色・角丸・17pt見出し・
  細いバーと、実効割当の取消captionを描く。未割当captionは省略し、狭幅は折返す。
- 実Ready採用までのpass回帰で、既存Depth shader成功枝が素材traceを空にしてSingle layoutだけを返す不整合を検出した。
  `layout_rebound=true` / `sources_rebound=false` の既存debug assertionが失敗したため、assertを維持したまま、
  同じ成功境界で実際に描画したresourceを同じNavigation occurrenceのLive sourceとして公開するよう補修した。
  LegacyRenderedとDepthのnavigator／紙めくり禁止は維持し、通常2D／360の描画経路は変更しない。
  独立reviewerもこの素材／layoutの公表境界の構造修正に同意した。
- 初回fixtureのUI textures deltaの明示破棄と、raw textureではなく正本final sourceを使う準備を揃えた。
  Readyの回帰はcompleted pending → 実advance → R16 upload／Ready採用まで同passで通し、直後に箱が消えることを検査する。
  パネルとの非交差・上部／下部の予約帯・1093×614内のcontainsも実viewer出力で検査する。
- snapshotは `depth_wait_indicator_small_dark.png` を1枚追加。生成は該当testだけ `UPDATE_SNAPSHOTS=1`。
  既存534枚を事前SHA-256と照合し変更0。実装担当と独立reviewerがPNGを目視し、文字・取消行・バーに欠けや重なりがなく、
  上部表示と右パネルを避けて画像領域内に収まることを確認した（snapshotは模擬chrome、実placementは上記pass回帰で検査）。

変更は11ファイル: `src/app/depth_view.rs`, `src/depth_parallax.rs`, `src/ui_fullscreen.rs`, `src/lib.rs`,
新規 `src/ui_depth_wait.rs`, `src/app/depth_view_tests.rs`, `src/ui_fullscreen/depth_tests.rs`,
`tests/ui_snapshot.rs`, 新規 `tests/snapshots/depth_wait_indicator_small_dark.png`,
`docs/depth-parallax-plan.md`, `docs/ui-responsiveness.md`。

検証（全cargo/build/testに `MSBUILDDISABLENODEREUSE=1`）:

- libの追加9回帰を含む深度＋既存animation gateの限定110/110成功、14.056秒
  (`target/depth-wait-verified.log`)。初回のfixture修正と公表漏れの修正前失敗は別ログに残し、最終成功と区別する。
- 新規snapshot生成1/1成功、1.69秒 (`target/depth-wait-snapshot-generate.log`)。
  UPDATEなしの新規箱・既存3Dパネル・従来page waitの関連4/4比較も成功、2.12秒
  (`target/depth-wait-snapshot-compare.log`)。新規testはlib9件とsnapshot1件の計10件。
- core check成功、48.21秒 (`target/depth-wait-check.log`)、workspace全体の `cargo fmt` / `cargo fmt --check` 成功、
  UI glyph checkerは危険な文字0 (`target/depth-wait-glyphs.log`)。
- 独立完成レビュー（GPT-6.1 Sol / xhigh）は最終source・文書・110件ログ・新PNGを照合し ACCEPT、P1/P2なし。
- `scripts/build-dev.ps1 -PreserveRuntime` は起動中の確認用core（PID85540）を検出し、上書き前にexit1で停止
  (`target/depth-wait-build-dev.log`)。利用者のプロセスを終了せず、確認用binaryは今回の変更へ更新できていない。
  起動中のcore終了後に再buildが必要。コンパイル失敗とは区別する。

全件gateは実行していない。stage/commit・製品バイナリの起動、通常profileの設定変更もしていない。

#### 段 B サブ PC 実機確認（依頼18）への対応 (2026-11-07)

観測者はサブ PC 担当。正本は `C:/home/mimageviewer-subpc/depth-b/results/notes.md`。
修正前に確認した原因経路・検証方針を以下に記録する。

- クリック: `plan_fs_canvas_click` が `fs_exclusive_view_active(...).is_some()` で
  Depth の通常ページ送りまで拒否し、採用側も Depth の transform 停止分岐内で click action を捨てていた。
  360° のドラッグ所有と Depth の fit 固定を混同した分類が原因。§10 の過去の canvas 無操作の記録は誤りで、
  transform の変更停止と通常ページ送りを分離する。保存済み2D transform は判定にも使わない。
- 動き欄: egui 0.37 の ComboBox popup と親パネルがともに Foreground。親へのクリック／slider drag による
  Area の前面化と、保持された popup の相対順序の組合せで popup が親の背面に残る経路を確認。
  実 panel painter の「上下選択→強さ5%へdrag→再開」で、押下／解放を同じ RawInput に入れると、
  再開した左右の行の最前面layerが親パネルになることを修正前テストで再現した。
  開いている ComboBox の実 response から popup IDを導き、`set_sublayer` で親子関係を登録する。
  親とpopupのForegroundを維持し、前面化の順序競合を所有境界で除く。guard・retry・repaintは加えない。
  サブPCのpress/releaseのframe分割はログにないので、実機でも同じ分割だったとは断定しない。
- 導入確認: `toggle_depth_parallax` → `request_depth_addon_setup` の Checking は呼出元を持たず、
  `poll_depth_setup` が scan 完了の installed を常に Done へ変えていたため、起動ごとに再度 D が必要だった。
  確認元を Checking の typed state に持ち、D／上部ボタン由来の installed 完了だけ、同じ画像と入口条件を再照合して入る。
  環境設定・download 完了からは入らない。外部の画像変更経路も照合対象に含める。

動き欄の調査範囲は、製品panel／viewer描画・toggle、既知menuのclose、main modal遷移時のpopup close、
wheel消費と、egui 0.37 の ComboBox／Popup／Area／Memory／ScrollArea。
`vendor/egui/src/containers/combo_box.rs` の popup ID は response IDの `with("popup")`、
`containers/popup.rs` の menu Area はForeground、`containers/area.rs` は親へのpress／dragで前面要求を出し、
`memory/mod.rs::Areas::end_pass` は同一順序の相対関係を保持する。
`Context::set_sublayer` は同じOrderの子を親の直上に置く契約なので、この固定panel／menuの所有関係に適用する。
popup memoryはviewport単位だが、独自のdisable latch／popup state resetを要する証拠はなかった。
既存のwheel消費は入力deltaだけを変更し、modal遷移のcloseはdialog開始境界だけなので、今回の永続する背面化の原因ではない。

クリック以外も入口と採用先を確認した（行番号は依頼18の基点）。

| 箇所／操作 | 分類と確認結果 |
|---|---|
| `plan_fs_canvas_click` / `apply_fs_wheel_frame_proposal` 左クリック | 360°の意味に依存。Depthはfitの実効transformでページ送りを分類し、transform適用と独立して採用する |
| 同 double-click | 通常2Dのtransform resetは維持。Depthでは保存transformを参照／resetせず、通常のpage-side clickとして扱う。360°は既存pose resetへ |
| `handle_middle_drag_zoom` 中ドラッグ、Ctrl+左ドラッグ／pan | 占有。Depthはfit固定なので停止。短い中クリックの割当実行とは独立 |
| `40622` 中短クリック → `viewer_chrome::dispatch_viewer_middle_short_click` | 入力slotの意味。Depthの一律拒否なし。既定は未割当。共有mouse/ring resolverへ渡す |
| `35826,37235` Extra1/Extra2／browser back-forward → `apply_mouse_back_forward_button` | 入力slotの意味。Depthの一律拒否なし。既定のfolder履歴／従来互換の移動を保持 |
| `recognize_viewer_right_drag_frame` / `apply_fs_secondary_input` 右短クリック・ring・gesture | 入力slotの意味。Depth固有の一律拒否なし。右短クリック既定の全画面終了は維持 |
| `app/gamepad_input.rs::apply_ring_action` 共通mouse/ring採用 | DepthでZ/補正と衝突する操作だけ止め、metadata割当はDepthパネル切替へ。ページ移動／表示終了の割当は止めない |
| wheel / Ctrl+wheel | 通常wheelはページ送り、Ctrl+wheelはfit固定のため停止。今回変更なし |
| front pointer所有・popup／パネル／seek矩形・focus復帰抑制 | モードに依存しないchrome所有。Depthパネルを触った入力を背面のページ送りに流さない規則は維持 |

Checking のモーダル外経路も確認した。`app/startup_ops.rs::poll_activation_open_paths` →
`activation_open_precheck` → `document_open_modal_admission_blocked` は Depth setup を含めず、
関連付けopenの解決・採用が進み得る。`remote_ipc/ui.rs::service_remote_session_requests` も
別に acquisition を開始し、viewer close barrierへ進む。したがって確認完了時には画像の同一性に加えて
`remote_session_blocks_local_control()` を照合する。Remoteが取得中でまだ旧画像が残る場合も入場させない。
clipboardからの新規取込は既存の fullscreen／common modal の受付条件で止まる。
新しい外部openの保留機構や再試行は足さず、照合不一致なら一回の入場要求を終える。

再現テストは、クリックが実child viewerへ届く経路（1920×1080、Zの有無×パネル表示の有無の4条件で左右往復）、
導入確認の8件（実D・小さなローカルpackの非同期scan、未導入／破損、環境設定、画像／世代／context変更、
入口再拒否、Remote取得中、案内移行後の非継続）、実panelの選択→slider→再展開1件
（1920×1080／1093×614 × 同一／別frameの4条件）を追加した。
マウスfixtureは初回passのfocus取得を落ち着いた前面状態へ揃える。既存の300ms focus復帰抑制は変更しない。
独立コードレビューで、click採用の分離・scanのtyped origin・popup親子layerの修正が所有境界の変更であり、
症状だけを隠すguardではないことを確認した。最終の自動検証結果は以下に記録する。

| 検証 | 結果 |
|---|---|
| `MSBUILDDISABLENODEREUSE=1 cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_\|panorama\|pano\|canvas_double_click/)' --test-threads 4` | **215/215 成功**（追加10件を含む）。`target/depth-subpc18-final.log`。全件は依頼どおり実行しない |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core`（同env） | 成功。`target/depth-subpc18-check.log` |
| `cargo test --offline -p mimageviewer --test ui_snapshot depth_`（同env、更新modeなし） | **3/3 成功**。通常panel・1093×614の全control到達性・待機表示。`target/depth-subpc18-ui.log`。既存PNG変更なし、追加PNGなし |
| `cargo fmt`（引数なし）／`cargo fmt --check`／`python scripts/check_ui_glyphs.py` | 成功、危険glyph 0件 |
| `scripts/build-dev.ps1 -PreserveRuntime`（同env、offline） | 成功。core／remote／EPUB worker／MCP／music-videoを準備。core更新日時 2026-11-07 18:20:54、391,422,464 bytes。製品は起動していない |

修正前の再現と途中のfixture補正は `target/depth-subpc18-focused.log`／`depth-subpc18-regression.log` に残す。
初回のmotionテストでは Harness のevent queueがpress/releaseを別frameへ分けていたため成功していた。
RawInputへの同時注入へ直した版で背面化を再現してから製品を修正し、最終215件で回復を確認した。
commit／stageは行わず、全件台帳用ディレクトリには触れていない。
実機での最終確認は利用者側へ引き継ぐ（D後の左右click、上下選択→強さ5%→再選択、導入済み起動直後の最初のD）。

### 段 C の実装記録 (2026-11-07)

- `depth_export.rs` に凍結した要求、形式・寸法・フレーム時刻、要求専用channelとcancelを
  持つ `DepthExportJob` を追加した。設定画面から進行・終端画面までは App 共通の単一
  `Operation` が所有し、local-modal lease は開始前取得→終端画面の閉鎖まで保持する。
  共通modalとdocument-open admissionに登録し、長い処理をモーダルにして割込みの組合せを除いた。
- パネルの書き出しはReadyかつ現在の描画snapshotが一致するときだけ有効。
  設定画面は形式・元テクスチャを超えない長辺・自動4動作・MP4のループ数だけを持つ。
  720px未満の素材では選択肢が空にならないよう元の長辺を1つ表示する。
  強さ・周期・焦点・整形は現パネル設定を凍結し、別の編集項目は作らない。
  パネルのクリックはframe内の意図だけを返し、実際の3D描画後に素材・ページ・一覧世代を
  照合して設定画面へ入る。保存時も最新のReady素材と描画snapshotを一組で読み、
  設定画面中のresizeや背景の素材更新を古い市松寸法／素材と混ぜない。
  worker要求を作った後はその一組を固定する。要求を作れるまで保存操作は理由付きで無効。
- `depth_parallax_wgpu/export.rs` が専用pipeline / uniform / bind group / readbackを所有する。
  3Dの描画は画面と同じ `DEPTH_SHADER`。下地専用のshaderへ黒・白・市松を描いた後、
  既存と同じpremultiplied blendを行うことでgamma空間の `rgb + bg*(1-a)` とalpha=1を得る。
  3Dの描き方は複製せず、画面用uniformを共有しない。市松の升目は画面の8logical pxを
  出力寸法比で縮尺し、表示画像の左上を原点にする。画面の `paint_quad()` と同じく、
  90度回転は画像sampleへ適用し、市松の原点は表示矩形の左上に保つ。
- GIFはimage 0.25のNeuQuantと無限ループ、APNGはpng 0.18のanimated frame。
  GIFは累積時刻を10ms単位、APNGは1ms単位へ丸めて各frameへ差分配分する。
  既定3秒はGIF45frame合計3000ms、APNG45frame合計3000msとなる。
  任意の小数周期も形式が表せる時刻精度で累積端点を一致させる。
- `mvexport::VideoOnlyMp4Writer` / `Mp4Muxer::new_video_only` を追加し、H.264選択・
  BT.709変換・faststart・`PartialOutput`を再利用。音楽ビデオとRemoteの入口は維持する。
  新しいoffline専用encoder入口ではOpenH264のframe skipを無効化し、全frame数を保証する。
  GIF/APNGも同じ一時ファイル→公開へ統一する。取消・失敗・Dropは未完了出力を回収する。
- workerだけがGPU submit / `poll(Wait)` / encode / I/Oを実行し、進行・終端でrootを起こす。
  UIに常時repaintを追加せず、モーダル中の3D自動運動のrepaintを止める。
  進行窓×はcancel→実処理の終了待ち表示、終端画面を閉じるまでleaseを維持する。
  fullscreenの所有contextを閉じたときも同じ取消遷移を通す。別contextの終了では触れない。
  ナビゲーションの共通受付でも書き出しモーダルを照合し、同frameの遅延入力と
  スライドショーの自動送りを止める。OSの窓閉鎖は通して既存の取消境界へ渡す。
  UIはjoinしない。まれな失敗はログ＋通知だけでretry・復旧stateは足さない。

機能上の設計変更はない。小画像の元寸法選択、APNGの拡張子を`.png`とすること、下地を
先に描くpremultiplied blend、OpenH264のoffline専用設定は§7の契約を満たす実装上の具体化。
公開文書・製品ページ・privacyは指示どおり段Dに残す。
#### 段 C の検証記録

対象は `depth-parallax` の本段の未コミット差分。全Cargo / build / testに
`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`。
製品・通常profileのアプリは起動せず、GUI検査はheadless fixtureだけで行った。

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|mvexport\|export/)' --test-threads 4` | **286/286成功**、exit0。実行34.573秒（compile5分47秒）。`target/depth-c-related-final.log`。新規lib19件を含む。形式ごとのフレーム数・遅延・周期合計・寸法・loop、MP4のCFR timestamps／音声なし／BT.709／faststart、各形式の取消／Drop／既存出力保持、GPUの灰色128完全一致・半透明／黒白市松の画面照合（既存GPU helperの±1LSB）、専用uniform、lease／外部open／終了owner／resize／新素材／凍結／manual・slideshow排他、既存音楽ビデオ回帰を確認 |
| 新規snapshot2枚とgeometry/clickを限定して`UPDATE_SNAPSHOTS=1 cargo test --offline -p mimageviewer --test ui_snapshot depth_export_` | **3/3成功**、exit0。`target/depth-c-ui-update.log`。1093×614と546.5×307でスクロール末尾、固定操作行、Save／Cancel／タイトル×の実クリックを確認 |
| workspace全体へ引数なし`cargo fmt`後、UPDATEなし`cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、4.43秒。新規3と既存panel2・wait1。`target/depth-c-ui-compare.log` |
| snapshot目視／既存画像の照合 | 新規2枚は55,224＋29,457＝84,681 bytes。実装担当・親担当・独立reviewerが、文字・選択肢・ループ既定4・操作行・タイトル×・進行数に欠け／重なりがないことを目視。進行窓は自然高を維持。既存535枚のSHA-256変更0（`target/depth-c-snapshot-before.json`） |
| `cargo fmt --check` / `python scripts/check_ui_glyphs.py` / `git diff --check` | 全てexit0、危険なUI文字0。`target/depth-c-fmt-check.log`, `target/depth-c-glyphs.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、55.14秒。`target/depth-c-check-final.log` |
| `scripts/build-dev.ps1 -PreserveRuntime`（1回実行） | **成功、exit0**。normal feature setのcoreと付属4バイナリを構築し、VCRT／PE検査はruntime4・PE5成功。`target/depth-c-build-dev.log`, `target/vcrt-pe-reports/dev-runtime.json`。確認用coreは`target/dev-runtime/mimageviewer-core.exe`（391,680,000 bytes、2026-11-07 19:26 JST更新）。製品は起動していない |
| 独立コードレビュー（GPT-6.1 Sol / xhigh、別context） | **最終ACCEPT、未解決P1/P2なし**。要求凍結、frame-local受付、OS closeと兄弟context、lease、GPU、時刻、既存MP4経路、最終286/286とUI6/6のログを照合。最新PNG2枚も目視し、既存535枚不変を確認。全件gate／実機検収は別担当 |

途中の失敗と補正は最終成功と区別する。初回型検査のUI参照外し1件、test compileの
FFmpeg packet iteratorのResult展開2件を修正した。初回focusedは40件中39成功／GPU1失敗
（`target/depth-c-focused.log`）。切り取り後のpaint_rectを固定48×64と仮定したfixtureを、
実paint_rectの24×48・実originを使う比較へ直した。併せて画面の市松はquarter-turnで回転せず
表示矩形左上が原点であることを確認し、exportの下地も揃えた。最終286件で再確認済み。
UI初回は3件中2成功／geometry1失敗。自然高のWindowよりScrollAreaのinput/clip範囲が
大きく残るため、設定／終端は実viewportを確保し、短い進行表示は自然高の直接描画とした。

sourceレビューで、fullscreen終了時の取消漏れ、ページ・回転・resizeと受付順序の混合、
設定画面中の新素材、slideshowによるモーダル迂回を発見し、上記の所有境界へ接続した。
`ctx.data_mut`内のpass番号参照によるlock再入候補も、pass取得をclosure外へ移して解消した。
後者はコードでの発見で、製品の実機deadlockを観測したという意味ではない。
新規検査はlib19＋integration3＝22件。全件gateは依頼どおり実行せずClaudeCodeへ引き継ぐ。

利用者の実機確認は次を対象にする（製品は起動していない）:

- Readyの3Dパネルから設定画面を開き、MP4／GIF／APNG、動き、長辺、MP4のループ数を選んで保存する。
  マウス追従からは左右が初期値、APNGには容量の注記があることを確認する。
- 保存したGIF／APNGの繰り返しと周期、MP4の無音再生・指定ループ数、回転と透過下地の見た目を確認する。
- 進行中の取消・進行窓×・元のfullscreen窓の閉鎖で中断し、途中ファイルが残らないことを確認する。
  完了／取消画面を閉じるまでページ移動・Remote取得・外部からのopenが止まり、閉じると復帰する。
- 設定画面を小さくしたり表示窓をresizeしてから保存し、操作ボタンに届くことと市松の縮尺が合うことを確認する。

実機起動はインストール版／tray常駐版を終了してから、repository rootで
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
このnormal feature setのcoreは実利用中の`%APPDATA%\mimageviewer`を使い、設定・データを更新し得る。
single-instance mutexも通常版と共有する。段Dの公開文書・実機検収・全件gateは本段の実施済み結果に含めない。

変更ファイル（段Cのみ、stage／commitはしない）:

- 状態・描画・worker: `src/depth_export.rs`, `src/depth_export/tests.rs`,
  `src/depth_parallax_wgpu.rs`, `src/depth_parallax_wgpu/export.rs`, `src/lib.rs`。
- UI・App接続: `src/app.rs`, `src/app/depth_export.rs`, `src/app/depth_view.rs`,
  `src/app/depth_view_tests.rs`, `src/app/update_phases.rs`, `src/ui_depth_export.rs`,
  `src/ui_depth_parallax.rs`, `src/ui_fullscreen.rs`, `src/ui_fullscreen/viewer_chrome.rs`。
- MP4共用部品: `src/video/mvexport/driver.rs`, `src/video/mvexport/mod.rs`,
  `src/video/mvexport/mux.rs`, `src/video/mvexport/video_only.rs`,
  `src/video/mvexport/video_only_tests.rs`, `src/video/stream/encoder.rs`。
- UI検査: `tests/ui_snapshot.rs`, `tests/snapshots/depth_export_settings_small_dark.png`,
  `tests/snapshots/depth_export_progress_small_dark.png`。
- 文書: 本書、`docs/architecture-overview.md`, `docs/async-architecture.md`,
  `docs/ui-responsiveness.md`, `docs/music-video-export-plan.md`, `docs/detached-rework-plan.md`,
  `docs/ui-snapshot-policy.md`。

#### 段 C 独立レビュー1回目 P2: 正常終了時の一時ファイル回収 (2026-11-08)

基点は段C commit `9f38dcd85` と master merge `c4425da6d`。指摘は正当。
旧実装はspawnの`JoinHandle`を破棄し、JobのDropでcancelを立てるだけだった。
プロセスがGPU待ち／圧縮の途中で終わると、workerの`PartialOutput`がDropされる前にthreadが
消える。従来のDropテストは別の完了通知を待ってから検査しており、正常終了の回収境界を検証していなかった。

**既存経路への接続**:

- `src/lib.rs` の `run_native` 外で `DepthExportProcessOwner` を保持し、Appへは受付clientだけを渡す。
  collection／book-composition／spread-shiftと同じ、既存 `finish_run_native` cleanupへ載せた。
  Ok／Errの共通cleanup先頭でcancel→handle取出し→lock外joinを完了し、他ownerの長いdrainと
  scriptの最終`process::exit`より前に回収する。outer ownerのDropも同じ回収関数を使う。
- `App::on_exit` と AppのDropの先頭ではclientをretireし、受付を閉じて登録済みworkerへcancelを立てるだけ。
  UI callback／UI frame内でjoinしない。Job receiverが先に失われてもprocess ownerがhandleを保持する。
- 受付確認・spawn・handle登録を一つの短いmutex区間で行い、retire／drainとの間に登録漏れを作らない。
  worker本体はこのmutexに触れない。完了済みthreadのhandleは次の受付で`is_finished`を確認して破棄し、
  activeなhandleは外側の終了回収へ残す。汎用reaper・retry・待機deadlineは追加しない。
- 既存TRT reaperはpool専用で、exportの回収口にはならない。音楽ビデオは別アプリ
  (`musicvideo-editor`) のClose受付が `ConfirmExport → StoppingExport → receiver終端` を待つ個別方式。
  これらを複製／変更せず、既にある本体のprocess cleanupを利用した。

**照合した終了入口**（新しいroot Close policyは追加しない）:

| 入口 | 到達先と回収 |
| --- | --- |
| main窓×／OS Close、通常のMainWindowClose割当 | root lifecycleのClose policyが実終了を受理するとeframe `on_exit` → `run_native`復帰 → 共通cleanupでjoin |
| tray「終了」 | `tray.rs` のquit flag＋WM_CLOSE → tray常駐へのhideを迂回 → 同じon_exit／共通cleanup |
| ファイルメニュー「終了」、QuitApplication割当（gamepad／ringを含む） | `request_application_quit` → shutdown flag＋root Close → 同じ経路 |
| installerのshutdown event | `single_instance.rs` が同じshutdown flagとroot Closeへ配送 → 同じ経路 |
| fullscreenだけ閉じる／兄弟readerを閉じる | 既存のcontext照合付き取消を維持。sourceのworkerはcancel、兄弟は不変。プロセス受付は閉じず、継続利用できる。最終app終了時は残るhandleも共通cleanupで回収 |
| trayへ隠す／CloseがCancelCloseされた場合 | 実終了ではないためprocess ownerをretireしない。既存のhide／fullscreen閉鎖policyを維持 |
| Remoteのセッション終了・ローカルへ戻る | Remote／agent IPCに独立の本体Quit命令は無い。セッション退役とプロセス終了を区別。Remote稼働中の本体終了も通常のon_exit／共通cleanup |
| rendererのErr復帰／creator失敗、on_exitを通らないApp Drop | App Dropのcancelと、Ok／Err共通のouter cleanup。App生成前の失敗ではworker未起動 |
| test-scriptの正常終了 | `exit_after_run_native`より前に共通cleanupが完了 |

`--play-test` の診断用直接`process::exit`、GUI起動前の早期exit、別worker／開発CLI、
強制終了・abortは通常の3D書き出し中の終了入口に含めない。異常終了の再試行／孤児復元機構は追加しない。

**cancel確認の区切り**:

全形式で毎frameのGPU描画／読み戻しの前後にcancelを確認し、確認後は次frameへ進まない。
GIFはそのframeのNeuQuant＋圧縮、APNGはそのframeの`write_image_data`、MP4はそのframeの
BT.709変換・encoder送信・packet排出が一つの区切り。MP4 finishの入口と全形式のpublish直前にも確認する。
したがって残り全frame／全loopを処理してから止まる経路はない。既存のGPU `poll(Wait)`、
圧縮、FFmpeg／ファイルI/Oの一呼出し自体は中断できず、その現在の区切りが返るまでは待つ。
最大長辺2160の1frameを区切りとし、時間制限つきの待ち・polling sleep・retryは足していない。
正常に戻る呼出しの完了と資源破棄まで外側で回収する。GPU driver等の異常停止にまで時間上限を約束するものではない。

変更ファイル: `src/depth_export.rs`, `src/depth_export/tests.rs`, `src/app.rs`,
`src/app/depth_export.rs`, `src/lib.rs`, `docs/async-architecture.md`, 本書。
**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_export::tests::depth_export_(process\|outer\|job_drop)/)' --test-threads 2` | **4/4成功**、exit0、実行6.441秒（初回compile8分19秒）。新規3件と既存Job Drop。`target/depth-c-exit-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|mvexport\|export\|root_lifecycle\|tray.*quit/)' --test-threads 4` | **303/303成功**、exit0、48.099秒。新規3件を含む終了回収、従来3D/GPU/全形式/音楽ビデオとroot lifecycle／tray回帰。先行4件はこの303件に含まれ、重複加算しない。`target/depth-c-exit-related.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/^run_native_failure::/)' --test-threads 4` | **14/14成功**、exit0、0.551秒。既存の正常／Err／scriptとcleanup順の終了境界を確認。`target/depth-c-exit-boundary.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、1分14秒。`target/depth-c-exit-check.log` |
| 最後にworkspace全体へ引数なし`cargo fmt` → `cargo fmt --check` | 成功、exit0。`target/depth-c-exit-fmt.log`。変更のないsourceで上記test結果を再利用 |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方exit0、危険なUI文字0。`target/depth-c-exit-glyphs.log` |
| 独立review（Sol / xhigh、同じreview contextで継続） | **最終ACCEPT、未解決P1/P2なし**。登録と終了の直列化、UIで待たない取消、outer join、実部分ファイル6条件と303/303・14/14ログを照合。reviewerによるCargo重複実行なし |

新しい終了回収testは、GIF／APNG／MP4それぞれで本物の途中ファイルが存在する区切りまで進め、
workerをgateで停止したまま実`eframe::App::on_exit`またはApp Dropを呼ぶ（6条件）。
UI側は完了を待たずcancel／新規受付拒否となり、worker再開後は別の完了receiverを待たず
`shutdown_and_join`だけで回収を確定する。return時のworker完了・一時ファイル消失・既存保存先保持を確認した。
spawnとshutdownの競合、shutdown後に残るclientの拒否、on_exitを経由しないouter owner Dropも検査した。
全件gateは指示どおりClaudeCode担当。stage／commit／製品起動は未実施、build-devも指示により実行しない。

#### 段 C 利用者決定: SNS 投稿向けの選択肢と既定 (2026-11-08、基点 `eda886076`)

利用者が実機で書き出し・保存を確認したうえで、次の3点を採択した。SNS の名称・上限は UI に表示しない。

- 長辺に1280 pxを追加。利用者提示の X 推奨（横1280×720／縦720×1280／正方形720×720）を理由に、
  長辺1280を選べるようにする。縦横比は従来どおり保ち、元のテクスチャより拡大しない。
- 形式ごとの長辺の既定を MP4 1280／GIF 720／APNG 1080 px とし、形式を切り替えたときにも
  共通の候補解決で既定へ戻す。利用者提示の GIF 上限（X 15MB／pixiv 16MB）に対し1080では大きくなりやすいため、
  GIF は720へ下げる。SNS ごとのプリセットは仕様変更が頻繁で中身もほぼ同じになるため作らない。
- 保存完了事象を保存先と任意のバイト数にし、worker がファイル公開後に metadata を取得する。
  取得失敗は通知のサイズ省略だけとし、再試行しない。トーストと完了ダイアログで既存の MB／GB 表記を共用する。

MP4 のループ回数の既定4回は、繰り返し再生しない投稿先でも短すぎないようにする利用者決定として維持する。
fps、MP4 12Mbps、縦横比、書き出しの非永続性、既存のモーダル／終了時回収経路は変更しない。
設計との差異なし。形式の押し直しでは手動の長辺を保持し、実際に形式が変わったときだけ既定へ戻す。

**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_export/)' --test-threads 4` | **20/20成功**、exit0、11.043秒（compile6分16秒）。`target/depth-c-sns-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export/)' --test-threads 4` | **292/292成功**、exit0、42.017秒。先行20件を含み重複加算しない。3D・全形式・音楽ビデオの回帰。`target/depth-c-sns-related.log` |
| `UPDATE_SNAPSHOTS=1 cargo test --offline -p mimageviewer --test ui_snapshot depth_export_settings_small_dark -- --exact` | **1/1成功**、exit0、3.27秒（compile7分12秒）。指定の設定PNGだけ更新。`target/depth-c-sns-snapshot-update.log` |
| 更新指定なし `cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、2.31秒。設定／進行の通常比較と1093×614・546.5×307での操作可視性・本文末尾への到達・実クリックを含む。`target/depth-c-sns-snapshot-compare.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、47.72秒。`target/depth-c-sns-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。sourceに追加差分なし。`target/depth-c-sns-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI文字0。`target/depth-c-sns-glyphs.log` |

長辺の全閾値の前後、元の長辺4000／1280／1279／1000／600の実UIクリックによる形式切替、
同じ形式を押し直したときの手動長辺保持を検査した。worker の実一時ファイル→rename 後の新しいサイズが
完了事象に載ること、公開後にファイルが消えた場合はサイズなしで成功すること、App の完了事象処理で
保存先と任意のサイズがトースト・完了メッセージの両方に入ることを検査した。

更新した`depth_export_settings_small_dark.png`（1093×614、56,335 bytes）を生成後に目視した。
1280 pxの選択表示、MP4、左右、ループ4回（約12.0秒）、説明文が読み取れ、保存先を選ぶ／キャンセル／
タイトルの×が画面内に収まる。元の長辺2000のfixtureなので2160は表示しない。他のPNGは変更しない。
製品の起動・実機操作は実施していない。

変更ファイル: `src/ui_depth_export.rs`, `src/depth_export.rs`, `src/depth_export/tests.rs`,
`src/app/depth_export.rs`, `tests/snapshots/depth_export_settings_small_dark.png`, 本書。
未解決事項なし。lib全件gateとbuild-devは指示どおりClaudeCode担当。stage／commit／製品起動は未実施。

#### 段 C サブ PC 依頼19: モーダルの層と初期サイズ (2026-11-08、基点 `b26f1b969`)

修正前にコードで特定した原因を記録する。実機の観測者はサブ PC 担当（RTX 4060 Ti、1920×1080、100%）。
`eda886076` での記録は `C:\home\mimageviewer-subpc\depth-c\results\notes.md` と同フォルダの
`screens/NG1*`・`screens/NG2*`。記録とNG2の前後画像を読み取り専用で照合した。

- **NG1**: `ui_depth_export::draw` は空の `Modal` と `Window` を独立した Foreground Area にしており、
  親子関係を登録していなかった。egui 0.37 の `Area::begin` は外側の backdrop のクリックでも
  `move_to_top` を実行する。同一 order の順序は `Areas::end_pass` に保存され、
  `Memory::allows_interaction` は top modal より下の Window の入力を止める。逆に backdrop より上に
  残った3Dパネルは操作できる。Windowだけ開き直しても同じ layer の順序を再利用する。
  既存 `draw_export_progress_dialog` は通常の Window だけで、この二層の問題を持たない。
  モーダルの実装は既存 `ui_dialogs::agent_integration::draw_registration_dialog` と同じく、
  backdrop を親、Window を sublayer にして一つの前面グループとして扱う方針とした。
- **NG2**: 同じ `depth_export_dialog` ID を設定340／進行170／終端220の各phaseで使い、
  `default_size` と `max_size` だけを指定していた。egui の `Resize::begin` は既存stateがあると
  defaultを再採用しないため、進行／終端から設定へ開き直しても小さい割当が残る。
  本文もその `available_height` に依存する。操作行の控除は `interact_size.y` ベースの概算で、
  実フォント＋paddingによるボタン高とseparator／item spacingの実占有を計測していない。
  `Resize` が前回の `last_content_size` まで自動拡大するため、入力で次のframeが来るたびに成長し得る。
  phaseと画面寸法からResizeの割当を毎回固定し、操作行を既存のフォント／折返し計測で予約する。
  設定／終端は本文の親領域も明示確保する。短い進行表示の外形は従来どおり本文の自然高とする。
  追加のrepaintやstate消去では隠さない。
- **注・Shift+B（2026-11-08利用者決定で更新）**: 依頼19の調査時は§6.5の占有制限をDepthにも適用していた。
  背景だけを止める描画上の必然性は無く、利用者がDepth中の切替を許可したため、今回キーとリングの両方を通す。
  キーは `is_panorama_mode_active` のみで抑止し、リングはDepth専用の拒否集合から背景切替を除く。
  Panoramaのキー抑止は維持。Panoramaのリングは基点 `58816f030` でも背景切替を許可しており、変更していない。
  下地は `try_paint_depth` へ毎描画渡し、書き出しは従来どおり開いた時の下地を保持する。

独立Sol/xhighの実装前レビューは、NG1の層所有とNG2のサイズ決定を直す構造修正として合意。
実Depthパネルとcanvasを背面にした入力、全phaseと再open、2160を含む素材、全形式、初回と後続frameのgeometry、
通常画面の明示Fits・小画面のscroll末尾到達を回帰対象にした。

実装は `ui_depth_export::draw` のbackdrop／Windowを `set_sublayer` と親の `move_to_top` でまとめた。
設定・進行・終端すべてと、main／fullscreen双方の既存描画入口に同じ処理を使う。
phaseごとに `fixed_size` でResize割当を決め直し、共通の `startup_dialog_button_rows_height` を
`pub(crate)` にしてボタンの実フォント・padding・折り返しを計測する。本文→separator→操作行の
2つのitem spacingと明示したseparator高2を控除し、設定／終端のScrollAreaには親の領域を確保した。
既存ダイアログのhelperの計算内容は変えず、guard・retry・追加repaint・Window stateの消去は足していない。

新規 `ui_depth_export_tests` の回帰:

- 実3Dパネルとcanvasがモーダルなしでは操作できるpositive controlを置き、モーダル中の外側クリックと
  パネルの「書き出し…」クリックを、同一frame／別frameのpress-release × 1093×614／1920×1080で検査。
  背面の操作が起きず、設定のGIF→APNG・保存・キャンセル、進行のキャンセル、終端の閉じる、全phaseの
  ドラッグ・タイトル×、Esc後の再openが動作する。backdrop直上がWindow、パネルはmodal未満であることも検査。
- 同じContextの設定→進行→終端→閉鎖→設定について、source長辺4000・全3形式で、最初と後続7frameの
  rect／操作行がidle・pointer移動によらず一致し、画面内に収まることを検査。
  通常の本文は `ScrollLayout::assert_height(Fits)` として不要なscrollを許さない。
- 長い保存先／失敗文は両解像度で初回からのgeometry不変・画面内包含・`Scrolls`を検査。
  既存integration到達性検査も、1093×614では明示Fits、546.5×307ではScrolls＋末尾到達とする。

**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/ui_depth_export::regression_tests/)' --test-threads 2` | 先行の入力／初回geometry **2/2成功**、exit0、1.352秒（compile6分20秒）。`target/depth-c-subpc-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export/)' --test-threads 4` | 最終の新規3件を含む **295/295成功**、exit0、44.811秒（compile3分38秒）。先行2件はこの中に含む。3D／GPU／全形式／音楽ビデオ／終了回収の回帰。`target/depth-c-subpc-related.log` |
| `UPDATE_SNAPSHOTS=1 cargo test --offline -p mimageviewer --test ui_snapshot depth_export_` | **3/3成功**、exit0、1.80秒（compile1分32秒）。設定／進行の2枚のみ更新。`target/depth-c-subpc-ui-update.log` |
| 更新指定なし `cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、1.22秒。強化したFits／小画面到達性と通常画像比較。`target/depth-c-subpc-ui-compare.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、19.03秒。`target/depth-c-subpc-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-c-subpc-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI文字0。`target/depth-c-subpc-glyphs.log` |
| 独立レビュー（Sol / xhigh、設計と同じcontextで継続） | **最終ACCEPT、未解決P1/P2なし**。最終source・295件・UI3件/6件のログとPNG2枚を照合。重複Cargo／製品起動なし |

PNGは1093×614の設定（56,335 bytes）と進行（29,457 bytes）を更新し、生成後に両方と変更前のPNGを目視比較した。
操作行・区切り線・下端の配置が高さ計算に合わせて変わる。設定のMP4／1280／左右／ループ4と本文、
進行のフレーム数・キャンセル、双方のタイトル×が画面内に収まり、文字・操作の見切れがない。他のPNGは変更しない。
サブ PC の観測と修正後のheadless検証を区別し、製品起動・修正後の実機操作は実施していない。

変更ファイル: `src/ui_depth_export.rs`, 新規`src/ui_depth_export_tests.rs`, `src/ui_dialogs/mod.rs`,
`tests/ui_snapshot.rs`, `tests/snapshots/depth_export_settings_small_dark.png`,
`tests/snapshots/depth_export_progress_small_dark.png`, 本書。
未解決のコード上の指摘なし。Shift+Bは上記の既存設計に従って維持した。
全件gateはClaudeCode担当。今回の指示に従いstage／commit／製品起動／build-devは実施していない。

#### 段 C 独立レビュー3回目: 下端のphase拡大 (2026-11-08、基点 `85062d72e`)

原因は `ui_depth_export::draw` の同一Window IDと、eguiの寸法採用の順序。
`AreaState` が保持する前回の外形で `Area::begin` が画面内への位置制限を済ませた後に、
`Window` の `fixed_size` が新しいphaseの外寸を適用する。下端の進行→終端、下端の終端を閉じて設定へ
開き直す場合は、新しい本文・操作行が初回frameだけ画面外へ伸び、次frameに位置が変わる。
先行回帰は中央でのphase遷移だけだったため、この保持位置の条件を覆っていなかった。

`current_pos` だけを渡す方法も検討したが、`Area::begin` はその後にドラッグ原点＋累積deltaで位置を
上書きするため、ドラッグを続けたまま非同期完了すると補正が失われる。
設定／終端だけ、既存 `AreaState::load` の既知寸法を新外寸へ同期し、左上位置を維持したまま
公開APIの `Areas::set_state` で同じlayerへ描画前に渡す。これで `Area::begin` 自身が新外寸を使って
ドラッグとconstrainを処理する。`set_state` は `Area::end` 自身と同じAPIであり、このpassで表示する
同じWindowのstateを更新する。初回／復元時の寸法未確定stateはeguiのsizingに任せ、作成・消去しない。
新しい位置stateは持たず、default_pos・ドラッグ・constrainを維持する。自然高の進行表示には固定割当170を
外形として使わず、下端へ移動した進行窓を不要に押し上げない。追加repaintは行わない。
この方針は独立Sol/xhighの実装前レビューでも確認した。

Shift+Bの注の指示元を利用者から設計担当（ClaudeCode）へ訂正した。利用者への確認中のため挙動は変更しない。
回帰は1093×614の同一Contextで、実pointer入力で進行窓を下端へドラッグし、解放後と押したままの両方で完了へ遷移する。
さらに下端の終端を実ボタンで閉じ、設定を再表示する。`Harness::step` を1回だけ実行して初回の矩形を採り、
初回＋後続5frameでWindowと操作行の画面内包含と位置不変を確認する。進行窓の下端位置が不要に上がらないことも検査する。

**検証結果**（全Cargoに`MSBUILDDISABLENODEREUSE=1`、build/testは`CARGO_BUILD_JOBS=1`）:

| 実行 | 結果・証拠 |
| --- | --- |
| 修正前 `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth_export_bottom_drag/)' --test-threads 1` | 新規回帰 **1/1失敗（意図したred）**、exit100、0.294秒（compile3分36秒）。初回rectが`[[337,481],[757,701]]`となり画面高さ614を超える。`target/depth-c-bottom-red.log` |
| 同じ限定コマンド、修正後（ドラッグ継続中の条件も追加） | **1/1成功**、exit0、0.466秒（compile3分41秒）。`target/depth-c-bottom-focused.log` |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export/)' --test-threads 4` | **296/296成功**、exit0、45.951秒。新規回帰1件と従来の3D／モーダル／各形式／終了回収を含む。先行1件はこの中に含み重複加算しない。`target/depth-c-bottom-related.log` |
| 更新指定なし `cargo test --offline -p mimageviewer --test ui_snapshot depth_` | **6/6成功**、exit0、1.94秒（compile1分31秒）。通常の設定／進行PNGと小画面到達性は不変。PNG更新なし。`target/depth-c-bottom-ui.log` |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、18.14秒。`target/depth-c-bottom-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-c-bottom-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI文字0。`target/depth-c-bottom-glyphs.log` |
| 独立レビュー（Sol / xhigh、実装前と同じcontextで継続） | **最終ACCEPT、未解決P1/P2なし**。最終差分・red／green・296件・UI6件と最終検査の証拠を照合。重複Cargo／製品起動なし |

変更ファイルは `src/ui_depth_export.rs`, `src/ui_depth_export_tests.rs`, 本書の3本。
Shift+Bの挙動、PNG、他のダイアログの実装は変更していない。
stage／commit／製品起動／build-devは指示どおり実施していない。全件gateはClaudeCode担当。

#### 段 C 追補: Depth 中の背景切替 (2026-11-08、基点 `58816f030`)

利用者決定に従い `handle_fs_key_input_with_operation` の背景キーの抑止を
`is_panorama_mode_active` へ限定し、`apply_ring_action` のDepth拒否集合から
`ImageBackgroundCycle` を外した。他の占有制限は変更していない。
**前提との差**: Panoramaのキーは従来どおり停止するが、リングは基点でも停止していなかった。
「Panoramaの既存挙動を変えない」を優先し、この差を回帰テストに明示した。

素材・下地の監査結果:

- 通常の背景変更は `fs_bg_style` (`ui_fullscreen.rs`:49784付近) が現在値から下地を作り、
  静止画描画:28778付近 → `draw_fs_image_with_receipt` → `try_paint_depth`
  (`app/depth_view.rs`:252) が毎回受け取る。下地は `paint_image_underlay` でcallbackの直前に描く。
  `DepthSourceKey` はtexture IDだけで、`DepthRenderParams`・GPU色/深度資源・LRUに下地を焼き込まない。
  AI無効なら同じresource IDのままでよい。GPU回帰は同一callback・uniform・色/深度で
  黒→白→市松→黒を描き、各下地の不透明/半透明画素を2Dと比較する。
- AIアップスケール有効時の既存キー／リングは黒/白2段で、`clear_adjustment_caches`
  (`app.rs`:93984) → `clear_final_pipeline_caches_for_idx_inner`:85120 を呼ぶ。
  adjustment・final AI/composite・comic等の派生結果を失効し、pendingを取消す。
  `final_ai_key_for_pixels`:87267 と `final_composite_key_for_pixels`:89011 はbgをkeyに含む。
  実行側 `ai/final_pipeline.rs`:142付近は `background_mode` の黒/白へ合成してから拡大する。
  再合成は `ctx.load_texture` で新規IDを作る (`app.rs`:90069 / 90356付近)。
  `TextureHandle::set` の使用元も確認し、final/processedの同一ID更新は無く、該当した
  `ui_adjustment_panel.rs`:8896 は編集パネルのmask preview用だった。
- **深度推論はやり直しになる**。新規texture IDを `resolve_fs_processed_payload_and_depth`
  (`ui_fullscreen.rs`:13268) がそのまま素材として返し、`DepthViewState::select_source`
  (`depth_parallax.rs`:342) が旧slot/color/renderedを退役する。LRUは旧IDで新素材には一致しないためreadbackへ進む。
  未完成の合成は `NotYet(IncompleteComposite)` → `wait_for_source`:329 で2D待機となる。
  これは§4.1の「表示中の画素が変われば新規素材」に従う動作で、推論を省く変更は行わない。
  ここでいう再推論は**実際に背景を焼き込んだAI出力が置換される場合**。AI有効フラグだけで処理が無く、
  無変換素材の共有IDが保たれる経路では再推論は不要。同じ表示画素のkeyを維持するのも§4.1どおり。
- **古い素材を参照したまま3Dを描かない**。`try_paint_depth`:275付近は現在payloadのtexture IDと
  `resource.key.texture_id` が違えばcallbackを作らない。GPU callbackもcontext / key / resource IDを照合する。
  旧深度がLRUに残っても表示中素材として使用されない。**3Dモード自体は終了しない**。
  背景値やキャッシュ失効は入口・継続条件ではなく、素材の段階だけがReady→待機→新規Readyへ進む。
- 書き出しは変更なし。`depth_export.rs::admit_depth_export`:670付近が開いた時のdisplay/settingsとleaseを持ち、
  `ExportCapture::into_request`:547付近が下地を要求へ写す。Settings中の同一ページcapture更新は既存どおりだが、
  `app.rs::modal_dialog_block_reason` の `depth_export` と `document_open_modal_admission_blocked`:42338、
  `ui_fullscreen.rs`:34893付近のキー早期returnが背景変更・移動を止める。
  Runningはその要求だけを見るため、書き出しとShift+Bの競合を新たに扱う必要はない。
  既存の `depth_export_freezes_display_and_retains_modal_admission_until_dismissal` 等でlease/受付停止を確認。

公開文書の監査は `htdocs/mimageviewer/manual/` に `3D` / `Shift.*B` を検索した。
3D写真節は段Dで未追加のため今回同期すべきキー記述は無く、Creative 3D LUTの記述は別機能。
既存の背景キー説明は今回の動作と矛盾しない。キー仕様と表示パイプラインの3D節を同期した。
MainWindow案は§6.4.1だけに記録し、Bの製品コード・detached-rework-plan §11は変更していない。

検証記録:

- handler回帰2件を追加: キー／リング×AI有無の循環・不透明画像の同じ案内・Depth owner維持、
  Panoramaの既存キー抑止／リング許可。既存GPU回帰1件を黒・白・市松の連続切替へ拡張。
- 実child viewer回帰1件を追加: 毎passの下地shapeと同一Depth resource、AI有効cycleのfinal cache失効・
  置換素材のtexture ID照合・旧GPU slot/callback退役を確認。実モデルは使わず、背景焼込み済みの
  `FinalAiEntry` と既存fixtureの失敗済みlazy runtimeで隔離する。
  初回はAI有効フラグだけでtexture IDの変更を期待して1件失敗した。無変換素材のID共有という正しい挙動だったため、
  製品コードを変えず、実際に背景を焼き込んだAI結果を与えるfixtureへ修正した。
- `cargo nextest run -p mimageviewer --lib -E 'test(/depth|background|bg_cycle|ring/)'`: **731件成功**。
- 上記成功後に追加した実viewer回帰は `cargo nextest run -p mimageviewer --lib -E 'test(depth_background_cycle_actual_viewer)'`
  で **1件成功**（fixture修正後、実行1.357秒）。既存731件の実行時から製品コードの追加変更はなく、結果を再利用した。
- `cargo test -p mimageviewer --test ui_snapshot`: **245件成功**。
- `cargo check -p mimageviewer --bin mimageviewer-core`: 成功（既存warningあり）。
- ワークスペース全体の `cargo fmt`、`git diff --check`: 成功。
  `python scripts/check_ui_glyphs.py`: dangerous glyph **0件**。
- `MSBUILDDISABLENODEREUSE=1`。全件lib/full gateは指示どおり未実施。
  build-dev・製品起動・stage・commitも行わない。UI文言の追加変更はない。
- 独立GPT-6.1 Sol / xhighレビュー: BのF11参照誤りを訂正し、未解決指摘なし。
  Bの採択とdetached構造合意は別途ClaudeCodeの判断待ち。

#### 段 C 続き: MainWindow 対応・案1 (2026-11-08、基点 `29bb7f54b`)

設計担当 ClaudeCode の採択と独立 Sol / xhigh の構造合意に従い、§6.4.1 の案1を実装した。

- `app/depth_view.rs::depth_presentation_eligible` に projected main context と
  `MainWindow | Fullscreen` の共通適格性を置く。入口の pure input はこの結果1つを受け取り、
  context と presentation を別々に判断する経路を持たない。`DepthEntryBlock` の画像表示・メイン表示の文言を更新した。
- `commit_viewer_presentation` は実切替または不適格な切替先で既存 setter を通して終了する。
  同じ適格な presentation では要求・LRU・slot・動きの時計を維持する。終了後のDは空LRUで新規開始。
  適格な切替先では終了トーストに実効キー（既定D）の再入場案内を出し、割り当てなしなら上部3Dボタンを案内する。
  対象外の別窓には再入場できるとの案内を出さない。
- MainWindow の canvas・fit・下地・回転・パネル・待機表示・マウス追従・書き出し・modal は
  既存共通経路を利用する。ROOT tail・入力側のOFF・推論通知先移管・追加repaint・fork/parkの変更はない。
- 状態遷移の全context/presentation matrix、同じ表示への再確定、F11両方向、F12、close/fork/parkを検査する。
  新しい遅延reply fixtureは本物のper-request channelを保持し、ReadingBack / Estimating 中のF11→D後に
  旧要求の完了が新owner／slotへ採用されないことを検査する。同じcontext／texture keyで再要求し、空LRUからの開始も確認する。
- ROOT embedded の実passでcallback、通常の素材再採用後に旧Depth描画を送らないこと、入口／描画直前の照合、保存2D transform不変、
  fit・下地・回転・リサイズ・パネルrect、待機／マウス／modal／小画面を検査する。
  書き出しは実パネルから開き、Settings／Running／Finishedの初回・後続・再open、
  外click／wheel／key／ring、dialog内操作、Remote leaseと外部openの拒否を確認する。
  素材再採用は既存 `FsUploadResult` → `poll_prefetch` を使い、source世代・新texture ID・旧edit/final失効を確認する。
  旧Readyを人工的に残してpayload ID guardだけを直接実行する検査ではない。同guardは変更せず、callback／receipt発行より前にあることを独立レビューで確認した。

変更ファイル（12本）:

- 製品境界・文言: `src/app/depth_view.rs`, `src/depth_parallax.rs`。
- 状態回帰・遅延reply fixture: `src/app/depth_view_tests.rs`, `src/depth_estimation.rs`（後者の変更は`cfg(test)`だけ）。
- 新規ROOT回帰: `src/app/depth_main_window_tests.rs`, `src/app/depth_export_main_window_tests.rs`,
  `src/ui_fullscreen/depth_main_window_tests.rs`。追加は計11テスト（状態・案内3、ROOT8）。
- テストmodule登録だけ: `src/app.rs`, `src/app/depth_export.rs`, `src/ui_fullscreen.rs`。
- 本書と `docs/detached-rework-plan.md` §11。公開文書・PNGは変更しない。

**検証記録**（全Cargoに `MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、通常feature/test profile、基点＋今回の未コミット差分）:

| 実行 | 結果・証拠 |
| --- | --- |
| 最終 `cargo nextest run --offline -p mimageviewer --lib -E 'test(/depth\|export\|presentation\|window_mode/)' --test-threads 4` | **975/975成功**、exit0、116.616秒（compile1.22秒）。今回の11件と既存全画面3D・書き出し・表示切替の回帰を含む。`target/depth-mainwindow-final-related.log` |
| 初回の関連run（通知の実効キー対応前） | **974/974成功**、exit0、123.643秒。`target/depth-mainwindow-related.log`。最終runと重複加算しない |
| 素材再採用1件の先行確認（上記と重複加算しない） | **1/1成功**、exit0、1.523秒（compile3分05秒）。`target/depth-mainwindow-material.log` |
| 通知の実効キーと共通matrixの限定確認 | **2/2成功**、exit0、1.294秒（compile3分44秒）。`target/depth-mainwindow-toast.log`。既定D／F13／none×両方向を含む |
| `cargo test --offline -p mimageviewer --test ui_snapshot`（更新指定なし） | **245/245成功**、exit0、45.68秒。PNG変更なし。`target/depth-mainwindow-ui.log`。後続の通知文だけの修正はこのpure描画fixture群から呼ばれず、描画・fixtureも変わらないため独立レビューで結果の再利用を確認 |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、28.37秒（既存warningあり）。`target/depth-mainwindow-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-mainwindow-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI glyph **0件**。`target/depth-mainwindow-glyphs.log` |
| 独立レビュー（GPT-6.1 Sol / xhigh、同じcontextで継続） | **最終受入可、未解決指摘なし**。固定D案内のP2・fixture修正と上記証跡を照合し、症状パッチではない構造変更として確認。レビュー側の重複Cargo・製品起動なし |

反復中の失敗はfixtureの問題として修正した。最初のcompile2件（MouseWheelのphase/source、モデルpathの借用順）と
後のprivate入力surface helper呼出し1件はコンパイルエラーであり、テスト成功・失敗の実行件数には算入しない。
初回実行は11件中5成功・6失敗。通常2D callbackの誤計数を同passのDepth receipt＋callbackで区別し、
初回Area sizingを済ませてからpanel geometryを検査する形にした。modal検査は下流のring関数直呼びから
実gamepad dispatchへ変更し、透明素材とdismiss後のpositive controlを追加した。再openも実パネルclickへ揃え、
非同期workerの終端を復帰1passで決め打ちしない。続く8件は7成功・1失敗（素材のseedだけでは採用にならないfixture）。
通常の `FsUploadResult` 採用へ直し、上記1件と初回の関連974件で成功した。ここまでのfixture対応はテスト側だけで行った。
974件成功後の独立レビューP2は、再入場トーストの固定Dが再割り当て・解除に合わない点。
既存 `first_chord_label(FsDepthParallax)` と割り当てなしの上部3Dボタン案内に修正し、既定D／F13／none×両方向の回帰を追加した。
限定libのfilterなし全件・workspace full gateは指示どおり実施していない。
製品起動・実機操作・build-dev・stage／commitは今回の指示に従い実施しない。公開文書は段Dへ残す。

#### 段 C 独立レビュー P3: F12 の終了案内を最後の通知へ統合 (2026-11-08、基点 `29bb7f54b`＋MainWindow対応差分)

MainWindow／Fullscreen の3D中に通常F12を使うと、共通commitが出した終了案内を
`toggle_detached_viewer_mode` 末尾の「別ウィンドウモード ON」が上書きしていた。
通知欄は既存の1値を維持し、発行順や追加slotで対処しない。

- `commit_viewer_presentation_deferred_feedback` を共通内部commitとし、既存と同じ適格性・setterでの退役・
  presentation代入を行って、終了時だけ通知文 `Option<String>` を返す。
  既存 `commit_viewer_presentation` はその文を発行するwrapperとして残し、他の単独通知callsiteは変更しない。
- F12専用 `apply_egui_viewer_presentation_for_current_item` は通知文を呼び出し元へ返し、
  通常F12はON/OFFの最後の1通知へ理由を括弧付きで統合する。途中の終了通知は発行しない。
- もう一つの同helper呼出し（メディア別窓の同期案内）も同じ規則で統合し、既存のViewer面・Warning・3.5秒を維持する。
  この枝は現在media限定でDepth対象外だが、返却した通知を捨てたり2回発行する経路を作らない。
  native動画の非同期確定にも既存案内があるが、Depthに入れないため非同期ownerへ変更を広げない。
- F11の後段には同型の通知上書きはない。新context構築、fork／park、close／openの各commitも監査した。
  detached述語・表示切替規則・資源寿命・通知slotは変更しない。
- 実際のF12処理関数を通す回帰を強化し、main×MainWindow／Fullscreenからの終了で
  最後の通知がONと3D終了理由を両方含み、発行は1回だけであることを検査する。
  3DなしのON/OFFの既存文言、メディア案内の文言・面・種類・時間も検査する。

今回追加で変更したファイルは `src/app/depth_view.rs`, `src/app.rs`, `src/app/depth_view_tests.rs`,
本書と `docs/detached-rework-plan.md` の5本。直前の未コミットMainWindow対応差分を維持する。

評価の経路でpresentationが変わって3Dが終わるときは、後の評価の通知が案内を上書きし得る。まれで、3Dは正しく終わるので割り切る（2026-11-08 設計担当判断）。

検証記録（全Cargoに `MSBUILDDISABLENODEREUSE=1`, `CARGO_BUILD_JOBS=1`、通常feature/test profile）:

| 実行 | 結果・証拠 |
| --- | --- |
| F12／表示切替の先行限定run | **624/624成功**、exit0、81.427秒（compile4分39秒）。`target/depth-mainwindow-p3-tests.log`。後続runと重複加算しない |

再レビュー後、評価の3ファイルと `app.rs` のrating publication部分を基点 `29bb7f54b` へ完全に戻した。
所有境界テスト・評価の既存通知を維持し、F12とメディア案内の統合だけを残す。

復元後の検証（同じ環境変数・通常feature/test profile）:

| 実行 | 結果・証拠 |
| --- | --- |
| `grid_cursor_live_selection_writes_are_owned_not_input_helpers` のexact filter | **1/1成功**、exit0、1.291秒（compile5分20秒）。`target/depth-mainwindow-rating-revert-grid.log`。テスト本体・許可リストは変更していない |
| `cargo nextest run --offline -p mimageviewer --lib -E 'test(/rating_membership\|undo_ops\|undo_stack\|meta_undo\|f12\|media_window_main_hint/)' --test-threads 4` | **134/134成功**、exit0、34.357秒（compile2.14秒）。`target/depth-mainwindow-rating-revert-related.log`。基点へ戻した評価・Undoと、残すF12通知の回帰を含む |
| `depth_media_main_hint_combines_feedback_without_changing_its_surface` のexact filter | **1/1成功**、exit0、0.417秒。`target/depth-mainwindow-rating-revert-hint.log`。メディア案内の合成・属性を維持 |
| `cargo check --offline -p mimageviewer --bin mimageviewer-core` | 成功、exit0、1分00秒（既存warningあり）。`target/depth-mainwindow-rating-revert-check.log` |
| 最後にworkspace全体へ引数なし `cargo fmt` → `cargo fmt --check` | 両方成功、exit0。`target/depth-mainwindow-rating-revert-fmt.log` |
| `python scripts/check_ui_glyphs.py` / `git diff --check` | 両方成功、exit0、危険なUI glyph **0件**。`target/depth-mainwindow-rating-revert-glyphs.log` |
| 基点との一致 | fmt後も指定3ファイルはバイト単位で基点と一致し、指定の `git diff 29bb7f54b -- src/app/rating_membership_ops.rs src/app/rating_membership_tests.rs src/undo_ops.rs` は空・exit0。`app.rs` のrating publication部分も基点と一致。`target/depth-mainwindow-rating-revert-parity.log` |

stage／commit・製品起動・build-dev・filterなし全件は実施しない。

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

### 段 D の更新記録 (2026-11-08、基点 `930fbc7a4`)

再起動後は `0c988a36d` 上で継続。元モデルIDの正本は同commitで訂正済みの
`tools/depth_model/build_depth_addon.py::NOTICE` にある `depth-anything/DA3MONO-LARGE`。
本作業ではbuilderを再編集せず、公開手順の参照先もこのURLに揃えた。
再開時に製品ページとshortcutsの全内容がNULになっていたため、この2ファイルだけHEADから復元して
記録済みの段D差分を再適用した。他の編集は維持し、下記検査で14ファイルのUTF-8とNULなしを確認した。

公開名は **「3D 写真」で確定**。実装用語はライセンスの変更通知と開発者向け手順に限定し、
公開マニュアル・製品カードに本体の版番号を追加しない。コードの挙動・更新履歴・画像素材は変更しない。
Rust の差分は `src/ui_dialogs/about.rs` のライセンス案内の文言追加のみ。

| 更新ファイル（repository root から） | 内容 |
| --- | --- |
| `htdocs/mimageviewer/manual/fullscreen.html` | `#depth-photo` に入口・初回取得・右パネル・書き出し・制限・F11切替時終了・透過背景切替を追加。`#depth-export`、`#depth-availability` も設ける |
| `htdocs/mimageviewer/manual/shortcuts.html` | D、3D中の I / Tab と Shift+B、連結読みで使えない操作の一覧を更新 |
| `htdocs/mimageviewer/manual/settings.html` | `#depth-addon` に導入・削除・保存先・ライセンスと派生物の変更内容を追加。保存場所一覧にも追加 |
| `htdocs/mimageviewer/manual/index.html` | 目次から `fullscreen.html#depth-photo` へ直接リンク |
| `htdocs/mimageviewer/manual/troubleshooting.html` | 開始できない条件・取得後の再操作・初回準備と内蔵GPU等の待ち時間を短く説明。FAQに同じ説明を重複追加しない |
| `htdocs/mimageviewer/index.html` | AIカード付近に3D写真カードを1枚追加。安心欄のPC内処理に奥行き計算を含め、ライセンス欄にDA3派生物を追加。通信のカードは既に任意機能とprivacyへのリンクで包括できるため維持 |
| `htdocs/mimageviewer/privacy.html` | 日英の任意ダウンロード・保存データ・PC内のAI処理に深度モデルを追加。GPU専用との読み違いを避け、PC内処理と記載 |
| `README.md`、`installer/readme.txt`、`installer/readme_portable.txt`、`src/ui_dialogs/about.rs` | 第三者ライセンス案内にDepth Anything 3（DA3 Mono-Large、Apache-2.0）・fp16化・入力固定・再学習なし・LICENSE/NOTICEの参照方法を追加 |
| `docs/release-checklist.md` | Phase 5に初回・モデル更新時の公開手順を追加。builder、4 assets、独立タグ、latestにしない、既存資産維持と新タグへの更新を明記 |
| `htdocs/sitemap.xml` | 生成スクリプトで更新した公開ページのlastmodを反映（URL増減なし） |
| `docs/depth-parallax-plan.md` | 段D・本節・§15の名称確定を記録 |

**記載した事実のコード根拠**（行番号より関数名を追跡の正本とする）:

| 事実 | 根拠 |
| --- | --- |
| メインウィンドウ/全画面の静止画・単ページに限定。動画/音声/アニメ/見開き/連結/編集/紙めくり/別窓/GPU描画なしを拒否 | `src/depth_parallax.rs::DepthEntryBlock / entry_allowed`、`src/app/depth_view.rs::depth_entry_allowed / depth_presentation_eligible`。実際の2ページ表示判定を使う |
| F11等の実表示切替で終了、再度D/ボタンで開始 | `src/app/depth_view.rs::commit_viewer_presentation_deferred_feedback / toggle_depth_parallax`、`src/ui_fullscreen.rs` の3D上部ボタン |
| D、I/Tab、Shift+Bの既定・3D中のパネル切替/背景切替 | `src/keymap.rs::default_chords` の `FsDepthParallax / FsToggleMetadata / FsBgCycle` と `docs/keymap.ini.default`、`src/ui_fullscreen.rs::handle_fs_key_input_with_operation`、`src/app/depth_view.rs::toggle_depth_panel` |
| 初回約0.67GB、同意後取得、完了で自動入場しない、導入・削除・保存先表示 | `src/ui_dialogs/depth_addon.rs::draw_dialog / poll_depth_setup / depth_setup_completion_message / draw_preferences`、`src/ui_dialogs/model_addon.rs::draw_actions`。環境設定ツリーは `src/ui_dialogs/preferences.rs` の `DepthAddon` |
| GitHub Releasesの固定タグ、データフォルダ内addons/depth、ポータブルはdata配下 | `src/addon.rs::AddonKind::default_base_url / index_name`、`src/depth_addon.rs::addon_root`、`src/data_dir.rs` |
| 奥行き計算はローカル、画像を計算のために外部送信しない | `src/depth_estimation.rs::estimate / infer_disparity` がローカルモデルを `AiRuntime::load_model / with_session` で実行。取得処理は別の `src/addon.rs` と `src/vlm_addon_download.rs` |
| 動き5種、強さ/周期/止まる深さ/整形の範囲と既定、共通設定として保存 | `src/depth_parallax.rs::MotionKind / DepthParallaxSettings`、`src/ui_depth_parallax.rs::draw_depth_parallax_panel_with_export`、`src/app/depth_view.rs::draw_depth_panel` |
| 書き出し3形式、fps・ループ、長辺5候補と形式別既定・縮小側の候補・拡大なし | `src/depth_export.rs::DepthExportFormat / DepthExportOptions`、`src/ui_depth_export.rs::long_edge_choices / default_long_edge / draw` |
| マウス追従時の書き出し初期値は左右、現在の設定と背景を反映、保存先と大きさを通知 | `src/app/depth_export.rs::admit_depth_export / poll_depth_export / ExportCapture::into_request`、`src/depth_export.rs` の `DepthExportSummary`（大きさ取得失敗時は省略） |
| 書き出し設定から完了まで閲覧を止め、進捗・取消を表示 | `src/app/depth_export.rs::Operation`、`src/app.rs::common_modal_dialog_open / document_open_modal_admission_blocked`、`src/ui_depth_export.rs::draw` |
| Remoteでは3Dを表示せず、リモート操作中のローカル開始も拒否、360°との切替・編集との排他 | `src/app/depth_view.rs::resume_depth_entry_after_check / set_fs_exclusive_view`、`src/app.rs::close_fs_exclusive_conflicts`、`src/ui_fullscreen.rs` の編集/360°入口と操作権判定 |
| DA3 Mono-Large、Apache-2.0、fp16化・1枚504×504入力固定・再学習なし、モデルとLICENSE/NOTICEを取得 | `tools/depth_model/build_depth_addon.py::NOTICE / build`、`src/depth_addon.rs::validate_index`、`src/addon.rs::install_for_kind` |

**設計との照合・記述を控えた事項**:

- §6.1の箇条書きに明記のない「紙めくり中」の拒否も、コードの `DepthEntryBlock::PageTurning` に従って掲載した。
- §9の「画像フルスクリーン」はkeymapの文脈名。利用者向けには§6.4.1と実コードどおりメインウィンドウ内表示も含めた。
- §1の書き出し「1周期」だけではMP4の長さを説明しきれないため、§7と実コードのループ回数1〜10・既定4回を記載した。
- 待ち時間の根拠は [研究調査 §8.5](depth-parallax-research.md)。
  初回GPU準備2〜3.5秒と内蔵GPU初回13〜19秒の記録があり、公開文書は「数秒以上かかることがある」に留めた。
  一般的なGPU/低速CPUへの所要時間の保証、極端な縦横比での品質、書き出しファイル容量の目安は掲載しない。
- モデルReleaseの公開状態・公開URLからの実取得は今回未確認。公開手順として記載し、取得済みとは主張しない。

**検査記録**（`0c988a36d`＋段Dの未コミット差分、通常feature）:

| 検査 | 結果 |
| --- | --- |
| `python scripts/check_site_cards.py` | exit 0、53枚・違反0 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険グリフ0 |
| `python scripts/gen-effetune-docs.py --check` | exit 0、137 version files＋entry＋redirectsが現行と一致 |
| `python scripts/test_gen_effetune_docs.py` | exit 0、33/33 |
| `python scripts/gen-sitemap-xml.py --check` | exit 0、216 URLs。生成によるURL増減なし |
| 内部リンク・HTML・UTF-8/NUL検査 | `target/depth-docs-audit.py`、exit 0。変更した14ファイルはUTF-8/NULなし、公開ページのfragment91件・新規ローカルリンク9件に不在先なし、新規の閉じ忘れ/重複IDなし |
| `python scripts/test_public_privacy_scope.py` | exit 1、7/9。privacyの既存「確認では」の検出と旧SFZ保存先の期待値で2件失敗。同じscriptをHEADの公開3ファイルへ適用して同じ2件失敗を再確認。今回の深度追記による追加失敗はなく、範囲外の表記・期待値は変更しない |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、通常dev。ログ `target/depth-docs-cargo-check.log`、既存warningあり |
| `cargo fmt --check` | exit 0 |
| `cargo test -p mimageviewer --lib ui_dialogs::about::tests::` | exit 0、7/7（snapshotを含むheadless）。通常test profile、コンパイル6分44秒・実行3.26秒、既存warningあり。再起動前の中断結果は採用せず再実行した。ログ `target/depth-docs-about-tests.log` |
| 同じモデル作成用venvで `python -m unittest discover -s tools/depth_model -p test_build_depth_addon.py` | exit 0、2/2。モデル取得・製品起動なし |
| `python scripts/verify-site.py htdocs https://www.mikage.to` | exit 1、392件すべてWinError 10061（接続拒否）で取得できず。公開内容の一致は未確認。ログ `target/depth-docs-verify-site.log` |
| `git diff --check` | exit 0。stageされたファイルなし |

`gen-changelog-html.py` は更新履歴、`gen-moire-sample.py` は画像素材を生成するため、依頼の範囲外として実行しない。

#### 段 D の公開・検収 (2026-11-08、`dcabbd100`)

- **独立レビュー** (GPT-6.1 Sol / xhigh、read-only、`git diff 0c988a36d dcabbd100`): P1〜P3 とも指摘なし。
  公開文書の事実 (入口条件・F11/F12・パネル・書き出し・保存先・同意・既定キー・360°/Remote) がコードと一致、
  privacy の日英が同内容、ライセンス表記が NOTICE と一致、release-checklist の手順が実物と整合。
- 製品ページの安心欄で、Codex の差分が既存の AI 説明文へのリンクと「画像内の文字」の記述を削っていたため、
  設計担当が元の文言に戻して 3D を足す形にした (コミット前に修正)。
- **モデルの公開**: 利用者の承認 (2026-11-08) を受けて GitHub Release `depth-model-v1` を作成。
  プレリリース・latest にしない、target は v5.0.0 (`446c59562`)。資産は `da3mono_large_fp16_504.onnx`
  (671,032,016 B)、`LICENSE`、`NOTICE` (704 B、元モデル URL 訂正版)、`depth-model-index.json`。
  公開前に元モデルのライセンスを再確認した: 公式 GitHub README の表・公式 Hugging Face
  (`depth-anything/DA3MONO-LARGE`) のモデルカードとも Apache-2.0。変換元 SpatialHub の ONNX リポジトリは
  見出しの license 欄が cc-by-nc-4.0 だが、本文で da3mono_large.onnx は Apache-2.0 を継承すると明記している
  (Giant / Large だけが CC BY-NC)。この食い違いを示したうえで利用者が公開を判断した。
- **サブ PC 確認依頼 21** (観測者: サブ PC 担当、ビルド `dcabbd100` の dev-runtime、モデルなしの隔離 data から実取得):
  14 項目中 13 OK。同意画面の文言 (約 0.67 GB・Apache License 2.0・depth-model-v1・保存先)、
  「今はしない」で何も作られない、途中取消で導入済みにならず次回は続きから再開 (`.partial` は完了後に消える)、
  完了で自動入場しない・完了通知の文言、`packs/da3mono-large-fp16-504-0f5ff3be208d/` の内容と onnx の SHA-256 が
  目録と一致、D で約 3 秒後に 3D が動く、再起動後は案内なしで入れる、環境設定からの削除と再導入、
  バージョン情報の「3D 写真」節、ログに ERROR / WARN / panic なし。
  - NG 1 件 (ダウンロード中に後ろのページが送れる) は、依頼文の期待が誤っていた。§3.3 のとおり
    取得中は modeless で、`depth_management_modal_boundary_keeps_download_modeless` がこれを固定している。不具合ではない。
  - 気づいた点: 環境設定の検索で枠が付いたとき、ライセンス行の末尾「)」が枠線に掛かって見えた。
    文言と配置は AI 説明文 (リリース済み) の行と同じで、3D 固有ではない。段 D の対象外として残す。
    取得完了後に `downloads/…` の空フォルダが残る点も VLM と共通の既存動作。
製品起動・確認用build・stage・commitは行っていない。

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

段Cで、映像だけのmuxer、GIFの累積遅延、独立GPU資源と画面の色・半透明下地の一致、
Remote leaseと外部open受付の排他をコードと回帰検査の対象へ具体化した。実行結果は§10に集約する。

公開名は「3D 写真」で確定し、この名前で公開文書を更新した（段D）。以下は未確認。

- 極端な縦横比 (3:1 以上) での深度の劣化 (§3.1)。
- 一般的な GPU・遅い CPU での所要時間 (研究調査 §8.5)。

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
