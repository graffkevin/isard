//! The symbol layer's program: its shaders (a glyph or an icon per instanced quad facing the screen, the glyph's
//! signed distance turned into its fill and halo, an icon premultiplied or a distance field taking the icon color) and
//! the names of its uniforms.

/// Vertex shader: the quad of a glyph or icon around its label's anchor, scaled and turned as its placement says
pub(super) const VERTEX: &str = r"
uniform float u_pixel_ratio;
// Size of a page of the atlas, texels
uniform vec2 u_atlas_size;
uniform highp sampler2D u_labels;
uniform highp sampler2D u_visible;
// Size of the icon atlas, texels
uniform vec2 u_icon_size;
layout(location = 0) in uint a_label;
layout(location = 1) in vec4 a_rect;
layout(location = 2) in vec4 a_uv;
// Font scale of a glyph; -1 for an icon, -2 for a signed distance field icon
layout(location = 3) in float a_scale;
// Turn of an icon about the anchor (radians, clockwise)
layout(location = 4) in float a_angle;
out vec2 v_uv;
// Page of the atlas
flat out float v_page;
// A glyph's fill, an icon's color (its opacity in alpha)
flat out vec4 v_fill;
flat out vec4 v_halo;
// Halo edge in distance units, and the font scale
flat out vec2 v_edges;
// 0: a glyph, 1: an icon, 2: a signed distance field icon
flat out float v_kind;

void main() {
  // Placed this frame: what shows and turns, the angle on screen, the text's and the icon's scales (see `place`)
  vec4 state = texelFetch(u_visible, table_texel(a_label), 0);
  uint bits = uint(state.x);
  bool icon = a_scale < 0.0;
  if ((bits & (icon ? 2u : 1u)) == 0u) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); return; }
  float scale = icon ? state.w : state.z;
  uint row = a_label * 5u;
  vec2 offset = texelFetch(u_labels, table_texel(row), 0).xy;
  v_fill = texelFetch(u_labels, table_texel(row + (icon ? 4u : 1u)), 0);
  v_halo = texelFetch(u_labels, table_texel(row + 2u), 0);
  float halo_width = texelFetch(u_labels, table_texel(row + 3u), 0).x;
  vec2 merc = u_origin + offset;
  vec4 c = u_matrix * vec4(project_up(plane_of(offset), merc, altitude(merc)), 1.0);
  c.z -= u_depth_bias;
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
  vec2 q = (a_rect.xy + corner * a_rect.zw) * scale;
  float angle = a_angle;
  if ((bits & (icon ? 8u : 4u)) != 0u) angle += state.y + (icon && (bits & 16u) != 0u ? 3.14159265 : 0.0);
  float cs = cos(angle), sn = sin(angle);
  vec2 px = vec2(q.x * cs - q.y * sn, q.x * sn + q.y * cs) * u_pixel_ratio;
  // Screen y runs down, clip y up
  c.xy += vec2(px.x, -px.y) / u_viewport * c.w;
  gl_Position = c;
  v_kind = icon ? -a_scale : 0.0;
  if (icon) {
    v_page = 0.0;
    v_uv = mix(a_uv.xy, a_uv.zw, corner) / u_icon_size;
    return;
  }
  // The pages are stacked in the atlas coordinates: the page from the top of the glyph (half a texel in, safe from
  // rounding), then the place in it
  float page = floor((a_uv.y + 0.5) / u_atlas_size.y);
  v_page = page;
  v_uv = (mix(a_uv.xy, a_uv.zw, corner) - vec2(0.0, page * u_atlas_size.y)) / u_atlas_size;
  float font = a_scale * scale;
  v_edges = vec2(0.75 - halo_width / (font * 8.0), font);
}";

/// Fragment shader: a glyph's fill and halo from its signed distance, an icon's color
pub(super) const FRAGMENT: &str = r"
uniform mediump sampler2DArray u_atlas;
uniform mediump sampler2D u_icons;
uniform float u_opacity;
in vec2 v_uv;
flat in float v_page;
flat in vec4 v_fill;
flat in vec4 v_halo;
flat in vec2 v_edges;
flat in float v_kind;
out vec4 color;

void main() {
  if (v_kind > 0.5) {
    // Premultiplied: its color back; a distance field icon (distance in alpha) takes the icon color
    vec4 t = texture(u_icons, v_uv);
    float g = max(fwidth(t.a) * 0.7, 1e-3);
    float a = (v_kind > 1.5 ? smoothstep(0.75 - g, 0.75 + g, t.a) : t.a) * v_fill.a;
    if (a <= 0.0) discard;
    color = vec4(v_kind > 1.5 ? v_fill.rgb : t.rgb / max(t.a, 1e-3), a * u_opacity);
    return;
  }
  float d = texture(u_atlas, vec3(v_uv, v_page)).r;
  float g = max(fwidth(d) * 0.7, 1e-3);
  float fill = smoothstep(0.75 - g, 0.75 + g, d);
  float halo = v_edges.x < 0.75 ? smoothstep(v_edges.x - g, v_edges.x + g, d) : 0.0;
  float a_fill = v_fill.a * fill;
  float a_halo = v_halo.a * halo * (1.0 - a_fill);
  float a = a_fill + a_halo;
  if (a <= 0.0) discard;
  color = vec4((v_fill.rgb * a_fill + v_halo.rgb * a_halo) / a, a * u_opacity);
}";

/// The uniforms of the program, in the order of `SymbolLayer::uniforms`
pub(super) const UNIFORMS: [&str; 8] = [
    "u_pixel_ratio",
    "u_atlas_size",
    "u_labels",
    "u_visible",
    "u_atlas",
    "u_opacity",
    "u_icon_size",
    "u_icons",
];
