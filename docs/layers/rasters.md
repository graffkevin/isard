# Rasters

```ts
map.addRaster({ url: 'https://…/{z}/{x}/{y}.png', minZoom: 6, maxZoom: 17, opacity: 0.55, saturation: 1 });
```

Any XYZ / WMTS tiles in Web Mercator, a WMS with `{bbox}`, tiles in longitudes and latitudes
([below](#equirectangular-tiles)), or tiles and WMS images in another CRS: Lambert-93, UTM… ([below](#other-grids)). All the tiles of a layer share a texture array (a fixed pool, recycled least
recently used first); consecutive raster layers are composited in one pass. A loading tile is replaced by its nearest
loaded ancestor, coarse tiles load first, transient errors are retried past the HTTP cache, missing tiles (404) are
covered by their ancestor.

## PMTiles

```ts
map.addRaster({ url: 'pmtiles://https://…/satellite.pmtiles', maxZoom: 19 });
await map.addPmtiles('https://…/archive.pmtiles');   // raster or vector, as its header says
```

A [PMTiles](https://docs.protomaps.com/pmtiles/) archive (v3): one file of tiles, read by HTTP range requests, with
no tile server. Its header and root directory come in one request, the leaf directories as needed (kept: 64 per
archive), then each tile in its own range; gzipped directories and tiles are decompressed by the browser
(`DecompressionStream`; brotli and zstd archives are refused). Past the archive's deepest level, its tiles are drawn
magnified. The server must answer ranges (`206`) and allow them across origins (CORS); a server sending a whole file
of up to 64 MB instead is read from memory. `blob:` URLs work too: a file dropped on the page.

`map.addPmtiles(url, { kind, style, opacity, size, attribution })` reads the header first: images (PNG, JPEG, WebP,
AVIF) give a raster layer, vector tiles a [vector source](../guide/basemaps#vector-tiles-of-your-own-style) drawn with
`style`, or by default `basicStyle` of the archive's layers (`vector_layers` of its metadata): each layer's polygons,
lines and points in a color, the `name` of points as labels. The archive's `attribution` is credited, as text.
`PMTiles` (the reader: `header()`, `metadata()`, `tile(z, x, y)`) is exported for other uses.

## Tiles drawn by your code

```ts
map.addRaster({
  url: '',
  size: 512,                             // sharper: the textures of the pool
  loader: async (z, x, y, signal) => {    // an ImageBitmap of size × size, null for nothing there
    const canvas = new OffscreenCanvas(512, 512);
    drawTile(canvas.getContext('2d'), z, x, y);
    return createImageBitmap(canvas.getContext('2d').getImageData(0, 0, 512, 512));
  },
});
```

The vector basemaps are drawn this way, in workers.

## Equirectangular tiles

```ts
map.addRaster({ url: 'https://trek.nasa.gov/tiles/Moon/EQ/…/default028mm/{z}/{y}/{x}.jpg', grid: 'equirectangular', maxZoom: 8 });
```

Tiles in longitudes and latitudes (Plate Carrée, EPSG:4326-like): two 256 px tiles side by side at level 0, rows from
the north, as NASA's Trek WMTS and the WorldCRS84Quad grid. They are reprojected by the map's workers (in the page with
`workers: false`) into the Web Mercator tiles the engine draws, as any [other grid](#other-grids); `minZoom` and
`maxZoom` are levels of the source grid, drawn one Mercator level finer (the last level of the service: 8 for the Moon's LRO mosaic, 7
for the Viking mosaic of Mars, from the `TileMatrix` list of its `WMTSCapabilities.xml`). The source tiles are fetched
with CORS and kept for the neighbouring tiles.

## Other grids

```ts
map.addRaster(geoplateformeWmts('GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93', { matrixSet: '2154_10cm_6_20', format: 'image/jpeg' }));
map.addRaster({ url, grid: await wmtsGrid(capabilitiesUrl, 'LAYER', 'MATRIXSET') });
map.addRaster({ url: '…&CRS=EPSG:2154&BBOX={bbox}&WIDTH={width}&HEIGHT={height}', grid: { crs: 'EPSG:2154' } });
```

A `grid` in another CRS: the tile matrices of a WMTS (`matrices`: resolution, origin, tile size per level, `{z}` the
level's `id`), or none for a WMS asked for each tile's box in that CRS. Each Web Mercator tile is planned in the page
(a mesh of control points projected into the source CRS, the source level of the same pixel size) and drawn by a
worker of the map from the source tiles it spans, sampled through the mesh, without seams. The CRSs, the WMTS
capabilities, plugging proj4 and the limits: [Projections](../guide/projections).
