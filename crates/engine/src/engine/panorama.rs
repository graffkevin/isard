//! Panoramas: the oriented images of a street-level panorama (a vehicle's cameras around one position) projected onto
//! the scene. The solid scene is drawn from the camera into a depth texture (as for the viewshed); a pass over the
//! screen puts each pixel back in the local frame, takes it into each camera of the panorama (its rotation and
//! position in the panorama, its intrinsics and its radial distortion), and samples the camera's image there,
//! weighted down toward the image's borders. Beyond `radius` metres, and where nothing was drawn (the sky), the pixel
//! is taken on a sphere of that radius around the panorama: the images as a backdrop.

use super::Engine;
use crate::render::gl::{self, Objects};
use geo::camera::View;
use glam::{DMat3, DMat4, DVec3};
use glow::HasContext;
use wasm_bindgen::prelude::*;

/// Cameras of a panorama at most
const CAMERAS: usize = 8;
/// Metres around the panorama its images color, seen from above (the street it was taken in)
const REACH_FROM_ABOVE: f64 = 40.0;
/// Numbers per camera given to `setPanorama`: rotation (9, row-major), position (3), focal x and y, center x and y,
/// skew, image width and height, principal point of the distortion x and y, its three coefficients, its limit
const CAMERA_NUMBERS: usize = 25;

const FRAGMENT: &str = r"#version 300 es
precision highp float;
precision highp sampler2DArray;
const int CAMERAS = 8;
uniform sampler2D u_depth;
uniform sampler2DArray u_images;
uniform mat4 u_inverse;
// The panorama in the local frame, and the radius of its backdrop sphere (pixels of the local frame)
uniform vec4 u_center;
// How far from the panorama the scene takes its images (pixels): its backdrop's radius at street level, a street's
// width seen from above (beyond, the images are stretched past recognition); 1 when the backdrop shows
uniform vec2 u_reach;
uniform int u_count;
// Local frame to each camera (x right, y down, z forward, metres)
uniform mat4 u_cameras[CAMERAS];
// Focal x, y, center x, y (pixels)
uniform vec4 u_intrinsics[CAMERAS];
// Skew, width, height of the image (pixels), its layer in u_images
uniform vec4 u_sizes[CAMERAS];
// Principal point of the distortion x, y; its coefficients r^3, r^5 (r^7 in u_limits.x), and the square of its limit
uniform vec4 u_distortions[CAMERAS];
uniform vec2 u_limits[CAMERAS];
uniform float u_opacity;
in vec2 v_ndc;
out vec4 color;

vec3 unproject(float z) {
  vec4 p = u_inverse * vec4(v_ndc, z, 1.0);
  return p.xyz / p.w;
}

void main() {
  float depth = texture(u_depth, v_ndc * 0.5 + 0.5).r;
  vec3 near = unproject(-1.0);
  vec3 ray = normalize(unproject(1.0) - near);
  vec3 p = depth < 1.0 ? unproject(depth * 2.0 - 1.0) : near + ray * 1e9;
  // Beyond the backdrop, or the sky: the point of the sphere around the panorama on the ray from the eye, when the
  // eye is within it (from outside, only what stands within it)
  vec3 o = near - u_center.xyz;
  if (length(p - u_center.xyz) > u_reach.x) {
    if (u_reach.y < 0.5 || dot(o, o) > u_center.w * u_center.w) discard;
    float b = dot(o, ray);
    float c = dot(o, o) - u_center.w * u_center.w;
    float t = -b + sqrt(max(b * b - c, 0.0));
    p = near + ray * t;
  }
  vec4 sum = vec4(0.0);
  for (int i = 0; i < CAMERAS; i++) {
    if (i >= u_count) break;
    vec4 c = u_cameras[i] * vec4(p - u_center.xyz, 1.0);
    if (c.z <= 0.0) continue;
    vec4 k = u_intrinsics[i];
    vec4 s = u_sizes[i];
    if (s.w < 0.0) continue;
    vec2 px = vec2(k.x * c.x / c.z + s.x * c.y / c.z + k.z, k.y * c.y / c.z + k.w);
    // MicMac's radial distortion (poly357), within its limit
    vec4 d = u_distortions[i];
    vec2 v = px - d.xy;
    float v2 = dot(v, v);
    if (v2 > u_limits[i].y) continue;
    px += (v2 * (d.z + v2 * (d.w + v2 * u_limits[i].x))) * v;
    vec2 uv = px / s.yz;
    vec2 edge = clamp(20.0 * min(uv, 1.0 - uv), 0.0, 1.0);
    float w = min(edge.x, edge.y);
    if (w <= 0.0) continue;
    vec3 image = texture(u_images, vec3(uv, s.w)).rgb;
    sum += vec4(image * w, w);
  }
  if (sum.a <= 0.0) discard;
  color = vec4(sum.rgb / sum.a, u_opacity * min(sum.a, 1.0));
}";

/// A camera of the panorama, as given
#[derive(Clone, Copy)]
struct Camera {
    /// Panorama frame to camera (x right, y down, z forward)
    rotation: DMat3,
    /// In the panorama frame, metres
    position: DVec3,
    intrinsics: [f32; 4],
    /// Skew, width, height
    size: [f32; 3],
    distortion: [f32; 4],
    limits: [f32; 2],
}

/// What the page set: where the panorama is, how it is turned, its cameras
struct Settings {
    /// Longitude, latitude (degrees), ellipsoidal height (metres)
    position: DVec3,
    /// Panorama frame to east, north, up
    rotation: DMat3,
    cameras: Vec<Camera>,
    /// Metres of the backdrop sphere
    radius: f64,
    opacity: f32,
}

/// The GPU side: the pass, the images, the screen's depth
pub(super) struct Panorama {
    settings: Settings,
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 12],
    vao: glow::VertexArray,
    /// The cameras' images, a layer each, at the size of the first to arrive (the images of a panorama share one: a
    /// reduced set of them, smaller than their calibration, is read as well)
    images: glow::Texture,
    image_size: Option<(i32, i32)>,
    /// Images received, by camera
    loaded: Vec<bool>,
    depth: gl::DepthTarget,
}

impl Panorama {
    fn new(gl: &glow::Context, settings: Settings) -> Result<Self, String> {
        let mut objects = Objects::new(gl);
        let program = objects.program(gl::FULL_SCREEN_VERTEX, FRAGMENT)?;
        let vao = objects.vertex_array()?;
        let images = objects.texture()?;

        objects.keep();

        let uniforms = gl::uniforms(
            gl,
            program,
            [
                "u_depth",
                "u_images",
                "u_inverse",
                "u_center",
                "u_count",
                "u_cameras",
                "u_intrinsics",
                "u_sizes",
                "u_distortions",
                "u_limits",
                "u_opacity",
                "u_reach",
            ],
        );
        let loaded = vec![false; settings.cameras.len()];

        Ok(Self {
            settings,
            program,
            uniforms,
            vao,
            images,
            image_size: None,
            loaded,
            depth: gl::DepthTarget::default(),
        })
    }

    pub(super) fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: objects owned by the panorama, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_texture(self.images);
        }
        self.depth.delete(gl);
    }
}

impl Engine {
    /// Draws the panorama over the frame, if one is set and has an image
    pub(super) fn draw_panorama(&mut self, view: &View) {
        let Some(mut panorama) = self.panorama.take() else {
            return;
        };
        let size = (
            (self.camera.width * self.pixel_ratio).round() as i32,
            (self.camera.height * self.pixel_ratio).round() as i32,
        );

        if panorama.loaded.iter().any(|&l| l)
            && let Ok((framebuffer, depth)) = panorama.depth.at(&self.gl, size)
        {
            // SAFETY: offscreen pass on the live context, the default framebuffer bound again after
            unsafe {
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                self.gl.viewport(0, 0, size.0, size.1);
                self.draw_solid_depth(view);
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                self.gl.viewport(0, 0, size.0, size.1);
            }
            self.draw_panorama_pass(&panorama, view, depth);
        }
        self.panorama = Some(panorama);
    }

    fn draw_panorama_pass(&self, panorama: &Panorama, view: &View, depth: glow::Texture) {
        let s = &panorama.settings;
        let k = view.pixels_per_metre(s.position.y);
        // The panorama's east-north-up frame (metres) in the local frame (pixels), and back
        let enu = view.enu_matrix(s.position);
        let center = enu.transform_point3(DVec3::ZERO);
        let to_enu = enu.inverse();
        // Local offsets from the panorama to each camera: through east-north-up, the panorama frame, the camera
        let mut cameras = Vec::with_capacity(CAMERAS * 16);
        let (mut intrinsics, mut sizes, mut distortions, mut limits) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());

        for (layer, c) in s.cameras.iter().enumerate().take(CAMERAS) {
            let rotation = c.rotation * s.rotation.transpose();
            let offset = DMat4::from_mat3(DMat3::from_cols(
                to_enu.x_axis.truncate(),
                to_enu.y_axis.truncate(),
                to_enu.z_axis.truncate(),
            ));
            let m = DMat4::from_mat3(rotation) * DMat4::from_translation(-(s.rotation * c.position)) * offset;

            cameras.extend(m.to_cols_array().map(|v| v as f32));
            intrinsics.extend(c.intrinsics);

            // A camera whose image has not arrived: its layer -1, left out
            let image = if panorama.loaded.get(layer).copied().unwrap_or(false) {
                layer as f32
            } else {
                -1.0
            };

            sizes.extend([c.size[0], c.size[1], c.size[2], image]);
            distortions.extend(c.distortion);
            limits.extend(c.limits);
        }

        let count = s.cameras.len().min(CAMERAS);
        let [
            depth_u,
            images_u,
            inverse_u,
            center_u,
            count_u,
            cameras_u,
            intrinsics_u,
            sizes_u,
            distortions_u,
            limits_u,
            opacity_u,
            reach_u,
        ] = &panorama.uniforms;
        // At street level the images reach the backdrop; from above, the street around the panorama only
        let (reach, backdrop) = if self.camera.first_person.is_some() {
            (s.radius, 1.0)
        } else {
            (REACH_FROM_ABOVE, 0.0)
        };
        let gl = &self.gl;

        // SAFETY: the pass over the default framebuffer, the state set back after
        unsafe {
            gl.use_program(Some(panorama.program));
            gl.uniform_1_i32(depth_u.as_ref(), 0);
            gl.uniform_1_i32(images_u.as_ref(), 1);
            gl.uniform_matrix_4_f32_slice(inverse_u.as_ref(), false, &gl::mat4(&view.inverse));
            gl.uniform_4_f32(
                center_u.as_ref(),
                center.x as f32,
                center.y as f32,
                center.z as f32,
                (s.radius * k) as f32,
            );
            gl.uniform_1_i32(count_u.as_ref(), count as i32);
            gl.uniform_matrix_4_f32_slice(cameras_u.as_ref(), false, &cameras);
            gl.uniform_4_f32_slice(intrinsics_u.as_ref(), &intrinsics);
            gl.uniform_4_f32_slice(sizes_u.as_ref(), &sizes);
            gl.uniform_4_f32_slice(distortions_u.as_ref(), &distortions);
            gl.uniform_2_f32_slice(limits_u.as_ref(), &limits);
            gl.uniform_1_f32(opacity_u.as_ref(), s.opacity);
            gl.uniform_2_f32(reach_u.as_ref(), (reach * k) as f32, backdrop);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(depth));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(panorama.images));
            gl.bind_vertex_array(Some(panorama.vao));
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::BLEND);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        gl::reset_state(gl);
    }
}

#[wasm_bindgen]
impl Engine {
    /// Shows a panorama: at `lon`, `lat` (degrees) and `height` (metres above the ellipsoid), turned by `rotation`
    /// (9 numbers, row-major: its frame to east, north, up), its `cameras` (25 numbers each: rotation from the panorama
    /// frame, row-major, 9; position in it, 3; focal x, y; center x, y; skew; image width, height; principal point of
    /// the distortion x, y; its coefficients r³, r⁵, r⁷; its limit, pixels), a backdrop sphere of `radius` metres, at
    /// `opacity`. Its images come after (`setPanoramaImage`)
    #[wasm_bindgen(js_name = setPanorama)]
    #[allow(clippy::too_many_arguments)]
    pub fn set_panorama(
        &mut self,
        lon: f64,
        lat: f64,
        height: f64,
        rotation: &[f64],
        cameras: &[f64],
        radius: f64,
        opacity: f32,
    ) -> Result<(), JsValue> {
        let matrix = |m: &[f64]| DMat3::from_cols_array(&[m[0], m[3], m[6], m[1], m[4], m[7], m[2], m[5], m[8]]);

        if rotation.len() < 9 {
            return Err("setPanorama: a rotation of 9 numbers expected".into());
        }

        let cameras: Vec<Camera> = cameras
            .as_chunks::<CAMERA_NUMBERS>()
            .0
            .iter()
            .take(CAMERAS)
            .map(|c| Camera {
                rotation: matrix(&c[..9]),
                position: DVec3::new(c[9], c[10], c[11]),
                intrinsics: [c[12], c[13], c[14], c[15]].map(|v| v as f32),
                size: [c[16], c[17], c[18]].map(|v| v as f32),
                distortion: [c[19], c[20], c[21], c[22]].map(|v| v as f32),
                // The r⁷ coefficient, the square of the limit (none: no limit)
                limits: [
                    c[23] as f32,
                    if c[24] > 0.0 { (c[24] * c[24]) as f32 } else { f32::MAX },
                ],
            })
            .collect();
        let settings = Settings {
            position: DVec3::new(lon, lat, height),
            rotation: matrix(rotation),
            cameras,
            radius: if radius.is_finite() && radius > 0.0 {
                radius
            } else {
                1000.0
            },
            opacity: opacity.clamp(0.0, 1.0),
        };

        if let Some(mut old) = self.panorama.take() {
            old.delete(&self.gl);
        }
        self.panorama = Some(Panorama::new(&self.gl, settings).map_err(|e| JsValue::from_str(&e))?);
        (self.redraw)();

        Ok(())
    }

    /// The image of camera `index` of the panorama (the images sharing the size of the first)
    #[wasm_bindgen(js_name = setPanoramaImage)]
    pub fn set_panorama_image(&mut self, index: usize, image: &web_sys::ImageBitmap) -> Result<(), JsValue> {
        let Some(panorama) = self.panorama.as_mut() else {
            return Ok(());
        };

        if index >= panorama.loaded.len() {
            return Ok(());
        }

        let (width, height) = if let Some(size) = panorama.image_size {
            size
        } else {
            {
                let size = (image.width() as i32, image.height() as i32);
                let layers = panorama.loaded.len().max(1) as i32;

                // SAFETY: storage of the panorama's texture, on the live context
                unsafe {
                    let gl = &self.gl;

                    gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(panorama.images));
                    gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, 1, glow::RGBA8, size.0, size.1, layers);

                    for (name, value) in [
                        (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                        (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                    ] {
                        gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, name, value as i32);
                    }
                }
                panorama.image_size = Some(size);

                size
            }
        };

        // SAFETY: upload into a layer of the panorama's texture, on the live context
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(panorama.images));
            self.raw.tex_sub_image_3d_with_image_bitmap(
                glow::TEXTURE_2D_ARRAY,
                0,
                0,
                0,
                index as i32,
                width.min(image.width() as i32),
                height.min(image.height() as i32),
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                image,
            )?;
            self.gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        }
        panorama.loaded[index] = true;
        (self.redraw)();

        Ok(())
    }

    /// Removes the panorama
    #[wasm_bindgen(js_name = clearPanorama)]
    pub fn clear_panorama(&mut self) {
        if let Some(mut panorama) = self.panorama.take() {
            panorama.delete(&self.gl);
            (self.redraw)();
        }
    }
}
