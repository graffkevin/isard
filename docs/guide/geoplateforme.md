# Géoplateforme (IGN) data

The [Géoplateforme](https://geoservices.ign.fr/) serves the IGN's data of France openly, without a key: maps,
orthophotos, relief, buildings, roads, administrative units. Isard takes all of it in one option or one call, and
credits the IGN by itself wherever its rasters are shown (`IGN_ATTRIBUTION`).

## Basemaps

```ts
GeoMap.create(container, { basemap: 'ign-plan-grey-vector' });
```

| Basemap | What it is |
|---|---|
| `'ign-plan-grey-vector'`, `'ign-plan-vector'` | PLAN.IGN vector tiles with their MapLibre styles (grey, standard), labels above your layers |
| `'ign-plan-grey'`, `'ign-plan'` | PLAN.IGN raster tiles (WMTS), grey or in colour |
| `'ign-ortho'` | the orthophotos (20 cm in France, worldwide imagery at low zoom) |

The names before 0.2 (`'plan'`, `'plan-grey'`, `'ortho'`, `'plan-vector'`, `'plan-grey-vector'`) still work, with a
warning. The PLAN.IGN styles are also `PLAN_STYLES.grey` and `PLAN_STYLES.standard`, for `vectorTiles()`:

```ts
const tiles = map.vectorTiles({ style: PLAN_STYLES.standard });
tiles.raster({ layers: (l) => l['source-layer'] === 'hydro_reseau' }); // its rivers alone, on their own layer
```

## Hillshade and relief

```ts
GeoMap.create(container, { basemap: 'ign-plan', hillshade: 0.4, terrain: 'ign' });
map.setTerrain({ model: 'ign', exaggeration: 1.5 });
```

`hillshade` lays the IGN's shading over the basemap. `terrain: 'ign'` lifts the ground by the RGE ALTI (1 to 5 m,
from the Géoplateforme's WMS as raw floats), its NGF-IGN69 altitudes raised by the RAF20 geoid height so that 3D Tiles
(ellipsoidal heights, such as the IGN's photomeshes) sit on it; `geoid: false` keeps them as served.

## Any WMTS or WMS layer

```ts
map.addRaster(geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS.IRC', { format: 'image/jpeg' })); // infrared
map.addRaster(geoplateformeWms('CADASTRALPARCELS.PARCELLAIRE_EXPRESS', { opacity: 0.8 }));   // the cadastre
```

`geoplateformeWmts` takes a layer of the
[WMTS capabilities](https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetCapabilities) (its Web Mercator grid, `PM`),
`geoplateformeWms` layers of the [WMS](https://data.geopf.fr/wms-r/wms?SERVICE=WMS&REQUEST=GetCapabilities), asked
for tile by tile in EPSG:3857. Both return the options of `addRaster` (opacity, zoom range, saturation…), the IGN's
credits included.

## Features: BD TOPO, ADMIN EXPRESS…

```ts
type Building = { hauteur: number | null };
const buildings = await geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.414, 48.84, 2.432, 48.851]);
map.addExtrusions({ data: buildings, height: (p) => p.hauteur ?? 6 });
```

`geoplateformeWfs` fetches the features of a WFS type in a box (west, south, east, north) as GeoJSON in longitudes and
latitudes, up to `count` (5000 by default): buildings, roads (`BDTOPO_V3:troncon_de_route`), communes
(`ADMINEXPRESS-COG.LATEST:commune`)… The features are your layers' data: credit them with the map's
`attributionControl: { customAttribution: IGN_ATTRIBUTION }` when no IGN basemap shows.

## Examples

The [network](../examples/network), [buildings](../examples/buildings), [GeoJSON](../examples/geojson),
[relief](../examples/relief), [globe](../examples/globe) and [3D Tiles](../examples/tiles3d) examples are built on
the Géoplateforme's data.
