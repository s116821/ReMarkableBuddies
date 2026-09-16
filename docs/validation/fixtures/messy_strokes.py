"""Deterministic synthetic cursive fixtures; not a person's handwriting sample."""
import json, math, random
from pathlib import Path

# Lowercase monoline cursive centerlines, ascenders above y=0 and descenders below 8.
glyphs = {
 'w': '0,7 2,1 1,7 3,8 5,2 4,7 6,8 8,2 7,6 10,5',
 'h': '0,7 3,-5 5,-7 6,-5 4,0 1,7 4,2 6,1 7,3 6,7 9,5',
 'y': '0,5 2,1 1,6 3,8 6,1 3,12 1,15 -1,14 1,10 8,5',
 'r': '0,7 3,1 4,4 6,2 7,3 6,7 9,5',
 'o': '0,6 2,2 5,1 6,4 5,7 2,8 1,6 2,2 5,1 7,4 9,3',
 't': '0,7 5,-5 2,6 3,8 7,5|0,0 7,-1',
 'a': '0,6 2,2 5,1 6,3 4,7 2,8 1,6 2,2 6,1 5,7 8,5',
 'u': '0,5 2,1 1,6 2,8 4,7 7,1 5,7 8,5',
 'n': '0,7 3,1 1,7 4,2 6,1 7,3 6,7 9,5',
 'c': '0,6 2,2 5,1 6,2 4,2 2,3 1,6 3,8 7,5',
 'e': '0,6 5,3 5,1 3,1 1,4 1,7 3,8 7,5',
 'G': '10,-3 8,-6 4,-5 1,-1 0,4 2,8 6,8 8,5 8,2 5,3 10,2',
 'f': '0,7 3,0 5,-6 7,-7 8,-5 6,-2 3,1 2,9 3,13 5,12 5,9 2,3 7,2|0,2 8,1',
 'l': '0,7 5,-6 7,-7 8,-4 5,0 2,5 3,8 8,5',
 'p': '0,5 3,1 1,14 2,7 4,2 6,1 8,3 7,6 4,8 2,7 10,5',
 '.': '1,8 1.5,8.5', '?': '0,-2 2,-5 6,-5 8,-2 7,1 4,4 4,5|4,8 4.5,8.5',
 '+': '0,3 8,3|4,-1 4,7', '/': '0,9 6,-5', '-': '0,3 7,3',
}

def fixture(text, seed, scale=2.0):
    rng=random.Random(seed)
    strokes=[]
    x=140.0
    previous=None
    for ch in text:
        if ch==' ':
            x+=rng.uniform(12,23); previous=None; continue
        slant=rng.uniform(.12,.45)
        baseline=57+rng.uniform(-2.5,2.5)
        width=rng.uniform(.85,1.15)
        for i, part in enumerate(glyphs[ch].split('|')):
            points=[]
            for p in part.split():
                u,v=map(float,p.split(','))
                points.append([round(x+scale*(u*width-slant*v)),round(baseline+scale*v)])
            if i==0 and previous and ch.islower():
                points.insert(0,previous)
            strokes.append(points)
            if i==0: previous=points[-1]
        x+=scale*(8 if ch.isalpha() else 5)*width+rng.uniform(-2,2)
    return strokes

Path('messy-rotation.json').write_text(json.dumps(fixture('why rot. attr.?',41)))
Path('messy-g.json').write_text(json.dumps(fixture('G unc.?',72,2.5)))
# Fresh held-out topic, not used to select the model or tune the first fixtures.
flat=fixture('why flat plate?',103,2.0)
Path('messy-flat-plate.json').write_text(json.dumps([[[x,y-8] for x,y in stroke] for stroke in flat]))
rotate=fixture('why rotate?',151,2.2)
Path('messy-rotate-full.json').write_text(json.dumps([[[x,y-8] for x,y in stroke] for stroke in rotate]))
# Deliberately illegible loops: there is no intended recoverable question.
rng=random.Random(91)
loops=[]
for j in range(3):
    loops.append([[round(190+i*3+12*math.sin(i*1.9+j)),round(60+12*math.sin(i*1.3+j))] for i in range(65)])
Path('illegible-question.json').write_text(json.dumps(loops))
