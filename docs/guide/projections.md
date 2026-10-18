# Projections

The map is drawn in Web Mercator, flat or on the globe. Data in another coordinate reference system (CRS) is
reprojected into it as it loads, as MapLibre does: raster tiles of another grid are redrawn into Web Mercator tiles,
GeoJSON in projected coordinates is turned into longitudes and latitudes. There is no view in another projection: a
map in Lambert-93 is shown in Web Mercator, its features at their place.

## Built-in CRSs

| CRS | |
|---|---|
| `EPSG:4326` (`CRS:84`, and the geographic ETRS89 `EPSG:4258`, RGF93 `EPSG:4171`) | longitudes and latitudes |
| `EPSG:3857` | Web Mercator in metres |
| `EPSG:2154` | Lambert-93, France's official projection (Lambert Conformal Conic on RGF93/GRS80) |
| `EPSG:3942` to `EPSG:3950` | the nine conic zones CC42 to CC50 of France |
| `EPSG:32601` to `EPSG:32660`, `EPSG:32701` to `EPSG:32760` | the UTM zones of WGS 84, north and south |
| `EPSG:25828` to `EPSG:25838` | the UTM zones of ETRS89 (Europe) |

A name may be written `EPSG:2154`, `urn:ogc:def:crs:EPSG::2154` or `http://www.opengis.net/def/crs/EPSG/0/2154` (as
WFS and GeoJSON files write it). The projections are computed in the package, without a dependency, exactly: within
0.1 mm of [PROJ](https://proj.org) on the reference points of the tests (Lambert-93 across France and Corsica, every CC
zone, UTM up to 6° off its central meridian and near the antimeridian), forward and back. WGS 84, ETRS89 and RGF93
are taken as one datum: they differ by less than a metre in Europe. CRSs on the old French datum NTF (Lambert I to IV,
Lambert II étendu) need a datum shift of up to a hundred metres: plug them with proj4 (below).

```ts
import { getCrs, lambertConformalConic, transverseMercator } from 'isard';

getCrs('EPSG:2154').forward([2.3461, 48.8462]); // [652011.88, 6860882.57]: the Panthéon in Lambert-93
getCrs('EPSG:32631').inverse([452025.15, 5410564.93]); // [2.3461, 48.8462]
```

`lambertConformalConic({ lat0, lon0, lat1, lat2, x0, y0, ellipsoid })` and `transverseMercator({ lon0, k0, x0, y0,
ellipsoid })` make other CRSs of these projections (Belgian Lambert 2008, a Gauss-Krüger zone…).

## Any other CRS: proj4

A CRS is any object with `forward([lon, lat])` and `inverse([x, y])`: a converter of
[proj4](https://github.com/proj4js/proj4js) is one, given as it is or by name.

```ts
import proj4 from 'proj4';
import { registerCrs } from 'isard';

proj4.defs('EPSG:27572', '+proj=lcc +lat_1=46.8 +lat_0=46.8 +lon_0=0 +k_0=0.99987742 +x_0=600000 +y_0=2200000 ' +
  '+a=6378249.2 +b=6356515 +towgs84=-168,-60,320,0,0,0,0 +pm=paris +units=m +no_defs');
registerCrs('EPSG:27572', proj4('EPSG:27572')); // Lambert II étendu, NTF: now a name like the others
map.addRaster({ url, grid: { crs: proj4('EPSG:27572'), matrices } }); // or the object itself
```

## Rasters in another grid

```ts
map.addRaster({
  url: 'https://…/wmts?…&TILEMATRIXSET=LAMB93&TILEMATRIX={z}&TILECOL={x}&TILEROW={y}',
  grid: {
    crs: 'EPSG:2154',
    matrices: [{ id: '0', resolution: 209715.2, origin: [0, 12000000] }, /* … one per level */],
    bounds: [-5.5, 41, 10, 51.5],
  },
});
```

A `grid` is the tile matrix set of a WMTS in any CRS: per level its `resolution` (CRS units per pixel), the `origin`
of its tile (0, 0) (the top-left corner), its tile size (256 by default), optionally its `matrixWidth`, `matrixHeight`
and `limits` (the tiles the service has), and `id`, written for `{z}` in the URL (`{x}` the column, `{y}` the row,
from the north). `bounds` (longitudes and latitudes) keeps the layer from asking for tiles where the source has
nothing. `minZoom` and `maxZoom` are then the map's zooms, by default those the levels span.

Each Web Mercator tile is planned in the page: a mesh of 17 × 17 points of the tile is projected into the source CRS
(289 projections a tile, whatever its size), the source level of the same pixel size picked, the source tiles under
the tile listed. A worker of the map then draws it (in the page with `workers: false`): the source tiles are fetched
(with CORS) and kept for the neighbouring tiles, laid side by side, and every pixel of the tile sampled from them, its
position interpolated in the mesh, its color between the four nearest source pixels. The mesh is exact to a hundredth
of a source pixel from zoom 5 (a third of a pixel at zoom 3, where a tile covers France), tiles meet without seams,
and a source tile edge falling on a tile edge fetches nothing more.

### From WMTS capabilities

```ts
import { wmtsGrid } from 'isard';

const grid = await wmtsGrid('https://…/wmts?SERVICE=WMTS&REQUEST=GetCapabilities', 'LAYER', 'MATRIXSET');
map.addRaster({ url: 'https://…/wmts?…&LAYER=LAYER&TILEMATRIXSET=MATRIXSET&TILEMATRIX={z}&TILECOL={x}&TILEROW={y}', grid });
```

`wmtsGrid` reads the tile matrix set of a layer (its first one if none is named) from the capabilities: resolutions
from the scale denominators (in degrees for a geographic CRS), corners (EPSG:4326's with the latitude first), the
layer's `TileMatrixLimits` and its `WGS84BoundingBox`. `parseWmtsGrid(xml, layer, matrixSet)` reads a document you
have.

### A WMS in another CRS

```ts
map.addRaster({
  url: 'https://…/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&CRS=EPSG:2154&BBOX={bbox}&WIDTH={width}&HEIGHT={height}&LAYERS=…&FORMAT=image/png&TRANSPARENT=TRUE',
  grid: { crs: 'EPSG:2154', bounds: [-5.5, 41, 10, 51.5] },
});
```

A grid without matrices asks for one image a tile: `{bbox}` is the box of the Web Mercator tile in the CRS (minimum
x, minimum y, maximum x, maximum y), a pixel larger, `{width}` and `{height}` the tile's size. In WMS 1.3, EPSG:4326
puts the latitude first: ask for `CRS=CRS:84` with a `grid` in `'CRS:84'` instead.

### Longitudes and latitudes

`grid: 'equirectangular'` is the grid of NASA's Trek and of WorldCRS84Quad (two 256 px tiles side by side at level 0)
as a preset of the above, its `minZoom` and `maxZoom` levels of that grid ([rasters](../layers/rasters#equirectangular-tiles)).

## The Géoplateforme in Lambert-93

The Géoplateforme serves some layers in Lambert-93 only, or in Lambert-93 too: PLAN IGN (`GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93`),
the high resolution orthophotos (`HR.ORTHOIMAGERY.ORTHOPHOTOS.L93`), the cadastre (`CADASTRALPARCELS.PARCELLAIRE_EXPRESS.L93`,
`CADASTRALPARCELS.PARCELS.L93`)… in tile matrix sets of 5 and 10 cm named
`2154_10cm_6_20`, `2154_5cm_6_22`… (one pyramid: 209 715.2 m a pixel at level 0, halved at each level, from the corner
(0, 12 000 000)). `geoplateformeWmts` knows them:

```ts
map.addRaster(geoplateformeWmts('GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93', { matrixSet: '2154_10cm_6_20', format: 'image/jpeg' }));
map.addRaster(geoplateformeWmts('CADASTRALPARCELS.PARCELLAIRE_EXPRESS.L93', { matrixSet: '2154_10cm_12_20' }));
map.addRaster(geoplateformeWms('CADASTRALPARCELS.PARCELLAIRE_EXPRESS', { crs: 'EPSG:2154' })); // the WMS in Lambert-93
```

The matrix set of each layer is in the [capabilities](https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetCapabilities)
(3 MB); another set is read with `wmtsGrid` and given as `grid`. The services answer with CORS, and 404 outside a
layer's limits (left empty). PLAN IGN reprojected from Lambert-93 and the same map in its Web Mercator grid match to
within 0.06 pixel across France (cross-correlation of the two, from Brest to Strasbourg and Bastia).

## GeoJSON in another CRS

```ts
const parcels = await geoplateformeWfs('CADASTRALPARCELS.PARCELLAIRE_EXPRESS:parcelle', box, { srsName: 'EPSG:2154' });
map.addFills({ data: parcels }); // reprojected: the collection names its CRS
map.addLines({ data: reproject(roads, 'EPSG:32631') }); // coordinates in UTM 31N, no crs member
```

The feature layers reproject a collection whose legacy `crs` member names its CRS (GeoJSON of 2008, the output of a
WFS asked for in another `SRSNAME`), once per collection. `reproject(data, crs)` does it for data without one, or in a
CRS of your own; it copies the features with new geometries (properties shared, altitudes kept). The features a layer
hands back (`pickFeature`, `hoverfeature`) are the reprojected ones, in longitudes and latitudes.

## Limits

- No view in another projection: the map is Web Mercator (or the globe); a map in Lambert-93 is reprojected into it.
- Datum shifts are not applied: the built-in CRSs share WGS 84's datum (within a metre); a CRS on another datum
  (NTF…) needs proj4 with its `+towgs84` or grid.
- Rasters are resampled once, into the map's tiles: a source drawn at its own pixel size, not sharper. Their colors are
  interpolated (bilinear), right for maps and imagery, not for classes or values to read back.
- A custom CRS projects in the page (the plan of each tile); built-in ones as well, the drawing is in workers either
  way.

The [Lambert-93 example](../examples/lambert93) shows PLAN IGN reprojected from Lambert-93 over the Web Mercator
basemap, the arrondissements of Paris from a WFS in EPSG:2154 and a GPX file.
