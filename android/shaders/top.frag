precision mediump float;
uniform sampler2D uGround;
uniform highp sampler2D uShadow;
uniform sampler2D uLamp;
uniform highp vec4 uMap;
// [0] rgb: the ambient light on a face that looks up; [1] rgb: the sun; [2] rgb: the haze, a: how far the
// street lamps are on
uniform vec4 uLight[3];
#ifdef SHADOWS
in highp vec3 vAt;
#else
in highp vec2 vAt;
#endif
in vec3 vLight;
out vec4 oColor;

void main()
{
#ifdef SHADOWS
    highp float over = vAt.z - texture(uShadow, vAt.xy * uMap.xy + uMap.zw).r;
    vec3 light = uLight[0].rgb * vLight.x + uLight[1].rgb * (vLight.y * clamp(over * SHADOW_K + 1.0, 0.0, 1.0));
#else
    vec3 light = uLight[0].rgb * vLight.x + uLight[1].rgb * vLight.y;
#endif
#ifdef LAMPS
    // Lamp light lies on the ground as it was drawn into the height grid's twin (at half strength).
    light += texture(uLamp, vAt.xy * uMap.xy + uMap.zw).rgb * uLight[2].a;
#endif
    vec3 c = texture(uGround, vAt.xy).rgb * light;
    oColor = vec4(mix(c, uLight[2].rgb, vLight.z), 1.0);
}
