// The ground and every roof: a picture from above, lit.
// A vertex is 12 bytes and every attribute starts on a 4-byte boundary: aAo reads the last two bytes of the
// position's z before what the point sees of the sky (z).
in vec4 aPosition;
in vec4 aAo;
in vec4 aNormal;
// xy: where the point is in its place's picture; z: its height, while shadows are read. The fragment stage
// finds the height grid from xy itself (uMap is its uniform here), so no second pair travels.
#ifdef SHADOWS
out highp vec3 vAt;
#else
out highp vec2 vAt;
#endif
// x: how much of the ambient colour the point takes; y: of the sun's; z: of the haze.
out mediump vec3 vLight;

void main()
{
    gl_Position = uMvp * vec4(aPosition.xyz, 1.0);
#ifdef SHADOWS
    vAt = aPosition.xzy;
#else
    vAt = aPosition.xz;
#endif
    vLight = vec3((uFrame[2].w + uFrame[3].z * aNormal.y) * aAo.z, max(dot(aNormal.xyz, uFrame[0].xyz), 0.0), haze_of(gl_Position.w));
}
