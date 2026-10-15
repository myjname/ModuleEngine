"""Generate the authored importer fixture; no downloaded/copyrighted game assets."""
import json,struct,zlib,binascii,pathlib
def chunk(tag,data):return struct.pack('>I',len(data))+tag+data+struct.pack('>I',binascii.crc32(tag+data)&0xffffffff)
pixels=b''.join(b'\x00'+b''.join(bytes((40,140,245,255) if (x+y)%2 else (235,240,255,255)) for x in range(8)) for y in range(8))
png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',8,8,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
points=[(0,1.8,0),(0,-1,0),(-0.9,0,0),(0.9,0,0),(0,0,-0.9),(0,0,0.9)]
faces=[(0,3,5),(0,5,2),(0,2,4),(0,4,3),(1,5,3),(1,2,5),(1,4,2),(1,3,4)]
positions=[points[i] for face in faces for i in face]
uvs=[p for _ in faces for p in ((0.5,0),(1,1),(0,1))]
binary=b'';views=[]
for data in [b''.join(struct.pack('<3f',*p) for p in positions),b''.join(struct.pack('<2f',*p) for p in uvs),b''.join(struct.pack('<H',i) for i in range(len(positions))),png]:
 binary+=b'\0'*((-len(binary))%4);views.append({'buffer':0,'byteOffset':len(binary),'byteLength':len(data)});binary+=data
doc={'asset':{'version':'2.0','generator':'ModuleEngine authored test fixture'},'scene':0,'scenes':[{'nodes':[0]}],'nodes':[{'mesh':0,'name':'TexturedCrystal'}],'buffers':[{'byteLength':len(binary)}],'bufferViews':views,'accessors':[{'bufferView':0,'componentType':5126,'count':24,'type':'VEC3','min':[-.9,-1,-.9],'max':[.9,1.8,.9]},{'bufferView':1,'componentType':5126,'count':24,'type':'VEC2'},{'bufferView':2,'componentType':5123,'count':24,'type':'SCALAR'}],'meshes':[{'primitives':[{'attributes':{'POSITION':0,'TEXCOORD_0':1},'indices':2,'material':0}]}],'materials':[{'name':'BlueChecker','pbrMetallicRoughness':{'baseColorFactor':[1,1,1,1],'baseColorTexture':{'index':0},'metallicFactor':0,'roughnessFactor':1}}],'textures':[{'source':0}],'images':[{'bufferView':3,'mimeType':'image/png'}]}
jsondata=json.dumps(doc,separators=(',',':')).encode();jsondata+=b' '*((-len(jsondata))%4);binary+=b'\0'*((-len(binary))%4)
glb=struct.pack('<III',0x46546c67,2,12+8+len(jsondata)+8+len(binary))+struct.pack('<II',len(jsondata),0x4e4f534a)+jsondata+struct.pack('<II',len(binary),0x004e4942)+binary
root=pathlib.Path(__file__).resolve().parents[1];dest=root/'assets/models/demo_crystal.glb';dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(glb);print(dest,len(glb))
