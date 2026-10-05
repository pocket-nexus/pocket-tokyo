// The sky: a dome of a few hundred triangles around the eye, coloured at its vertices.
// uSky: [0] the haze at the horizon, [1] the zenith, [2] towards the sun, [3] the glow around the sun.
uniform mat4 uMvp;
uniform vec4 uSky[4];
in vec3 aPosition;
out mediump vec3 vColor;

void main()
{
    vec4 p = uMvp * vec4(aPosition, 1.0);
    // At the far plane, whatever the dome's radius.
    gl_Position = vec4(p.xy, p.w, p.w);
    float up = clamp(aPosition.y, 0.0, 1.0);
    float s = clamp(dot(aPosition, uSky[2].xyz), 0.0, 1.0);
    float s2 = s * s;
    float s8 = s2 * s2 * s2 * s2;
    float s64 = s8 * s8 * s8 * s8 * s8 * s8 * s8 * s8;
    // Below the horizon lies the land beyond the city, under the same haze.
    float down = clamp(-aPosition.y * 3.0, 0.0, 1.0);
    vColor = mix(uSky[0].rgb, uSky[1].rgb, sqrt(up)) * (1.0 - 0.3 * down) + uSky[3].rgb * (s8 * 0.18 + s64 * 0.35) * (1.0 - down);
}
