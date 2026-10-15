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
// Editable independent compute pass: inputs binding 0, output binding 1.
[[vk::binding(0,0)]] Texture2D<float4> color;
[[vk::binding(1,0),vk::image_format("rgba32f")]] RWTexture2D<float4> outputColor;
[numthreads(8,8,1)] void main(uint3 tid:SV_DispatchThreadID) {
    uint w,h;outputColor.GetDimensions(w,h);if(tid.x>=w||tid.y>=h)return;
    float4 c=color.Load(int3(tid.xy,0));outputColor[tid.xy]=float4(c.rgb*values[0].x,c.a);
}

