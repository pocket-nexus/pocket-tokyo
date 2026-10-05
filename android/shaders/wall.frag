precision mediump float;
uniform sampler2D uFacade;
uniform sampler2D uNight;
// [0] rgb: the ambient light on a face that looks sideways; [1] rgb: the sun; [2] rgb: the haze
uniform vec4 uLight[3];
in highp vec2 vUv;
#ifdef SHADOWS
in vec4 vLight;
#else
in vec3 vLight;
#endif
#ifdef LAMPS
in vec4 vTint;
#else
in vec3 vTint;
#endif
out vec4 oColor;

void main()
{
    vec4 t = texture(uFacade, vUv);
#ifdef SHADOWS
    vec3 light = uLight[0].rgb * vLight.x + uLight[1].rgb * (vLight.y * clamp(vLight.w, 0.0, 1.0));
#else
    vec3 light = uLight[0].rgb * vLight.x + uLight[1].rgb * vLight.y;
#endif
    vec3 c = t.rgb * (1.0 + vTint.rgb * t.a) * light;
#ifdef LAMPS
    c += texture(uNight, vUv).rgb * vTint.a;
#endif
    oColor = vec4(mix(c, uLight[2].rgb, vLight.z), 1.0);
}
