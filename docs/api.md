# API

Everything is typed: the declarations ship with the package, under `strict` and `exactOptionalPropertyTypes`. This
page is an overview; the [reference](/reference/) lists every class, option and type, generated from those
declarations.

## `GeoMap.create(container, options)`

| Option | Default | |
|---|---|---|
| `body` | `'earth'` | `'moon'`, `'mars'`, or `{ radius, flattening, atmosphere, color }` ([other bodies](./guide/basemaps#the-moon-mars-and-other-bodies)) |
| `center`, `zoom`, `pitch`, `bearing` | Saint-Mandé, 13, 0, 0 | the camera, MapLibre's conventions |
| `projection` | `'globe'` | or `'mercator'` |
| `basemap` | `'plan-grey'` | `'plan'`, `'plan-grey-vector'`, `'plan-vector'`, `'ortho'`, `'none'` |
| `hillshade` | 0.4 | opacity of the IGN hillshade, 0 for none |
| `terrain` | off | `true` or `{ url, exaggeration, maxZoom, geoid }` |
| `atmosphere` | none | sky and ground fog: `{ skyColor, horizonColor, fogColor, skyHorizonBlend, horizonFogBlend, fogGroundBlend }` |
| `sky` | true | stars and halo around the globe |
| `background` | `#f2f2f0` | behind the map |
| `minZoom`, `maxZoom`, `maxPitch` | 0, 22, 70 | `maxPitch` up to 85 |
| `loading` | `'immediate'` | `'smooth'`: fewer downloads during flights, fade-in |
| `interactive` | true | mouse, touch and keyboard controls |
| `keyboard` | true | arrows, `+`, `-`, `0` once the map has the focus ([keys](./guide/events#keyboard)) |
| `ariaLabel` | `'Map'` | accessible name of the map |
| `attributionControl` | true | `false`, or `{ compact, customAttribution }` |
| `coreWasmUrl`, `wasmUrl`, `workerUrl`, `basemapWorkerUrl` | next to the module | for bundlers; `wasmUrl` alone: the full engine from the start |
| `tiles3d` | false | load the engine with 3D Tiles from the start (else with the first 3D Tiles layer) |
| `workers` | cores - 1 (1 to 4) | 3D Tiles decoding workers, `false` for the main thread |
| `decoders`, `glyphs` | official builds, Géoplateforme fonts | to self-host them |

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
| `setTerrain(terrain)` | `true`, `false` or its options |
| `setAtmosphere(options)` | undefined to remove it |
| `vectorTiles({ style, workers })` (the map's workers, shared) | `.raster({ layers, extraLayers, size, opacity, before })`, `.labels({ layers })` |

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
| `addRaster(options)` | `RasterLayer` | `setStyle(opacity, saturation)` |
| `addTiles3d(options)` | `Tiles3dLayer` | `setStyle`, `stats` |

Every handle: `setVisible(b)` (feature layers), `moveBefore(layer)`, `remove()`. Every `add…` takes `before`.

## Events and picking

| | |
|---|---|
| `on(type, listener)` | returns the function that stops listening; see [Events](./guide/events) |
| `pickFeature(point, { radius, layers })` | `{ layer, index, feature }` |
| `pick(point, radius, layers)` | `{ layer: id, index }` |
| `stats()` | tile counters of the raster layers |

## Overlays

| | |
|---|---|
| `new Marker({ element, anchor, offset, draggable, className })` | `setLonLat`, `addTo`, `remove`, `setPopup`, `on('drag')` |
| `new Popup({ closeButton, closeOnClick, maxWidth, anchor, offset })` | `setLonLat`, `setHTML`, `setText`, `setDOMContent`, `addTo` |
| `addControl(control, position)`, `removeControl(control)` | `NavigationControl`, `AttributionControl`, or yours |
| `getContainer()`, `getCanvas()`, `destroy()` | |
