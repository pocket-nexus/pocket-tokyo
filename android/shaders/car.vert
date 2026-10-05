// Cars: boxes built each frame in the world's own coordinates. uMap takes world x, z to the height grid.
// Lit, shadowed and hazed vertex by vertex, as what is painted is.
uniform highp sampler2D uShadow;
in vec4 aPosition;
in vec4 aNormal;
in vec4 aColor;
out mediump vec3 vColor;

void main()
{
    gl_Position = uMvp * vec4(aPosition.xyz, 1.0);
    float over = (aPosition.y - Y0) / Y_SPAN - textureLod(uShadow, aPosition.xz * uMap.xy + uMap.zw, 0.0).r;
    float lit = clamp(over * SHADOW_K + 1.0, 0.0, 1.0);
    // (alpha 1 marks a lamp: it shines by its own colour once the lights are on)
    vec3 ambient = uFrame[1].rgb + uFrame[2].rgb * aNormal.y + aColor.a * uFrame[1].w * 2.0;
    vec3 sun = uFrame[4].rgb * (max(dot(aNormal.xyz, uFrame[0].xyz), 0.0) * lit);
    vColor = mix(aColor.rgb * (ambient + sun), uFrame[5].rgb, haze_of(gl_Position.w));
}
