import json, os, sys
from collections import Counter

path = os.path.expandvars(r'%APPDATA%\mimageviewer\logs\perf_events.jsonl')
if len(sys.argv) > 1:
    path = sys.argv[1]

queue_counts = Counter()
queue_by_priority = Counter()
pdf_busy_samples = []
pdf_send_times = []
pdf_recv_times = []
pdf_render_durations = []
all_cats = Counter()
all_kinds = Counter()
cat_kind = Counter()

with open(path, 'r', encoding='utf-8') as f:
    for line in f:
        try:
            ev = json.loads(line)
        except Exception:
            continue
        cat = ev.get('cat')
        kind = ev.get('kind')
        all_cats[cat] += 1
        all_kinds[kind] += 1
        cat_kind[(cat, kind)] += 1
        if cat == 'thumb' and kind == 'enqueue':
            q = ev.get('queue')
            pri = ev.get('priority')
            queue_counts[q] += 1
            queue_by_priority[(q, pri)] += 1
        if cat == 'pdf' and kind == 'pool_send':
            busy = ev.get('busy', 0)
            pdf_busy_samples.append(busy)
            pdf_send_times.append(ev.get('t', 0))
        if cat == 'pdf' and kind == 'pool_recv':
            pdf_recv_times.append(ev.get('t', 0))
            dur = ev.get('duration_ms') or ev.get('elapsed_ms') or ev.get('ms')
            if dur is not None:
                pdf_render_durations.append(dur)

print('=== top categories ===')
for k, v in all_cats.most_common(20):
    print(f'  cat={k!r}: {v}')

print('\n=== top (cat, kind) ===')
for (c, k), v in cat_kind.most_common(40):
    print(f'  {c!r:>20} / {k!r:<30}: {v}')

print('\n=== thumb.enqueue queue distribution ===')
print('  queue:', dict(queue_counts))
print('  by (queue, priority):', dict(queue_by_priority))

print('\n=== pdf.pool_send ===')
print('  count:', len(pdf_busy_samples))
if pdf_busy_samples:
    print('  busy max:', max(pdf_busy_samples))
    bc = Counter(pdf_busy_samples)
    print('  busy histogram:', dict(sorted(bc.items())))
if pdf_send_times:
    print(f'  first->last span: {pdf_send_times[-1] - pdf_send_times[0]:.3f} s')

print('\n=== pdf.pool_recv ===')
if pdf_recv_times:
    print(f'  count: {len(pdf_recv_times)}, span: {pdf_recv_times[-1] - pdf_recv_times[0]:.3f} s')
if pdf_render_durations:
    sd = sorted(pdf_render_durations)
    n = len(sd)
    p50 = sd[n // 2]
    p95 = sd[int(n * 0.95)]
    print(f'  render duration: n={n} p50={p50:.1f} p95={p95:.1f} max={sd[-1]:.1f} ms')
