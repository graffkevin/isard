# Performance

Isard is built for maps where everything changes at every mouse move. What makes it fast:

- **Geometry uploaded once, only the view drawn.** A network is cut into pieces sorted by square of a 256 × 256 grid;
  each frame draws only the squares in view (a quadtree descent against the frustum, a few draw calls for runs of
  squares). Of 2 million pieces around Paris, about 18,000 are drawn at zoom 14.
- **Values in textures.** New values are one upload of 4 bytes per node (or per feature); colors, ramps, widths,
  filters are read by the shaders. Restyling never touches the geometry.
- **Worked out per pixel.** Along an edge with costs, a point takes the quickest way there, min(t(A) + cost from A,
  t(B) + cost from B): an edge reached from both ends shows where the two fronts meet.
- **One draw call for the ground.** The raster layers of a stack (basemap, hillshade…) share texture arrays and are
  composited in one fragment shader, each pixel shaded once.
- **Frames on demand.** Nothing runs while idle; the hover listener runs in the frame it draws.
- **Work off the main thread.** Images decoded by the browser, 3D Tiles decoded in workers (Draco, meshopt, KTX2),
  vector tiles drawn in workers on CPU canvases (no read-back on the main thread at the upload).
- **No holes, no backlog.** Missing tiles covered by their nearest ancestor or descendants, coarse tiles first, stale
  requests aborted, transient errors retried, arrivals uploaded within a budget per frame, foveated loading and skipped
  levels during flights.

## Measured

geo-reach, an accessibility map of the whole Île-de-France (13 regions of networks with transit), moved from MapLibre
to Isard on an Intel UHD (TGL GT1), Chromium, 1280 × 800:

| | MapLibre | Isard |
|---|---|---|
| Ready | 8.5 – 9.9 s | 6.7 – 8.4 s |
| Hover sweep, frames over 33 ms | 5 – 7 % | 2.7 – 8 % |
| Hover sweep, 95th percentile | 30 – 37 ms | 18 – 35 ms |
| JavaScript downloaded | 1.25 MB + 508 KB worker | 285 KB + 654 KB of WebAssembly |

The repository's benchmark (`/bench/`) plays the same flight and orbit over the Lille photomesh with Isard, iTowns
and 3DTilesRendererJS.
