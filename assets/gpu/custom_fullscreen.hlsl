// Independent fullscreen graphics Custom. Texture.Load keeps sampler policy
// explicit; 0.2 manifests expose images and push scalars, not sampler objects.
struct Config {float4 camera[4];float4 values[4];};
[[vk::push_constant]] ConstantBuffer<Config> config;
[[vk::binding(0,0)]] Texture2D<float4> color;
float4 main(float4 position:SV_Position):SV_Target0 {
    float4 c=color.Load(int3(int2(position.xy),0));
    return float4(c.rgb*config.values[0].x,c.a);
}
