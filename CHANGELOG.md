# Changelog

Every notable change of `isard` (published as `@ign-junn/isard` up to 0.1.1). The format follows [Keep a Changelog](https://keepachangelog.com), the
versions [Semantic Versioning](https://semver.org) (before 1.0.0, a minor version may change an API).

## [Unreleased]

### Added

- `setRequestHeaders(prefix, headers)`: headers added to every request isard makes to the URLs starting with a prefix
  (an access token for a private Géoplateforme stream, a server behind single sign-on), from the page, its workers
  (vector tiles, 3D Tiles, point clouds) and its engine (raster tiles, tilesets): for the whole page, again with a
  new token when it is renewed, without headers to stop.
- A color for 3D Tiles (`color` in `addTiles3d` and `setStyle`): it tints the textures of the meshes and colors the
  untextured ones (light grey without it).
- 3D Tiles features picked (`pickFeature`, 'hoverfeature'): a building of a 3D Tiles layer comes with its
  properties, from the tile's `EXT_mesh_features` feature IDs and `EXT_structural_metadata` property table (3D Tiles
  1.1: scalars, booleans, strings, enums) or a b3dm's `_BATCHID` and batch table (1.0, its JSON arrays); the
  Bâtiments 3D of the IGN give their BD TOPO attributes (cleabs, hauteur, nature, usage…).

## [0.7.0] - 2026-10-20

### Added

- A height above the ground for markers and popups (`height` option, `setHeight(height, base)`) and for
  `map.project`: a raised marker has a stem down to what it stands on (`base` metres: a roof, a mesh; the ground by
  default), its popup raised with it. The viewshed example: each pin at
  its observer's eye, the cone's tilt and vertical field of view in its popup (120° by default, as an eye rather
  than a camera: the ground in front of a raised observer is in view).
- `map.pickSurface(point)`: what is drawn under a screen point, on the relief or standing on it (a roof, a wall, a
  mesh), its position and height above the relief; `map.surfaceAt(lonLat)`: the height of the top of what stands at
  a position, the scene drawn from above into one pixel (no jump across the edges of roofs, as the camera's ray
  would give).
- The viewshed's cone in 3D (`coneColor`): its fan through the axis, at the eye's height and tilted with it, from
  the eye to the range, solid where nothing hides it and faint behind the scene. The viewshed examples no longer
  draw the cone's edges on the ground (hidden under a mesh, wrong for a raised or tilted cone), and have an opacity
  slider. Their observers stand 1 m above what they stand on by default (0.1 m steps), the stem of the pin
  down to that surface; a dragged pin follows the pointer at once, what it stands on and the viewshed a few times a
  second and at the release.
- The viewshed on a mesh example: the textured mesh of Strasbourg (Eurométropole, 2022, 3D Tiles) around the
  cathedral, its walls and roofs hiding and tinted, an observer clicked or dragged onto the mesh standing on it; the
  observers of both viewshed examples in one shared module.

### Changed

- The viewshed's colors are multiplied into the scene instead of laid over it: textures and shading stay under the
  green and the red (a mesh no longer looks veiled). `opacity` sets how strongly they tint, 0.7 by default.

### Fixed

- 3D Tiles past their GPU memory budget no longer load without end: the tiles of the view that were not drawn yet
  (loaded, waiting for their siblings, or parents of drawn tiles) were unloaded first, then asked again, and so on
  (about 400 requests and 180 frames every 10 s over the Strasbourg mesh at zoom 18, pitch 70). The tiles of the view
  are now kept, and nothing more is asked past the budget: the view settles (in 20 s there), parents standing in.
- The viewshed over 3D Tiles still loading: its depth passes drew the finest tiles only, not the coarser ones standing
  in for those missing, and saw the relief under the mesh there (a tint floating beside the mesh). Its depth targets
  now have a stencil, and the meshes are drawn into them as the frame draws them.
- A view tilted to the horizon over 512 px tiles (the vector basemaps) no longer flickers without end: it wanted
  more tiles than the pool holds (105 at 63°, 193 at 70°, for 96 slots), each upload evicting a tile still drawn
  (the coarsest first, the far ones and their stand-ins), asked again, and so on: blank rectangles, tiles of the
  wrong level, a map never idle. The tiles a frame draws and their stand-ins are now held before its uploads, and
  only as many tiles are asked for as there is room left (the farthest wait, drawn with their stand-ins).
- Fewer tiles toward the horizon of a tilted view: past the screen center's distance, a tile may stay larger on
  screen the farther it is (up to 4 times), seen at a grazing angle and in the fog: about half as many tiles at
  70° (106 for the vector basemap over Montmartre, 193 before), 13 of them drawn with stand-ins instead of about
  100. The views from above do not change.
- The labels of a tilted vector basemap follow the levels of its ground tiles: far away, those of lower zooms
  (fewer, bigger names) instead of the near ones piled up at the horizon; label tiles that failed are asked again
  after 3 s.
- `geoplateformeWfs` past 5000 features: the WFS returns 5000 per request, the rest of the box was missing (holes
  among the buildings of the viewshed example, 7015 of them). The pages are asked together after the first, which
  tells how many there are; `count` (default 50 000) caps the whole.

## [0.6.1] - 2026-10-20

### Added

- `GeoMap.setViewshed` takes several observers (16 at most): what any of them sees is seen, what is hidden from all
  those that reach it is hidden. The viewshed example: a click on the map adds an observer, a click on its pin opens
  its settings in a popup (one open at a time), a button removes it or all of them; on a building, the observer
  stands on its roof.

## [0.6.0] - 2026-10-19

### Added

- `GeoMap.setViewshed` (a prototype): what can be seen from a point within a range, tinted over the map, and what is
  hidden. The relief, extrusions, 3D Tiles and models are drawn from the observer into a depth cube, as for the
  shadows of a point light, and each pixel on screen compares its distance to the observer with it. All around, or
  one way only as a camera: a cone given by a direction, a tilt and angles of view. Only what the map loaded for its
  view hides anything. The viewshed example: from the Sacré-Cœur over Montmartre's buildings.
- The QGIS plugin "isard 3D view" (`integrations/qgis`, QGIS 3.28 to 4, `bun run qgis:package` → an archive for
  "Install from ZIP"): the viewer in a panel (Qt WebEngine) or the browser, served by a local server; the camera
  following the map canvas (and the other way round), vector layers sent as styled GeoJSON (extruded by a field),
  XYZ, WMS, WMTS, MBTiles, vector tiles and COPC point clouds, clicks selecting the features in QGIS.
- PMTiles (v3) archives read by range requests, without a tile server: `map.addPmtiles(url)` (a raster layer, or
  vector tiles drawn with a style, by default `basicStyle` of the archive's layers), `addRaster({ url: 'pmtiles://…' })`,
  and `pmtiles://` sources in a style (as MapLibre's protocol names them), their tiles read in the basemap workers.
  Directories cached per archive, gzip through `DecompressionStream`, `blob:` URLs; the reader exported (`PMTiles`,
  `zxyToTileId`, `tileIdToZxy`).
- The viewer (`apps/viewer`, built into `apps/viewer/dist`, not in the package): a full-window map for local files
  dropped or opened (GeoJSON, KML, GPX, TopoJSON, COPC, PMTiles, glTF), a list of layers, and a bridge other
  applications drive with JSON messages (Server-Sent Events and POST, `postMessage`, `window.isardViewer`): camera,
  basemap, relief, layers, selection to the viewer; ready, moves, clicks with the feature picked, errors back. Its
  local server, `bun scripts/viewer.ts [files…] [--port] [--open]` (`bun run viewer`): the viewer, the bridge with
  its state replayed, the files given with HTTP ranges, MBTiles as XYZ tiles (bun:sqlite), the files added as layers
  on startup; 127.0.0.1 only, other hosts and origins refused. The guide's page "The viewer" describes the protocol.

### Changed

- The package's modules grow by about 5.5 KB gzipped (comments included; in a bundle only when used) and the basemap
  worker by 1.6 KB, to 12.9 KB: the PMTiles reader.

## [0.5.1] - 2026-10-19

### Fixed

- A tilted globe stays a sphere longer: the blend into the Mercator plane is put off by how much farther than its
  center the camera sees, up to three levels (zoom 10 for a view along the horizon). Pitched views at zooms 5 to 7
  showed a flat ground up to a straight horizon, where the Mercator plane stretched the far ground towards the poles
  (Mars's northern ice cap behind Olympus Mons).
- A globe seen from close and pitched no longer shows only its base: the tiles of its first two levels are always
  split instead of being culled by their samples (a quarter of the world apart, they could all lie out of the
  frustum or of the cap the camera sees), and a tile's point nearest to the one under the camera is tested too.
- Finer relief arriving under a tilted globe no longer shakes the view (zooms 8 to 10, pitched): the eye stays still
  as on the plane, where it was only kept off the globe.
- While the globe blends into the plane, the ground under the cursor is the one drawn there (clicks, `ground`,
  dragging and zooming around the cursor): it was taken on the sphere or on the plane, whichever was nearer, tens of
  pixels off towards the edges of a pitched view.
- Under relief, a canyon or a basin at the horizon of the globe shows the halo beyond instead of the globe's base
  colour (Valles Marineris seen along its length): the base and the sky's sphere lie as deep as the body's lowest
  ground.
- The stars behind the globe no longer twinkle as it turns: a star is drawn at least a pixel wide, its light spread
  over it (narrower, it lit only the pixels whose centre it crossed), and a layer of stars too fine for the screen
  fades out.

### Added

- The globe example goes to the Moon and Mars with its arrows (a new map for each body).

## [0.5.0] - 2026-10-19

### Added

- Entry points by theme: `isard/formats` (KML, GPX, TopoJSON), `isard/projections` (CRSs, reprojection, WMTS grids)
  and `isard/geoplateforme` (the IGN's WMTS, WMS, WFS, the Lambert-93 grid); `isard` still exports all of them.
- Point clouds, `map.addPointCloud({ url })` → `PointCloudLayer` (`setStyle`, `stats`, `info`, `remove`): COPC files
  (LAZ 1.4 point formats 6 to 8 with their octree, read by HTTP range requests: the IGN's LiDAR HD), and 3D Tiles
  point clouds (`format: '3dtiles'`, the default for a `.json` URL). The octree traversed by screen-space error within
  a point budget (`pointBudget`, 3 million), the nearest, coarsest and most central nodes first, their downloads
  aborted once out of the selection, a GPU memory budget; colored by RGB, class (the ASPRS and LiDAR HD palette,
  `classifications` to recolor or hide classes), altitude (a ramp) or intensity; points of a fixed size or sized by
  the spacing of their level, round or square; Eye-Dome Lighting (`edl`); depth shared with the relief, the buildings
  and the 3D Tiles; `pickFeature` gives a point with its class, intensity, altitude, returns and color, read back from
  the GPU.
- COPC positions reprojected as they are decoded, in the workers: the CRS from the file's WKT (built-in CRSs, or
  `registerCrs`) or `crs` (any proj4 CRS), computed exactly on a grid of each node and interpolated within a
  millimetre; NGF-IGN69 altitudes (Lambert-93, the CC zones; `geoid`) raised by the RAF20 geoid height onto the
  ellipsoid of the relief; `verticalScale` (the WKT's vertical unit by default).
- The point cloud module, `isard_points_bg.wasm` (43 KB gzipped, the LAZ decoder of the `laz` crate): loaded by the
  decoding workers with the first COPC chunk, the engines do not hold it (`pointsWasmUrl` when a bundler moves it).
- Requests held to a rate (`requestsPerSecond`, 8 a second for the Géoplateforme), answers `429` asked again after a
  pause.
- 3D Tiles: `.pnts` tiles (positions plain or quantized, RGB, RGBA, RGB565, constant colors, normals plain or
  oct-encoded, `3DTILES_draco_point_compression`) and glTF points (mode 0, meshopt, quantization, Draco), lit by their
  normals; `pointSize` in the style of a 3D Tiles layer.
- The point clouds example: Notre-Dame de Paris in the IGN's LiDAR HD; the Géoplateforme guide on LiDAR HD.

### Changed

- The engine with 3D Tiles is 385 KB gzipped (was 345 KB): the point clouds (their program, the COPC layer, the pnts
  and glTF points); the engine without 3D Tiles does not change.

## [0.4.0] - 2026-10-18

### Added

- Icons in the symbol layers: a vector basemap's icons from the `sprite` of its style (one sprite or several with
  their ids, `@2x` on high-density screens), points of interest, road shields with their numbers centred, one-way
  arrows every `symbol-spacing` along the roads; `icon-image` (tokens and expressions), `icon-size`, `icon-anchor`,
  `icon-offset`, `icon-rotate`, `icon-rotation-alignment`, `icon-allow-overlap`, `icon-ignore-placement`,
  `icon-optional`, `text-optional`, `text-ignore-placement`, `icon-text-fit` with its padding (an image's `stretchX`,
  `stretchY` and `content` kept), `icon-color` of signed distance field images, `icon-opacity`. The text and the icon
  of a label have their own boxes in the collision grid. The images used are packed into one RGBA atlas.
- `map.addImage(name, image, { pixelRatio, sdf, stretchX, stretchY, content })`, `hasImage`, `removeImage`, and the
  symbol layers' `icon`, `iconSize`, `iconAnchor`, `iconOffset`, `iconRotate`, `iconRotationAlignment`, `iconOverlap`,
  `iconOptional`, `iconTextFit`, `iconTextFitPadding`, `iconColor`, `iconOpacity` and `textOptional`; `overlap` also
  takes `'allow'` and `'ignore'`; `placement` may be a function of the feature.
- Custom layers: `map.addCustomLayer({ renderingMode, onAdd, render, onRemove })`, the page's own WebGL drawing (raw,
  or a three.js scene) called at its place among the layers in every frame, as MapLibre's; `frame.matrixAt(lonLat,
  altitude)` gives the model-view-projection of a local east-north-up frame in metres there, made in f64 relative to
  the camera (exact at any zoom, on the plane and the globe); `'3d'` shares the depth buffer of the relief, the
  buildings and the 3D Tiles; the WebGL state the page changes is set back after it; `onAdd` again after a context
  loss. The `threejs` example.
- glTF models: `map.addModels({ url | data, instances: [{ lonLat, altitude, heading, pitch, roll, scale }],
  altitudeReference })`, a glTF binary drawn at positions in one instanced draw per primitive, decoded as 3D Tiles
  content is (meshopt, quantization, Draco and KTX2 in the workers), standing on the relief by default, with depth;
  `setInstances`; `pickFeature` gives the instance's index. Textured surfaces unlit as the 3D Tiles, the others in
  their material's base color, lit from above. The first model loads the engine with 3D Tiles, as a 3D Tiles layer
  does. The `models` example.

### Changed

- A vector basemap's text written over its icon (a road shield's number) is drawn with it, no longer left out;
  `text-rotation-alignment: viewport` keeps a text along a line horizontal; `symbol-placement` may be an expression;
  past the source's deepest level, the labels take the style's layers of the view's zoom (points of interest at 17).
- The decoder keeps the base color factor of a glTF material (`color` of the decoded primitives, when not white).

## [0.3.0] - 2026-10-18

### Changed

- Fill layers share their vertices (the caps index the points of the rings, the walls stand on them) and build the
  prism of the relief only on their first frame in relief: 100,000 pentagons upload 17 MB flat and 49 MB in relief,
  instead of 123 MB either way, and are triangulated in 37 ms instead of 59.
- Extrusion layers draw only the blocks in view: a batch is sorted by square of a grid over its blocks, as the
  lines and fills are (one batch for a whole city drew every building every frame).
- The style of the features takes the texels its layer reads: 48 bytes a feature for lines, 96 for circles, 80 for
  extrusions, 16 for fills (128 for all before).
- The symbol layers keep their tables from one set of labels to the next while they fit (two textures were created
  and deleted at each update, several times a second while the map moves).
- 3D Tiles: a mesh used by several nodes of a tile is read once, its texture coordinates and triangles shared by its
  instances; an image used by several primitives is handed over and decoded once, and uploaded once (on the main
  thread, or when the decoding workers give the same bitmap); the selection of the tiles reuses its buffers from frame
  to frame.
- Fill and extrusion layers: a polygon whose rings start before the previous polygon's end takes only those after it
  (no ring shared by two polygons): malformed offsets cost no more than the rings there are.
- Ground layers: `ISARD_CIRCUMFERENCE`, the body's circumference in metres (`ISARD_EARTH` still works).
- The basemap's labels are shaped once per vector tile while it is kept: as the view moves, only the labels of the
  tiles coming into view are shaped (an update of 36 tiles of 150 labels after a pan of one column: 55 ms → 11 ms on
  the main thread), the others reused as they were.
- The Terrarium and Terrain-RGB relief is decoded and the equirectangular rasters are reprojected by the vector
  basemap's workers (OffscreenCanvas), off the main thread; in the page with `workers: false`.
- The glyph atlas sent again (a new engine, more pages) is sent down to the last glyph of each page only.
- Equirectangular rasters are reprojected as any other grid (each tile planned in the page through a mesh of 17 × 17
  points, sampled in a worker), their colors interpolated between source rows as along them.

### Added

- `basemapTileSize`: the texture size of the vector basemap's tiles (default 512; 256 for a quarter of the drawing,
  uploads and GPU memory, lines a little softer).
- Data in other CRSs, reprojected into the map (no new view projection): built-in EPSG:4326, EPSG:3857, Lambert-93
  (EPSG:2154), the CC zones (EPSG:3942 to 3950) and the UTM zones of WGS 84 and ETRS89, within 0.1 mm of PROJ; any
  other through an object with `forward` and `inverse` such as `proj4('EPSG:…')`, directly or named by `registerCrs`;
  `getCrs`, `lambertConformalConic`, `transverseMercator`.
- Rasters in another tile grid: `addRaster({ url, grid: { crs, matrices, bounds } })` for the tile matrices of a WMTS
  in any CRS, or a grid without matrices for a WMS asked for each tile's `{bbox}` in its CRS; reprojected by the map's
  workers through a mesh of control points, without seams (PLAN IGN from Lambert-93 matches its Web Mercator tiles to
  within 0.06 pixel). `wmtsGrid` and `parseWmtsGrid` read the grid of a layer from WMTS capabilities.
- Géoplateforme: `geoplateformeWmts(layer, { matrixSet: '2154_10cm_6_20' })` for its Lambert-93 layers (PLAN IGN,
  orthophotos, cadastre…), `geoplateformeWms(layers, { crs: 'EPSG:2154' })`, `geoplateformeWfs(…, { srsName })`.
- GeoJSON in another CRS: the feature layers reproject a collection whose `crs` member names one (a WFS's output in
  EPSG:2154); `reproject(data, crs)` for the others.
- `parseKml`, `parseGpx`, `parseTopoJson`: KML (placemarks, MultiGeometry, gx:Track, extended data, style colors), GPX
  (waypoints, routes, tracks with altitudes and times) and TopoJSON as GeoJSON, with their own XML reader (no
  `DOMParser`: in workers too), each a module of its own.
- The Lambert-93 example: PLAN IGN reprojected from Lambert-93, the arrondissements of Paris from a WFS in EPSG:2154, a
  GPX file.

### Fixed

- Picking in relief: lines, circles and networks hidden by a hill (or by extruded blocks drawn before them) are no
  longer picked: the ground's depth is drawn into the picking pass and the layers are tested against it as on screen.
- Each frame starts from the full GL state (depth and stencil writes, scissor, tests, blending), whatever a layer or a
  picking pass left.

## [0.2.1] - 2026-10-18

### Changed

- A mouse moving over a map with no `hover` nor `hoverfeature` listener draws nothing (it drew a frame per move);
  the position under the pointer is computed only for `hover` listeners.
- Vector basemap workers: each keeps its share of the pool's decoded tiles (96 in all, instead of 96 each); an opaque
  tile is moved out of its canvas instead of being copied; their canvases are reused. Images no longer used (evicted
  source tiles of equirectangular rasters and of a style's raster layers, a style's sprite) are freed at once.
- The pointer's position no longer reads the layout at every move; the Terrarium and Terrain-RGB relief decodes
  faster.
- `RasterLayer.setStyle(opacity, saturation)` is deprecated: `setStyle({ opacity, saturation })`, either value alone
  (the positional form still works, and still sets the saturation back to 1 when it is not given).

### Added

- `tiles3d: 'preload'`: the engine with 3D Tiles downloaded and compiled once the map is idle, for its first 3D Tiles
  layer; the map is rebuilt on it in a task of its own.
- `RasterLayer.setStyle({ opacity, saturation })` (`RasterStyle`), either value alone.
- NASA's relief of the Moon and Mars is credited by the attribution control (`terrain: true` there).

### Fixed

- A WebGL context restored while the engine with 3D Tiles loads, or after it failed to, no longer throws: the 3D
  Tiles layers wait for it, the others are created again; a layer that fails to be created again is reported and the
  others are created. Buildings linked to a network above them keep the link after a context loss or the move onto
  the engine with 3D Tiles.
- A destroyed map stays destroyed when its context comes back (no engine rebuilt, no tiles fetched), and glyphs
  arriving after `destroy` no longer reach the freed engine.
- An equirectangular raster with a `minZoom` has no holes at its first levels.
- The hover picking stops polling when the engine cannot pick, and gives up an answer that never comes.
- `VectorTiles` no longer changes the style object it is given; `destroy()` removes its raster and label layers
  from the map, and the map forgets it and a removed labels layer.
- An unknown basemap name throws `isard: unknown basemap '…'` (it failed on an undefined); a `'terrarium'` or
  `'mapbox'` relief given an image service or a `{bbox}` URL throws instead of fetching the template, and every
  `{z}`, `{x}`, `{y}` of its URL is replaced.
- A layer removed twice is removed once from the engine.
- Values that are not numbers or out of range no longer stop the engine: a minimum zoom above the maximum, a pitch
  limit, camera, size or pick radius that is not a number (ignored); an infinite point in a circle, line or network
  layer (left out); index arrays of lines, fills and labels pointing past their data.
- A 3D Tiles glb reusing one accessor in many primitives, or a tileset grafting itself, is an error instead of using
  up the memory; a texture of the wrong size or format is left out instead of drawing black.
- More features than the GPU's textures hold throw an error instead of drawing black; a failed restyle or relabel
  keeps the layer drawable.
- The relief's altitudes near the antimeridian are read on the right side of it; the RAF20 geoid is applied only when
  asked (`geoid: true`) and only on the Earth.
- Tiles of a page's loader at another size are resized; lines and buildings of translucent colors no longer darken
  where their pieces overlap.
- Browsing over a regional raster source no longer remembers every missing tile; 3D Tiles siblings are no longer
  prefetched near the memory budget, only to be unloaded again.

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

### Migrating from @ign-junn/isard 0.1.1

- The package: `npm install isard`, `import … from 'isard'` (instead of `@ign-junn/isard`).
- The basemaps: the default is `'osm-positron'` (OpenFreeMap), no longer PLAN.IGN; the Géoplateforme's are
  `'ign-plan'`, `'ign-plan-grey'`, `'ign-ortho'`, `'ign-plan-vector'`, `'ign-plan-grey-vector'` (the former names
  still work, with a warning). `basemap: 'ign-plan-grey'` brings back the former default (`'plan-grey'`).
- The hillshade: `setBasemap(basemap)` and the `hillshade` option default to 0 (0.4 before): pass `0.4` to keep the
  IGN hillshade.
- The relief: `terrain: { exaggeration }` without `url` (and `terrain: true`) is now the worldwide Terrain Tiles;
  `terrain: 'ign'` or `{ model: 'ign' }` for the IGN's model. A custom `terrain.url` no longer gets the RAF20 geoid by
  default: add `geoid: true` to keep it.
- The credits: `AttributionControl` shows those of the sources in use (OpenFreeMap and OpenStreetMap, the IGN only
  for its data, the Terrain Tiles, NASA); `DEFAULT_ATTRIBUTION` is deprecated (`IGN_ATTRIBUTION`); credit your own
  rasters with `RasterOptions.attribution`, other data with `customAttribution`.

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
