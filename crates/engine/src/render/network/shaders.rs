//! Shaders of the network layer: the pieces expanded on screen (`overlay::LINE`), the value along each edge, the
//! colors and the picking

/// After `PROJECT`, `drape::ALTITUDE`, `overlay::ANCHOR` and `overlay::LINE`
pub const VERTEX: &str = r"
uniform float u_width;
uniform bool u_with_costs;
uniform highp sampler2D u_values;
uniform highp usampler2D u_edges;
uniform highp sampler2D u_costs;
// Width factor of each edge (rail lines wider than streets)
uniform highp sampler2D u_scales;
uniform float u_max;
uniform float u_halo;
// The edge of the piece
layout(location = 2) in uint a_edge;
#ifdef PICK
flat out uint v_id;
#endif
out float v_fraction;
flat out vec4 v_ends;
out float v_across;
flat out float v_half;

ivec2 texel(uint i) { return ivec2(int(i & 4095u), int(i >> 12)); }

void main() {
  uvec2 nodes = texelFetch(u_edges, texel(a_edge), 0).xy;
  vec2 costs = u_with_costs ? texelFetch(u_costs, texel(a_edge), 0).xy : vec2(0.0);
  // An edge closed both ways is not drawn
  if (u_with_costs && costs.x >= 1e29 && costs.y >= 1e29) { hide(); return; }
  v_ends = vec4(texelFetch(u_values, texel(nodes.x), 0).r, texelFetch(u_values, texel(nodes.y), 0).r, costs);
  // The halo of an edge reached at neither end within the maximum draws nothing: out of the clip space, it costs no
  // pixel (a million wide quads zoomed out would saturate the graphics card)
  bool near_a = v_ends.x >= 0.0 && v_ends.x <= u_max;
  bool near_b = v_ends.y >= 0.0 && v_ends.y <= u_max;
  if (u_halo > 0.0 && !near_a && !near_b) { hide(); return; }
  float width = u_width * texelFetch(u_scales, texel(a_edge), 0).r;
  // The piece is a strip of `u_segments` parts (1 on flat ground; in relief each part follows the ground), its ends
  // pushed out by half the half width
  StripVertex v;
  if (!strip_vertex(width, 0.5, v)) { hide(); return; }
  v_fraction = mix(a_fractions.x, a_fractions.y, v.t);
  v_across = v.side * v.half_width;
  v_half = v.half_width;
#ifdef PICK
  v_id = a_edge + 1u;
#endif
}";

/// Value at a pixel of an edge; negative: unreached
pub const VALUE: &str = r"
uniform bool u_with_costs;
in float v_fraction;
flat in vec4 v_ends;

float value() {
  float ta = v_ends.x, tb = v_ends.y;
  bool ra = ta >= 0.0 && ta < 1e29;
  bool rb = tb >= 0.0 && tb < 1e29;
  if (!u_with_costs) return ra && rb ? mix(ta, tb, v_fraction) : -1.0;
  // The quickest way here: from A forwards or from B backwards
  float t = 1e30;
  if (ra && v_ends.z < 1e29) t = ta + v_fraction * v_ends.z;
  if (rb && v_ends.w < 1e29) t = min(t, tb + (1.0 - v_fraction) * v_ends.w);
  return t >= 1e29 ? -1.0 : t;
}
";

pub const FRAGMENT: &str = r"
uniform float u_max;
uniform float u_contour;
uniform float u_opacity;
// 0: the lines; 1: the halo (wide, faint, reached parts only)
uniform float u_halo;
uniform sampler2D u_ramp;
// Color of the parts beyond the maximum (or unreached), straight alpha
uniform vec4 u_unreached;
// Share of the maximum over which the colors fade out before it, 0 for none
uniform float u_fade;
in float v_across;
flat in float v_half;
out vec4 color;

vec3 ramp(float t) { return texture(u_ramp, vec2((clamp(t, 0.0, 1.0) * 255.0 + 0.5) / 256.0, 0.5)).rgb; }

void main() {
  float v = value();
  bool reached = v >= 0.0 && v <= u_max;
  float fade = u_fade > 0.0 && reached ? 1.0 - smoothstep(u_max * (1.0 - u_fade), u_max, v) : 1.0;
  if (u_halo > 0.0) {
    if (!reached) discard;
    // Quadratic falloff from the middle of the line: overlapping streets add up and fill the blocks
    float d = 1.0 - clamp(abs(v_across) / v_half, 0.0, 1.0);
    color = vec4(ramp(v / u_max), d * d * u_opacity * fade);
    return;
  }
  float edge = clamp(v_half - abs(v_across), 0.0, 1.0);
  vec3 rgb = reached ? ramp(v / u_max) : u_unreached.rgb;
  float alpha = reached ? fade : u_unreached.a;
  if (reached && u_contour > 0.0) {
    float d = abs(fract(v / u_contour + 0.5) - 0.5) * u_contour;
    float w = fwidth(v);
    rgb *= mix(0.45, 1.0, smoothstep(w * 0.5, w * 1.5, d));
  }
  color = vec4(rgb, alpha * edge * u_opacity);
}";

/// Picking pass: the edge number (+1) in RGB, the layer in alpha; no blending, no antialiasing
pub const PICK_FRAGMENT: &str = r"
uniform float u_layer;
in float v_across;
flat in float v_half;
flat in uint v_id;
out vec4 color;
void main() {
  if (abs(v_across) > v_half) discard;
  color = vec4(float(v_id & 255u), float((v_id >> 8) & 255u), float((v_id >> 16) & 255u), u_layer) / 255.0;
}";
