# Layers in React

`<Layer>` takes a layer as [`map.add`](../layers/#from-a-configuration) does: its `type` and the options of its `add…`
method.

```tsx
<Layer type="raster" url="https://…/{z}/{x}/{y}.png" opacity={0.6} />
<Layer type="extrusions" data={buildings} height={(p) => p.hauteur ?? 6} color="#e4e1d8" />
<Layer type="tiles3d" {...batiments3d()} />
```

## Updates

A layer is created when its component mounts, removed when it unmounts. In between, a change of its props updates it
by the least it needs:

| What changed | |
|---|---|
| `data` | `setData`: the features sent again, the layer kept |
| its style (`color`, `width`, `opacity`, `filter`, `visible`…) | `setStyle`, the changed keys only |
| `instances` of models, `uniforms` and `textures` of a ground shader, `costs` and `edgeWidths` of a network | their own setters |
| what is set at creation (`url`, `grid`, `geoid`, `nodes`, `code`…), a key taken away, the `type` | the layer made anew, in its place |

Props are compared by identity: data, arrays and functions made anew at each render are sent again at each render.
Keep them in a state or a memo:

```tsx
const data = useMemo(() => toGeoJson(rows), [rows]);
const color = useCallback((p: Station) => (p.open ? '#2a9d8f' : '#999'), []);
<Layer type="circles" data={data} color={color} />
```

## Order

The layers of the components are drawn in the order they mounted, the first one at the bottom (under the labels of a
vector basemap). `order` places them otherwise, from the bottom up, whenever they mount:

```tsx
<Layer type="fills" data={areas} order={0} />
{showRoutes && <Layer type="lines" data={routes} order={1} />} {/* above the fills, mounted after or before */}
```

## The handle

The ref, or `onLayer`, gives the layer's handle, for what is not a prop (`setValues` of a network, `featureAt` of 3D
Tiles, `ready`); a layer made anew gives its new handle:

```tsx
const network = useRef<NetworkLayer>(undefined);
<Layer ref={network} type="network" nodes={nodes} edges={edges} />
useMapEvent('hover', (e) => network.current?.setValues(times(e.lonLat)));
```
