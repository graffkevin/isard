//! The glyph and icon atlases an engine shares between its symbol layers, and the calls of the page that allocate
//! and fill them.

use crate::engine::Engine;
use crate::render::gl;
use glow::HasContext;
use wasm_bindgen::prelude::*;

/// The glyph atlas of an engine: signed distances, one byte per texel, in pages of one size (a texture array)
pub struct GlyphAtlas {
    /// The texture array of the pages (R8)
    pub texture: glow::Texture,
    /// Size of a page
    pub size: (i32, i32),
    /// Pages in use
    pub pages: i32,
}

/// Most pages of the glyph atlas (the least `MAX_ARRAY_TEXTURE_LAYERS` of WebGL2)
const MAX_PAGES: i32 = 256;

/// The icon atlas of an engine: RGBA, premultiplied (distance field icons: the distance in alpha)
pub struct IconAtlas {
    /// The texture (RGBA8)
    pub texture: glow::Texture,
    /// Its size, pixels
    pub size: (i32, i32),
}

#[wasm_bindgen]
impl Engine {
    /// The glyph atlas shared by the symbol layers, one page: `width` × `height` bytes of signed distances
    /// (allocated again, filled with `data` if given)
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) as `setGlyphPages` does: a size that is not positive, or the texture not created
    #[wasm_bindgen(js_name = setGlyphAtlas)]
    pub fn set_glyph_atlas(&mut self, width: i32, height: i32, data: Option<Box<[u8]>>) -> Result<(), JsValue> {
        self.set_glyph_pages(width, height, 1)?;
        if let Some(data) = data {
            self.update_glyph_page(0, 0, 0, width, height, &data);
        }

        Ok(())
    }

    /// The glyph atlas shared by the symbol layers, `pages` pages of `width` × `height` bytes of signed distances,
    /// allocated again and empty (`updateGlyphPage` fills them). A glyph on page `p` at row `v` has the atlas
    /// coordinate `v + p × height`
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when `width` or `height` is not positive (or their product overflows), `pages`
    /// is not 1 to 256, or the atlas texture cannot be created
    #[wasm_bindgen(js_name = setGlyphPages)]
    pub fn set_glyph_pages(&mut self, width: i32, height: i32, pages: i32) -> Result<(), JsValue> {
        gl::area(width, height)
            .filter(|_| width > 0 && height > 0 && (1..=MAX_PAGES).contains(&pages))
            .ok_or("glyph atlas: a positive size and 1 to 256 pages expected")?;
        // SAFETY: (re)allocation of the atlas on the live context
        unsafe {
            let gl = &self.gl;

            if let Some(old) = self.glyphs.take() {
                gl.delete_texture(old.texture);
            }

            let texture = gl.create_texture()?;

            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(texture));
            gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY, 1, glow::R8, width, height, pages);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, key, value as i32);
            }
            self.glyphs = Some(GlyphAtlas {
                texture,
                size: (width, height),
                pages,
            });
        }

        Ok(())
    }

    /// The icon atlas shared by the symbol layers: `width` × `height` RGBA texels, premultiplied, allocated again
    /// and empty (`updateIconAtlas` fills it)
    ///
    /// # Errors
    ///
    /// Throws (to JavaScript) when `width` or `height` is not positive (or their product overflows), or the
    /// atlas texture cannot be created
    #[wasm_bindgen(js_name = setIconAtlas)]
    pub fn set_icon_atlas(&mut self, width: i32, height: i32) -> Result<(), JsValue> {
        gl::area(width, height)
            .filter(|_| width > 0 && height > 0)
            .ok_or("icon atlas: a positive size expected")?;
        // SAFETY: (re)allocation of the atlas on the live context
        unsafe {
            let gl = &self.gl;

            if let Some(old) = self.icons.take() {
                gl.delete_texture(old.texture);
            }

            let texture = gl.create_texture()?;

            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, width, height);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, key, value as i32);
            }
            self.icons = Some(IconAtlas {
                texture,
                size: (width, height),
            });
        }

        Ok(())
    }

    /// A region of the icon atlas: `width` × `height` RGBA texels (premultiplied) at (`x`, `y`)
    #[wasm_bindgen(js_name = updateIconAtlas)]
    pub fn update_icon_atlas(&mut self, x: i32, y: i32, width: i32, height: i32, data: &[u8]) {
        let Some(atlas) = &self.icons else {
            return;
        };
        let Some(len) = gl::area(width, height).and_then(|a| a.checked_mul(4)) else {
            return;
        };

        if data.len() < len || !gl::region_fits(x, y, width, height, atlas.size) {
            return;
        }

        // SAFETY: upload inside the atlas
        unsafe {
            let gl = &self.gl;

            gl.bind_texture(glow::TEXTURE_2D, Some(atlas.texture));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                x,
                y,
                width,
                height,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data[..len])),
            );
        }
    }

    /// A region of the first page of the glyph atlas: `width` × `height` bytes at (`x`, `y`)
    #[wasm_bindgen(js_name = updateGlyphAtlas)]
    pub fn update_glyph_atlas(&mut self, x: i32, y: i32, width: i32, height: i32, data: &[u8]) {
        self.update_glyph_page(0, x, y, width, height, data);
    }

    /// A region of a page of the glyph atlas: `width` × `height` bytes at (`x`, `y`) of the page
    #[wasm_bindgen(js_name = updateGlyphPage)]
    pub fn update_glyph_page(&mut self, page: i32, x: i32, y: i32, width: i32, height: i32, data: &[u8]) {
        let Some(atlas) = &self.glyphs else {
            return;
        };
        let Some(len) = gl::area(width, height) else {
            return;
        };

        if data.len() < len || !(0..atlas.pages).contains(&page) || !gl::region_fits(x, y, width, height, atlas.size) {
            return;
        }

        // SAFETY: upload inside a page of the atlas
        unsafe {
            let gl = &self.gl;

            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(atlas.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_3d(
                glow::TEXTURE_2D_ARRAY,
                0,
                x,
                y,
                page,
                width,
                height,
                1,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data[..len])),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        }
    }
}
