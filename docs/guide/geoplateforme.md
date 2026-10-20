# Géoplateforme (IGN) data

The [Géoplateforme](https://geoservices.ign.fr/) serves the IGN's data of France openly, without a key: maps,
orthophotos, relief, buildings, roads, administrative units. Isard takes all of it in one option or one call, and
credits the IGN by itself wherever its rasters are shown (`IGN_ATTRIBUTION`). The helpers of this page are also
gathered in their own entry point: `import { geoplateformeWfs, geoplateformeWmts } from 'isard/geoplateforme'`.

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

The layers served in Lambert-93 (PLAN IGN, the high resolution orthophotos, the cadastre…) are reprojected by the map:

```ts
map.addRaster(geoplateformeWmts('GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93', { matrixSet: '2154_10cm_6_20', format: 'image/jpeg' }));
map.addRaster(geoplateformeWms('CADASTRALPARCELS.PARCELLAIRE_EXPRESS', { crs: 'EPSG:2154' }));
```

See [Projections](./projections#the-geoplateforme-in-lambert-93).

## Features: BD TOPO, ADMIN EXPRESS…

```ts
type Building = { hauteur: number | null };
const buildings = await geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.414, 48.84, 2.432, 48.851]);
map.addExtrusions({ data: buildings, height: (p) => p.hauteur ?? 6 });
```

`geoplateformeWfs` fetches the features of a WFS type in a box (west, south, east, north) as GeoJSON in longitudes and
latitudes, up to `count` (5000 by default): buildings, roads (`BDTOPO_V3:troncon_de_route`), communes
(`ADMINEXPRESS-COG.LATEST:commune`)… The features are your layers' data: credit them with the map's
`attributionControl: { customAttribution: IGN_ATTRIBUTION }` when no IGN basemap shows. With `srsName: 'EPSG:2154'`
they come in Lambert-93 (the box stays in degrees), the collection naming its CRS: the layers reproject it.

## LiDAR HD

```ts
const tiles = await geoplateformeWfs<{ url_npl: string }>('IGNF_LIDAR-HD_METADONNEE:metadata', [2.349, 48.852, 2.351, 48.854]);
map.addPointCloud({ url: tiles.features[0].properties.url_npl, colorBy: 'classification', edl: true });
```

The IGN's LiDAR HD covers France with about ten points per square metre, classified (ground, vegetation, buildings,
water, bridges…), in tiles of 1 km² distributed as [COPC](../layers/pointclouds): `addPointCloud` reads a tile by
range requests, the nodes in view only, its Lambert-93 and NGF-IGN69 altitudes placed on the relief (`terrain: 'ign'`)
by the RAF20 geoid. The URL of each tile is the `url_npl` of the WFS type `IGNF_LIDAR-HD_METADONNEE:metadata` (its
footprint the feature's geometry). The Géoplateforme allows 10 requests a second: the layer asks 8 at most
(`requestsPerSecond`). Licence Ouverte (Etalab 2.0): credit the IGN.

## Private sources

A private stream of the Géoplateforme (or a server behind single sign-on) wants an access token on every request:

```ts
import { setRequestHeaders } from 'isard';

setRequestHeaders('https://data.geopf.fr/private/', { Authorization: `Bearer ${token}` });
```

Every request isard makes to a URL starting with the prefix gets the headers: from the page (styles, sprites,
glyphs, WFS, capabilities), its workers (vector tiles, 3D Tiles, point clouds) and its engine (raster tiles, relief,
tilesets). For the whole page, the maps to come too; called again with a renewed token, without headers to stop. The
server must allow these headers (CORS).

## Examples

The [network](../examples/network), [buildings](../examples/buildings), [GeoJSON](../examples/geojson),
[relief](../examples/relief), [globe](../examples/globe), [3D Tiles](../examples/tiles3d) and
[Lambert-93](../examples/lambert93) and [point cloud](../examples/pointcloud) examples are built on the Géoplateforme's
data.
