#include <metal_stdlib>
using namespace metal;

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

struct SourceVertex {
    float2 position;
    float2 uv;
};

struct StrokeVertex {
    float2 position;
    float4 color;
};

vertex VertexOut source_vertex(const device SourceVertex* vertices [[buffer(0)]],
                               uint vertex_id [[vertex_id]]) {
    SourceVertex input = vertices[vertex_id];
    VertexOut output;
    output.position = float4(input.position, 0.0, 1.0);
    output.uv = input.uv;
    output.color = float4(1.0);
    return output;
}

vertex VertexOut stroke_vertex(const device StrokeVertex* vertices [[buffer(0)]],
                               uint vertex_id [[vertex_id]]) {
    StrokeVertex input = vertices[vertex_id];
    VertexOut output;
    output.position = float4(input.position, 0.0, 1.0);
    output.uv = float2(0.0);
    output.color = input.color;
    return output;
}

fragment float4 source_fragment(VertexOut in [[stage_in]],
                                texture2d<float> source [[texture(0)]]) {
    constexpr sampler source_sampler(coord::normalized, address::clamp_to_edge,
                                     filter::linear);
    return source.sample(source_sampler, in.uv);
}

fragment float4 stroke_fragment(VertexOut in [[stage_in]]) {
    return in.color;
}
