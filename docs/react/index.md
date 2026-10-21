# React

`isard/react` gives the map and what it shows as components: a map created when it mounts and destroyed when it
unmounts, layers, markers, popups and panels added with their component and removed with it, their props changes
sent as the least each one needs.

```sh
npm install isard react react-dom
```

```tsx
import { IsardMap, Layer, Marker, Popup } from 'isard/react';

export function Map({ routes }: { routes: FeatureCollection }) {
  return (
    <IsardMap preset="ign-3d" center={[2.3499, 48.8566]} zoom={15} style={{ height: 480 }}>
      <Layer type="lines" data={routes} color="#0057b8" width={3} />
      <Marker lonLat={[2.3499, 48.853]}>
        <Popup offset={[0, -34]}>
          <b>Notre-Dame</b>
        </Popup>
      </Marker>
    </IsardMap>
  );
}
```

React 18 or 19; the components are rendered in the browser only (the engine is WebGL2): with server rendering, load
the component that holds the map on the client (`'use client'`, `next/dynamic` with `ssr: false`). With Vite, the
[plugin](../guide/getting-started#with-a-bundler) serves the engine as with the rest of the package.

## The map

`<IsardMap>` takes the [options of `GeoMap.create`](../reference/) as props, and fills its element: give it a size
(`style`, `className`). Its children are drawn once the engine is loaded, asynchronously; under React's StrictMode, the
map of the first mount is dropped as it should be.

| Props | |
|---|---|
| `center`, `zoom`, `pitch`, `bearing` | the camera at creation, then left to the user's gestures |
| `camera` | a camera the map follows: set again whenever it changes |
| `basemap`, `hillshade`, `terrain`, `projection`, `minZoom`, `maxZoom`, `maxPitch`, `sky`, `atmosphere`, `background`, `loading` | taken again when they change |
| `preset`, `body`, `interactive`, the URLs of the engine… | read at creation: a new `key` makes the map anew |
| `onClick`, `onMove`, `onMoveStart`, `onMoveEnd`, `onHover`, `onHoverFeature`, `onIdle`, `onResize`, `onRender`, `onLeave` | the map's [events](../guide/events) |
| `onLoad(map)`, `onError(error)` | the map created, or its creation failed (no WebGL2) |

The ref is the `GeoMap` once created, for what has no component (`flyTo`, `pickFeature`, `setFirstPerson`…):

```tsx
const map = useRef<GeoMap>(undefined);
<IsardMap ref={map} …/>
<button onClick={() => map.current?.flyTo({ center: paris, zoom: 16 })}>Paris</button>
```

A controlled camera keeps it in a state:

```tsx
const [camera, setCamera] = useState<Partial<Camera>>({ center: [2.35, 48.85], zoom: 14 });
<IsardMap camera={camera} onMove={setCamera} />
```

## Hooks

Under the map, `useMap()` gives the `GeoMap`, and `useMapEvent(type, listener)` listens to one of its events while the
component is mounted (the listener may change at each render):

```tsx
function Coordinates() {
  const [lonLat, setLonLat] = useState<LonLat>();
  useMapEvent('hover', (e) => setLonLat(e.lonLat));
  return <output>{lonLat?.map((v) => v.toFixed(5)).join(', ')}</output>;
}
```

## See also

[Layers](./layers), [markers, popups, panels](./overlays), [viewsheds and swipe](./effects), and the
[React example](../examples/react).
