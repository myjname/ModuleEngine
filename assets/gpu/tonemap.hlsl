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
[[vk::binding(0,0)]] Texture2D<float4> color;
[[vk::binding(1,0),vk::image_format("rgba32f")]] RWTexture2D<float4> outputColor;
[numthreads(8,8,1)] void main(uint3 tid:SV_DispatchThreadID) {
    uint w,h;outputColor.GetDimensions(w,h);if(tid.x>=w||tid.y>=h)return;
    int2 p=int2(tid.xy);float4 c=color.Load(int3(p,0));float3 x=max(c.rgb,0)*max(values[0].x,0);
    float3 mapped=values[0].y>0.5?saturate((x*(2.51*x+0.03))/(x*(2.43*x+0.59)+0.14)):x/(1.0+x);
    outputColor[p]=float4(mapped,c.a);
}

