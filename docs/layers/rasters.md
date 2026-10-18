# Rasters

```ts
map.addRaster({ url: 'https://…/{z}/{x}/{y}.png', minZoom: 6, maxZoom: 17, opacity: 0.55, saturation: 1 });
```

Any XYZ / WMTS tiles in Web Mercator, a WMS with `{bbox}`, or tiles in longitudes and latitudes
([below](#equirectangular-tiles)). All the tiles of a layer share a texture array (a fixed pool, recycled least
recently used first); consecutive raster layers are composited in one pass. A loading tile is replaced by its nearest
loaded ancestor, coarse tiles load first, transient errors are retried past the HTTP cache, missing tiles (404) are
covered by their ancestor.

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
the north, as NASA's Trek WMTS and the WorldCRS84Quad grid. They are reprojected in the page into the Web Mercator
tiles the engine draws, one row of pixels at a time; `minZoom` and `maxZoom` are levels of the source grid, drawn one
Mercator level finer (the last level of the service: 8 for the Moon's LRO mosaic, 7 for the Viking mosaic of Mars,
from the `TileMatrix` list of its `WMTSCapabilities.xml`). The source tiles are fetched with CORS and kept for the neighbouring tiles.
