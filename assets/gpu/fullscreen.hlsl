struct FullscreenVarying { float4 position:SV_Position; };
FullscreenVarying mainVS(uint vertex:SV_VertexID) {
    FullscreenVarying o;float2 p=float2((vertex<<1)&2,vertex&2);
    o.position=float4(p*2.0-1.0,0.0,1.0);return o;
}
