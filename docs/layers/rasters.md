# Rasters

```ts
map.addRaster({ url: 'https://…/{z}/{x}/{y}.png', minZoom: 6, maxZoom: 17, opacity: 0.55, saturation: 1 });
```

Any XYZ / WMTS tiles in Web Mercator, or a WMS with `{bbox}`. All the tiles of a layer share a texture array (a fixed
pool, recycled least recently used first); consecutive raster layers are composited in one pass. A loading tile is
replaced by its nearest loaded ancestor, coarse tiles load first, transient errors are retried past the HTTP cache,
missing tiles (404) are covered by their ancestor.

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
