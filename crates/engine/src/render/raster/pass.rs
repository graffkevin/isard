//! The raster pass: a stack of up to `STACK` sources drawn in one instanced call, and the ground drawn again (the
//! ground layers, the depth of the picking)

use super::cache::Sample;
use super::shaders::{DEPTH_FRAGMENT, FRAGMENT, VERTEX};
use super::{INSTANCE, RasterSource, STACK};
use crate::render::gl::{self, Grid, Objects, Projection};
use geo::camera::View;
use glow::HasContext;

/// Instance attributes: location, floats, byte offset in the instance (rects, then 3 per layer, then the heights)
const ATTRIBUTES: [(u32, i32, i32); 3 + 3 * STACK] = attributes();

const fn attributes() -> [(u32, i32, i32); 3 + 3 * STACK] {
    let mut out = [(0, 0, 0); 3 + 3 * STACK];
    out[0] = (1, 4, 0);
    out[1] = (2, 4, 16);
    let mut k = 0;
    while k < STACK {
        let base = 32 + k as i32 * 44;
        out[2 + 3 * k] = (3 + 3 * k as u32, 4, base);
        out[3 + 3 * k] = (4 + 3 * k as u32, 4, base + 16);
        out[4 + 3 * k] = (5 + 3 * k as u32, 3, base + 32);
        k += 1;
    }
    out[2 + 3 * STACK] = (15, 4, 32 + STACK as i32 * 44);
    out
}

/// The instances of one draw, kept for drawing again
struct Stack {
    vao: glow::VertexArray,
    instances: glow::Buffer,
    /// Tiles of the last draw (still in `instances`) and its exaggeration
    last: Option<(i32, f32)>,
    /// Mercator bounds of those tiles, in instance order
    tiles: Vec<(glam::DVec2, glam::DVec2)>,
}

impl Stack {
    fn new(gl: &glow::Context, grid: &Grid, objects: &mut Objects) -> Result<Self, String> {
        let stride = (INSTANCE * 4) as i32;
        let vao = objects.vertex_array()?;
        let instances = objects.buffer()?;
        // SAFETY: state setup of objects just created on a live WebGL2 context
        unsafe {
            gl.bind_vertex_array(Some(vao));
            grid.bind(gl);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
            for (location, size, offset) in ATTRIBUTES {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.bind_vertex_array(None);
            Ok(Self {
                vao,
                instances,
                last: None,
                tiles: Vec::new(),
            })
        }
    }
}

/// Draws a stack of raster sources in one pass: the tiles of the first one (the base), each composited with what
/// the others have on its area
pub struct RasterPass {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; 13],
    projection: Projection,
    /// The ground's depth, for picking positions in relief
    depth: glow::Program,
    depth_uniforms: [Option<glow::UniformLocation>; 3],
    depth_projection: Projection,
    /// The first stack of the frame (the ground, drawn again by the ground layers and the picking), then the others
    stacks: [Stack; 2],
    /// Stacks drawn this frame
    drawn: usize,
    data: Vec<f32>,
}

impl RasterPass {
    pub fn new(gl: &glow::Context, grid: &Grid) -> Result<Self, String> {
        // Everything created is deleted again if a step fails
        let mut objects = Objects::new(gl);
        let vertex = format!("#version 300 es\n{}{VERTEX}", gl::PROJECT);
        let program = objects.program(&vertex, FRAGMENT)?;
        let depth = objects.program(&vertex, DEPTH_FRAGMENT)?;
        let stacks = [Stack::new(gl, grid, &mut objects)?, Stack::new(gl, grid, &mut objects)?];
        objects.keep();
        Ok(Self {
            program,
            uniforms: gl::uniforms(
                gl,
                program,
                [
                    "u_matrix",
                    "u_count",
                    "u_opacity",
                    "u_saturation",
                    "u_tiles0",
                    "u_tiles1",
                    "u_tiles2",
                    "u_tiles3",
                    "u_heights",
                    "u_relief",
                    "u_eye",
                    "u_fog",
                    "u_fog_range",
                ],
            ),
            projection: Projection::new(gl, program),
            depth,
            depth_uniforms: gl::uniforms(gl, depth, ["u_matrix", "u_heights", "u_relief"]),
            depth_projection: Projection::new(gl, depth),
            stacks,
            drawn: 0,
            data: Vec::new(),
        })
    }

    /// A new frame: its first stack will be the ground
    pub fn begin_frame(&mut self) {
        self.drawn = 0;
        self.stacks[0].last = None;
        self.stacks[0].tiles.clear();
    }

    /// Draws again the ground tiles of this frame (its first stack) that `keep` accepts (by their Mercator bounds),
    /// with the program in use: a ground layer's (it uses `VERTEX`, and has set its uniforms). Returns the
    /// exaggeration of that draw (0: flat), None if nothing was drawn yet
    pub fn redraw(
        &self,
        gl: &glow::Context,
        view: &View,
        keep: impl Fn(glam::DVec2, glam::DVec2) -> bool,
    ) -> Option<f32> {
        let ground = &self.stacks[0];
        let (count, relief) = ground.last?;
        // SAFETY: draws with the vertex array and instances owned by this pass; the pointers are set back after
        unsafe {
            gl.bind_vertex_array(Some(ground.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(ground.instances));
            // Runs of consecutive accepted tiles, each one draw
            let mut run: Option<usize> = None;
            let n = (count as usize).min(ground.tiles.len());
            for i in 0..=n {
                let accepted = i < n && keep(ground.tiles[i].0, ground.tiles[i].1);
                match (run, accepted) {
                    (None, true) => run = Some(i),
                    (Some(first), false) => {
                        Self::point_at(gl, first);
                        Grid::draw(gl, view, (i - first) as i32, relief > 0.0);
                        run = None;
                    }
                    _ => {}
                }
            }
            Self::point_at(gl, 0);
            gl.bind_vertex_array(None);
        }
        Some(relief)
    }

    /// Points the instance attributes at instance `first` (WebGL2 has no base instance)
    unsafe fn point_at(gl: &glow::Context, first: usize) {
        let stride = (INSTANCE * 4) as i32;
        let base = first as i32 * stride;
        // SAFETY: the caller bound the vertex array and the instance buffer
        unsafe {
            for (location, size, offset) in ATTRIBUTES {
                gl.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, base + offset);
            }
        }
    }

    /// Draws the ground of the last frame again (its tiles, `view` being that frame's) into the bound framebuffer,
    /// its depth packed by `DEPTH_FRAGMENT`; depth test on, the caller clears and scissors
    pub fn draw_depth(&self, gl: &glow::Context, view: &View, terrain: &RasterSource) {
        let ground = &self.stacks[0];
        let Some((count, relief)) = ground.last else {
            return;
        };
        let [matrix, heights, relief_u] = &self.depth_uniforms;
        // SAFETY: draw with objects owned by this pass and the terrain's pool
        unsafe {
            gl.use_program(Some(self.depth));
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_1_f32(relief_u.as_ref(), relief);
            self.depth_projection.set(gl, view);
            gl.uniform_1_i32(heights.as_ref(), STACK as i32);
            gl.active_texture(glow::TEXTURE0 + STACK as u32);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(terrain.pool));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(Some(ground.vao));
            Grid::draw(gl, view, count, relief > 0.0);
            gl.bind_vertex_array(None);
        }
    }

    /// One draw call for the whole stack (1 to `STACK` sources, prepared for this frame), in relief over `terrain`
    /// (prepared too) when given: `relief` is its vertical exaggeration. `now`: the time (ms), for the fade-in;
    /// `fog`: color and range of the atmosphere's fog (`Atmosphere::fog`)
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gl: &glow::Context,
        view: &View,
        sources: &mut [&mut RasterSource],
        mut terrain: Option<(&mut RasterSource, f32)>,
        frame: u64,
        now: f64,
        fade_ms: f64,
        fog: Option<([f32; 3], [f32; 2])>,
    ) {
        let n = sources.len().min(STACK);
        if n == 0 {
            return;
        }
        let stack = &mut self.stacks[self.drawn.min(1)];
        self.drawn += 1;
        self.data.clear();
        let tiles = std::mem::take(&mut sources[0].plan.tiles);
        stack.tiles.clear();
        stack.tiles.extend(tiles.iter().map(|t| t.bounds()));
        for &tile in &tiles {
            let (min, max) = tile.bounds();
            let shift = tile.shift(view.center);
            // Plane positions here; the sphere ones are computed on the GPU from the Mercator rect
            let plane = |m: glam::DVec2| (glam::DVec2::new(m.x + shift, m.y) - view.center) * view.world_size;
            let (a, b) = (plane(min), plane(max));
            self.data
                .extend_from_slice(&[a.x as f32, a.y as f32, b.x as f32, b.y as f32]);
            self.data
                .extend_from_slice(&[min.x as f32, min.y as f32, max.x as f32, max.y as f32]);
            for source in sources.iter_mut().take(n) {
                source.sample(tile, frame, now, fade_ms).write(&mut self.data);
            }
            for _ in n..STACK {
                Sample::NONE.write(&mut self.data);
            }
            let heights = terrain
                .as_mut()
                .map_or([0.0, 0.0, 1.0, -1.0], |(source, _)| source.heights(tile, frame));
            self.data.extend_from_slice(&heights);
        }
        sources[0].plan.tiles = tiles;
        let mut opacity = [0f32; STACK];
        let mut saturation = [1f32; STACK];
        for (k, source) in sources.iter().take(n).enumerate() {
            opacity[k] = source.options.opacity;
            saturation[k] = source.options.saturation;
        }
        let count = (self.data.len() / INSTANCE) as i32;
        let relief = terrain.as_ref().map_or(0.0, |(_, exaggeration)| *exaggeration);
        stack.last = Some((count, relief));
        // SAFETY: draw with objects owned by this pass and the sources' pools
        unsafe {
            gl.use_program(Some(self.program));
            let [
                matrix,
                count_u,
                opacity_u,
                saturation_u,
                t0,
                t1,
                t2,
                t3,
                heights_u,
                relief_u,
                eye,
                fog_u,
                fog_range,
            ] = &self.uniforms;
            let ([r, g, b], [start, end]) = fog.unwrap_or(([0.0; 3], [0.0; 2]));
            gl.uniform_3_f32(eye.as_ref(), view.eye.x as f32, view.eye.y as f32, view.eye.z as f32);
            gl.uniform_3_f32(fog_u.as_ref(), r, g, b);
            gl.uniform_2_f32(fog_range.as_ref(), start, end);
            gl.uniform_matrix_4_f32_slice(matrix.as_ref(), false, &gl::mat4(&view.view_proj));
            gl.uniform_1_i32(count_u.as_ref(), n as i32);
            gl.uniform_1_f32_slice(opacity_u.as_ref(), &opacity);
            gl.uniform_1_f32_slice(saturation_u.as_ref(), &saturation);
            gl.uniform_1_f32(relief_u.as_ref(), relief);
            self.projection.set(gl, view);
            for (k, unit) in [t0, t1, t2, t3].into_iter().enumerate() {
                gl.uniform_1_i32(unit.as_ref(), k as i32);
                gl.active_texture(glow::TEXTURE0 + k as u32);
                // Unused units get the base pool, so every sampler has a valid texture
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(sources[if k < n { k } else { 0 }].pool));
            }
            // Without relief the elevation unit stays empty: the shader never reads it
            gl.uniform_1_i32(heights_u.as_ref(), STACK as i32);
            gl.active_texture(glow::TEXTURE0 + STACK as u32);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, terrain.as_ref().map(|(source, _)| source.pool));
            gl.active_texture(glow::TEXTURE0);
            // The shader composites the stack and outputs premultiplied alpha
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            let lifted = relief > 0.0;
            if lifted {
                // In relief the ground hides itself (hills) and the 3D layers drawn after; the skirts hang either side
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LEQUAL);
                gl.depth_mask(true);
                gl.disable(glow::CULL_FACE);
            }
            gl.bind_vertex_array(Some(stack.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(stack.instances));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, gl::bytes(&self.data), glow::STREAM_DRAW);
            Grid::draw(gl, view, count, lifted);
            if lifted {
                gl.disable(glow::DEPTH_TEST);
                gl.enable(glow::CULL_FACE);
            }
            gl.bind_vertex_array(None);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
        }
    }

    /// Frees the GPU objects of the pass
    pub fn delete(&self, gl: &glow::Context) {
        // SAFETY: objects owned by this pass, not used afterwards
        unsafe {
            gl.delete_program(self.program);
            gl.delete_program(self.depth);
            for stack in &self.stacks {
                gl.delete_vertex_array(stack.vao);
                gl.delete_buffer(stack.instances);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_attributes_cover_the_instance() {
        // The heights come last, at the end of the instance
        let (location, size, offset) = ATTRIBUTES[ATTRIBUTES.len() - 1];
        assert_eq!((location, size), (15, 4));
        assert_eq!(offset as usize + 16, INSTANCE * 4);
        // Locations 1 to 15, each once
        let mut locations: Vec<u32> = ATTRIBUTES.iter().map(|a| a.0).collect();
        locations.sort_unstable();
        assert_eq!(locations, (1..=15).collect::<Vec<_>>());
    }
}
