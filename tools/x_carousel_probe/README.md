# x_carousel_probe

SNS のカルーセル表示が画像と画像の間に入れる**隙間**を、「元画像の何 px か」で実測するための道具。
[SNS 分割書き出し](../../docs/sns-split-export-plan.md) の継ぎ目比率 (X = 枠幅の 1.7%) は、これで測った値。

このツールは開発検証用であり、mIV 本体の配布物には含めない。

X はこの表示レイアウトを過去に変えている (2026 年 7 月にグリッドからカルーセルへ)。
**変わったと分かったら、同じ手順で測り直して `SnsTarget::seam_ratio_parts()` を更新する。**

## 必要な Python パッケージ

```powershell
python -m pip install pillow numpy
```

---

## 手順 1: 投稿せずに測る (まずこれ)

**設計上いちばん効く「隙間は表示幅に比例するのか、固定 px なのか」は、自分で投稿しなくても
他人の公開投稿で分かる。**投稿ページを開き、ブラウザのコンソールへ次を貼る。

```javascript
(()=>{const rows={};document.querySelectorAll('article').forEach((a,ai)=>{[...a.querySelectorAll('img')].filter(i=>/pbs\.twimg\.com\/media/.test(i.src)).forEach(i=>{const b=i.getBoundingClientRect();if(b.width<60)return;const k=ai+'@'+Math.round(b.y/8);(rows[k]=rows[k]||[]).push({i,b});});});const out=[];Object.keys(rows).forEach(k=>{const m=rows[k].sort((p,q)=>p.b.x-q.b.x);if(m.length<2)return;m.forEach((o,n)=>{const b=o.b,p=n?m[n-1].b:null;out.push({row:k,n,x:+b.x.toFixed(1),w:+b.width.toFixed(1),h:+b.height.toFixed(1),ar:+(b.width/b.height).toFixed(3),nw:o.i.naturalWidth,nh:o.i.naturalHeight,natAr:+(o.i.naturalWidth/o.i.naturalHeight).toFixed(3),name:(o.i.src.match(/[?&]name=([^&]+)/)||[])[1]||'',gap:p?+(b.x-(p.x+p.width)).toFixed(2):null,gapPct:p?+((b.x-(p.x+p.width))/b.width*100).toFixed(3):null});});});console.log('innerWidth',innerWidth,'dpr',devicePixelRatio);console.table(out);})()
```

行 (y 座標) ごとにまとめるので、引用ポストの画像が本体のカルーセルに混ざらない。

読み取るもの:

- **`gap` / `gapPct`** — 本題。**ブラウザ幅を変えて 2〜3 点取る**こと。`gapPct` が一定なら
  比例、`gap` の px が一定で `gapPct` が動くなら固定 px
- **`ar` と `natAr` の一致** — 一致していればその比率は切り取られていない
- **`nw` / `nh` / `name`** — 配信し直している解像度。出力解像度を上げる意味があるかが分かる
- **`y` が全部同じ** ならカルーセル (横一列)、**2 種類ある** なら旧グリッド表示

**アプリ (iOS / Android) は DevTools が使えないので、スクリーンショットの画素を直接数える。**
ブラウザとアプリで値が違った実績があるので、アプリをブラウザで代用しないこと
(実測 2026-09-01: Web 5.33 CSS px / iOS アプリ 4.00 CSS px)。

---

## 手順 2: 自分で投稿して測る

```powershell
python tools\x_carousel_probe\gen_test_images.py
```

`out/` に 2 / 3 / 4 枚投稿用の画像 (各 1536x2048 = 3:4) と、合成した状態の参照画像ができる。

各画像には、N 枚を横に並べた合成キャンバス上の x 座標が色として埋め込んである。

```
R = 255 * x / (W-1)   粗 (単調増加。何周目かを決める)
G = x mod 256         密 (256px 周期。1px 分解能)
B = 96                固定 (背景色と区別する目印)
```

縮小されても連続階調なので壊れない。実測で **50% 縮小 + JPEG q75 でも中央値 0px /
90 パーセンタイル 1px** の精度で復元できる。上下の白帯は目視用の目盛り (10px 刻み、
200px ごとに数字)、上下端のマゼンタ線は縦方向に切り取られたかの確認用。

**`x2_1.png` と `x2_2.png` をこの順で 1 つのポストに添付して投稿する。**隙間ゼロで切って
あるので、SNS が入れる隙間の分だけ絵がずれて見えるはず。継ぎ目が写るようにスクリーン
ショットを撮り (上下の白帯ではなく**色帯を通る高さ**で)、

```powershell
python tools\x_carousel_probe\decode_x.py shot.png --tiles 2
```

`GAP` の行に出る `% of one tile` が求める値。ダーク / ディム / ライトのどの背景でも動く。

`image` 行の `source x` が 1 枚目で 0 付近から 1535 付近、2 枚目で 1536 付近から 3071 付近に
なっていれば横方向に切り取られていない。上下のマゼンタ線が両方見えていれば縦も切られていない。

---

## 手順 3: 書き出した結果を検証する

投稿用に分割した実ファイルが、元画像のどこをどれだけ捨てているかは、FFT 相互相関で位置合わせ
すれば測れる (テスト画像は要らない)。2026-09-01 の実投稿検証ではこの方法で、
1792x2304 の絵を 3 枚へ分割した結果が「x=0 / 600 / 1200 に等間隔、捨てた帯 9px (枠幅 591px)」
= 設計値 1.7% どおりであることを確認した。

`decode_x.py` の `locate()` と同じ手順 (グレースケール化 → 1/4 縮小 → 平均を引いて FFT
相互相関 → argmax) を、書き出したファイルと元画像に対して適用する。

---

## 注意

- 投稿は公開される。テストが済んだら削除してよい
- PowerShell の表示が化ける場合は `chcp 65001`。`decode_x.py` の出力自体は ASCII のみ
- `out/` は `.gitignore` 済み。スクリプトから何度でも作り直せる
