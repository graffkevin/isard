# Getting started

## Install

```sh
bun add isard
# or
npm install isard
```

The package is ES modules with their types (your bundler keeps what you use), the engine in two builds
(`isard_core_bg.wasm`, 225 KB gzipped, and `isard_bg.wasm`, 385 KB with 3D Tiles and point clouds), the point cloud
module (`isard_points_bg.wasm`, 43 KB, the LAZ decoder, loaded with the first COPC point cloud) and two workers:
`decoder.worker.js` decodes 3D Tiles and point clouds, `basemap.worker.js` draws vector basemaps. It needs WebGL2, and runs in every
current browser.

## A first map

```html
<div id="map" style="position: absolute; inset: 0"></div>
```

```ts
import { GeoMap } from 'isard';

const map = await GeoMap.create(document.getElementById('map')!, {
  center: [2.4246, 48.8457], // longitude, latitude
  zoom: 14,
});
```

The basemap is OpenFreeMap's Positron by default (OpenStreetMap data, worldwide, no key); `basemap` picks another
([basemaps](./basemaps)), and the attribution control credits the data shown.

`GeoMap.create` loads the engine once per page (it is compiled once), creates a canvas filling the container and
returns the map. The engine it loads has no 3D Tiles, a third smaller: the first 3D Tiles layer loads the full one
(shared with the decoding workers), and the map moves its camera and layers onto it: the view may hold or flash
for a frame or two while the map is rebuilt on it. A map that shows 3D Tiles at once asks for it from the start, to
download one engine instead of two and skip that move (`'preload'` downloads it once the map is idle, see
[3D Tiles](../layers/tiles3d#the-engine-with-3d-tiles)):

```ts
const map = await GeoMap.create(container, { tiles3d: true });
```

Nothing more is needed: the map follows the container's size, and drag,
wheel, double click, right drag (turn and tilt) and two-finger gestures move the camera.

## With a bundler

By default the engines and the workers are looked up next to the module (`new URL('./isard_core_bg.wasm',
import.meta.url)`). A bundler that moves files around needs to be told where they are. With Vite, the plugin does it:

```ts [vite.config.ts]
import isard from 'isard/vite';

export default defineConfig({ plugins: [isard()] });
```

In development it keeps the package out of Vite's dependency pre-bundling (its files are found next to it, linked
outside the project too); in a build it copies the five files (the engines `isard_core_bg.wasm`, `isard_bg.wasm`,
`isard_points_bg.wasm` and the workers `decoder.worker.js`, `basemap.worker.js`) to `dist/isard/` and tells the maps
of the page (`isard({ folder })` for another folder).

Elsewhere, serve those five files from one folder and give it:

```ts
const map = await GeoMap.create(container, { assetsUrl: '/static/isard/' });
```

Each file also has its own option (`coreWasmUrl`, `wasmUrl`, `pointsWasmUrl`, `workerUrl`, `basemapWorkerUrl`),
which wins over `assetsUrl`; given `wasmUrl` without `coreWasmUrl`, the map loads the full engine from the start.

## In React

`isard/react` gives the map, its layers, markers, popups and panels as components:

```tsx
import { IsardMap, Layer, Marker, Popup } from 'isard/react';

<IsardMap preset="ign-3d" center={[2.3499, 48.8566]} zoom={15} style={{ height: 480 }}>
  <Layer type="lines" data={routes} color="#0057b8" width={3} />
  <Marker lonLat={[2.3499, 48.853]}>
    <Popup offset={[0, -34]}>Notre-Dame</Popup>
  </Marker>
</IsardMap>
```

See [React](../react/).

## Next

- [Concepts](./concepts): the camera, frames on demand, the order of layers.
- [Layers](../layers/): what can be drawn.
- [Examples](../examples/): live, with their code.
