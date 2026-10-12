import numpy as np, json, sys, collections
def gt_at(gt,t,margin_a=0.15,margin_b=0.08):
    act=set(); steady=True; any_=False
    for st,en,ms in gt:
        if st-0.05<=t<=en+0.25: # inside or release tail
            any_=True
            if st+margin_a<=t<=en-margin_b: act|=set(ms)
            else: steady=False
    return act if (any_ and steady and act) else None
def cls(d):
    if d in(1,-1): return 'semi'+('+' if d>0 else '-')
    if d in(2,-2): return 'whole'
    if d in(12,-12,24,-24): return 'oct'
    if d in(19,28,31,34,7,-19,-7): return 'harm'
    return 'other'
def load(name):
    gt=json.load(open(f'{name}.json'))
    m=np.loadtxt(f'{name}_miv.csv',delimiter=',')
    miv=(m[:,0],m[:,1+88:],m[:,1:89])
    d=np.load(f'{name}_et.npz')
    et=(d['t']-0.08,d['r2'])
    return gt,miv,et
def evaluate(name, times, vals, rel, label):
    gt=json.load(open(f'{name}.json'))
    n=0; top_ok=0; topk_hits=0; topk_total=0; errs=collections.Counter(); spur=collections.Counter(); lit_tp=lit_fp=lit_fn=0
    for t,v in zip(times,vals):
        g=gt_at(gt,t)
        if g is None: continue
        n+=1
        mx=v.max()
        if mx<=1e-6: lit_fn+=len(g); continue
        k=len(g); top=np.argsort(-v)[:k]+21
        topk_hits+=len(set(top)&g); topk_total+=k
        if k==1:
            tm=next(iter(g)); a=int(np.argmax(v))+21
            if a==tm: top_ok+=1
            else: errs[cls(a-tm)]+=1
        thr=0.5*mx if rel else 0.5
        lit=set(np.nonzero(v>=thr)[0]+21)
        lit_tp+=len(lit&g); lit_fn+=len(g-lit)
        for x in lit-g:
            lit_fp+=1
            spur[cls(min(g,key=lambda q:abs(q-x))-x and x-min(g,key=lambda q:abs(q-x)))]+=1
    prec=lit_tp/max(1,lit_tp+lit_fp); rec=lit_tp/max(1,lit_tp+lit_fn)
    s=f'{name:7s} {label:9s} frames={n:4d} topK={topk_hits/max(1,topk_total):.3f}'
    if errs or top_ok: s+=f' top1={top_ok/n:.3f} err={dict(errs)}'
    s+=f' | lit P={prec:.3f} R={rec:.3f} FP/frame={lit_fp/max(1,n):.2f} {dict(spur)}'
    print(s)
for name in sys.argv[1:]:
    gt,(mt,mtrail,mraw),(et_t,et_v)=load(name)
    evaluate(name,mt,mtrail,True,'mIV-key')
    evaluate(name,mt,mraw,True,'mIV-raw')
    evaluate(name,et_t,et_v,False,'EffeTune')
