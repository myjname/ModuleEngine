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
[[vk::binding(1,0)]] Texture2D<float4> normals;
[[vk::binding(2,0)]] Texture2D<float> depth;
[[vk::binding(3,0),vk::image_format("rgba32f")]] RWTexture2D<float4> outputColor;
[numthreads(8,8,1)] void main(uint3 tid:SV_DispatchThreadID) {
    uint w,h;outputColor.GetDimensions(w,h);if(tid.x>=w||tid.y>=h)return;
    int2 p=int2(tid.xy);float4 c=color.Load(int3(p,0));float d=depth.Load(int3(p,0));
    if(d<=0){outputColor[p]=c;return;}
    float3 n=normals.Load(int3(p,0)).xyz;float occluded=0;
    int2 offsets[8]={int2(3,0),int2(-3,0),int2(0,3),int2(0,-3),int2(5,5),int2(-5,5),int2(5,-5),int2(-5,-5)};
    for(int i=0;i<8;++i){int2 q=clamp(p+offsets[i],int2(0,0),int2(w-1,h-1));
        float other=depth.Load(int3(q,0));float delta=d-other;
        float3 otherNormal=normals.Load(int3(q,0)).xyz;
        occluded+=(other>0&&delta>0.025&&delta<1.4)?saturate(1.0-delta/1.4)*(0.5+0.5*abs(dot(n,otherNormal))):0;
    }
    float ao=1.0-saturate(values[0].x)*occluded/8.0;
    outputColor[p]=float4(c.rgb*ao,c.a);
}

