# Convert EffeTune DSP harmnet_weights.h (MIT, github.com/frieve-a/effetune,
# dsp/plugins/analyzer/note_spectrogram/) into the embedded little-endian f32 weight asset.
import struct, sys
s = open(sys.argv[1], encoding='utf-8').read()
body = s[s.index('kWeights = {') + 12:]
body = body[:body.index('};')]
vals = [float.fromhex(v.strip().rstrip('F')) for v in body.split(',') if v.strip()]
open(sys.argv[2], 'wb').write(struct.pack('<%df' % len(vals), *vals))
print(len(vals), 'weights')
