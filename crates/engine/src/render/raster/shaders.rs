//! Shaders of the raster pass: the ground's vertex shader (shared with the ground layers), the compositing of a
//! stack, and the depth pass of the picking

/// The ground's vertex shader, shared by the raster pass, its depth pass and the ground layers (`render::ground`):
/// invariant, so that they all put the ground at the same depth
pub const VERTEX: &str = r"
invariant gl_Position;
uniform mat4 u_matrix;
layout(location = 0) in vec2 a_grid;
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_merc;
// Per layer: uv rect of the tile, uv rect of its stand-in, (slot of the tile, slot of the stand-in, fade)
layout(location = 3) in vec4 a_uv0;
layout(location = 4) in vec4 a_back0;
layout(location = 5) in vec3 a_slot0;
layout(location = 6) in vec4 a_uv1;
layout(location = 7) in vec4 a_back1;
layout(location = 8) in vec3 a_slot1;
layout(location = 9) in vec4 a_uv2;
layout(location = 10) in vec4 a_back2;
layout(location = 11) in vec3 a_slot2;
layout(location = 12) in vec4 a_uv3;
layout(location = 13) in vec4 a_back3;
layout(location = 14) in vec3 a_slot3;
// Heights: offset and scale of this tile in its elevation tile, slot of that tile (< 0: none)
layout(location = 15) in vec4 a_height;
uniform highp sampler2DArray u_heights;
// Vertical exaggeration; 0: flat
uniform float u_relief;
uniform vec3 u_eye;
out vec4 v_uv[4];
flat out vec3 v_slot[4];
out float v_distance;
// Position on the plane (pixels from the camera center): the ground layers find the Mercator position from it
out vec2 v_plane;
vec2 g;
vec4 uv(vec4 front, vec4 back) { return vec4(mix(front.xy, front.zw, g), mix(back.xy, back.zw, g)); }

// Altitude (metres) at a point of the tile, bilinear between the samples (R32F cannot be filtered)
float height() {
  if (u_relief <= 0.0 || a_height.w < 0.0) return 0.0;
  vec2 p = clamp((a_height.xy + g * a_height.z) * 256.0 - 0.5, 0.0, 255.0);
  ivec2 i = ivec2(floor(p));
  ivec2 j = min(i + 1, ivec2(255));
  vec2 f = p - vec2(i);
  int s = int(a_height.w);
  float h00 = texelFetch(u_heights, ivec3(i.x, i.y, s), 0).r;
  float h10 = texelFetch(u_heights, ivec3(j.x, i.y, s), 0).r;
  float h01 = texelFetch(u_heights, ivec3(i.x, j.y, s), 0).r;
  float h11 = texelFetch(u_heights, ivec3(j.x, j.y, s), 0).r;
  return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y) * u_relief;
}

void main() {
  // Skirt vertices come shifted by +2: they hang under the edge, by 5 % of the tile width
  g = a_grid;
  float skirt = 0.0;
  if (g.x > 1.5) {
    g -= 2.0;
    skirt = 1.0;
  }
  v_uv[0] = uv(a_uv0, a_back0); v_slot[0] = a_slot0;
  v_uv[1] = uv(a_uv1, a_back1); v_slot[1] = a_slot1;
  v_uv[2] = uv(a_uv2, a_back2); v_slot[2] = a_slot2;
  v_uv[3] = uv(a_uv3, a_back3); v_slot[3] = a_slot3;
  vec2 merc = mix(a_merc.xy, a_merc.zw, g);
  float lat = atan(sinh(3.1415927 * (1.0 - 2.0 * merc.y)));
  float width = (a_merc.z - a_merc.x) * 40075016.686 * cos(lat);
  v_plane = mix(a_rect.xy, a_rect.zw, g);
  vec3 p = project_up(v_plane, merc, height() - skirt * 0.05 * width);
  v_distance = distance(p, u_eye);
  gl_Position = u_matrix * vec4(p, 1.0);
}";

pub const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform highp sampler2DArray u_tiles0;
uniform highp sampler2DArray u_tiles1;
uniform highp sampler2DArray u_tiles2;
uniform highp sampler2DArray u_tiles3;
uniform int u_count;
uniform float u_opacity[4];
uniform float u_saturation[4];
// Fog of the atmosphere: color, then the distances where it starts and is full (none if the second is 0)
uniform vec3 u_fog;
uniform vec2 u_fog_range;
in vec4 v_uv[4];
flat in vec3 v_slot[4];
in float v_distance;
out vec4 color;

// One layer, straight alpha: the tile mixed with its stand-in while it fades in; slot < 0 = none
vec4 layer(highp sampler2DArray tiles, vec4 uv, vec3 slot, float opacity, float saturation) {
  vec4 c = slot.x >= 0.0 ? texture(tiles, vec3(uv.xy, slot.x)) : vec4(0.0);
  if (slot.z < 1.0) {
    c = slot.y >= 0.0 ? mix(texture(tiles, vec3(uv.zw, slot.y)), c, slot.z) : vec4(c.rgb, c.a * slot.z);
  }
  float grey = dot(c.rgb, vec3(0.299, 0.587, 0.114));
  return vec4(mix(vec3(grey), c.rgb, saturation), c.a * opacity);
}

// Over operator, premultiplied result
void over(inout vec4 acc, vec4 c) {
  acc = vec4(c.rgb * c.a + acc.rgb * (1.0 - c.a), c.a + acc.a * (1.0 - c.a));
}

void main() {
  vec4 acc = vec4(0.0);
  over(acc, layer(u_tiles0, v_uv[0], v_slot[0], u_opacity[0], u_saturation[0]));
  if (u_count > 1) over(acc, layer(u_tiles1, v_uv[1], v_slot[1], u_opacity[1], u_saturation[1]));
  if (u_count > 2) over(acc, layer(u_tiles2, v_uv[2], v_slot[2], u_opacity[2], u_saturation[2]));
  if (u_count > 3) over(acc, layer(u_tiles3, v_uv[3], v_slot[3], u_opacity[3], u_saturation[3]));
  if (u_fog_range.y > 0.0) acc.rgb = mix(acc.rgb, u_fog * acc.a, smoothstep(u_fog_range.x, u_fog_range.y, v_distance));
  color = acc;
}";

/// Depth of the ground, packed in RGB (24 bits), alpha 1 where there is ground: the picking of positions in relief
pub const DEPTH_FRAGMENT: &str = r"#version 300 es
precision highp float;
out vec4 color;
void main() {
  vec3 e = fract(gl_FragCoord.z * vec3(1.0, 255.0, 65025.0));
  e -= e.yzz * vec3(1.0 / 255.0, 1.0 / 255.0, 0.0);
  color = vec4(e, 1.0);
}";
