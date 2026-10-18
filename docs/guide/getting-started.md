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
import.meta.url)`). A bundler that moves files around is given their URLs instead. With Vite:

```ts
import { GeoMap } from 'isard';
import basemapWorkerUrl from 'isard/basemap.worker.js?url';
import decoderWorkerUrl from 'isard/decoder.worker.js?url';
import wasmUrl from 'isard/isard_bg.wasm?url';
import coreWasmUrl from 'isard/isard_core_bg.wasm?url';
import pointsWasmUrl from 'isard/isard_points_bg.wasm?url';

const map = await GeoMap.create(container, {
  coreWasmUrl,
  wasmUrl,
  pointsWasmUrl,
  workerUrl: decoderWorkerUrl,
  basemapWorkerUrl,
});
```

Given `wasmUrl` without `coreWasmUrl`, the map loads the full engine from the start.

```ts [vite.config.ts]
export default defineConfig({
  // The engine is loaded by URL: kept out of the dependency pre-bundling
  optimizeDeps: { exclude: ['isard'] },
});
```

## In React

The map is created once per component and destroyed with it. `create` is asynchronous: a component unmounted before
it resolves destroys the map at once.

```tsx
function Map({ onReady }: { onReady: (map: GeoMap) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let map: GeoMap | undefined;
    let gone = false;
    GeoMap.create(ref.current!).then((m) => {
      if (gone) return m.destroy();
      map = m;
      onReady(m);
    });
    return () => {
      gone = true;
      map?.destroy();
    };
  }, []);
  return <div ref={ref} className="map" />;
}
```

Later props go through refs and the map's methods, not new maps. A destroyed map ignores late calls (labels being
shaped, tiles arriving), so React's double mount in development is harmless.

## Next

- [Concepts](./concepts): the camera, frames on demand, the order of layers.
- [Layers](../layers/): what can be drawn.
- [Examples](../examples/): live, with their code.
