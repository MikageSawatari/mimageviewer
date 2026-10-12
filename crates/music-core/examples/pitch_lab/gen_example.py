import re, sys
src = open(r'G:/home/mimageviewer-pitchlab/src/ui_music_spectrum.rs', encoding='utf-8').read()
lines = src.splitlines()
def block(start_pat):
    i = next(k for k,l in enumerate(lines) if re.match(start_pat, l))
    # include preceding doc comments
    j = i
    while j>0 and lines[j-1].lstrip().startswith('///'): j -= 1
    depth=0; out=[]
    for k in range(j, len(lines)):
        out.append(lines[k]); depth += lines[k].count('{') - lines[k].count('}')
        if k>=i and depth==0 and ('}' in lines[k] or lines[k].rstrip().endswith(';')): break
    return '\n'.join(out)
consts = [l for l in lines if re.match(r'^const (KEY_|EVEN_|ODD_)', l)]
fns = ['fn update_keyboard_sustain','struct KeyboardVisuals','fn compute_keyboard_visuals','fn kth_largest','fn local_declutter_strength','fn smoothstep','fn harmonic_energies','fn midi_to_hz']
body = '\n'.join(consts) + '\n\n' + '\n\n'.join(block('^'+f) for f in fns)
tmpl = open('example_template.rs', encoding='utf-8').read()
open(r'G:/home/mimageviewer-pitchlab/crates/music-core/examples/keyboard_dump.rs','w',encoding='utf-8').write(tmpl.replace('//@@EXTRACTED@@', body))
print('ok', len(consts), 'consts')
