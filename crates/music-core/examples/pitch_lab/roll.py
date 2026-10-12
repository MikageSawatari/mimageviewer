import numpy as np, matplotlib, sys
matplotlib.use('Agg'); import matplotlib.pyplot as plt
lo,hi=28,96
names=sys.argv[1:]
fig,axes=plt.subplots(3,len(names),figsize=(9*len(names),13),squeeze=False)
for c,n in enumerate(names):
    m=np.loadtxt(f'{n}_miv.csv',delimiter=','); h=np.loadtxt(f'{n}_hn.csv',delimiter=',')
    miv=m[:,89:]; h0=h[:,1:89]; h3=h[:,1+88*3:1+88*4]
    h3=np.vstack([h3[8:],np.zeros((8,88))])  # head3 refers to 8 hops (160 ms) earlier
    panels=[(miv,'mIV 現行の鍵盤の明るさ',m[:,0]),(h0,'HarmNet (EffeTune) 即時推定',h[:,0]-0.02),(h3,'HarmNet 160ms 先読み版 (mIV なら遅延なしで使える)',h[:,0]-0.02)]
    for r,(v,title,t) in enumerate(panels):
        ax=axes[r][c]
        ax.imshow(v[:,lo-21:hi-21+1].T,origin='lower',aspect='auto',cmap='magma',vmin=0,vmax=1,
                  extent=[t[0],t[-1],lo-0.5,hi+0.5],interpolation='nearest')
        for k in range(lo,hi+1):
            if k%12 in (1,3,6,8,10): ax.axhspan(k-0.5,k+0.5,color='#3060ff',alpha=0.10,lw=0)
            if k%12==0: ax.axhline(k-0.5,color='w',lw=0.4,alpha=0.5)
        ax.set_yticks([k for k in range(lo,hi+1) if k%12==0]); ax.set_yticklabels([f'C{k//12-1}' for k in range(lo,hi+1) if k%12==0])
        ax.set_title(f'{n}: {title}',fontname='Yu Gothic',fontsize=12)
        ax.set_xlabel('秒',fontname='Yu Gothic')
fig.tight_layout(); fig.savefig('roll_compare.png',dpi=80)
