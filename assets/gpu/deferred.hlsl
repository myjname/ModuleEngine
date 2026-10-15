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
[[vk::binding(0,0)]] Texture2D<float4> albedo;
[[vk::binding(1,0)]] Texture2D<float4> normals;
[[vk::binding(2,0)]] Texture2D<float> depth;
[[vk::binding(3,0),vk::image_format("rgba32f")]] RWTexture2D<float4> outputColor;
[numthreads(8,8,1)] void main(uint3 tid:SV_DispatchThreadID) {
    uint w,h;outputColor.GetDimensions(w,h);if(tid.x>=w||tid.y>=h)return;
    int2 p=int2(tid.xy);float4 a=albedo.Load(int3(p,0));float3 n=normals.Load(int3(p,0)).xyz;
    float d=depth.Load(int3(p,0));float diffuse=0.20+1.65*abs(dot(n,normalize(float3(-0.5,1.0,0.7))));
    float specular=0.18*pow(saturate(abs(n.z)),12.0);
    outputColor[p]=d>0?float4(a.rgb*diffuse+specular,a.a):a;
}

