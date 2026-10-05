// Walls: the facade pictures, tinted by the building's own colour and lit.
// aAo reads the bytes from offset 4: z is what the point sees of the sky, w how bright the building's rooms
// are. aLate reads the normal's four bytes unsigned: w is how late in the dusk the rooms come on.
uniform highp sampler2D uShadow;
in vec4 aPosition;
in vec4 aAo;
in vec4 aNormal;
in vec4 aLate;
in vec4 aColor;
in vec2 aUv;
out highp vec2 vUv;
// x: how much of the ambient colour the point takes; y: of the sun's; z: of the haze; w, while shadows are
// read: how far above the shadow's height it stands, as the fragment stage clamps it.
#ifdef SHADOWS
out mediump vec4 vLight;
#else
out mediump vec3 vLight;
#endif
// The building's own: its colour and, once the lights are on, how bright its rooms are at this hour. The
// same at every vertex of a wall (`flat` buys nothing on this GPU: 15.67 ms a frame with it, 15.44 without).
#ifdef LAMPS
out mediump vec4 vTint;
#else
out mediump vec3 vTint;
#endif

void main()
{
    gl_Position = uMvp * vec4(aPosition.xyz, 1.0);
    vUv = vec2(aUv.x, aUv.y * FACADE_V);
    vec3 light = vec3((1.0 + uFrame[3].w * aNormal.y) * aAo.z, max(dot(aNormal.xyz, uFrame[0].xyz), 0.0), haze_of(gl_Position.w));
#ifdef SHADOWS
    // The shadow's height is read here, a little way out from the wall (under the wall lies the building's
    // own roof), and what travels is the point's height over it: a shadow's edge still crosses a wall
    // at its own height, pixel by pixel.
    vec2 map = (aPosition.xz + aNormal.xz * WALL_OUT) * uMap.xy + uMap.zw;
    float over = aPosition.y - textureLod(uShadow, map, 0.0).r;
    vLight = vec4(light, over * SHADOW_K + 1.0);
#else
    vLight = light;
#endif
    // A wall texel takes the building's colour and a window keeps its own: the picture's alpha says which,
    // and the fragment program multiplies by 1 + vTint * alpha. Each building's rooms come on at their own
    // moment of the dusk.
#ifdef LAMPS
    vTint = vec4(aColor.rgb * WALL_GAIN - 1.0, clamp((uFrame[1].w - aLate.w * 0.6) * 4.0, 0.0, 1.0) * aAo.w * 2.0);
#else
    vTint = aColor.rgb * WALL_GAIN - 1.0;
#endif
}
