# egui-wgpu 0.37.0 のローカル変更（段2b）

0.37.0原本を配置した後に、旧vendorと0.33.3原本の実差分を項目別に付け直した。
READMEだけを移植台帳にはしていない。以下は**実装担当の内部確認**である。
root製品graph・依存切替・eframeの接続は対象外。

## 原本と境目

- 元: ローカルregistryの`egui-wgpu-0.37.0`、依存wgpuは準備lockで30.0.1。
- archive SHA-256: `0309cbe1febed48af5b974e4bcc424b35e9664863a4d402697147b57a488c864`。
- VCS: `7c0b40402271e8d40cda69f87cf3c2617c2e4382`、`crates/egui-wgpu`。
- `../import-upstream.py egui-wgpu`でarchive／index checksum／展開sourceをbyte照合して12ファイルを配置した。
  `.cargo-ok`、target、registryのchecksum管理ファイルはコピーしていない。
- 原本のfile別hash、license追加元・hashは[upstream-provenance.json](../upstream-provenance.json)。
- `target/egui037-stage2b-handoff/original/`は原本12ファイルだけのpackageを保存する。
  mipmap／atlas／startup／回帰／PATCHES／licenseのmIV追加ファイルは原本の境目には含めない。
  適用後は同handoffの`patched/`。具体的な原本不変・変更・追加一覧はCHECKPOINT.jsonに記録する。

## 差分台帳

| 項目 | 分類 | 新版への適応 |
| --- | --- | --- |
| managed mipmap allocation | (b) | opt-in時だけ全mip鎖とRENDER_ATTACHMENTを確保。通常textureのusageは拡大しない |
| mipmap再生成 | (b) | whole／partial upload後に既存の下位levelを再生成。公開generateは内部submitの契約を維持 |
| mipmap sampler／view | (b) | mip filterとLOD clamp、Noneへ変更した際のviewのlevel-0制限を保持。0.37のtexture flags binding／predictable filteringを維持 |
| 公開MipmapGenerator・texel count | (b) | BGL Option配列・immediate_size・multiview_maskをwgpu30へ適応。奇数寸法・非正方形の正確なcountを維持 |
| atlas ledger・renderer identity・texture_size | (b) | RGBAのみのImageDataとID別delta列へ適応。各ID内のwhole→partial順序、before／afterとfreeを記録 |
| atlas overflow skip-write | (b) 既存範囲のみ | 旧処理を保持。根治策として広げず、通常配送はpixel回帰で検証 |
| 無描画delta完結 | (b) | renderer準備済みならsurface lookup前にset drain。通常／no-surface／早期終了の同じownerでsubmit→free drain |
| 対象surfaceだけの破棄 | (b) | safe／unsafe set_window(id,None)とも対象surface・depth・MSAAだけを同一helperで除去。rendererは共有のまま |
| Windowsのsurface窓寸法制約 (§1.395) | (b) 新規 | Painterが全viewportのHWNDへdevice上限を登録し、外寸・configure寸法をmax_texture_dimension_2d以下に保つ。safe／unsafe取り付けはdevice確立→登録→実窓補正→実client寸法でinstall。共通resizeはsubclass迂回時に実窓を一度補正。status再configure／Lost再作成も検証済み寸法を使う。device上限8192・非Windowsは維持し、解除はWM_NCDESTROYだけ |
| startup GPU stage／root present observer | (b) | 新しいasync adapter列挙・Instance保持へ適応。set／終了処理の既存TextureDelivery spanを保持し、RootPresentedは実Queue::present返却後のみ |
| atlas-probe／capture outcome | (b) | drain前に適用権のない文字列summaryだけ採取。完結後の結果分類を保持し、capture-only等の新版経路も区別 |
| screenshot／readback診断 | (b) | 本家ScreenshotCallbackへ適応。viewport相関、format／map／mapped-range失敗、callback request数を診断する |
| upstream callback・hidden capture | (a) | 0.37のoffscreen capture継続、callback one-shot、画像共有をそのまま利用。旧channel／Event配送／追加repaintを再実装しない |
| upstream submission guard | (a)+(b) | 上流の早期終了時submit保証を利用し、mutable deltaとmipmapの実submit状態・free完結へ拡張 |
| SurfaceConfig／stencil | (a) | upstreamのruntime SurfaceConfig、depth／stencil formatとsample数を維持。stencil更新だけを理由にbufferを追加しない |
| shader空行 | (c) | 再適用しない。egui.wgslとtexture_copy.wgslは0.37原本のまま |
| 旧capture channel／古いsurface-error分類 | (c) | 新版callback／CurrentSurfaceTextureへ置換。acquire失敗でもcapture継続する本家機能を保持 |
| 旧Cargo.toml／同梱lockの移植 | (c) | 新manifestを基準にpollster dev依存とlocal license includeのみ追加。準備workspaceのpatch／lockへ統一 |
| lib test接続 | (a)+(b) | 新原本のlibはtest有効が既定なので旧test=true指定は不要。pollsterと新版回帰moduleを接続 |
| license本文 | (b) | 旧vendorのMIT／Apache本文をbyte変更なしで同梱。新原本のMIT OR Apache-2.0宣言を維持 |

`src/setup.rs`、`src/egui.wgsl`、`src/texture_copy.wgsl`、README、Cargo.toml.orig、
同梱Cargo.lock、.cargo_vcs_info.jsonは原本byteを保持する。
実行時は同梱lockではなく`vendor-next/Cargo.lock`を使用する。

## TexturesDeltaの所有・消費

公開Painter consumerは`&mut TexturesDelta`を受ける。各IDのdelta列を順にdrainし、
upload／mipmap submitを完了してからfreeをdrainする。別IDのHashMap順序は規定しない。
rendererが未準備ならset／freeをどちらも消費せず、既存の呼出元pending ownerへ残す。
適用権を持つ診断clone、無条件clear、二重適用、GPU待機、新pending owner、retryは追加しない。

TextureDeliveryは本家のqueue guardを拡張した呼出し内の完結ownerである。
通常描画のsubmit後はfallback空submitを抑止する。内部updateはUnchanged／Pending／Submittedを返し、
最後のmipmap生成でsubmit済みなら空submitせず、その後に通常writeがあればsubmitする。
update_buffers後の早期終了もsubmitを完結し、その後だけfreeを処理する。
surface再作成失敗も同じ完結処理へ合流する。共有rendererやtextureをwindow終了で一括廃棄しない。

headlessの未配送fixtureだけは、metadata検査後にTexturesDelta::clear、またはFullOutputの
drop_without_applying_deltasで意図的に終端する。GPU upload試験は実consumerでdrainする。

## 回帰・検証

回帰は実windowを作らない。offscreen GPUでwhole／partial／free・no-surface・mipmapと
sampler・画像readback・callback・公開set_window(None)の対象限定attachment終了を検査する。
実Surfaceはwindow無しでは作れないため、surface mapには共通helperのdrop sentinelを使い、
safe／unsafe公開入口は実GPU depth／MSAAと共有rendererで補完する。

通常構成は`--features winit`、診断構成は`--features winit,egui/miv-repaint-witness`。
wgpu独自の旧診断featureは存在せず、capture probeはruntime設定だったため、架空のfeatureは作らない。
probe off／onをreadback回帰で通す。wgpuのみの最小graphもegui無しでcheckする。

通常／診断構成は各16件全成功、skipなし。既存egui／egui-winit／egui-imeは106／43／26件成功。
GPU readbackは画素内容の証拠とし、submissionは実queue.submit返却直後だけ記録する
cfg(test) thread-local counterで別に検査する。productionの新field／flag／queueは作らない。
初回のreadbackのみの変異はsubmit欠落を検出できなかったため証拠に採用せず、
補強後のfallback無効化変異は1件失敗、対象限定cleanupを全clearへ戻す変異も1件失敗。
修正済みbytesへ復元後は全16件再成功した。

command・最終件数・変異・制約は[段2検証台帳](../../docs/egui-0.37-stage2-verification.md)に記録する。
AutoVsync／latency 1・環境変数の製品設定を新WgpuConfiguration.surfaceへ接続するのは段3で行い、
この段ではrootの旧設定を変更しない。D3D11 presenter／Naga27の別経路も変更しない。

## 段3: 正式vendorへの接続

準備graphの内容を `vendor/egui-wgpu` へ正式配置し、rootのpatchと依存wgpu30へ接続した。
原本記録は `vendor/upstream-provenance.json` / `vendor/import-upstream.py` に保持する。
standalone検証用のmanifestは自身をworkspace rootとし、egui/egui-winit/eframeも正式vendorへpatchする。
上記の `vendor-next/Cargo.lock` は段2時点の記録であり、段3以降はrootと正式standalone graphの通常cargo解決を使用する。

native presenterが既存renderer ownerでtexture更新のGPU commandを提出するため、
`renderer::TextureUpdateSubmission` をcrate rootから明示公開した。新しいrendererやdelta queueは追加しない。
製品の各FullOutput consumerはmap型deltaを順序どおりdrainし、必要なsubmissionの後でfreeする。
正式graphでの検証結果は [段3台帳](../../docs/egui-0.37-stage3-verification.md) に記録する。

## Windows surface窓寸法制約 (2026-10-18、§1.395)

`window_size_limit`のsubclassはHWND所有スレッドで登録し、値だけのdevice上限を保持する。
WM_WINDOWPOSCHANGING／WM_GETMINMAXINFOではDefSubclassProcによるwinitの補正後に上限を適用する。
surfaceだけのclampは行わず、enforceは実窓を補正してGetClientRectの実寸を返す。
初回登録・補正失敗はWgpuErrorへ返す。resize補正失敗はログを残して対象surfaceを外す。
set_window(None)／viewport削除でsubclassは外さない。同じHWND・Painter・固定device上限の寿命で
無害であり、Painterへのpointerを持たず、WM_NCDESTROYで自動解除する。
追加pending／retry／App側の窓制御・detached述語変更はない。

追加回帰は純関数4件、非表示Win32 HWND7件、実Painter1件（1024のdevice、初回safe／unsafe取り付け、
subclass迂回resize、status再configure、Lost再作成、depth／MSAA）。GPU前提不成立は明示失敗とする。
`test-lib-full.ps1`は本体lib限定なので含まれないが、`test-full.ps1`の無filter vendor nextestに含まれる。
専用実行: `cargo nextest run --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib`。

この実Painter回帰で、既存のLost再作成が旧surfaceを新surfaceのconfigureまで保持し、DXGIの
同一HWNDのswap chain制約で`Invalid surface`になることも確認した。Windowsだけ、関数の既存契約
どおり旧surfaceを先にdropし、検証済みの寸法とHWND値だけを新surfaceへ渡す順序へ修正した。


## Stage 3: discard texture delivery (design revision 4.6, 2026-10-17)

Implementation record and implementer internal inspection only. Independent
acceptance and command results belong in the stage-3 verification ledger.

No production renderer change is needed. Existing Painter::apply_textures_delta
and TextureDelivery complete early-discard delivery without a surface.

Two discard_texture_delivery offscreen tests exercise actual Context retries,
whole/partial updates, existing older pending work, last callback TextureHandle
Drop inside the immediate renderer, returned upload submit counts, real rendered
pixel readback before later free, and sibling texture retention. An unavailable
renderer test retains whole/partial and merges a later free non-cancellatively,
then completes through the existing consumer once the resource state is ready.
A missing headless adapter fails these tests instead of being counted as PASS.
No OS window, application/profile, present, or snapshot PNG is involved.

## Stage 7: first-Full atlas publication regression (2026-10-17)

No production renderer change. A new offscreen regression uses the real ready
Painter/TextureDelivery/Renderer on a pristine Context before any Full, then
checks first empty/discard/immediate Full delivery, actual atlas dimensions,
user texture pixel readback, and later free-only completion. The headless adapter
is mandatory. Before the egui collector repair it detects the same Managed(0)
zero-width create_texture validation error as the clean-install startup report.
The initial reservation is retained by egui, not silently discarded by wgpu.
