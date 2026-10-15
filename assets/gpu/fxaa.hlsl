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
float4 fetch(int2 p,uint2 size){return color.Load(int3(clamp(p,int2(0,0),int2(size)-1),0));}
float3 sampleLinear(float2 p,uint2 size){int2 i=int2(floor(p));float2 f=frac(p);return lerp(lerp(fetch(i,size).rgb,fetch(i+int2(1,0),size).rgb,f.x),lerp(fetch(i+int2(0,1),size).rgb,fetch(i+1,size).rgb,f.x),f.y);}
float luma(float3 c){return dot(c,float3(0.299,0.587,0.114));}
[numthreads(8,8,1)] void main(uint3 tid:SV_DispatchThreadID) {
    uint w,h;outputColor.GetDimensions(w,h);if(tid.x>=w||tid.y>=h)return;
    uint2 size=uint2(w,h);int2 p=int2(tid.xy);float4 center=fetch(p,size);
    float nw=luma(fetch(p+int2(-1,-1),size).rgb),ne=luma(fetch(p+int2(1,-1),size).rgb);
    float sw=luma(fetch(p+int2(-1,1),size).rgb),se=luma(fetch(p+int2(1,1),size).rgb),m=luma(center.rgb);
    float low=min(m,min(min(nw,ne),min(sw,se))),high=max(m,max(max(nw,ne),max(sw,se)));
    if(high-low<max(0.0833,high*0.166)){outputColor[p]=center;return;}
    float2 dir=float2(-((nw+ne)-(sw+se)),(nw+sw)-(ne+se));
    float reduce=max((nw+ne+sw+se)*0.03125,0.0078125);
    dir=clamp(dir/(min(abs(dir.x),abs(dir.y))+reduce),-8.0,8.0);
    float3 a=0.5*(sampleLinear(float2(p)+dir*(-1.0/6.0),size)+sampleLinear(float2(p)+dir*(1.0/6.0),size));
    float3 b=0.5*a+0.25*(sampleLinear(float2(p)+dir*(-0.5),size)+sampleLinear(float2(p)+dir*0.5,size));
    float lb=luma(b);outputColor[p]=float4(lb<low||lb>high?a:b,center.a);
}

