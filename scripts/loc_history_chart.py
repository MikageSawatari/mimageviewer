#!/usr/bin/env python3
"""Render the output of loc_history.py as a self-contained HTML report.

    python scripts/loc_history.py
    python scripts/loc_history_chart.py

Reads target/loc-history/loc_history.json and writes loc_history.html next to
it: three charts (cumulative LOC by language, weekly increment, cumulative LOC
by area) plus a table view.  No external assets, no network.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

# Categorical slots, in the fixed order that clears the adjacent-pair CVD and
# normal-vision gates in both modes (dataviz reference palette).  Never cycled:
# past eight series the tail folds into "その他".
LIGHT = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#4a3aa7", "#e34948"]
DARK = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"]
MAX_SERIES = 8
OTHER = "その他"


def fold(points: list[dict], field: str) -> tuple[list[str], list[list[int]]]:
    """Rank series by their final size, keep the top slots, fold the tail."""
    final = points[-1][field]
    ranked = sorted(final, key=lambda k: -final[k])
    if len(ranked) > MAX_SERIES:
        keys = ranked[: MAX_SERIES - 1] + [OTHER]
        tail = set(ranked[MAX_SERIES - 1 :])
    else:
        keys = ranked
        tail = set()
    series = []
    for p in points:
        row = [p[field].get(k, 0) for k in keys if k != OTHER]
        if tail:
            row.append(sum(v for k, v in p[field].items() if k in tail))
        series.append(row)
    return keys, series


HTML = """<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>mImageViewer — LOC の推移</title>
<style>
  :root {
    color-scheme: light;
    --surface-1: #fcfcfb;
    --plane: #f9f9f7;
    --text-primary: #0b0b0b;
    --text-secondary: #52514e;
    --muted: #898781;
    --grid: #e1e0d9;
    --axis: #c3c2b7;
    --border: rgba(11,11,11,0.10);
__LIGHT_VARS__
  }
  @media (prefers-color-scheme: dark) {
    :root:where(:not([data-theme="light"])) {
      color-scheme: dark;
      --surface-1: #1a1a19;
      --plane: #0d0d0d;
      --text-primary: #ffffff;
      --text-secondary: #c3c2b7;
      --muted: #898781;
      --grid: #2c2c2a;
      --axis: #383835;
      --border: rgba(255,255,255,0.10);
__DARK_VARS__
    }
  }
  :root[data-theme="dark"] {
    color-scheme: dark;
    --surface-1: #1a1a19;
    --plane: #0d0d0d;
    --text-primary: #ffffff;
    --text-secondary: #c3c2b7;
    --muted: #898781;
    --grid: #2c2c2a;
    --axis: #383835;
    --border: rgba(255,255,255,0.10);
__DARK_VARS__
  }
  * { box-sizing: border-box; }
  body {
    margin: 0;
    padding: 28px 20px 56px;
    background: var(--plane);
    color: var(--text-primary);
    font: 14px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif;
  }
  main { max-width: 1040px; margin: 0 auto; }
  h1 { font-size: 21px; margin: 0 0 4px; letter-spacing: .01em; }
  .sub { color: var(--text-secondary); margin: 0 0 24px; font-size: 13px; }
  .tiles { display: flex; flex-wrap: wrap; gap: 10px; margin-bottom: 24px; }
  .tile {
    flex: 1 1 150px; background: var(--surface-1); border: 1px solid var(--border);
    border-radius: 10px; padding: 12px 14px;
  }
  .tile .label { color: var(--text-secondary); font-size: 12px; }
  .tile .value { font-size: 25px; line-height: 1.25; margin-top: 2px; }
  .tile .value .unit { font-size: 13px; color: var(--text-secondary); margin-left: 2px; }
  .card {
    background: var(--surface-1); border: 1px solid var(--border);
    border-radius: 10px; padding: 16px 16px 12px; margin-bottom: 18px;
  }
  .card h2 { font-size: 15px; margin: 0 0 2px; font-weight: 600; }
  .card .note { color: var(--text-secondary); font-size: 12px; margin: 0 0 12px; }
  .card .foot { color: var(--muted); font-size: 12px; margin: 6px 0 0; }
  .plot { position: relative; }
  .legend { display: flex; flex-wrap: wrap; gap: 4px 16px; margin-top: 10px; font-size: 12px; }
  .legend span { display: inline-flex; align-items: center; gap: 6px; color: var(--text-secondary); }
  .legend i { width: 10px; height: 10px; border-radius: 3px; display: inline-block; }
  .tip {
    position: absolute; pointer-events: none; opacity: 0; transition: opacity .08s;
    background: var(--surface-1); border: 1px solid var(--border); border-radius: 8px;
    padding: 8px 10px; font-size: 12px; min-width: 150px;
    box-shadow: 0 6px 20px rgba(0,0,0,.18); z-index: 3;
  }
  .tip .head { color: var(--text-secondary); margin-bottom: 5px; }
  .tip .row { display: flex; align-items: center; gap: 6px; white-space: nowrap; }
  .tip .row b { font-weight: 400; flex: 1; }
  .tip .row em { font-style: normal; font-variant-numeric: tabular-nums; }
  .tip i { width: 8px; height: 8px; border-radius: 2px; display: inline-block; }
  .tip .total { border-top: 1px solid var(--border); margin-top: 5px; padding-top: 5px; }
  details { background: var(--surface-1); border: 1px solid var(--border); border-radius: 10px; padding: 12px 16px; }
  summary { cursor: pointer; font-size: 14px; font-weight: 600; }
  .scroll { overflow-x: auto; margin-top: 12px; }
  table { border-collapse: collapse; font-size: 12px; font-variant-numeric: tabular-nums; }
  th, td { padding: 4px 10px; text-align: right; white-space: nowrap; border-bottom: 1px solid var(--grid); }
  th:first-child, td:first-child { text-align: left; }
  thead th { color: var(--text-secondary); font-weight: 600; }
  .method { color: var(--text-secondary); font-size: 12px; margin-top: 22px; }
  .method li { margin: 3px 0; }
</style>
</head>
<body>
<main>
  <h1>mImageViewer — ソースコード行数の推移</h1>
  <p class="sub">__RANGE__ ・ 週次スナップショット __NPOINTS__ 点 ・ __NCOMMITS__ コミット</p>

  <div class="tiles" id="tiles"></div>

  <div class="card">
    <h2>総行数の推移（言語別）</h2>
    <p class="note">各週時点のリポジトリに存在する行数。積み上げの高さが総行数。</p>
    <div class="plot" id="plot-lang"></div>
    <div class="legend" id="legend-lang"></div>
  </div>

  <div class="card">
    <h2>週ごとの増加量</h2>
    <p class="note" id="delta-note">前週との差分（削除を差し引いた純増）。</p>
    <div class="plot" id="plot-delta"></div>
    <div class="legend" id="legend-delta"></div>
    <p class="foot" id="delta-foot"></p>
  </div>

  <div class="card">
    <h2>総行数の推移（領域別）</h2>
    <p class="note">同じ行数を、リポジトリ内の置き場所で分けたもの。</p>
    <div class="plot" id="plot-area"></div>
    <div class="legend" id="legend-area"></div>
  </div>

  <details>
    <summary>数値を表で見る</summary>
    <div class="scroll" id="table"></div>
  </details>

  <ul class="method">
    <li>数え方は物理行数（空行・コメントを含む）。cloc のような分類はしていません。</li>
    <li>除外: <code>vendor/</code> 配下（<code>vendor/eframe</code>、<code>hls.min.js</code> などの第三者コード）、<code>target/</code>、<code>Cargo.lock</code>、生成物の <code>changelog.html</code>、および画像・フォント・DLL などのバイナリ。</li>
    <li>累計のグラフは各週のスナップショット（その日 23:59 以前で最も新しいコミットのツリー）で、現在値はリポジトリの実際の行数と一致します。</li>
    <li>「執筆時点で計上」はマージコミットを除いた各コミットの差分を author date で振り分けたもの。マージ時の衝突解決ぶんが落ち、追加→削除→再追加は二重に数えるため近似で、合計は実際の行数と __DRIFT__ ずれます。</li>
    <li>再生成: <code>python scripts/loc_history.py</code>（累計）と <code>python scripts/loc_history.py --attribution authored --json target/loc-history/loc_authored.json --csv target/loc-history/loc_authored.csv</code>（執筆時点）を実行してから <code>python scripts/loc_history_chart.py</code></li>
  </ul>
</main>

<script>
const DATA = __DATA__;
const fmt = n => n.toLocaleString('en-US');
const shortNum = n => n >= 1e6 ? (n / 1e6).toFixed(n % 1e6 === 0 ? 0 : 1) + 'M'
                    : n >= 1000 ? Math.round(n / 1000) + 'k' : String(n);
const mmdd = iso => { const [, m, d] = iso.split('-'); return `${+m}/${+d}`; };
const SVG = 'http://www.w3.org/2000/svg';
const el = (name, attrs, parent) => {
  const n = document.createElementNS(SVG, name);
  for (const k in attrs) n.setAttribute(k, attrs[k]);
  if (parent) parent.appendChild(n);
  return n;
};
const cssVar = name => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/* stat tiles ----------------------------------------------------------- */
const last = DATA.points.at(-1);
const weeks = DATA.points.length - 1;
const codeOnly = last.total - (last.by_language['Markdown'] || 0);
const tiles = [
  ['総行数（現在）', fmt(last.total), '行'],
  ['うち Rust', fmt(last.by_language['Rust'] || 0), '行'],
  ['ドキュメント以外', fmt(codeOnly), '行'],
  ['週あたり平均', '+' + fmt(Math.round(last.total / weeks)), '行/週'],
];
document.getElementById('tiles').innerHTML = tiles.map(([l, v, u]) =>
  `<div class="tile"><div class="label">${l}</div><div class="value">${v}<span class="unit">${u}</span></div></div>`
).join('');

/* shared plumbing ------------------------------------------------------ */
function ticks(max) {
  const raw = max / 5;
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = [1, 2, 2.5, 5, 10].map(m => m * mag).find(s => s >= raw) || mag * 10;
  const top = Math.ceil(max / step) * step;  // the axis must cover max, not stop below it
  const out = [];
  for (let v = 0; v <= top + 1e-9; v += step) out.push(v);
  return out;
}

function frame(host, h, maxY) {
  host.innerHTML = '';
  const w = Math.max(host.clientWidth, 280);
  const svg = el('svg', { width: w, height: h, viewBox: `0 0 ${w} ${h}` }, host);
  const padL = 52, padR = 14, padT = 10, padB = 26;
  const x = i => padL + (i * (w - padL - padR)) / Math.max(DATA.points.length - 1, 1);
  const y = v => h - padB - (v / maxY) * (h - padT - padB);
  for (const t of ticks(maxY)) {
    el('line', { x1: padL, x2: w - padR, y1: y(t), y2: y(t), stroke: t === 0 ? cssVar('--axis') : cssVar('--grid'), 'stroke-width': 1 }, svg);
    const lab = el('text', { x: padL - 8, y: y(t) + 4, 'text-anchor': 'end', fill: cssVar('--muted'), 'font-size': 11 }, svg);
    lab.textContent = shortNum(t);
  }
  const every = w < 560 ? 4 : 2;
  DATA.points.forEach((p, i) => {
    if (i % every && i !== DATA.points.length - 1) return;
    const lab = el('text', { x: x(i), y: h - 8, 'text-anchor': 'middle', fill: cssVar('--muted'), 'font-size': 11 }, svg);
    lab.textContent = mmdd(p.date);
  });
  return { svg, w, h, x, y, padL, padR, padT, padB };
}

function tooltip(host) {
  const tip = document.createElement('div');
  tip.className = 'tip';
  host.appendChild(tip);
  return tip;
}

/* stacked area --------------------------------------------------------- */
function stacked(hostId, legendId, keys, rows, colors) {
  const host = document.getElementById(hostId);
  const draw = () => {
    const totals = rows.map(r => r.reduce((a, b) => a + b, 0));
    const maxY = ticks(Math.max(...totals)).at(-1);
    const { svg, w, h, x, y, padL, padR, padT, padB } = frame(host, 340, maxY);
    const cum = rows.map(() => 0);
    const bands = [];
    keys.forEach((key, s) => {
      const lower = cum.slice();
      rows.forEach((r, i) => (cum[i] += r[s]));
      const top = rows.map((_, i) => `${x(i)},${y(cum[i])}`);
      const bot = rows.map((_, i) => `${x(i)},${y(lower[i])}`).reverse();
      el('path', {
        d: `M${top.join(' L')} L${bot.join(' L')} Z`,
        fill: colors[s], stroke: cssVar('--surface-1'), 'stroke-width': 2, 'stroke-linejoin': 'round',
      }, svg);
      bands.push({ key, color: colors[s], mid: (cum.at(-1) + lower.at(-1)) / 2, size: rows.at(-1)[s] });
    });
    /* direct labels on bands thick enough to hold one */
    bands.forEach(b => {
      if ((b.size / maxY) * (h - padT - padB) < 15) return;
      const t = el('text', { x: w - padR - 6, y: y(b.mid) + 4, 'text-anchor': 'end', 'font-size': 11, fill: cssVar('--surface-1'), 'font-weight': 600 }, svg);
      t.textContent = b.key;
    });
    const guide = el('line', { y1: padT, y2: h - padB, stroke: cssVar('--text-secondary'), 'stroke-width': 1, opacity: 0 }, svg);
    const tip = tooltip(host);
    const hit = el('rect', { x: 0, y: 0, width: w, height: h, fill: 'transparent' }, svg);
    hit.addEventListener('mousemove', ev => {
      const bounds = host.getBoundingClientRect();
      const px = ev.clientX - bounds.left;
      let i = Math.round(((px - padL) / (w - padL - padR)) * (DATA.points.length - 1));
      i = Math.min(Math.max(i, 0), DATA.points.length - 1);
      guide.setAttribute('x1', x(i)); guide.setAttribute('x2', x(i)); guide.setAttribute('opacity', .5);
      tip.innerHTML = `<div class="head">${DATA.points[i].date}</div>` +
        keys.map((k, s) => `<div class="row"><i style="background:${colors[s]}"></i><b>${k}</b><em>${fmt(rows[i][s])}</em></div>`).reverse().join('') +
        `<div class="row total"><b>合計</b><em>${fmt(totals[i])}</em></div>`;
      tip.style.opacity = 1;
      const tw = tip.offsetWidth;
      tip.style.left = Math.min(Math.max(x(i) - tw / 2, 0), w - tw) + 'px';
      tip.style.top = '4px';
    });
    hit.addEventListener('mouseleave', () => { tip.style.opacity = 0; guide.setAttribute('opacity', 0); });
    document.getElementById(legendId).innerHTML = keys.map((k, s) =>
      `<span><i style="background:${colors[s]}"></i>${k} <em style="font-style:normal;color:var(--muted)">${fmt(rows.at(-1)[s])}</em></span>`
    ).join('');
  };
  draw();
  new ResizeObserver(draw).observe(host);
}

/* grouped bars --------------------------------------------------------- */
function bars(hostId, legendId, series, colors) {
  const host = document.getElementById(hostId);
  const draw = () => {
    const maxY = ticks(Math.max(...series.flatMap(s => s.values))).at(-1);
    const n = series[0].values.length;
    const { svg, w, h, x, y, padL, padR, padB } = frame(host, 240, maxY);
    const slot = (w - padL - padR) / Math.max(n - 1, 1);
    const group = Math.max(Math.min(slot * 0.68, 30), 4);
    const bw = Math.max((group - (series.length - 1) * 2) / series.length, 2);
    const tip = tooltip(host);
    for (let i = 0; i < n; i++) {
      const marks = series.map((s, k) => {
        const v = s.values[i];
        const bx = x(i) - group / 2 + k * (bw + 2), by = y(v), bh = h - padB - by;
        const r = Math.min(4, bw / 2, bh);
        return el('path', {
          d: `M${bx},${h - padB} L${bx},${by + r} Q${bx},${by} ${bx + r},${by} L${bx + bw - r},${by} Q${bx + bw},${by} ${bx + bw},${by + r} L${bx + bw},${h - padB} Z`,
          fill: colors[k],
        }, svg);
      });
      const hit = el('rect', { x: x(i) - slot / 2, y: 0, width: slot, height: h, fill: 'transparent' }, svg);
      hit.addEventListener('mouseenter', () => {
        marks.forEach(m => m.setAttribute('opacity', .78));
        tip.innerHTML = `<div class="head">${DATA.points[i].date} までの週</div>` +
          series.map((s, k) => `<div class="row"><i style="background:${colors[k]}"></i><b>${s.name}</b><em>+${fmt(s.values[i])}</em></div>`).join('') +
          `<div class="row total"><b>累計</b><em>${fmt(DATA.points[i].total)}</em></div>`;
        tip.style.opacity = 1;
        const tw = tip.offsetWidth;
        tip.style.left = Math.min(Math.max(x(i) - tw / 2, 0), w - tw) + 'px';
        tip.style.top = '4px';
      });
      hit.addEventListener('mouseleave', () => { marks.forEach(m => m.removeAttribute('opacity')); tip.style.opacity = 0; });
    }
    document.getElementById(legendId).innerHTML = series.length < 2 ? '' : series.map((s, k) =>
      `<span><i style="background:${colors[k]}"></i>${s.name}</span>`).join('');
  };
  draw();
  new ResizeObserver(draw).observe(host);
}

/* build ---------------------------------------------------------------- */
const palette = () => matchMedia('(prefers-color-scheme: dark)').matches && document.documentElement.dataset.theme !== 'light'
  || document.documentElement.dataset.theme === 'dark' ? DATA.dark : DATA.light;

stacked('plot-lang', 'legend-lang', DATA.lang_keys, DATA.lang_rows, palette());
stacked('plot-area', 'legend-area', DATA.area_keys, DATA.area_rows, palette());
const deltas = DATA.points.map((p, i) => p.total - (i ? DATA.points[i - 1].total : 0));
const series = [];
if (DATA.authored) series.push({ name: '執筆時点で計上', values: DATA.authored });
series.push({ name: DATA.authored ? 'master に載った時点で計上' : '週ごとの純増', values: deltas });
bars('plot-delta', 'legend-delta', series, [palette()[0], palette()[1]]);

const lead = series[0];
const peak = lead.values.indexOf(Math.max(...lead.values));
document.getElementById('delta-foot').textContent =
  `最大は ${DATA.points[peak].date} までの週で +${fmt(lead.values[peak])} 行。` +
  `最初の棒は開始週なので 0 からの増分、最後の棒は ${DATA.spanLastDays} 日ぶん。`;
if (DATA.authored) {
  document.getElementById('delta-note').innerHTML =
    '同じ作業を 2 通りに割り当てたもの。<b>執筆時点</b>は各コミットの author date、' +
    '<b>master に載った時点</b>は週ごとのスナップショット差分。長く生きたブランチは後者だとマージ週に一括計上される。';
}

/* table view ----------------------------------------------------------- */
document.getElementById('table').innerHTML =
  '<table><thead><tr><th>日付</th><th>コミット</th><th>ファイル数</th><th>総行数</th><th>増分</th>' +
  DATA.lang_keys.map(k => `<th>${k}</th>`).join('') + '</tr></thead><tbody>' +
  DATA.points.map((p, i) => `<tr><td>${p.date}</td><td>${p.commit}</td><td>${fmt(p.files)}</td><td>${fmt(p.total)}</td><td>+${fmt(deltas[i])}</td>` +
    DATA.lang_rows[i].map(v => `<td>${fmt(v)}</td>`).join('') + '</tr>').join('') +
  '</tbody></table>';
</script>
</body>
</html>
"""


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--json", default="target/loc-history/loc_history.json")
    ap.add_argument(
        "--authored",
        default="target/loc-history/loc_authored.json",
        help="optional --attribution authored run; its weekly increments are "
        "drawn beside the snapshot ones. Skipped if absent or misaligned.",
    )
    ap.add_argument("--out", default="target/loc-history/loc_history.html")
    args = ap.parse_args()

    raw = json.loads(Path(args.json).read_text(encoding="utf-8"))
    points = raw["points"]

    authored: list[int] | None = None
    authored_path = Path(args.authored)
    if authored_path.exists():
        alt = json.loads(authored_path.read_text(encoding="utf-8"))["points"]
        if [p["date"] for p in alt] == [p["date"] for p in points]:
            authored = [p["total"] - (alt[i - 1]["total"] if i else 0) for i, p in enumerate(alt)]
        else:
            print(f"note: {authored_path} covers different dates -- skipping it")
    lang_keys, lang_rows = fold(points, "by_language")
    area_keys, area_rows = fold(points, "by_area")

    import datetime as dt
    import subprocess

    span_last = (
        dt.date.fromisoformat(points[-1]["date"]) - dt.date.fromisoformat(points[-2]["date"])
    ).days
    ncommits = subprocess.run(
        ["git", "rev-list", "--count", points[-1]["commit"]],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()

    data = {
        "points": points,
        "lang_keys": lang_keys,
        "lang_rows": lang_rows,
        "area_keys": area_keys,
        "area_rows": area_rows,
        "light": LIGHT[: len(lang_keys)],
        "dark": DARK[: len(lang_keys)],
        "spanLastDays": span_last,
        "authored": authored,
    }

    html = (
        HTML.replace("__DATA__", json.dumps(data, ensure_ascii=False, separators=(",", ":")))
        .replace("__LIGHT_VARS__", "\n".join(f"    --series-{i + 1}: {c};" for i, c in enumerate(LIGHT)))
        .replace("__DARK_VARS__", "\n".join(f"      --series-{i + 1}: {c};" for i, c in enumerate(DARK)))
        .replace("__RANGE__", f"{points[0]['date']} 〜 {points[-1]['date']}")
        .replace("__NPOINTS__", str(len(points)))
        .replace("__NCOMMITS__", f"{int(ncommits):,}")
        .replace(
            "__DRIFT__",
            f"{sum(authored) - points[-1]['total']:+,} 行（{(sum(authored) / points[-1]['total'] - 1) * 100:+.1f}%）"
            if authored
            else "わずかに",
        )
    )

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(html, encoding="utf-8")
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
