#pragma once

// Adapted from electron-liquid-glass's renderer.cc (hicccc77, MIT).
// See LICENSE.electron-liquid-glass in this directory.
inline constexpr char kGlassShader[] = R"hlsl(
cbuffer Glass : register(b0) {
    float2 sourceOrigin; float2 sourceSpan;
    float2 outSize; float2 direction;
    float2 glassSize; float bezel; float maxBend;
    float margin; float rotation; float saturation; float aberration;
    float4 radii;
    float4 clipRect;
};

Texture2D sourceTexture : register(t0);
SamplerState linearClamp : register(s0);

float4 VSMain(uint id : SV_VertexID) : SV_Position {
    float2 position = float2((id << 1) & 2, id & 2);
    return float4(position * float2(2, -2) + float2(-1, 1), 0, 1);
}

float2 rotateUV(float2 uv) {
    if (rotation == 2.0) return float2(uv.y, 1.0 - uv.x);
    if (rotation == 3.0) return 1.0 - uv;
    if (rotation == 4.0) return float2(1.0 - uv.y, uv.x);
    return uv;
}

float4 sourceAt(float2 uv) {
    return sourceTexture.Sample(linearClamp, rotateUV(uv));
}

float4 PSBlur(float4 position : SV_Position) : SV_Target {
    float2 uv = sourceOrigin + (position.xy / outSize) * sourceSpan;
    return sourceAt(uv) * 0.4026
        + (sourceAt(uv + direction) + sourceAt(uv - direction)) * 0.2442
        + (sourceAt(uv + direction * 2.0) + sourceAt(uv - direction * 2.0)) * 0.0545;
}

float2 blurUV(float2 pixel) {
    return (pixel + margin) / (glassSize + 2.0 * margin);
}

float4 PSLens(float4 position : SV_Position) : SV_Target {
    float2 pixel = position.xy;
    if (any(pixel < clipRect.xy) || any(pixel > clipRect.zw)) return 0.0;
    float2 desktopUV = sourceOrigin + blurUV(pixel) * sourceSpan;
    if (any(desktopUV < 0.0) || any(desktopUV > 1.0)) return 0.0;

    float2 halfSize = glassSize * 0.5;
    float2 p = pixel - halfSize;
    uint corner = p.y < 0.0 ? (p.x < 0.0 ? 0 : 1) : (p.x < 0.0 ? 3 : 2);
    float radius = min(radii[corner], min(halfSize.x, halfSize.y));
    float2 q = abs(p) - halfSize + radius;
    float distance = min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - radius;
    float coverage = saturate(0.5 - distance);
    if (coverage <= 0.0) return 0.0;

    float depth = max(0.0, -distance);
    float2 normal = 0.0;
    float2 displacement = 0.0;
    float bump = 0.0;
    float rim = 0.0;
    if (depth < bezel) {
        float softness = max(radius * 0.8, 1.0);
        float2 rounded = 0.5 * (q + sqrt(q * q + softness * softness));
        normal = rounded * sign(p + 1e-6) / max(length(rounded), 1e-4);
        float t = depth / bezel;
        rim = (1.0 - t) * (1.0 - t);
        // The sample displacement reaches zero at both ends of the rim.
        float u = saturate(t < 0.62 ? t / 0.62 : (1.0 - t) / 0.38);
        bump = u * u * (3.0 - 2.0 * u);
        displacement = normal * (bump * maxBend);
    }

    float3 color;
    float displacementLength = length(displacement);
    if (displacementLength > 0.5) {
        // Integrate across the narrow compressed part of the sample field.
        float2 tangent = float2(normal.y, -normal.x) * displacementLength;
        float spread = 0.05 + 0.10 * bump;
        float radial[5] = { 1.0 - 2.0 * spread, 1.0 - spread, 1.0, 1.0 + spread, 1.0 + 2.0 * spread };
        const float lateral[5] = { -0.36, 0.18, 0.0, -0.18, 0.36 };
        const float weights[5] = { 0.14, 0.22, 0.28, 0.22, 0.14 };
        color = 0.0;
        [unroll] for (int i = 0; i < 5; ++i) {
            float2 d = displacement * radial[i] + tangent * lateral[i];
            color += weights[i] * float3(
                sourceTexture.Sample(linearClamp, blurUV(pixel + d)).r,
                sourceTexture.Sample(linearClamp, blurUV(pixel + d * (1.0 - aberration * 0.05))).g,
                sourceTexture.Sample(linearClamp, blurUV(pixel + d * (1.0 - aberration * 0.10))).b);
        }
    } else {
        color = sourceTexture.Sample(linearClamp, blurUV(pixel + displacement)).rgb;
    }

    float luma = dot(color, float3(0.2126, 0.7152, 0.0722));
    color = lerp(luma.xxx, color, saturation);
    float light = dot(normal, float2(-0.40, -0.92));
    color = saturate(color + rim * (0.065 * saturate(light) - 0.03 * saturate(-light)));
    return float4(color * coverage, coverage);
}
)hlsl";
