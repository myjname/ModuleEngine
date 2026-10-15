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
};
struct SceneVarying {
    float4 position : SV_Position;
    [[vk::location(0)]] float3 normal : TEXCOORD0;
    [[vk::location(1)]] float4 color : COLOR0;
    [[vk::location(2)]] float depth : TEXCOORD1;
    [[vk::location(3)]] nointerpolation float2 ids : TEXCOORD2;
};
SceneVarying mainVS(Vertex v) {
    float3 local = v.position - eyeNear.xyz;
    float3 p = float3(dot(local,rightAspect.xyz),dot(local,upFar.xyz),dot(local,forwardFocal.xyz));
    float nearZ=eyeNear.w, farZ=upFar.w;
    SceneVarying o;
    o.position=float4(p.x*forwardFocal.w/rightAspect.w,-p.y*forwardFocal.w,
                     p.z*farZ/(farZ-nearZ)-farZ*nearZ/(farZ-nearZ),p.z);
    o.normal=v.normal; o.color=v.color; o.depth=p.z; o.ids=v.ids; return o;
}
struct Targets {float4 normal:SV_Target0;float depth:SV_Target1;};
Targets main(SceneVarying v) {Targets o;o.normal=float4(normalize(v.normal),v.ids.y);o.depth=v.depth;return o;}

