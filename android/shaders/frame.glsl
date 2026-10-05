// What every scene program's vertex stage takes per frame, as one array (uFrame):
//   [0] xyz  towards the light that casts shadows (the sun, the moon); w: haze per metre
//   [1] rgb  light on a face that looks sideways (the mean of sky and ground light); w: how far the city's
//            lights are on
//   [2] rgb  what a face that looks up has more than that; w: the share of [1] in an upward face's light
//   [3] x    most haze a point can be under; y: seconds; z: the share of [2] in an upward face's light
//   [4] rgb  the sun
//   [5] rgb  the haze
// Per draw: uMvp, which also undoes the normalization of the positions, and uMap, the scale (xy) and offset
// (zw) from a vertex's own x and z to the height grid's texture coordinates.
//
// This GPU pays for a triangle by what it interpolates across it: twelve to sixteen numbers a vertex made a
// triangle cost three times what three do. So a vertex hands the fragments as little as it can. Light
// travels as two numbers, how much of the ambient colour and how much of the sun's a point takes (the
// colours are the fragment stage's uniforms); what is painted is lit and hazed here outright.
//
// A point is lit by the sun when it stands at or above the shadow height of its place (uShadow, in the
// units of the vertices' own heights): fully at that height, not at all 1 / SHADOW_K below it.
uniform mat4 uMvp;
uniform vec4 uMap;
uniform vec4 uFrame[6];

// How much of the haze colour a point takes, by its depth: 1 - exp(-x), near enough.
float haze_of(float depth)
{
    float x = depth * uFrame[0].w;
    return min(1.0 - 1.0 / (1.0 + x + 0.5 * x * x), uFrame[3].x);
}
