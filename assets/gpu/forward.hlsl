// Shared ABI: 128-byte Vulkan push constants, camera in vectors 0..3,
// pass scalar parameters in values[0..3]. No hidden renderer-owned globals.
struct Config {
    float4 eyeNear; float4 rightAspect; float4 upFar; float4 forwardFocal;
    float4 values[4];
};
[[vk::push_constant]] ConstantBuffer<Config> config;
#define eyeNear config.eyeNear
#define rightAspect config.rightAspect
#define upFar config.upFar
#define forwardFocal config.forwardFocal
#define values config.values
struct Vertex {
    [[vk::location(0)]] float3 position : POSITION;
    [[vk::location(1)]] float3 normal : NORMAL;
    [[vk::location(2)]] float4 color : COLOR;
    [[vk::location(3)]] float2 ids : TEXCOORD0;
    [[vk::location(4)]] float2 uv : TEXCOORD1;
    [[vk::location(5)]] float4 atlasRect : TEXCOORD2;
};
struct SceneVarying {
    float4 position : SV_Position;
    [[vk::location(0)]] float3 normal : TEXCOORD0;
    [[vk::location(1)]] float4 color : COLOR0;
    [[vk::location(2)]] float depth : TEXCOORD1;
    [[vk::location(3)]] nointerpolation float2 ids : TEXCOORD2;
    [[vk::location(4)]] float2 uv : TEXCOORD3;
    [[vk::location(5)]] nointerpolation float4 atlasRect : TEXCOORD4;
};
SceneVarying mainVS(Vertex v) {
    float3 local = v.position - eyeNear.xyz;
    float3 p = float3(dot(local,rightAspect.xyz),dot(local,upFar.xyz),dot(local,forwardFocal.xyz));
    float nearZ=eyeNear.w, farZ=upFar.w;
    SceneVarying o;
    o.position=float4(p.x*forwardFocal.w/rightAspect.w,-p.y*forwardFocal.w,
                     p.z*farZ/(farZ-nearZ)-farZ*nearZ/(farZ-nearZ),p.z);
    o.normal=v.normal; o.color=v.color; o.depth=p.z; o.ids=v.ids; o.uv=v.uv; o.atlasRect=v.atlasRect; return o;
}
// Native geometry material contract: UV0, repeat wrapping, nearest base-color.
[[vk::binding(0,0)]] Texture2D<float4> materialAtlas;
float3 baseColor(SceneVarying v) {
    int2 p=int2(v.atlasRect.xy+min(floor(frac(v.uv)*v.atlasRect.zw),v.atlasRect.zw-1));
    float3 s=materialAtlas.Load(int3(p,0)).rgb;
    float3 decoded=lerp(s/12.92,pow((s+0.055)/1.055,2.4),step(0.04045,s));
    return v.color.rgb*decoded;
}
struct Targets {float4 color:SV_Target0;float4 normal:SV_Target1;float depth:SV_Target2;};
Targets main(SceneVarying v) {
    float diffuse=0.20+1.65*abs(dot(normalize(v.normal),normalize(float3(-0.5,1.0,0.7))));
    Targets o; o.color=float4(baseColor(v)*diffuse,v.ids.x);
    o.normal=float4(normalize(v.normal),v.ids.y);o.depth=v.depth;return o;
}

