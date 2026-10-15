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
[[vk::binding(1,0),vk::image_format("rgba8")]] RWTexture2D<float4> frame;
[numthreads(8,8,1)] void main(uint3 tid:SV_DispatchThreadID) {
    uint w,h;frame.GetDimensions(w,h);if(tid.x>=w||tid.y>=h)return;
    float3 x=saturate(color.Load(int3(tid.xy,0)).rgb);
    float3 encoded=float3(x.x<=0.0031308?12.92*x.x:1.055*pow(x.x,1.0/2.4)-0.055,
                          x.y<=0.0031308?12.92*x.y:1.055*pow(x.y,1.0/2.4)-0.055,
                          x.z<=0.0031308?12.92*x.z:1.055*pow(x.z,1.0/2.4)-0.055);
    frame[tid.xy]=float4(encoded,1.0);
}

