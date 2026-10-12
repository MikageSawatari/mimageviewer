import sys
import numpy as np, json, sys, collections
import os; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from evaluate import gt_at
for name in sys.argv[1:]:
    gt=json.load(open(f'{name}.json')); m=np.loadtxt(f'{name}_miv.csv',delimiter=',')
    t=m[:,0]; trail=m[:,89:]; raw=m[:,1:89]
    up=collections.defaultdict(list); dn=collections.defaultdict(list); rup=collections.defaultdict(list); rdn=collections.defaultdict(list)
    for i,tt in enumerate(t):
        g=gt_at(gt,tt)
        if not g: continue
        for q in g:
            k=q-21
            if 0<k<87:
                up[q].append(trail[i,k+1]/max(trail[i,k],1e-6)); dn[q].append(trail[i,k-1]/max(trail[i,k],1e-6))
                rup[q].append(raw[i,k+1]/max(raw[i,k],1e-6)); rdn[q].append(raw[i,k-1]/max(raw[i,k],1e-6))
    print(name, 'midi: key(up/down) raw(up/down)  [ratio of neighbour to true key, median]')
    for q in sorted(up):
        print(f'  {q:3d} {440*2**((q-69)/12):7.1f}Hz key {np.median(up[q]):.2f}/{np.median(dn[q]):.2f}  raw {np.median(rup[q]):.2f}/{np.median(rdn[q]):.2f}')
