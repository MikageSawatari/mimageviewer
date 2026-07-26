# Codex 回答 (第 2 弾): VST3 残課題

前提: 2026-04-30 時点の `C:/home/mimageviewer` をコード読解した結果です。実機の SSL Meter Pro / Insight2 / mIV Test Latency による再現確認はしていません。

## 課題 1 (P1): parapara 表示 + チラつき

### 場所
- `src/video/dsp/mod.rs:548` `DspBridge::set_all_guis_visible`
- `src/video/dsp/mod.rs:369` `DspBridge::show_slot_gui`
- `src/video/dsp/gui.rs:635` `set_window_visible`
- `src/video/dsp/gui.rs:610` `set_window_topmost`
- `src/video/dsp/gui.rs:560` `snapshot_z_order`

### 原因
現状は `set_all_guis_visible(true)` が `src/video/dsp/mod.rs:581-597` で slot 順に `show_slot_gui` を呼び、各 HWND が `ShowWindow(SW_SHOWNA)` で即表示されます。その後 `src/video/dsp/mod.rs:607-611` で snapshot 順に `SetWindowPos(HWND_TOPMOST/HWND_NOTOPMOST)` を呼ぶため、DWM には「1 個ずつ表示」→「最後に並び替え」が見えます。

`DeferWindowPos` はこの用途に合います。`SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE` を指定すれば、非アクティブ表示と z-order 更新を 1 batch にできます。`ShowWindow(SW_SHOWNA)` と完全に同じ API ではありませんが、`SWP_NOACTIVATE` を付ければ foreground を奪わないので、目的上は SHOWNA 相当として扱えます。

### 修正案
`gui.rs` に batch helper を追加し、既存 HWND の一括 show では `show_slot_gui` の高速パスを使わず、対象 HWND を集めて `DeferWindowPos` で show + z-order をまとめて確定します。新規作成が混じる場合だけ従来どおり `show_slot_gui` で作成し、その後 batch に含めます。

具体的なコード片:

```rust
// src/video/dsp/gui.rs
pub fn show_windows_in_z_order(hwnds_top_to_bottom: &[u64], topmost: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos,
        HWND_NOTOPMOST, HWND_TOPMOST, HWND_TOP,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    if hwnds_top_to_bottom.is_empty() {
        return;
    }
    unsafe {
        let mut hdwp = BeginDeferWindowPos(hwnds_top_to_bottom.len() as i32);
        if hdwp.is_invalid() {
            // fallback: bottom-to-top で個別適用
            for &h in hwnds_top_to_bottom.iter().rev() {
                let z = if topmost { HWND_TOPMOST } else { HWND_TOP };
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    HWND(h as *mut _),
                    Some(z),
                    0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            return;
        }

        // bottom-to-top に積む。最後に呼んだ HWND が最前面になる。
        for &h in hwnds_top_to_bottom.iter().rev() {
            let insert_after = if topmost {
                HWND_TOPMOST
            } else if h == *hwnds_top_to_bottom.last().unwrap() {
                HWND_NOTOPMOST
            } else {
                HWND_TOP
            };
            hdwp = DeferWindowPos(
                hdwp,
                HWND(h as *mut _),
                Some(insert_after),
                0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            if hdwp.is_invalid() {
                return;
            }
        }
        let _ = EndDeferWindowPos(hdwp);
    }
}
```

`set_all_guis_visible(true)` 側は、snapshot にある HWND を優先して top-to-bottom list を作ります。

```rust
// src/video/dsp/mod.rs: set_all_guis_visible(true) の show 経路
let snapshot = self.last_snapshot_filtered_to_shown(&shown_hwnds);
let ordered = if snapshot.is_empty() {
    gui::snapshot_z_order(&shown_hwnds)
} else {
    snapshot
};
let topmost = self.gui_topmost_desired.load(Ordering::Acquire);
gui::show_windows_in_z_order(&ordered, topmost);
```

代替案として、`DeferWindowPos` を避けるなら `SW_HIDE` 状態のまま `SetWindowPos` で bottom-to-top に並べてから、最後に `ShowWindow(SW_SHOWNA)` を bottom-to-top で呼ぶ形になります。ただし show 自体は複数回に分かれるので、チラつきの解消度は `DeferWindowPos` より落ちます。

## 課題 2 (P1): latency 縮小 + cpal callback 内処理

### 場所
- `src/video/audio.rs:216` `run_pump`
- `src/video/audio.rs:252` `TARGET_BUFFER_SECS = 0.3`
- `src/video/audio.rs:296-304` `DspBridge::process_block` 呼び出し
- `src/video/audio.rs:386` `fill_output`
- `src/video/dsp/mod.rs:819` `DspBridge::process_block`
- `src/video/dsp/bridge.rs:345` `push_audio`, `src/video/dsp/bridge.rs:378` `pull_audio`

### 原因
現在の VST 反映遅延はほぼ `AudioBuffer` の fill 量です。`run_pump` は `src/video/audio.rs:283-290` で 300ms cap まで先に処理して詰めるので、ユーザーが EQ を動かしても、既に加工済みで ring に入った 0-300ms 分は古い音のまま出ます。

一般的な DAW は audio callback ないし audio engine の realtime render thread で plugin `process()` を呼びます。ここで重要なのは「callback 内で処理する」こと自体ではなく、「処理対象が同一プロセス内、事前確保済み、bounded time、非 blocking」であることです。mIV は bridge プロセス IPC (`push_audio` + `pull_audio`) が同期往復なので、cpal callback 内にそのまま入れると deadline miss のリスクが高いです。

### 修正案
まず 100-150ms へ縮小するのが現実的です。WASAPI Shared の内部 buffer、cpal callback 周期、bridge IPC ジッタ、プラグイン処理時間を考えると、100ms 未満は実機計測なしでは攻めすぎです。

具体的なコード片:

```rust
// src/video/audio.rs
const TARGET_BUFFER_SECS: f64 = if cfg!(windows) {
    0.12
} else {
    0.20
};
const LOW_WATER_SECS: f64 = 0.04;

// buffer が cap 近辺なら sleep するが、固定 10ms ではなく短くする
while !cancel.load(Ordering::Acquire) {
    let len_secs = {
        let b = buffer.lock().unwrap();
        b.samples.len() as f64 / b.samples_per_sec
    };
    if len_secs < TARGET_BUFFER_SECS {
        break;
    }
    std::thread::sleep(std::time::Duration::from_millis(2));
}
```

さらに安全に詰めるなら callback 内 IPC ではなく、pump を「低水位追従」にします。callback は今までどおり pop だけ、pump は `LOW_WATER_SECS..TARGET_BUFFER_SECS` に収まるよう先読み量を絞る。これなら VST 操作反映は 100ms 前後まで下がり、RT thread は守れます。

cpal callback 内で IPC を回す案は P3 です。どうしても行うなら `try_process_block(deadline)` を作り、deadline 超過時は即 bypass/直前ブロック再利用に落とす必要があります。

```rust
// callback 内に入れるなら、blocking recv は禁止に近い扱いにする
match dsp.try_process_block(input, output, Duration::from_millis(2)) {
    Ok(()) => {}
    Err(ProcessMissedDeadline) => output.copy_from_slice(input), // or previous good block
    Err(_) => output.fill(0.0),
}
```

「最後に到達したブロック末尾を再利用」は非常用 fallback としては妥当ですが、音楽的には buzz / comb / pitch artifact が出やすいです。通常時の設計にはせず、miss counter とログ、一定回数で bypass へ落とす方が安全です。

## 課題 3 (P1): PDC 実装

### 場所
- `src/video/dsp/mod.rs:262-305` `latency_samples` を `Loaded` で保存
- `src/video/dsp/bridge.rs:107-111` `Loaded` / `LatencyChanged`
- `src/video/audio.rs:296-304` VST 処理後に PTS 無調整で push
- `src/video/audio.rs:331-339` `AudioBuffer.next_pts_secs`
- `src/video/audio.rs:477` `clock.set_audio_pts(pts_now)`
- `src/video/clock.rs:236` `AvClock::set_audio_pts`

### 原因
`PluginSlot::latency_samples` は保存されていますが、audio PTS や video pacing に反映されていません。latency N samples の plugin は「入力 PTS=t の音を、出力では t+N/sr に出す」ので、`fill_output` が報告する `pts_now` は現在の実装だと入力基準のまま進み、動画だけ先に見えます。

### 修正案
最小実装は「動画クロックを plugin latency 分だけ遅らせる」です。mIV は audio master なので、audio callback が報告する anchor を `input_pts - total_latency_secs` に補正すると、`now_secs()` が遅れ、動画表示もその分後ろへ引かれます。

まず DspBridge に合算 latency accessor を追加します。

```rust
// src/video/dsp/mod.rs
pub fn total_latency_samples(&self) -> u32 {
    let inner = self.inner.lock().unwrap();
    inner
        .slots
        .iter()
        .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
        .map(|s| s.latency_samples)
        .sum()
}
```

`AudioBuffer` に `pdc_latency_secs` を持たせ、`run_pump` から更新します。

```rust
struct AudioBuffer {
    samples: VecDeque<f32>,
    next_pts_secs: f64,
    pdc_latency_secs: f64,
    // ...
}

// run_pump の push 前
#[cfg(windows)]
{
    let latency_secs = dsp_bridge
        .as_ref()
        .map(|b| b.total_latency_samples() as f64 / buf.sample_rate as f64)
        .unwrap_or(0.0);
    buf.pdc_latency_secs = latency_secs;
}
```

`fill_output` では audio anchor を出力音が表す入力 PTS に補正して報告します。

```rust
// src/video/audio.rs: fill_output
let pts_for_video = (pts_now - buf.pdc_latency_secs).max(0.0);
drop(buf);
if pump_serial >= clock.current_seek_serial() {
    clock.set_audio_pts(pts_for_video);
}
```

これは「動画を遅らせる」方式で、完全 PDC ではありません。完全 PDC は audio 側を N samples 先読みして plugin へ流し、出力先頭 N samples を trim する必要があります。動画プレイヤーとしては最小実装の方がリスクが低く、mIV Test Latency の確認にも向いています。

`LatencyChanged` は現在どこにも反映されていないので、bridge 受信経路を常時 poll するか、audio `pull_audio` の制御イベント処理と衝突しない別 channel が必要です。latency が変わったら:

- `PluginSlot.latency_samples` を更新
- `AudioBuffer` を flush するか、少なくとも `next_pts_secs` と `pdc_latency_secs` を同時更新
- `AvClock` は seek 相当の discontinuity として anchor を再設定

複数 plugin 直列では latency は合算です。各 plugin 個別補償ではなく、chain 全体の `N1+N2+N3` を使って video clock を shift するのが最初の実装として正しいです。

テスト手順:

1. mIV Test Latency を 0 samples で通し、A/V 同期が現状と一致することを確認。
2. 4800 samples @ 48kHz (=100ms) を設定し、PDC OFF で動画が約 100ms 先行することを録画/ログで確認。
3. PDC ON で `clock.now_secs()` が `latency_secs` 分だけ遅れて進み、見た目の同期が戻ることを確認。
4. 直列で 2400 + 4800 samples を挿し、合算 150ms になることを確認。
5. 再生中に latency を変更し、flush/anchor reset 後にジャンプや逆行がないことを確認。

## 課題 4 (P2): peak 超過時の挙動

### 場所
- `src/video/audio.rs:339` processed samples を ring にそのまま push
- `src/video/audio.rs:434-443` `out[written] = s * vol`
- `src/video/dsp/mod.rs:819` plugin chain 出力を制限なしで返す

### 原因
f32 samples が `> 1.0` / `< -1.0` のまま `fill_output` を通ります。WASAPI Shared / OS mixer 側で最終的に整数変換や device 出力に到達すると hard clip し、歪みます。ホスト側には limiter も clip indicator もありません。

### 修正案
音楽制作 DAW なら master out に limiter を勝手に入れない選択もありますが、mIV は動画ビューアなので「耳に痛い事故」を防ぐ価値が高いです。デフォルトは軽い hard clamp ではなく、soft clip + indicator が妥当です。音質を勝手に変えたくないユーザー向けに設定で OFF 可能にします。

最小コード片:

```rust
#[inline]
fn soft_clip(x: f32) -> f32 {
    // cheap tanh-ish clip, no allocation
    let x = x.clamp(-4.0, 4.0);
    x / (1.0 + x.abs() * 0.25)
}

// fill_output
let y = s * vol;
if y.abs() > 1.0 {
    clock.report_audio_over(y.abs()); // AtomicU32 peak bits など
}
out[written] = if limiter_enabled {
    soft_clip(y).clamp(-1.0, 1.0)
} else {
    y
};
```

peak detection は RT callback 内で max を計算し、Atomic に publish するだけにします。UI 側は VST 管理パネル/HUD で 200-500ms ホールドの `OVER` 表示を出します。ログや文字列生成は callback 内ではしないでください。

```rust
// fill_output 内
let mut peak = 0.0f32;
// loop 内で peak = peak.max(y.abs());
clock.publish_audio_peak(peak);
```

「host は clip しない」が正解になるのは制作用途で、かつユーザーが meter / limiter を自分で挿す前提のときです。mIV では indicator は P2、soft limiter は設定付き P3 と見るのがよいです。

## 課題 5 (P2): リサイズイベント backlog / バッファされたリサイズを再生する挙動

### 場所
- `src/video/dsp/gui.rs:207-223` `WM_SIZE` を全て channel へ送る
- `src/video/dsp/mod.rs:695-705` mIV 側では latest だけ drain
- `src/video/dsp/mod.rs:743-750` `notify_host_resize` を fire-and-forget 送信
- `crates/vst3-host/src/main.cpp:321-329` bridge は受けた notify を全部処理
- `crates/vst3-host/src/plugin_loader.cpp:466-478` `view_->onSize` 同期 call
- `crates/vst3-host/src/host_app.cpp:132-153` resizeView feedback 抑止

### 原因
mIV 側 channel では latest だけ採用していますが、bridge への stdin command は back-pressure なしです。`view_->onSize` が 16ms より遅い plugin では、bridge の control loop が古い `notify_host_resize` を順番に消化します。結果として drag 後に「過去サイズを再生」しているように見えます。

`WM_ENTERSIZEMOVE` 対策は `resizeView -> SetWindowPos` の振動には効きますが、`notify_host_resize -> onSize` の backlog には効きません。

### 修正案
最も効果が高いのは bridge 側の latest-only coalescing です。stdin から `notify_host_resize` を読んだら即処理せず pending に入れ、control loop の 1 tick で最新 1 件だけ `onSize` します。古いサイズを処理しないので backlog 再生が止まります。

簡易コード片:

```cpp
// crates/vst3-host/src/main.cpp
std::optional<std::pair<uint32_t, uint32_t>> pending_resize_;

if (cmd == "notify_host_resize") {
    uint32_t w = static_cast<uint32_t>(extract_number_field(msg, "width"));
    uint32_t h = static_cast<uint32_t>(extract_number_field(msg, "height"));
    if (w > 0 && h > 0) pending_resize_ = {w, h};
    return true;
}

void pump_pending_resize() {
    if (!pending_resize_ || !loader_) return;
    auto [w, h] = *pending_resize_;
    pending_resize_.reset();
    loader_->notify_host_resize(w, h);
}
```

次に、mIV 側にも throttle を入れます。値は 33ms (=30fps) から始めるのが妥当です。Insight2 のような重い UI には 16ms は速すぎる可能性があります。

```rust
// PluginSlot に追加
last_resize_notify: Option<Instant>,
pending_resize: Option<(u32, u32)>,

// pump_gui_signals
const GUI_RESIZE_NOTIFY_MIN_INTERVAL: Duration = Duration::from_millis(33);
if now.duration_since(last) >= GUI_RESIZE_NOTIFY_MIN_INTERVAL {
    send_notify(w, h);
    slot.last_resize_notify = Some(now);
} else {
    slot.pending_resize = Some((w, h));
}
```

ack 方式も有効ですが、control protocol に response を足すため実装が大きくなります。まずは bridge latest-only + 33ms throttle を推奨します。

`WM_ENTERSIZEMOVE` 中は一切 notify せず、`WM_EXITSIZEMOVE` で最終サイズ 1 回だけ送る方式は安定しますが、drag 中の内容追従が止まるため体感は Bitwig より悪くなります。設定や fallback としてはありですが、標準挙動にはしない方がよいです。

## 着手順序の提案

1. P1: PDC 最小実装。音ズレは機能正当性の問題で、テストプラグインで検証しやすい。
2. P1: latency cap を 120-150ms に縮小し、underrun/CPU/IPC miss を計測。callback 内 IPC はまだ入れない。
3. P2: resize latest-only coalescing。Insight2 体感改善に直結し、PDC/latency と独立している。
4. P1: GUI batch show (`DeferWindowPos`)。見た目改善だが Win32 z-order の副作用確認が必要。
5. P2: peak indicator。まず表示だけ入れ、soft limiter は設定付きで後続。

依存関係として、latency 縮小と PDC は同じ audio clock / buffer に触るため近い順で実施するのが安全です。GUI batch show と resize backlog は同じ VST GUI 領域ですが、z-order と resize IPC は独立なので並行しやすいです。
