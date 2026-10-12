import numpy as np, effetune as e, sys
name=sys.argv[1]
x=np.fromfile(f'{name}.f32',dtype='<f4').reshape(-1,2).T.copy()
frames=[]
def cb(fr):
    if fr.kind=='noteSpectrogram': frames.append(fr)
chain=e.Chain([e.NoteSpectrogram(minimum_midi=21,maximum_midi=108)])
chain.process(x,sample_rate=48000,block_size=960,on_telemetry=cb)
hop=frames[0].hop_seconds
T=[];H0=[];R2=[];R8=[];VOL=[]
for fr in frames:
    T.append(fr.time_seconds)
    H0.append(np.array(fr.levels).reshape(88,5).max(1))
    revs={r.age:np.array(r.levels).reshape(88,5).max(1) for r in fr.revisions}
    R2.append(revs.get(2,np.zeros(88)))
    R8.append(np.array(fr.revised_levels).reshape(88,5).max(1) if fr.revised_levels else np.zeros(88))
    VOL.append(np.array(fr.volume_db))
np.savez(f'{name}_et.npz',t=np.array(T),h0=np.array(H0),r2=np.array(R2),r8=np.array(R8),hop=hop,vol=np.array(VOL))
print(name,len(frames),'frames hop',hop,'lat',chain.latency_samples(48000), 'vol shape',np.array(VOL).shape)
