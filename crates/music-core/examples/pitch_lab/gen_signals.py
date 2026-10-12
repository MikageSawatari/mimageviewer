import numpy as np, json, sys
SR=48000
rng=np.random.default_rng(1)
def hz(m): return 440*2**((m-69)/12)
def tone(f0, dur, amps, vib_cents=0, vib_rate=5.5, inharm=0.0, decay=None, formants=None):
    n=int(dur*SR); t=np.arange(n)/SR
    if vib_cents: inst=f0*2**(vib_cents/1200*np.sin(2*np.pi*vib_rate*t))
    else: inst=np.full(n,f0)
    ph=2*np.pi*np.cumsum(inst)/SR
    y=np.zeros(n)
    for h,a in enumerate(amps,1):
        fh=f0*h*np.sqrt(1+inharm*h*h)
        if fh>SR*0.45: break
        g=a
        if formants is not None:
            g*=sum(np.exp(-0.5*((fh-fc)/bw)**2)*w for fc,bw,w in formants)+0.05
        env=np.exp(-t*decay*(1+0.3*h)) if decay else 1
        y+=g*env*np.sin(ph*h*np.sqrt(1+inharm*h*h)+rng.uniform(0,2*np.pi))
    att=min(n,int(0.01*SR)); y[:att]*=np.linspace(0,1,att); y[-att:]*=np.linspace(1,0,att)
    return y
saw=[1/h for h in range(1,40)]
vocal_amps=[1/h**1.2 for h in range(1,40)]
VOWEL_A=[(730,90,1.0),(1090,110,0.6),(2440,160,0.25)]
tests={}
def build(name, events, total):
    y=np.zeros(int(total*SR)); gt=[]
    for st,dur,midis,kw in events:
        seg=sum(tone(hz(m+kw.get('cents',0)/100),dur,kw.get('amps',saw),**{k:v for k,v in kw.items() if k not in('cents','amps','gain')})*kw.get('gain',1) for m in midis)
        i=int(st*SR); y[i:i+len(seg)]+=seg
        gt.append([st,st+dur,list(midis)])
    y=0.3*y/np.max(np.abs(y))
    st=np.stack([y,y],1).astype('<f4'); st.tofile(f'{name}.f32')
    json.dump(gt,open(f'{name}.json','w'))
# A: saw tones every semitone 28..88
ev=[];t=0.2
for m in range(28,89): ev.append((t,0.5,[m],{})); t+=0.65
build('saw',ev,t+0.3)
# B: detuned +25 / -25 cents
ev=[];t=0.2
for m in range(31,86,3):
    for c in (25,-25): ev.append((t,0.5,[m],{'cents':c})); t+=0.65
build('detune',ev,t+0.3)
# C: vocal-like with vibrato
ev=[];t=0.2
for m in range(45,78,2): ev.append((t,0.7,[m],{'amps':vocal_amps,'vib_cents':35,'formants':VOWEL_A})); t+=0.85
build('vocal',ev,t+0.3)
# D: piano-like
ev=[];t=0.2
for m in range(33,88,3): ev.append((t,0.8,[m],{'inharm':0.0004,'decay':2.0,'amps':[1/h**0.8 for h in range(1,40)]})); t+=0.95
build('piano',ev,t+0.3)
# E: triads
ev=[];t=0.2
for r in range(48,73,2):
    for q in ((0,4,7),(0,3,7)): ev.append((t,0.7,[r+q[0],r+q[1],r+q[2]],{'amps':[1/h**1.3 for h in range(1,40)]})); t+=0.85
build('chords',ev,t+0.3)
# F: mix: bass + pad chord + vocal melody (each event 0.6 s)
ev=[];t=0.2
prog=[(36,(48,52,55)),(41,(53,57,60)),(43,(55,59,62)),(33,(45,48,52))]
mel=[67,69,71,72,74,72,71,69,67,65,64,62,64,65,67,69]
for i,mm in enumerate(mel):
    b,ch=prog[(i//4)%4]
    ev.append((t,0.6,[b],{'amps':[1/h**1.5 for h in range(1,20)],'gain':1.2}))
    ev.append((t,0.6,list(ch),{'amps':[1/h**1.8 for h in range(1,20)],'gain':0.35}))
    ev.append((t,0.6,[mm],{'amps':vocal_amps,'vib_cents':30,'formants':VOWEL_A,'gain':0.9}))
    t+=0.6
build('mix',ev,t+0.3)
print('done')
