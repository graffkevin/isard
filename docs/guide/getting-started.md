# Getting started

## Install

```sh
bun add @ign-junn/isard
# or
npm install @ign-junn/isard
```

The package is one ES module with its types, the engine (`isard_bg.wasm`, 320 KB gzipped) and two workers:
`decoder.worker.js` decodes 3D Tiles, `basemap.worker.js` draws vector basemaps. It needs WebGL2, and runs in every
current browser.

## A first map

```html
<div id="map" style="position: absolute; inset: 0"></div>
```

```ts
import { GeoMap } from '@ign-junn/isard';

const map = await GeoMap.create(document.getElementById('map')!, {
  center: [2.4246, 48.8457], // longitude, latitude
  zoom: 14,
  basemap: 'plan-grey-vector',
});
```

`GeoMap.create` loads the engine once per page (it is compiled once and shared with the workers), creates a canvas
filling the container and returns the map. Nothing more is needed: the map follows the container's size, and drag,
wheel, double click, right drag (turn and tilt) and two-finger gestures move the camera.

## With a bundler

By default the engine and the workers are looked up next to the module (`new URL('./isard_bg.wasm',
import.meta.url)`). A bundler that moves files around is given their URLs instead. With Vite:

```ts
import { GeoMap } from '@ign-junn/isard';
import basemapWorkerUrl from '@ign-junn/isard/basemap.worker.js?url';
import decoderWorkerUrl from '@ign-junn/isard/decoder.worker.js?url';
import wasmUrl from '@ign-junn/isard/isard_bg.wasm?url';

const map = await GeoMap.create(container, { wasmUrl, workerUrl: decoderWorkerUrl, basemapWorkerUrl });
```

```ts [vite.config.ts]
export default defineConfig({
  // The engine is loaded by URL: kept out of the dependency pre-bundling
  optimizeDeps: { exclude: ['@ign-junn/isard'] },
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
    GeoMap.create(ref.current!, { basemap: 'plan-grey-vector' }).then((m) => {
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
