# Ground shaders

Your fragment shader, run over the ground itself: the ground tiles the basemap just drew are drawn once more with it,
so it lies on the relief exactly as the basemap does, at any tilt, one fragment per pixel of ground where it shows.
For fields the application computes and the GPU draws: water depth from a terrain grid and levels per river section,
heat maps, distances.

```ts
const water = map.addGround({
  anchor: [x0Lon, y0Lat],                       // the offsets are taken from here
  bounds: [west, south, east, north],           // only the ground there is drawn again
  code: `
    uniform highp usampler2D u_terrain;         // cm, R16UI
    uniform float u_level;
    uniform float u_cell;
    vec4 ground(vec2 offset) {
      ivec2 cell = ivec2(floor(offset * ISARD_CIRCUMFERENCE / u_cell));
      float depth = u_level - float(texelFetch(u_terrain, cell, 0).r) / 100.0;
      if (depth <= 0.0) return vec4(0.0);
      float a = mix(0.6, 0.9, clamp(depth / 3.0, 0.0, 1.0));
      return vec4(vec3(0.1, 0.4, 0.8) * a, a);  // premultiplied alpha
    }`,
  textures: { u_terrain: { format: 'r16ui', width, height, data: terrain } },
  uniforms: { u_level: 12, u_cell: cellMercatorMetres },
});
water.setUniform('u_level', 13.5);
water.updateTexture('u_levels', levels, { width: 1024, height: rows }); // a region, a few KB per step
```

- `offset` is the Mercator offset of the pixel from `anchor`, exact to the millimetre at any zoom (worked out from the
  position relative to the camera, never from absolute f32 coordinates). Times `ISARD_CIRCUMFERENCE`: metres of EPSG:3857.
- Textures in any format: `r8`, `r8ui`, `r16ui`, `r16i`, `r32ui`, `r32f`, `rg16ui`, `rg32f`, `rgba8`, `rgba8ui`,
  `rgba16ui`, `rgba32f`; 2D or 2D arrays (`depth`). Integer ones are read with `texelFetch`.
- Uniforms by name: numbers for any type. Also available: `u_zoom`, `u_pixel_ratio`, `fwidth` for pixel-wide lines.
- Several ground layers with bounds (finer grids where they exist) draw only the tiles they meet.

<LiveExample slug="relief" height="440px" />
