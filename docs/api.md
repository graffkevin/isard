# API

Everything is typed: the declarations ship with the package, under `strict` and `exactOptionalPropertyTypes`. This
page is an overview; the [reference](/reference/) lists every class, option and type, generated from those
declarations.

## Entry points

| Import | |
|---|---|
| `isard` | `GeoMap`, the layers' handles, markers, popups, panels, controls, and everything below |
| `isard/react` | `IsardMap`, `Layer`, `CustomLayer`, `Marker`, `Popup`, `Panel`, `Control`, `Viewshed`, `Swipe`, `useMap`, `useMapEvent` ([React](./react/)) |
| `isard/vite` | the Vite plugin serving the engines and workers ([with a bundler](./guide/getting-started#with-a-bundler)) |
| `isard/formats` | `parseKml`, `parseGpx`, `parseTopoJson` |
| `isard/projections` | `getCrs`, `registerCrs`, `lambertConformalConic`, `transverseMercator`, `reproject`, `wmtsGrid`, `parseWmtsGrid`, the `Crs` and `TileGrid` types |
| `isard/geoplateforme` | `geoplateformeWmts`, `geoplateformeWms`, `geoplateformeWfs`, `lambert93Grid`, `IGN_ATTRIBUTION` |

The helpers of `isard/formats`, `isard/projections` and `isard/geoplateforme` are also exported by `isard` itself.

## `GeoMap.create(container, options)`

| Option | Default | |
|---|---|---|
| `preset` | none | `'ign'`, `'ign-3d'`, `'ign-ortho-3d'`: the IGN's data in a word, the other options over it ([IGN data](./guide/geoplateforme#in-a-word)) |
| `body` | `'earth'` | `'moon'`, `'mars'`, or `{ radius, flattening, atmosphere, color, relief }` ([other bodies](./guide/basemaps#the-moon-mars-and-other-bodies)) |
| `center`, `zoom`, `pitch`, `bearing` | Saint-Mandé, 13, 0, 0 | the camera, MapLibre's conventions |
| `projection` | `'globe'` | or `'mercator'` |
| `basemap` | `'osm-positron'` | `'osm-liberty'`, `'osm-bright'`, `'ign-plan-grey'`, `'ign-plan'`, `'ign-plan-grey-vector'`, `'ign-plan-vector'`, `'ign-ortho'`, `'none'` ([basemaps](./guide/basemaps)) |
| `hillshade` | 0 | opacity of the IGN hillshade (France), 0 for none |
| `terrain` | off | `true` (the worldwide Terrain Tiles; NASA's on the Moon and Mars), `'ign'` (France) or `{ model, url, encoding, exaggeration, maxZoom, geoid }` ([relief](./guide/basemaps#relief)) |
| `atmosphere` | none | sky and ground fog: `{ skyColor, horizonColor, fogColor, skyHorizonBlend, horizonFogBlend, fogGroundBlend }` |
| `sky` | true | stars and halo around the globe |
| `background` | `#f2f2f0` | behind the map |
| `minZoom`, `maxZoom`, `maxPitch` | 0, 22, 70 | `maxPitch` up to 85 |
| `loading` | `'immediate'` | `'smooth'`: fewer downloads during flights, fade-in |
| `interactive` | true | mouse, touch and keyboard controls |
| `keyboard` | true | arrows, `+`, `-`, `0` once the map has the focus ([keys](./guide/events#keyboard)) |
| `ariaLabel` | `'Map'` | accessible name of the map |
| `attributionControl` | true | the credits of the data shown; `false`, or `{ compact, customAttribution }` |
| `assetsUrl` | next to the module | the folder isard's five files are served from (`isard/vite` sets it: [with a bundler](./guide/getting-started#with-a-bundler)) |
| `coreWasmUrl`, `wasmUrl`, `pointsWasmUrl`, `workerUrl`, `basemapWorkerUrl` | next to the module | each file on its own, over `assetsUrl`; `wasmUrl` alone: the full engine from the start; `pointsWasmUrl`: the point cloud module of the workers |
| `tiles3d` | false | `true`: the engine with 3D Tiles from the start (else loaded with the first 3D Tiles layer, model or point cloud, a short stall); `'preload'`: downloaded and compiled once the map is idle ([3D Tiles](./layers/tiles3d#the-engine-with-3d-tiles)) |
| `workers` | cores - 1 (1 to 4) | 3D Tiles decoding workers, `false` for the main thread (the PNG relief and the reprojected rasters too) |
| `basemapTileSize` | 512 | texture size of the vector basemap's tiles; 256: a quarter of the work and memory, lines a little softer |
| `decoders`, `glyphs` | official builds, OpenFreeMap's fonts | to self-host them |

## Grouped methods

The map's methods are also grouped by theme, for a shorter completion list; each is the same as the map's own:

| | |
|---|---|
| `map.camera` | `get`, `set`, `easeTo`, `flyTo`, `bounds`, `firstPerson`, `setFirstPerson`, `project`, `unproject` |
| `map.query` | `feature` (`pickFeature`), `surface` (`pickSurface`), `surfaceAt`, `elevationAt`, `metersPerPixel` |
| `map.effects` | `viewshed` (`setViewshed`), `swipe` (`setSwipe`) |

## Camera

| Method | |
|---|---|
| `getCamera()`, `setCamera(camera)` | `{ center, zoom, pitch, bearing }` |
| `easeTo(camera, ms \| { duration, easing })` | ease-out by default |
| `flyTo(camera, { duration, curve, speed, easing })` | van Wijk and Nuij's path, as MapLibre |
| `getBounds()` | `[west, south, east, north]` of the ground in view |
| `setMinZoom(z)`, `setMaxZoom(z)`, `setMaxPitch(p)` | |
| `project(lonLat, height)`, `unproject(point)` | CSS pixels ↔ degrees, on the relief (or `height` metres above it) |
| `setFirstPerson({ lonLat, height, bearing, pitch, fov })`, `firstPerson` | an eye at street level, the drag turning it; undefined to turn around a point again ([street level](./guide/streetview#the-street-level-view)) |
| `metersPerPixel(point)`, `elevationAt(lonLat)` | metres per CSS pixel at a screen point (a scale bar); the relief's altitude at a position |
| `setDoubleClickZoom(enabled)` | a double click zooms or not (off while a drawing takes double clicks) |
| `setProjection(p)`, `setLoading(mode)`, `setBackground(color)`, `setSky(b)` | |

## Basemap and relief

| Method | |
|---|---|
| `setBasemap(basemap, hillshade)` | under the other layers |
| `setTerrain(terrain)` | `true`, `'ign'`, `false` or its options |
| `setAtmosphere(options)` | undefined to remove it |
| `vectorTiles({ style, workers, workerUrl, attribution })` (the map's workers, shared) | `.raster({ layers, extraLayers, size, opacity, minZoom, maxZoom, before })`, `.labels({ layers, maxTiles })`, `.ready`, `.destroy()` (removes its layers) |

`OPENFREEMAP_STYLES` (`positron`, `liberty`, `bright`) and `PLAN_STYLES` (`standard`, `grey`) are the URLs of the
styles the basemaps use, for `vectorTiles({ style })`.

## Layers

| Method | Returns | Main methods of the handle |
|---|---|---|
| `addLines(options)` | `LineLayer` | `setData`, `setStyle`, `setValues`, `setFilter` |
| `addCircles(options)` | `CircleLayer` | the same |
| `addFills(options)` | `FillLayer` | the same |
| `addSymbols(options)` | `SymbolLayer` | `setData`, `setStyle`, `setFilter` |
| `addExtrusions(options)` | `ExtrusionLayer` | `setData`, `setTile`, `removeTile`, `setValues`, `linkNetwork`, `setAnchors` |
| `addNetwork(options)` | `NetworkLayer` | `setValues`, `setCosts`, `setEdgeWidths`, `setStyle`, `stats` |
| `addGround(options)` | `GroundLayer` | `setTexture`, `updateTexture`, `setUniform`, `setBounds` |
| `addPmtiles(url, { kind, style, opacity, size, attribution })` | `Promise<RasterLayer \| VectorTiles>` | a PMTiles archive read by range requests: raster, or vector with `style` (default `basicStyle` of its layers); `addRaster({ url: 'pmtiles://…' })` and `pmtiles://` style sources too ([PMTiles](./layers/rasters#pmtiles)) |
| `addRaster(options)` | `RasterLayer` | `setStyle({ opacity, saturation })`; options `url`, `grid` (`'mercator'`, `'equirectangular'`, or a `TileGrid` in another CRS: tile matrices or a WMS's `{bbox}`, reprojected by the map's workers, see [projections](./guide/projections)), `minZoom`, `maxZoom`, `size`, `loader` (the page draws each tile), `attribution` (credited while shown); `geoplateformeWmts`, `geoplateformeWms` give the options of the Géoplateforme's layers ([IGN data](./guide/geoplateforme)) |
| `addTiles3d({ url, maxScreenSpaceError, opacity, color, geoid })` | `Tiles3dLayer` | `setStyle`, `stats`; features picked with their properties, `color` tinting the layer, `geoid` for altitudes ([3D Tiles](./layers/tiles3d)) |
| `addPanoramas({ url, orientationsUrl, calibrationUrl, crs, geoid, backgroundDistance })` | `Promise<Panoramas>` | `show`, `hide`, `nearest`, `enter`, `go`, `exit`: oriented street-level images projected onto the scene, a walk from one to the next ([panoramas](./guide/streetview)) |
| `addPointCloud({ url, format, colorBy, pointSize, pointBudget, edl, classifications, crs })` | `PointCloudLayer` | `setStyle`, `stats`, `info`; a COPC file (range requests) or 3D Tiles points, points picked with their attributes ([point clouds](./layers/pointclouds)) |
| `addModels({ url \| data, instances, altitudeReference })` | `ModelLayer` | `setInstances`; instances `{ lonLat, altitude, heading, pitch, roll, scale }`, picked by index ([models](./layers/models)) |
| `addCustomLayer({ renderingMode, onAdd, render, onRemove }, { before })` | `CustomLayer` | your WebGL drawing in every frame, `frame.matrixAt(lonLat, altitude)` its matrices ([custom layers](./layers/custom)) |

Every handle: `setVisible(b)` (feature layers), `moveBefore(layer)`, `remove()`, `ready` (resolves once drawn).
Every `add…` takes `before`; `add({ type, ...options })` takes any of them from a configuration ([from a
configuration](./layers/#from-a-configuration)).

| | |
|---|---|
| `setSwipe({ position, layers, side })` | `layers` shown on one side of a vertical line only, the layers beneath on the other ([swipe](./layers/#comparing-two-layers-swipe)) |
| `setViewshed(options \| options[])` | what can be seen from observers, tinted ([viewshed](./examples/viewshed)) |

## Events and picking

| | |
|---|---|
| `on(type, listener)` | returns the function that stops listening; see [Events](./guide/events) |
| `pickFeature(point, { radius, layers })` | `{ layer, index, feature }`; waits for the GPU (a stall): for clicks |
| `pick(point, radius, layers)` | `{ layer: id, index }`, the same way |
| `pickSurface(point)`, `surfaceAt(lonLat)` | what is drawn under a point, or the top of what stands at a position (a roof, a mesh), and its height above the relief ([surfaces](./guide/events#surfaces)) |
| `hoverPicking = { radius, layers }` | what the `hoverfeature` event looks for under the pointer, picked without waiting for the GPU |
| `stats()` | tile counters of the raster layers |

## Overlays

| | |
|---|---|
| `new Marker({ element, anchor, offset, draggable, height, className })` | `setLonLat`, `setHeight(height, base)`, `addTo`, `remove`, `setPopup`, `on('drag')`; a raised marker has a stem down to what it stands on |
| `new Panel({ element, width, heading, pitch, height })` | an element standing in 3D at a position (a video, a page), metres wide: `setLonLat`, `setPlacement`, `addTo`, `remove` ([3D panels](./guide/overlays#panels)) |
| `new Popup({ closeButton, closeOnClick, maxWidth, anchor, offset })` | `setLonLat`, `setHTML`, `setText`, `setDOMContent`, `addTo` |
| `addControl(control, position)`, `removeControl(control)` | `NavigationControl`, `AttributionControl`, or yours |
| `getContainer()`, `getCanvas()`, `destroy()` | |

## Data and helpers

| | |
|---|---|
| `geoplateformeWmts(layer, options)`, `geoplateformeWms(layers, options)` | raster options of the Géoplateforme's layers, crediting the IGN; `matrixSet: '2154_10cm_6_20'` (WMTS) or `crs: 'EPSG:2154'` (WMS) for Lambert-93 |
| `bdtopoBuildings(box, { defaultHeight, color })`, `batiments3d(options)` | the options of the BD TOPO buildings of a box (`addExtrusions`) and of the IGN's Bâtiments 3D (`addTiles3d`, `geoid` set) |
| `geoplateformeWfs(typeName, [west, south, east, north], { count, signal, srsName })` | the Géoplateforme's features in a box, as GeoJSON (in `srsName` if given, reprojected by the layers) |
| `wmtsGrid(capabilitiesUrl, layer, matrixSet)`, `parseWmtsGrid(xml, layer, matrixSet)` | the `TileGrid` of a WMTS layer, for `addRaster({ url, grid })` |
| `getCrs(crs)`, `registerCrs(name, crs)`, `lambertConformalConic(…)`, `transverseMercator(…)` | CRSs: built in (EPSG:4326, 3857, 2154, 3942-3950, UTM), proj4's converters, or yours ([projections](./guide/projections)) |
| `reproject(data, crs)` | GeoJSON in a CRS (default the one its `crs` member names) to longitudes and latitudes |
| `parseKml(text)`, `parseGpx(text)`, `parseTopoJson(topology, name)` | other formats as GeoJSON ([formats](./layers/features#other-crss-and-formats)) |
| `IGN_ATTRIBUTION`, `OPENFREEMAP_ATTRIBUTION`, `TERRAIN_TILES_ATTRIBUTION` | credits of the sources, for `customAttribution` or `attribution` |
| `setRequestHeaders(prefix, headers)` | headers added to every request isard makes to URLs starting with `prefix`, from the page, its workers and its engine: an access token for private sources ([private sources](./guide/geoplateforme#private-sources)) |
| `GLYPHS_URL`, `DEFAULT_FONT` | the default glyphs (OpenFreeMap's fonts) and font of labels |
| `new PMTiles(url \| blob)`, `zxyToTileId`, `tileIdToZxy` | a PMTiles archive's `header()`, `metadata()`, `tile(z, x, y)` |
| `basicStyle(source, layers)` | a plain MapLibre style for vector tiles known by their layers' names |
| `parseColor(color)`, `interpolate(stops, zoom)` | a CSS color as RGBA 0-1; the value of zoom stops at a zoom |
