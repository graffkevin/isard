# Basemaps and relief

## Basemaps

```ts
GeoMap.create(container);                             // OpenFreeMap's Positron, worldwide
GeoMap.create(container, { basemap: 'osm-liberty' });
map.setBasemap('ign-ortho');                          // the IGN's orthophotos (France)
```

| Basemap | What it is |
|---|---|
| `'osm-positron'` (default) | [OpenFreeMap](https://openfreemap.org)'s Positron: OpenStreetMap data worldwide in light greys, quiet under your data |
| `'osm-liberty'`, `'osm-bright'` | OpenFreeMap's OSM Liberty and OSM Bright, in colour |
| `'ign-plan-grey'`, `'ign-plan'` | the Géoplateforme's PLAN.IGN raster tiles (WMTS, France), grey or in colour |
| `'ign-plan-grey-vector'`, `'ign-plan-vector'` | PLAN.IGN vector tiles with their MapLibre styles |
| `'ign-ortho'` | the IGN's orthophotos |
| `'none'` | the background color only |

The vector basemaps (OpenFreeMap's and PLAN.IGN's) have their areas and roads drawn in workers and their labels above
your layers, from the glyphs of their style. OpenFreeMap is free and needs no key; Positron is the default because it
recedes: light greys, few colours and labels, so what you draw over it reads first (the role `'plan-grey'` had before).

The Géoplateforme's basemaps had shorter names before 0.2: `'plan'`, `'plan-grey'`, `'ortho'`, `'plan-vector'` and
`'plan-grey-vector'` are still accepted, with a warning, as the same names with the `ign-` prefix (`LegacyBasemap`).
All of the Géoplateforme's data (basemaps, hillshade, relief, WMTS, WMS and WFS layers) is gathered in
[Géoplateforme (IGN) data](./geoplateforme).

`hillshade` (0 to 1, default 0) adds the IGN hillshade over the basemap (France, Géoplateforme).

The attribution control credits what the map shows, as it changes: OpenFreeMap, OpenMapTiles and OpenStreetMap for
OpenFreeMap's basemaps, the IGN for the Géoplateforme's basemaps, hillshade and terrain model, the Terrain Tiles for
the worldwide relief; your own data's credits go in `attributionControl: { customAttribution }`.

### Vector tiles of your own style

A vector basemap is two layers: the areas, roads and circles of the style (and its raster layers, extrusions drawn
flat), drawn tile by tile in workers into the raster pool (so they lie on the relief and composite with the hillshade),
and its labels in a symbol layer, from the glyphs of the style. Any MapLibre style works: its legacy filters and
functions and the common expressions are evaluated, a TileJSON `url` source is resolved. Icons are not drawn: a text
written over its icon (a road shield's number) is left out. `vectorTiles()` takes OpenFreeMap's Positron by default
(`OPENFREEMAP_STYLES`, `PLAN_STYLES` for the Géoplateforme's).

```ts
const tiles = map.vectorTiles({ style: 'https://…/style.json' });
tiles.raster();            // the areas and roads
tiles.labels();            // the labels, kept on top

// A subset of the style on its own layer, restyled: the rivers over a flood layer
tiles.raster({
  layers: () => false,
  extraLayers: [
    { id: 'rivers', type: 'line', 'source-layer': 'waterway', paint: { 'line-color': '#2f7fc4', 'line-width': 2 } },
  ],
});
```

The vector tiles of a map share its workers (cores - 1, 1 to 4, created on first use): a vector tile is downloaded
and decoded once, whatever the styles or subsets drawn from it, by the vector basemap and by your own `vectorTiles()`.
Asking for another `workers` count or `workerUrl` than the map's gives those tiles workers of their own. The tiles
near the center of the view are fetched first (`fetch`'s `priority`).

<LiveExample slug="vector" height="420px" />

## Relief

```ts
GeoMap.create(container, { terrain: true });                              // worldwide
GeoMap.create(container, { terrain: { model: 'ign', exaggeration: 1.6 } }); // France, RGE ALTI
map.setTerrain({ url: 'https://…/{z}/{x}/{y}.png', encoding: 'mapbox' });
```

The relief lifts the ground, three levels coarser than the imagery. Networks, lines, circles, labels and buildings
stand on the ground and the hills hide what lies behind them; fills and ground shaders are draped exactly.

| Model | What it is |
|---|---|
| `true`, `'terrain-tiles'` | the [Terrain Tiles](https://registry.opendata.aws/terrain-tiles/) of AWS's open data registry: worldwide, SRTM, EU-DEM, GMTED… merged by Mapzen (30 m in most places), Terrarium PNGs decoded in the page; the sea floor flattened at sea level |
| `'ign'` | the IGN's RGE ALTI and its fill-ins (France, 1 to 5 m), raw float altitudes from the Géoplateforme's WMS |

The French geoid: the IGN's model gives NGF-IGN69 altitudes, raised by the height of the RAF20 geoid (some 45 to 50 m
in France) so that 3D Tiles (ellipsoidal heights) sit on the ground; `geoid: false` keeps them as served. Other models
are taken as served (`geoid: true` raises them over France too).

`url` takes altitudes of 256 × 256 tiles: a WMS `GetMap` with `{bbox}` (EPSG:3857 metres), tiles with `{z}`, `{x}`,
`{y}`, or an ArcGIS image service (a URL ending in `/ImageServer`), which the map asks for raw altitudes
(`exportImage`, `format=bsq`, its validity mask marking the no-data). They are raw 32-bit floats, or the colors of PNG
tiles with `encoding: 'terrarium'` or `'mapbox'` (Terrain-RGB), decoded in the page. On the Moon and Mars,
`terrain: true` takes NASA's models (see [below](#relief-of-the-moon-and-mars)).

## Sky and fog

A tilted camera sees the horizon: `atmosphere` draws a sky over the plane and fades the ground into fog, with the
settings of MapLibre's `setSky`.

```ts
map.setAtmosphere({ skyColor: '#9cc7ec', horizonColor: '#e8f1f8', fogColor: '#f2f5f8', fogGroundBlend: 0.4 });
map.setMaxPitch(78);
```

On the globe, stars and an atmosphere halo surround the Earth (`sky: false` to turn them off).

<LiveExample slug="relief" height="420px" />

## The Moon, Mars and other bodies

```ts
const map = await GeoMap.create(container, { body: 'moon', center: [23.47, 0.67], zoom: 1.2 });
map.addRaster({
  url: 'https://trek.nasa.gov/tiles/Moon/EQ/LRO_WAC_Mosaic_Global_303ppd_v02/1.0.0/default/default028mm/{z}/{y}/{x}.jpg',
  grid: 'equirectangular',
  maxZoom: 8,
});
```

`body` sets what the map is drawn on, at its creation: `'earth'` (WGS 84, the default), `'moon'` (a sphere of
1 737 400 m), `'mars'` (a sphere of 3 396 190 m, its equatorial radius) or any ellipsoid,
`{ radius, flattening, atmosphere, color }` (equatorial radius in metres, flattening 0 for a sphere, its atmosphere,
the color of the globe past the tiles). Zoom levels keep their meaning on screen, the Web Mercator tile pyramid, and
longitudes and latitudes are the body's own (on the Moon, the mean Earth frame: longitude 0 faces the Earth; on Mars,
planetocentric latitudes and longitudes east from -180° to 180°, those of NASA's and the USGS's mosaics). What is measured in metres follows the body: the scale between zoom levels and
the ground, the relief, buildings, 3D Tiles.

Off the Earth, the map has none of the Earth's defaults: no basemap, no credits (add the imagery's with
`attributionControl: { customAttribution }`), a black background, and no halo around a body without an
atmosphere (the stars stay, `sky: false` hides them). The relief's altitudes are taken as served (no geoid). The
planetary services mostly serve tiles in longitudes and latitudes: `grid: 'equirectangular'` reprojects them in the
page (see [rasters](../layers/rasters#equirectangular-tiles)).

<LiveExample slug="moon" height="460px" />

```ts
const map = await GeoMap.create(container, { body: 'mars', center: [-80, 0], zoom: 1.2 });
map.addRaster({
  url: 'https://trek.nasa.gov/tiles/Mars/EQ/Mars_Viking_MDIM21_ClrMosaic_global_232m/1.0.0/default/default028mm/{z}/{y}/{x}.jpg',
  grid: 'equirectangular',
  maxZoom: 7,
});
```

An atmosphere draws a halo around the globe: `atmosphere: true` is the Earth's light blue, a CSS color another one, its
alpha the halo's strength (Mars: `'rgba(232, 168, 120, 0.6)'`, a faint butterscotch of dust), false none (the default
of a body of your own). Mars is a sphere rather than its IAU ellipsoid (flattening 1/170): its mosaics and landing
sites give planetocentric latitudes, which an ellipsoid would read as geodetic ones, up to 0.33° (20 km) apart; on a
sphere both agree. A body of your own with its `flattening` takes geodetic latitudes, as WGS 84 does.

<LiveExample slug="mars" height="460px" />

### Relief of the Moon and Mars

```ts
const map = await GeoMap.create(container, {
  body: 'mars',
  center: [-133.6, 15.6],
  zoom: 6.3,
  pitch: 66,
  terrain: true,
});
```

`terrain: true` lifts the ground by NASA's terrain models, from the image services of its Moon and Mars Trek (ArcGIS,
CORS open, public domain): on the Moon, LOLA's global model (128 pixels per degree, 237 m, heights above the sphere of
1 737 400 m); on Mars, MOLA's (128 and 64 pixels per degree merged, 463 m, above the areoid). They are asked for raw
32-bit floats directly in the body's Mercator (a Mercator on its sphere, given in WKT), so that nothing is reprojected
in the page, finest at level 8. `exaggeration` and `maxZoom` apply as on the Earth; another model is a `url`: a
`/ImageServer` of the body (Trek serves the Mercury, Vesta or Ceres ones), or tiles whose `{bbox}` is in the body's
Mercator metres (its half circumference, π × radius, at the edges of the world).

The relief of a body sets what may rise into view: `BodyOptions.relief`, the lowest and highest altitudes of its ground
(the Moon: -9.2 to 10.8 km, the South Pole-Aitken basin to the Selenean summit; Mars: -8.3 to 21.3 km, Hellas to
Olympus Mons; the Earth: -450 m to 8.9 km, the Dead Sea to Everest; ±1 % of the radius for a body of your own). It bounds the layers in view, the prisms of draped fills, what
counts as no-data (below it), the horizon of the globe; and since 20 km of relief show from much further away on a body
of a few thousand kilometres, the relief starts at a coarser level than on the Earth (3 on the Moon and Mars, 6 on the Earth). The examples fly to Tycho and Copernicus, Olympus Mons and Valles Marineris (`#tycho`, `#olympus-mons`… in
their URL). There is no hillshade computed from the relief: the mosaics carry the Sun's own shading, and Trek's shaded
reliefs can be added with `addRaster` like any equirectangular layer.
