# Basemaps and relief

## Basemaps

```ts
GeoMap.create(container, { basemap: 'plan-grey-vector', hillshade: 0.4 });
map.setBasemap('ortho', 0);
```

| Basemap | What it is |
|---|---|
| `'plan-grey'`, `'plan'` | PLAN.IGN raster tiles (WMTS), grey or in colors |
| `'plan-grey-vector'`, `'plan-vector'` | PLAN.IGN vector tiles with their MapLibre styles: areas and roads drawn in workers, labels above your layers |
| `'ortho'` | the orthophotos |
| `'none'` | the background color only |

`hillshade` (0 to 1) adds the IGN hillshade over the basemap.

### Vector tiles of your own style

A vector basemap is two layers: the areas, roads and circles of the style, drawn tile by tile in workers into the
raster pool (so they lie on the relief and composite with the hillshade), and its labels in a symbol layer. Any
MapLibre style works: its legacy filters and functions and the common expressions are evaluated.

```ts
const tiles = map.vectorTiles({ style: 'https://…/style.json' });
tiles.raster();            // the areas and roads
tiles.labels();            // the labels, kept on top

// A subset of the style on its own layer, restyled: the rivers over a flood layer
tiles.raster({
  layers: () => false,
  extraLayers: [
    { id: 'rivers', type: 'line', 'source-layer': 'hydro_reseau', paint: { 'line-color': '#2f7fc4', 'line-width': 2 } },
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
GeoMap.create(container, { terrain: { exaggeration: 1.6, maxZoom: 13, geoid: false } });
map.setTerrain(true);
```

The IGN terrain model (RGE ALTI and its fill-ins, from the Géoplateforme's WMS as raw float altitudes) lifts the
ground, three levels coarser than the imagery. Altitudes get the RAF20 geoid height by default, so that 3D Tiles
(ellipsoidal heights) sit on the ground; `geoid: false` keeps NGF-IGN69 altitudes. Networks, lines, circles,
labels and buildings stand on the ground and the hills hide what lies behind them; fills and ground shaders are draped
exactly.

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

Off the Earth, the map has none of the Géoplateforme defaults: no basemap, no hillshade, no IGN credits (add the
imagery's with `attributionControl: { customAttribution }`), a black background, and no halo around a body without an
atmosphere (the stars stay, `sky: false` hides them). A relief needs its `url`, its altitudes taken as served (no
geoid). The planetary services mostly serve tiles in longitudes and latitudes: `grid: 'equirectangular'` reprojects
them in the page (see [rasters](../layers/rasters#equirectangular-tiles)).

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
