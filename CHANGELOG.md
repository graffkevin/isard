# Changelog

Every notable change of `isard` (published as `@ign-junn/isard` up to 0.1.1). The format follows [Keep a Changelog](https://keepachangelog.com), the
versions [Semantic Versioning](https://semver.org) (before 1.0.0, a minor version may change an API).

## [0.2.0] - 2026-10-18

### Changed

- The package is renamed from `@ign-junn/isard` to `isard` (`npm install isard`, `import { GeoMap } from 'isard'`);
  `@ign-junn/isard` is deprecated. The project is independent of the IGN; the Géoplateforme's data stays supported, as
  one source among others.
- The default basemap is OpenFreeMap's Positron (`'osm-positron'`: OpenStreetMap data, worldwide, free, no key), with
  `'osm-liberty'` and `'osm-bright'`. The Géoplateforme's basemaps are named `'ign-plan'`, `'ign-plan-grey'`,
  `'ign-ortho'`, `'ign-plan-vector'` and `'ign-plan-grey-vector'`; their former names (`LegacyBasemap`) are deprecated,
  still accepted with a warning. `vectorTiles()` takes Positron by default (`OPENFREEMAP_STYLES`).
- No IGN hillshade by default (`hillshade` defaults to 0).
- `terrain: true` on the Earth is the worldwide Terrain Tiles of AWS's open data registry (Terrarium PNGs decoded in
  the page, the sea floor at sea level); the IGN's model is `terrain: 'ign'` (or `{ model: 'ign' }`), and only it is
  raised by the RAF20 geoid by default (`geoid` defaults to false for the other models). The Earth's relief range is
  its dry land, -450 to 8900 m (the prisms of draped fills reach above Everest); the Moon's relief starts at level 3.
- Labels use OpenFreeMap's fonts by default (`GLYPHS_URL`, `DEFAULT_FONT`: Noto Sans Regular, Bold, Italic); a vector
  basemap's labels take the glyphs of its style.
- The attribution control credits the sources in use, following them: OpenFreeMap, OpenMapTiles and OpenStreetMap,
  the IGN only for the Géoplateforme's data, the Terrain Tiles (`OPENFREEMAP_ATTRIBUTION`, `IGN_ATTRIBUTION`,
  `TERRAIN_TILES_ATTRIBUTION`; `DEFAULT_ATTRIBUTION` deprecated).
- Licence: © 2026 Kevin Graff.

### Added

- The Géoplateforme's data in one call: `geoplateformeWmts`, `geoplateformeWms` (raster options of any of its layers,
  crediting the IGN) and `geoplateformeWfs` (its features as GeoJSON); a guide page, "Géoplateforme (IGN) data".
- `RasterOptions.attribution`: a raster's credits, shown while it is on the map; `VectorTilesOptions.attribution`.
- Relief tiles of PNG encodings: `terrain.encoding`, `'terrarium'` or `'mapbox'` (Terrain-RGB), decoded in the page.
- Vector basemaps: the raster layers of a style (its shaded relief), extrusions drawn flat, `line-gap-width`, text
  sizes of zoom expressions; a text written over its icon (a road shield's number) is left out, icons not being drawn.

- 3D Tiles: implicit tiling (quadtree and octree, binary or JSON subtrees), several contents per tile, sparse and
  zero-filled accessors, `texCoord` sets, content recognized by its bytes; horizon culling on the globe.
- Style expressions of vector basemaps: arithmetic, `format`, conversions, `coalesce`, `length`, `at`… (an unknown
  operator is warned about once and evaluates to `null`).
- Every CSS color wherever a color is given (`#rgb`, `rgb()`, `hsl()`, named colors).
- Exported types used by the API: `AnimationOptions`, `FlyToOptions`, `AtmosphereOptions`, `DecoderUrls`,
  `LabelCandidate`, `Json`, `TileKey`; an API reference in the documentation.
- `hoverfeature`: what is under the pointer, picked without stalling the GPU (`hoverPicking` for its radius and
  layers); `PickResult`, `FeatureHit`, `MapHoverFeatureEvent` exported.
- Labels never run out of glyph space: the atlas grows into pages.
- Keyboard navigation (arrows, +/-, Shift to rotate and pitch, 0 to reset), `keyboard` and `ariaLabel` options, a
  focusable map with a visible focus.
- Maps of other bodies than the Earth: the `body` option, `'moon'` (a sphere of 1 737 400 m, no atmosphere) or any
  ellipsoid (`BodyOptions`: radius, flattening, atmosphere, color); the metres of the camera, relief, buildings and
  3D Tiles follow it. Off the Earth, none of the Géoplateforme defaults (basemap, hillshade, IGN relief, geoid, credits)
  and a black background; `AttributionControl` without the IGN credits. A `moon` example (NASA's LRO mosaic).
- Mars: `body: 'mars'` (a sphere of its equatorial radius, 3 396 190 m, for the planetocentric latitudes of its data)
  with a thin dusty halo. The halo is the body's: `BodyOptions.atmosphere` takes a CSS color (its alpha the strength)
  besides true (the Earth's blue) and false. A `mars` example (the Viking colour mosaic of NASA's Mars Trek).
- Raster tiles in longitudes and latitudes (`grid: 'equirectangular'`, the grid of NASA's Trek and of WorldCRS84Quad),
  reprojected in the page into Web Mercator tiles.
- Relief of the Moon and Mars: `terrain: true` there takes NASA's terrain models from the image services of its Trek
  (LOLA, 237 m; MOLA, 463 m), asked for raw floats in the body's Mercator. `terrain.url` takes an ArcGIS image service
  (`…/ImageServer`) on any body, and `{bbox}` is in the body's Mercator metres. `BodyOptions.relief` (its lowest and
  highest ground) bounds what the relief may raise into view, the prisms of draped fills, the no-data and the horizon;
  the relief starts at a coarser level on a body of a larger relief for its size (3 on the Moon and Mars). The
  `moon` and `mars` examples have relief and pitched views (Tycho, Copernicus, Olympus Mons, Valles Marineris).

### Fixed

- Layers whose first level is coarse for the view (the relief of the Moon and Mars) load under a pitched camera on the
  globe: the tile under the view center no longer measures too small to refine to it.
- A listener that throws no longer stops the map from drawing; a failed engine download can be retried.
- Workers that fail to load reject their requests with an explanation instead of leaving them pending.
- 3D Tiles content and network arrays that are malformed are errors instead of a crash of the engine; a tileset whose
  root fails no longer prevents `idle`, and is retried.
- Negative integer properties of vector tiles; vector tiles spread evenly over the workers.
- Destroying a map stops its downloads and workers and frees its GPU memory; the device pixel ratio is followed.
- Lines, circles, labels and networks no longer jitter at high zoom over large layers.
- Circles and labels past the horizon of the globe are hidden instead of showing through it.

### Changed

- Fewer GPU readbacks (zoom anchored on the relief from the CPU), labels placed only when something moved, layers
  out of view not drawn, an LRU for raster tiles.
- The vector basemap and the vector tiles of a map share one pool of workers and their decoded tiles; raster, vector
  and 3D Tiles near the center of the view are fetched first (fetch priority).
- Circles and fills drawn by the squares in view only.
- The package ships its modules as they are (an application's bundler keeps what it uses), the workers and the
  engine next to them; releases are published from the Release workflow, with provenance.
- A map without 3D Tiles downloads a third less WebAssembly: maps load the engine without 3D Tiles
  (`isard_core_bg.wasm`, 220 KB gzipped instead of 330 KB) and the full one for their first 3D Tiles layer only
  (`tiles3d: true` to load it from the start, `coreWasmUrl` for bundlers). Given `wasmUrl` alone, a map loads the full
  engine from the start, as before.

## [0.1.1] - 2026-10-16

### Fixed

- Vector basemaps: the style's sprite is downloaded once, and only for the layers drawing fill patterns (it was by
  every worker of every renderer); a vector tile always goes to the same worker, which keeps it decoded for all the
  raster tiles drawn from it.

## [0.1.0] - 2026-10-16

First published version.

### Added

- `GeoMap`: Géoplateforme basemaps (PLAN.IGN raster or vector, ortho), hillshade and relief (RGE ALTI, exaggeration,
  geoid optional), globe and Web Mercator, camera of MapLibre's conventions (`easeTo`, `flyTo`, pitch up to 85 with a
  sky and ground fog), events (`move`, `moveend`, `hover` once per frame, `click`, `idle`…), picking within a radius.
- Layers: rasters (fetched or drawn by the page), networks colored by a value per node, GeoJSON lines, circles, fills
  (on the relief at any size) and labels (Géoplateforme fonts, shared collisions), buildings (values, or a network's
  times), ground shaders (the application's GLSL over the ground), 3D Tiles (b3dm, glb, Draco, meshopt, KTX2).
- Vector basemaps from MapLibre styles, drawn in workers into the raster pool, their labels above the other layers.
- Markers, popups, navigation and attribution controls.
