"""Original deterministic Crate Garden art and music; Python standard library only.

Run from any directory. Outputs belong to this example's assets folder. Existing
different bytes are rejected unless --replace is explicitly supplied. No external
models, recordings, copyrighted melodies or third-party packages are used.
"""
from __future__ import annotations
import argparse
import base64
import hashlib
import io
import json
import math
from pathlib import Path
import struct
import wave
import zlib

ROOT = Path(__file__).resolve().parent
PALETTE = [
    (235, 226, 198), (255, 246, 222), (174, 155, 126), (128, 78, 44),
    (204, 148, 78), (231, 185, 106), (54, 78, 85), (53, 166, 153),
    (31, 82, 87), (247, 203, 151), (78, 193, 139), (181, 204, 186),
    (59, 127, 75), (184, 111, 78), (119, 147, 133), (246, 215, 112),
]

def png_palette() -> bytes:
    width, height = 128, 8
    raw = b"".join(b"\x00" + b"".join(bytes((*PALETTE[x // 8], 255)) for x in range(width)) for _ in range(height))
    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")

class Mesh:
    def __init__(self):
        self.positions, self.uv, self.indices = [], [], []
    def face(self, points, color):
        start = len(self.positions)
        self.positions.extend(points)
        self.uv.extend([((color + .5) / len(PALETTE), .5)] * len(points))
        for index in range(1, len(points) - 1):
            self.indices.extend([start, start + index, start + index + 1])
    def box(self, size, center=(0., 0., 0.), color=0, bevel=.025, top=None, angle=0.):
        hx, hy, hz = (v / 2 for v in size)
        b = min(bevel, hx * .45, hy * .45, hz * .45)
        def ring(y, inset):
            x, z = hx - inset, hz - inset
            c = max(.001, min(b, x * .3, z * .3))
            return [(-x+c,y,-z),(x-c,y,-z),(x,y,-z+c),(x,y,z-c),(x-c,y,z),(-x+c,y,z),(-x,y,z-c),(-x,y,-z+c)]
        def transform(p):
            x, y, z = p
            return (center[0]+x*math.cos(angle)-y*math.sin(angle), center[1]+x*math.sin(angle)+y*math.cos(angle), center[2]+z)
        rings = [ring(-hy,b),ring(-hy+b,0),ring(hy-b,0),ring(hy,b)]
        for layer in range(3):
            for i in range(8):
                j=(i+1)%8
                self.face([transform(rings[layer][i]),transform(rings[layer+1][i]),transform(rings[layer+1][j]),transform(rings[layer][j])], color if layer<2 else (top if top is not None else color))
        self.face([transform(p) for p in reversed(rings[-1])], top if top is not None else color)
        self.face([transform(p) for p in rings[0]], color)
    def ring(self, outer=.35, inner=.24, height=.04, color=10):
        for i in range(24):
            a,b=2*math.pi*i/24,2*math.pi*(i+1)/24
            p=lambda r,y,t:(r*math.cos(t),y,r*math.sin(t))
            self.face([p(inner,height/2,a),p(inner,height/2,b),p(outer,height/2,b),p(outer,height/2,a)],color)
            self.face([p(outer,-height/2,a),p(outer,height/2,a),p(outer,height/2,b),p(outer,-height/2,b)],color)
            self.face([p(inner,-height/2,b),p(inner,height/2,b),p(inner,height/2,a),p(inner,-height/2,a)],color)
    def gltf(self, name):
        positions=struct.pack("<"+"f"*(3*len(self.positions)),*(v for p in self.positions for v in p))
        uv=struct.pack("<"+"f"*(2*len(self.uv)),*(v for p in self.uv for v in p))
        indices=struct.pack("<"+"H"*len(self.indices),*self.indices)
        data=positions+uv+indices
        lo=[min(p[a] for p in self.positions) for a in range(3)]
        hi=[max(p[a] for p in self.positions) for a in range(3)]
        model={"asset":{"version":"2.0","generator":"Crate Garden original deterministic art"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"name":name,"mesh":0}],
            "buffers":[{"byteLength":len(data),"uri":"data:application/octet-stream;base64,"+base64.b64encode(data).decode()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":len(positions),"target":34962},{"buffer":0,"byteOffset":len(positions),"byteLength":len(uv),"target":34962},{"buffer":0,"byteOffset":len(positions)+len(uv),"byteLength":len(indices),"target":34963}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":len(self.positions),"type":"VEC3","min":lo,"max":hi},{"bufferView":1,"componentType":5126,"count":len(self.uv),"type":"VEC2"},{"bufferView":2,"componentType":5123,"count":len(self.indices),"type":"SCALAR"}],
            "images":[{"uri":"palette.png"}],"samplers":[{"magFilter":9728,"minFilter":9728,"wrapS":33071,"wrapT":33071}],"textures":[{"sampler":0,"source":0}],
            "materials":[{"name":"Garden palette","pbrMetallicRoughness":{"baseColorFactor":[1,1,1,1],"baseColorTexture":{"index":0},"metallicFactor":0,"roughnessFactor":1},"doubleSided":True}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"TEXCOORD_0":1},"indices":2,"material":0,"mode":4}]}]}
        return (json.dumps(model,separators=(",",":"))+"\n").encode()

def models():
    m=Mesh();m.box((.98,.08,.98),color=2,bevel=.018,top=0);yield "floor",m
    m=Mesh();m.box((.92,.84,.92),center=(0,-.06,0),color=14,bevel=.06,top=11);m.box((.98,.13,.98),center=(0,.415,0),color=11,bevel=.035,top=1);yield "wall",m
    m=Mesh();m.box((.72,.72,.72),color=4,bevel=.045,top=5)
    for y in [-.25,.25]:m.box((.76,.095,.76),center=(0,y,0),color=3,bevel=.015,top=5)
    for angle in [-math.pi/4,math.pi/4]:m.box((.07,.66,.028),center=(0,0,.379),color=3,bevel=.007,angle=angle)
    for x in [-.27,.27]:
        for y in [-.25,.25]:m.box((.035,.035,.016),center=(x,y,.389),color=6,bevel=.006)
    yield "crate",m
    m=Mesh();m.box((.42,.45,.34),center=(0,-.10,0),color=7,bevel=.055,top=10)
    m.box((.40,.28,.38),center=(0,.205,0),color=9,bevel=.045,top=1)
    m.box((.60,.055,.50),center=(0,.305,0),color=8,bevel=.025,top=7)
    m.box((.38,.12,.36),center=(0,.36,0),color=7,bevel=.035,top=10)
    for x in [-.13,.13]:m.box((.17,.12,.25),center=(x,-.36,.045),color=8,bevel=.025)
    for x in [-.085,.085]:m.box((.035,.05,.018),center=(x,.205,.197),color=8,bevel=.006)
    yield "pawn",m
    m=Mesh();m.ring();
    for x,z in [(-.07,0),(.07,0),(0,-.07),(0,.07)]:m.box((.10,.018,.10),center=(x,.018,z),color=15,bevel=.008)
    yield "goal",m
    m=Mesh();m.box((.22,.18,.22),center=(0,-.16,0),color=13,bevel=.04,top=3)
    for x,z,y in [(0,0,.08),(-.09,0,.01),(.09,.04,.04),(0,-.09,.02)]:m.box((.23,.27,.23),center=(x,y,z),color=12,bevel=.065,top=10)
    yield "decor",m

RATE=24000
def audio(name):
    # Music is a separate, lightly plucked major-key phrase. Effects retain
    # their original envelopes and bytes independently of the soundtrack.
    duration=20. if name=="music" else {"move":.075,"push":.15,"goal":.26,"blocked":.09,"undo":.17,"win":.70,"ui":.08}[name]
    samples=[0.]*round(duration*RATE)
    def note(at,length,midi,gain=.12,soft=True):
        start=round(at*RATE);count=round(length*RATE);freq=440*2**((midi-69)/12)
        for i in range(min(count,len(samples)-start)):
            t=i/RATE;env=min(1.,t/.008)*max(0.,1-i/count)**2
            wave_value=math.sin(math.tau*freq*t)
            if not soft:wave_value=.65*wave_value+.20*math.sin(math.tau*freq*2*t)+.15*math.sin(math.tau*freq*3*t)
            samples[start+i]+=wave_value*env*gain
    if name=="music":
        def piano(at,midi,gain,length=1.15):
            start=round(at*RATE);count=round(length*RATE);freq=440*2**((midi-69)/12)
            for i in range(min(count,len(samples)-start)):
                t=i/RATE
                attack=min(1.,t/.025)
                release=min(1.,max(0.,(count-1-i)/(RATE*.16)))
                decay=math.exp(-t/.48)
                tone=(math.sin(math.tau*freq*t)+.16*math.sin(math.tau*freq*2*t)+.025*math.sin(math.tau*freq*3*t))/1.185
                samples[start+i]+=tone*attack*release*decay*gain
        # 96 BPM, eight bars, C/F/C/G then C/F/G/C. No low organ-like pad,
        # minor-key drone, beating detuned layers or sudden chord attacks.
        chords=[(60,64,67),(65,69,72),(60,64,67),(62,67,71),
                (60,64,67),(65,69,72),(62,67,71),(60,64,67)]
        melody=[(72,76),(77,76),(79,76),(74,71),
                (72,76),(77,81),(79,74),(76,72)]
        for bar,chord in enumerate(chords):
            for beat,index in enumerate((0,1,2,1)):
                piano(bar*2.5+beat*.625,chord[index],.045)
            for beat,key in zip((0.,1.25),melody[bar]):
                piano(bar*2.5+beat+.08,key,.075,1.35)
    elif name=="win":
        for i,key in enumerate([72,76,79,84]):note(i*.12,.28,key,.20,False)
    elif name=="goal":
        note(0,.19,79,.14);note(.055,.19,84,.11)
    elif name=="undo":
        note(0,.15,76,.12);note(.045,.12,67,.12)
    elif name=="blocked":note(0,.085,43,.10,False)
    elif name=="push":note(0,.14,48,.13,False);note(.015,.11,55,.05)
    elif name=="ui":note(0,.07,79,.10)
    else:note(0,.065,64,.10,False)
    raw=bytearray()
    for i,value in enumerate(samples):
        # Fade both ends of the music loop; every output remains bounded PCM16.
        if name=="music":value*=min(1.,i/(RATE*.06),(len(samples)-1-i)/(RATE*.06))
        raw.extend(struct.pack("<h",round(max(-.95,min(.95,value))*32767)))
    result=io.BytesIO()
    with wave.open(result,"wb") as wav:
        wav.setnchannels(1);wav.setsampwidth(2);wav.setframerate(RATE);wav.writeframes(raw)
    return result.getvalue()

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--replace",action="store_true");args=parser.parse_args()
    generated={"assets/models/palette.png":png_palette()}
    generated.update({f"assets/models/{name}.gltf":mesh.gltf(name) for name,mesh in models()})
    generated.update({f"assets/audio/{name}.wav":audio(name) for name in ["music","move","push","goal","blocked","undo","win","ui"]})
    for relative,data in generated.items():
        path=ROOT/relative
        if path.exists() and path.read_bytes()!=data and not args.replace:raise SystemExit(f"Refusing to replace different existing asset: {path}")
    for relative,data in generated.items():
        path=ROOT/relative;path.parent.mkdir(parents=True,exist_ok=True)
        if not path.exists() or path.read_bytes()!=data:path.write_bytes(data)
    manifest={name:{"sha256":hashlib.sha256(data).hexdigest(),"bytes":len(data),"origin":"original deterministic Crate Garden generator"} for name,data in generated.items()}
    (ROOT/"asset-origins.json").write_text(json.dumps(manifest,indent=2)+"\n",encoding="utf-8")
    print(json.dumps({"assets":len(generated),"total_bytes":sum(len(data) for data in generated.values()),"root":str(ROOT)}))

if __name__=="__main__":main()
