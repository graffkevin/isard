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
