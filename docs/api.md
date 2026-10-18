# API

Everything is typed: the declarations ship with the package, under `strict` and `exactOptionalPropertyTypes`. This
page is an overview; the [reference](/reference/) lists every class, option and type, generated from those
declarations.

## `GeoMap.create(container, options)`

| Option | Default | |
|---|---|---|
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
| `coreWasmUrl`, `wasmUrl`, `workerUrl`, `basemapWorkerUrl` | next to the module | for bundlers; `wasmUrl` alone: the full engine from the start |
| `tiles3d` | false | `true`: the engine with 3D Tiles from the start (else loaded with the first 3D Tiles layer or model, a short stall); `'preload'`: downloaded and compiled once the map is idle ([3D Tiles](./layers/tiles3d#the-engine-with-3d-tiles)) |
| `workers` | cores - 1 (1 to 4) | 3D Tiles decoding workers, `false` for the main thread (the PNG relief and the reprojected rasters too) |
| `basemapTileSize` | 512 | texture size of the vector basemap's tiles; 256: a quarter of the work and memory, lines a little softer |
| `decoders`, `glyphs` | official builds, OpenFreeMap's fonts | to self-host them |

## Camera

| Method | |
|---|---|
| `getCamera()`, `setCamera(camera)` | `{ center, zoom, pitch, bearing }` |
| `easeTo(camera, ms \| { duration, easing })` | ease-out by default |
| `flyTo(camera, { duration, curve, speed, easing })` | van Wijk and Nuij's path, as MapLibre |
| `getBounds()` | `[west, south, east, north]` of the ground in view |
| `setMinZoom(z)`, `setMaxZoom(z)`, `setMaxPitch(p)` | |
| `project(lonLat)`, `unproject(point)` | CSS pixels ↔ degrees, on the relief |
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
| `addRaster(options)` | `RasterLayer` | `setStyle({ opacity, saturation })` (the positional `setStyle(opacity, saturation)` is deprecated); options `url`, `grid` (`'mercator'`, `'equirectangular'`, or a `TileGrid` in another CRS: tile matrices or a WMS's `{bbox}`, reprojected by the map's workers, see [projections](./guide/projections)), `minZoom`, `maxZoom`, `size`, `loader` (the page draws each tile), `attribution` (credited while shown); `geoplateformeWmts`, `geoplateformeWms` give the options of the Géoplateforme's layers ([IGN data](./guide/geoplateforme)) |
| `addTiles3d(options)` | `Tiles3dLayer` | `setStyle`, `stats` |
| `addModels({ url \| data, instances, altitudeReference })` | `ModelLayer` | `setInstances`; instances `{ lonLat, altitude, heading, pitch, roll, scale }`, picked by index ([models](./layers/models)) |
| `addCustomLayer({ renderingMode, onAdd, render, onRemove }, { before })` | `CustomLayer` | your WebGL drawing in every frame, `frame.matrixAt(lonLat, altitude)` its matrices ([custom layers](./layers/custom)) |

Every handle: `setVisible(b)` (feature layers), `moveBefore(layer)`, `remove()`. Every `add…` takes `before`.

## Events and picking

| | |
|---|---|
| `on(type, listener)` | returns the function that stops listening; see [Events](./guide/events) |
| `pickFeature(point, { radius, layers })` | `{ layer, index, feature }`; waits for the GPU (a stall): for clicks |
| `pick(point, radius, layers)` | `{ layer: id, index }`, the same way |
| `hoverPicking = { radius, layers }` | what the `hoverfeature` event looks for under the pointer, picked without waiting for the GPU |
| `stats()` | tile counters of the raster layers |

## Overlays

| | |
|---|---|
| `new Marker({ element, anchor, offset, draggable, className })` | `setLonLat`, `addTo`, `remove`, `setPopup`, `on('drag')` |
| `new Popup({ closeButton, closeOnClick, maxWidth, anchor, offset })` | `setLonLat`, `setHTML`, `setText`, `setDOMContent`, `addTo` |
| `addControl(control, position)`, `removeControl(control)` | `NavigationControl`, `AttributionControl`, or yours |
| `getContainer()`, `getCanvas()`, `destroy()` | |

## Data and helpers

| | |
|---|---|
| `geoplateformeWmts(layer, options)`, `geoplateformeWms(layers, options)` | raster options of the Géoplateforme's layers, crediting the IGN; `matrixSet: '2154_10cm_6_20'` (WMTS) or `crs: 'EPSG:2154'` (WMS) for Lambert-93 |
| `geoplateformeWfs(typeName, [west, south, east, north], { count, signal, srsName })` | the Géoplateforme's features in a box, as GeoJSON (in `srsName` if given, reprojected by the layers) |
| `wmtsGrid(capabilitiesUrl, layer, matrixSet)`, `parseWmtsGrid(xml, layer, matrixSet)` | the `TileGrid` of a WMTS layer, for `addRaster({ url, grid })` |
| `getCrs(crs)`, `registerCrs(name, crs)`, `lambertConformalConic(…)`, `transverseMercator(…)` | CRSs: built in (EPSG:4326, 3857, 2154, 3942-3950, UTM), proj4's converters, or yours ([projections](./guide/projections)) |
| `reproject(data, crs)` | GeoJSON in a CRS (default the one its `crs` member names) to longitudes and latitudes |
| `parseKml(text)`, `parseGpx(text)`, `parseTopoJson(topology, name)` | other formats as GeoJSON ([formats](./layers/features#other-crss-and-formats)) |
| `IGN_ATTRIBUTION`, `OPENFREEMAP_ATTRIBUTION`, `TERRAIN_TILES_ATTRIBUTION` | credits of the sources, for `customAttribution` or `attribution` |
| `GLYPHS_URL`, `DEFAULT_FONT` | the default glyphs (OpenFreeMap's fonts) and font of labels |
| `parseColor(color)`, `interpolate(stops, zoom)` | a CSS color as RGBA 0-1; the value of zoom stops at a zoom |
