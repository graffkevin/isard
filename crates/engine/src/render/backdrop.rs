//! What is drawn behind and under the tiles, before them (`Engine::draw_backdrop`), only in the globe projection
//! (fading out with it) except the plane's atmosphere.
//!
//! - `globe`: the plain sphere under the tiles (polar caps, the globe's outline while tiles load).
//! - `sky`: space behind the globe (sky, stars, atmosphere halo), and the atmosphere's sky and fog over the plane.

pub(crate) mod globe;
pub(crate) mod sky;
