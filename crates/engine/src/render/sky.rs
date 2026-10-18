//! Space behind the globe: a deep blue to black sky, procedural stars fixed to the body's frame (they turn with the
//! globe, as in Google Earth) and an atmosphere halo around its silhouette (none around an airless body). One
//! full-screen triangle, no texture; drawn only in the globe projection, fading out with it.
//!
//! Over the plane, an optional `Atmosphere` (`MapLibre`'s sky): a gradient from the horizon up to the sky, and fog
//! below the horizon, where a tilted camera sees past the end of the ground; the ground itself fades into the fog
//! towards the far plane (the raster pass reads `Atmosphere::fog`).

use crate::render::gl;
use geo::camera::View;
use glow::HasContext;

const VERTEX: &str = r"#version 300 es
out vec2 v_ndc;
void main() {
  // One triangle covering the screen
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
  v_ndc = p;
  gl_Position = vec4(p, 1.0, 1.0);
}";

const FRAGMENT: &str = r"#version 300 es
precision highp float;
uniform mat4 u_inverse;
uniform vec3 u_eye;
uniform float u_radius;
uniform vec3 u_east;
uniform vec3 u_north;
uniform vec3 u_up;
uniform float u_factor;
// The atmosphere's halo: its color and strength (0 around an airless body)
uniform vec4 u_halo;
in vec2 v_ndc;
out vec4 color;

float hash(vec3 p) {
  p = fract(p * 0.1031);
  p += dot(p, p.zyx + 31.32);
  return fract((p.x + p.y) * p.z);
}

// Stars of one layer: at most one per cell of a grid on the direction sphere
float stars(vec3 dir, float cells, float density, float size) {
  vec3 p = dir * cells;
  vec3 cell = floor(p);
  float h = hash(cell);
  if (h > density) return 0.0;
  vec3 star = cell + 0.5 + 0.35 * vec3(hash(cell + 1.7) - 0.5, hash(cell + 3.1) - 0.5, hash(cell + 5.3) - 0.5);
  float d = length(p - star);
  float brightness = 0.35 + 0.65 * hash(cell + 9.2);
  return brightness * smoothstep(size, 0.0, d);
}

void main() {
  vec4 near = u_inverse * vec4(v_ndc, -1.0, 1.0);
  vec4 far = u_inverse * vec4(v_ndc, 1.0, 1.0);
  vec3 dir = normalize(far.xyz / far.w - near.xyz / near.w);
  // Local frame (x east, y south, z up) to the body's frame: the sky turns with the globe
  vec3 earth = normalize(u_east * dir.x - u_north * dir.y + u_up * dir.z);

  float s = stars(earth, 140.0, 0.08, 0.10) + stars(earth, 320.0, 0.05, 0.12) * 0.7 + stars(earth, 60.0, 0.03, 0.06);
  vec3 space = mix(vec3(0.003, 0.006, 0.02), vec3(0.015, 0.03, 0.08), 0.5 + 0.5 * earth.z) + vec3(s);

  // Atmosphere: glow where the ray passes close to the sphere without hitting it
  vec3 center = vec3(0.0, 0.0, -u_radius);
  vec3 toCenter = center - u_eye;
  float along = max(dot(toCenter, dir), 0.0);
  float miss = length(toCenter - dir * along) / u_radius;
  // Thin and bright at the limb, fading fast into space
  float glow = step(1.0, miss) * exp(-(miss - 1.0) * 28.0);
  space = mix(space, u_halo.rgb, glow * 0.9 * u_halo.a);

  color = vec4(space, u_factor);
}";

/// Sky over the plane, with `MapLibre`'s settings
const ATMOSPHERE: &str = r"#version 300 es
precision highp float;
uniform mat4 u_inverse;
uniform vec3 u_sky;
uniform vec3 u_horizon;
uniform vec3 u_fog;
// Sky-horizon and horizon-fog blends, as fractions of the vertical field of view
uniform vec2 u_blend;
uniform float u_factor;
in vec2 v_ndc;
out vec4 color;

void main() {
  vec4 near = u_inverse * vec4(v_ndc, -1.0, 1.0);
  vec4 far = u_inverse * vec4(v_ndc, 1.0, 1.0);
  vec3 dir = normalize(far.xyz / far.w - near.xyz / near.w);
  // Elevation of the ray over the horizon, radians (local frame: z up)
  float elevation = asin(clamp(dir.z, -1.0, 1.0));
  vec3 c = elevation >= 0.0
    ? mix(u_horizon, u_sky, smoothstep(0.0, max(u_blend.x, 1e-3) * 0.6435, elevation))
    : mix(u_horizon, u_fog, smoothstep(0.0, max(u_blend.y, 1e-3) * 0.3218, -elevation));
  color = vec4(c, 1.0 - u_factor);
}";

/// Sky, horizon and fog over the plane (`MapLibre`'s `setSky`)
#[derive(Clone, Copy, Debug)]
pub struct Atmosphere {
    pub sky: [f32; 3],
    pub horizon: [f32; 3],
    pub fog: [f32; 3],
    pub sky_horizon_blend: f32,
    pub horizon_fog_blend: f32,
    /// Share of the distance to the far plane over which the ground fades into the fog (0: no fog on the ground)
    pub fog_ground_blend: f32,
}

impl Atmosphere {
    /// Fog of the ground: its color and the distances (pixels of the local frame) where it starts and is full
    #[must_use]
    pub fn fog(&self, view: &View) -> ([f32; 3], [f32; 2]) {
        let far = view.far as f32;
        let blend = self.fog_ground_blend.clamp(0.0, 1.0);
        (self.fog, [far * (1.0 - blend), far])
    }
}

/// The Earth's halo, a light blue at full strength
const HALO: [f32; 4] = [0.42, 0.66, 1.0, 1.0];

/// A halo from the page (RGB 0-1 and its strength), clamped; none without its 4 numbers
#[must_use]
pub fn halo(settings: Option<&[f32]>) -> [f32; 4] {
    match settings {
        Some(&[r, g, b, a]) => [r, g, b, a].map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }),
        _ => [0.0; 4],
    }
}

pub struct Sky {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 8],
    vao: glow::VertexArray,
    pub enabled: bool,
    /// The atmosphere halo around the globe: RGB 0-1 and its strength, 0 for none (an airless body)
    pub halo: [f32; 4],
    atmosphere_program: glow::Program,
    atmosphere_uniforms: [Option<glow::UniformLocation>; 6],
    pub atmosphere: Option<Atmosphere>,
}

impl Sky {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        // Everything created is deleted again if a step fails
        let mut objects = gl::Objects::new(gl);
        let program = objects.program(VERTEX, FRAGMENT)?;
        let uniforms = gl::uniforms(
            gl,
            program,
            [
                "u_inverse",
                "u_eye",
                "u_radius",
                "u_east",
                "u_north",
                "u_up",
                "u_factor",
                "u_halo",
            ],
        );
        let atmosphere_program = objects.program(VERTEX, ATMOSPHERE)?;
        let atmosphere_uniforms = gl::uniforms(
            gl,
            atmosphere_program,
            ["u_inverse", "u_sky", "u_horizon", "u_fog", "u_blend", "u_factor"],
        );
        let vao = objects.vertex_array()?;
        objects.keep();
        Ok(Self {
            program,
            uniforms,
            vao,
            enabled: true,
            halo: HALO,
            atmosphere_program,
            atmosphere_uniforms,
            atmosphere: None,
        })
    }

    pub fn draw(&self, gl: &glow::Context, view: &View) {
        self.draw_atmosphere(gl, view);
        let g = &view.globe;
        if !self.enabled || g.factor <= 0.0 {
            return;
        }
        let [inverse, eye, radius, east, north, up, factor, halo] = &self.uniforms;
        // SAFETY: draw with objects owned by the sky
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_4_f32_slice(inverse.as_ref(), false, &gl::mat4(&view.inverse));
            gl.uniform_3_f32(eye.as_ref(), view.eye.x as f32, view.eye.y as f32, view.eye.z as f32);
            gl.uniform_1_f32(radius.as_ref(), g.radius as f32);
            for (location, axis) in [(east, g.east), (north, g.north), (up, g.up)] {
                gl.uniform_3_f32(location.as_ref(), axis.x as f32, axis.y as f32, axis.z as f32);
            }
            gl.uniform_1_f32(factor.as_ref(), g.factor as f32);
            gl.uniform_4_f32_slice(halo.as_ref(), &self.halo);
            // The screen triangle turns the other way round than the globe's front faces
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.enable(glow::CULL_FACE);
        }
    }

    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by the sky, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_program(self.atmosphere_program);
            gl.delete_vertex_array(self.vao);
        }
    }

    /// The sky over the plane, fading out as the globe takes over
    fn draw_atmosphere(&self, gl: &glow::Context, view: &View) {
        let Some(a) = &self.atmosphere else {
            return;
        };
        let factor = if self.enabled { view.globe.factor } else { 0.0 };
        if factor >= 1.0 {
            return;
        }
        let [inverse, sky, horizon, fog, blend, factor_u] = &self.atmosphere_uniforms;
        // SAFETY: draw with objects owned by the sky
        unsafe {
            gl.use_program(Some(self.atmosphere_program));
            gl.uniform_matrix_4_f32_slice(inverse.as_ref(), false, &gl::mat4(&view.inverse));
            for (location, [r, g, b]) in [(sky, a.sky), (horizon, a.horizon), (fog, a.fog)] {
                gl.uniform_3_f32(location.as_ref(), r, g, b);
            }
            gl.uniform_2_f32(blend.as_ref(), a.sky_horizon_blend, a.horizon_fog_blend);
            gl.uniform_1_f32(factor_u.as_ref(), factor as f32);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.enable(glow::CULL_FACE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo_of_the_body() {
        // The Earth's by default, as it always was
        assert_eq!(HALO, [0.42, 0.66, 1.0, 1.0]);
        assert_eq!(halo(Some(&HALO)), HALO);
        // Another color and strength, clamped
        assert_eq!(halo(Some(&[0.8, 0.6, 0.4, 0.5])), [0.8, 0.6, 0.4, 0.5]);
        assert_eq!(halo(Some(&[1.5, -1.0, f32::NAN, 2.0])), [1.0, 0.0, 0.0, 1.0]);
        // None around an airless body, or without its 4 numbers
        assert_eq!(halo(None), [0.0; 4]);
        assert_eq!(halo(Some(&[0.5, 0.5, 0.5])), [0.0; 4]);
    }
}
