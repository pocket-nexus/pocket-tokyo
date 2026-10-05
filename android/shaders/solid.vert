// Painted geometry: structures, street furniture, trees, a landmark. Lit, shadowed and hazed here, vertex by
// vertex: a colour is all that crosses a triangle.
uniform highp sampler2D uShadow;
in vec4 aPosition;
in vec4 aAo;
in vec4 aNormal;
in vec4 aColor;
out mediump vec3 vColor;

void main()
{
    gl_Position = uMvp * vec4(aPosition.xyz, 1.0);
    float over = aPosition.y - textureLod(uShadow, aPosition.xz * uMap.xy + uMap.zw, 0.0).r;
    float lit = clamp(over * SHADOW_K + 1.0, 0.0, 1.0);
    // (alpha 1 marks a lamp: it shines by its own colour once the lights are on)
    vec3 ambient = (uFrame[1].rgb + uFrame[2].rgb * aNormal.y) * aAo.z + aColor.a * uFrame[1].w * 1.6;
    // Thin members are seen from both sides: light them by the side that faces the light.
    vec3 sun = uFrame[4].rgb * ((abs(dot(aNormal.xyz, uFrame[0].xyz)) * 0.8 + 0.2) * lit);
    vColor = mix(aColor.rgb * (ambient + sun), uFrame[5].rgb, haze_of(gl_Position.w));
}
