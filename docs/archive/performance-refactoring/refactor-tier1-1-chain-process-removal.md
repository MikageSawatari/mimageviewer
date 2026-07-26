# Refactor Tier 1 #1: chain_process / scratch buffer removal

Date: 2026-05-09
Reviewer target: Codex (read-only)
Author: Claude (working tree change)

## Goal

`src/video/dsp/mod.rs` から、chain bridge 移行で到達不能になった以下を削除し、
`process_block` を simplify する:

1. `fn chain_process` (関数本体、~36 行)
2. `DspBridgeInner::scratch_a` / `scratch_b` フィールドと初期化
3. `process_block` の multi-bridge ping-pong ブランチ

## Why this is safe (= dead code であることの根拠)

chain bridge 移行 (commit `3da59b5`) 以後、`add_plugin` は最初の slot 以外では
`first.bridge.clone()` を再利用する:

```rust
// src/video/dsp/mod.rs (旧コード、変更前)
let (bridge_arc, slot_id) = {
    let inner = self.inner.lock().unwrap();
    if let Some(first) = inner.slots.first() {
        let slot_id = inner.next_slot_id.max(inner.slots.len() as u64).max(1);
        (first.bridge.clone(), slot_id)         // ← 既存 bridge を再利用
    } else {
        // ... Bridge::spawn (= 1 セッション 1 回だけ) ...
        (Arc::new(bridge), 0)
    }
};
```

これにより、enable 中の全 `PluginSlot.bridge` は **同一の Arc<Bridge>** を共有する。
`process_block` 旧コードは:

```rust
let mut active_bridges: Vec<Arc<Bridge>> = inner.slots.iter()
    .filter(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
    .map(|s| s.bridge.clone())
    .collect();
active_bridges.dedup_by(|a, b| Arc::ptr_eq(a, b));   // ← ここで全部 1 個に潰れる
```

つまり `active_bridges.len() > 1` の分岐は構造的に到達不能。

bridge 内部の `audio_loop` は `input → loader[0] → loader[1] → ... → output` を
in-place で処理するため、Rust 側からは bridge を 1 回 `process_audio_blocking` するだけで
チェーン全体が完了する。

`Bridge::spawn` の他の呼び出し箇所 (`scanner.rs` 内の plugin probe) は使い捨てで
`DspBridgeInner.slots` に乗らないため、process_block の dedup ロジックには影響しない。

## What changed

### 1. `chain_process` 関数を削除
- 旧位置: `src/video/dsp/mod.rs` line ~2061-2090 (`fn chain_process(bridges: &[Arc<Bridge>], ...) -> Result<(), String>`)
- 約 36 行削除

### 2. `DspBridgeInner` から scratch フィールドを削除
- 旧 field: `scratch_a: Vec<f32>`, `scratch_b: Vec<f32>` (line 134-137)
- 旧 init: `DspBridge::new` 内の `scratch_a: Vec::new(), scratch_b: Vec::new()` (line 210-211)

### 3. `process_block` を以下に simplify

```rust
pub fn process_block(&self, src: &[f32], dst: &mut [f32]) -> Result<(), String> {
    debug_assert_eq!(src.len(), dst.len());

    // ── ホットパス: chain bridge を Mutex 短時間保持で取る ──
    // process_block は audio-pump からのみ呼ばれるが、UI からの add/remove と
    // 競合する可能性があるので Mutex で snapshot を取る (= IPC roundtrip 中は
    // ロック解放済み)。
    let bridge = {
        let inner = self.inner.lock().unwrap();
        inner
            .slots
            .iter()
            .find(|s| !s.bypass && matches!(s.state, SlotState::Loaded))
            .map(|s| s.bridge.clone())
    };

    let Some(bridge) = bridge else {
        // 全 slot が bypass か未ロード: パススルー
        dst.copy_from_slice(src);
        return Ok(());
    };

    bridge
        .process_audio_blocking(src, dst, 100)
        .map_err(|e| format!("process_audio: {e}"))
}
```

旧:
- `Vec<Arc<Bridge>>` を全 active slot から collect → dedup → `len()` で 0/1/N 分岐
- N 分岐は `chain_process` に渡して ping-pong

新:
- 最初の `!bypass && Loaded` slot から `Arc<Bridge>` を 1 個だけ取る (`find().map()`)
- None → パススルー、Some → `process_audio_blocking` 1 回

### Mutex 規約への影響
旧コードは process_block 内で Mutex を 2 回 lock していた (snapshot + scratch 戻し)。
新コードは 1 回だけ。Mutex 保持時間は減少 = audio-pump thread と UI add/remove 競合が減る方向。

## Verification

| 項目 | 結果 |
|---|---|
| `cargo build --bin mimageviewer-core` | ✅ OK (54.43s) |
| `cargo test --lib --no-fail-fast` | ✅ 687 passed, 0 failed, 1 ignored |
| 静的: 削除した関数・フィールドへの参照 | ✅ grep で 0 件 (削除完了確認) |

**動作面の smoke test (= 実機での音声再生検証) はまだ未実施**。Codex レビューで P1 が
無いことを確認後、動画 + VST3 1 個 + VST3 2 個 + bypass トグル + add/remove で
音声が途切れないか実機検証する予定。

## Codex に見てほしい点

1. **「全 PluginSlot は同一 Arc<Bridge> を共有する」前提が真に成立しているか**
   - `add_plugin` 経路以外で `PluginSlot.bridge` がセットされるパスが無いか
   - `disable()` → 再 `enable()` のサイクルで一時的に複数 Bridge が混在する可能性
   - エラーパス (`Loaded` 状態に到達しない slot) で Bridge instance が分裂しないか
2. **新 process_block の `find` で取れる bridge が「正しい代表」か**
   - 全 slot で bridge が同一なら `find` の結果は何でも良いが、もし将来 per-slot bridge 復活
     の可能性があるなら `Vec` に戻す柔軟性を残しておくべきか
   - 答え: 現在の chain bridge 設計を既定とするなら不要、ただし設計ドキュメントへの
     明記推奨 (= [docs/vst3-integration.md](../../vst3-integration.md) §2 で既に明示済み)
3. **Mutex 保持回数を 2 → 1 に減らした影響**
   - audio-pump 側で Mutex 競合が減るのは望ましいが、UI add/remove が短時間に
     ボロボロ来た場合の semantics が変わっていないか (= snapshot のタイミング)
4. **`debug_assert_eq!(src.len(), dst.len())` は維持済み**。これ以外に削除して
   しまったアサーションが無いか
5. **bypass=true の slot が並んでいて、最初の `!bypass && Loaded` が後ろに
   居るケース** (例: `[bypass, bypass, active]`)
   - 旧: bridge が同一 Arc なので何でも返る
   - 新: `find` は最初の `active` を見つけて返る → 動作同じ (Arc は同一)

## ロールバック手順

不具合が見つかったら:

```bash
git checkout src/video/dsp/mod.rs
```

(他のファイルは触っていないので mod.rs だけで戻せる)

## 次の Tier 1 項目

#1 が Codex P1 ゼロで承認されたら、続けて:
- Tier 1 #2: `native_presenter.rs` の drawing function 群 (約 1760 行) を
  `native_presenter/overlay_draw.rs` に丸ごと移動
