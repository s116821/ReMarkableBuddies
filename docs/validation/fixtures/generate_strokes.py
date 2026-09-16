import json, math
from pathlib import Path
# Small monoline test alphabet. Coordinates are normalized tablet pixels.
glyphs = {
'A':'0,7 2,0 4,7|1,4 3,4','C':'4,1 3,0 1,0 0,1 0,6 1,7 3,7 4,6',
'D':'0,7 0,0 2,0 4,2 4,5 2,7 0,7','E':'4,0 0,0 0,7 4,7|0,3 3,3',
'G':'4,1 3,0 1,0 0,1 0,6 1,7 4,7 4,4 2,4','H':'0,0 0,7|4,0 4,7|0,3 4,3',
'I':'0,0 4,0|2,0 2,7|0,7 4,7','M':'0,7 0,0 2,3 4,0 4,7','N':'0,7 0,0 4,7 4,0',
'O':'1,0 3,0 4,1 4,6 3,7 1,7 0,6 0,1 1,0','R':'0,7 0,0 3,0 4,1 4,2 3,3 0,3|2,3 4,7',
'S':'4,1 3,0 1,0 0,1 0,2 1,3 3,4 4,5 4,6 3,7 1,7 0,6','T':'0,0 4,0|2,0 2,7',
'U':'0,0 0,6 1,7 3,7 4,6 4,0','V':'0,0 2,7 4,0','W':'0,0 1,7 2,4 3,7 4,0',
'Y':'0,0 2,3 4,0|2,3 2,7','?':'0,1 1,0 3,0 4,1 4,2 2,4 2,5|2,6.5 2,7',
'F':'4,0 0,0 0,7|0,3 3,3','L':'0,0 0,7 4,7','B':'0,7 0,0 3,0 4,1 4,2 3,3 0,3|3,3 4,4 4,6 3,7 0,7',
'P':'0,7 0,0 3,0 4,1 4,2 3,3 0,3','X':'0,0 4,7|4,0 0,7'
}
def question(text):
    paths=[]
    x=(768-len(text)*18)//2
    for ch in text:
        if ch!=' ':
            for stroke in glyphs[ch].split('|'):
                paths.append([[round(x+float(p.split(',')[0])*3), round(47+float(p.split(',')[1])*3)] for p in stroke.split()])
        x+=18
    return paths
ellipse=[[round(385+274*math.cos(i*2*math.pi/100)),round(222+70*math.sin(i*2*math.pi/100))] for i in range(101)]
for name,text in [('g-question','WHAT G AND UNCERTAINTY?'),('rotation-question','WHY ROTATE ATTRACTORS?')]:
    Path(name+'.json').write_text(json.dumps(question(text)))
Path('abstract-circle.json').write_text(json.dumps([ellipse]))
