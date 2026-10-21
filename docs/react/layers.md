# Layers in React

`<Layer>` takes a layer as [`map.add`](../layers/#from-a-configuration) does: its `type` and the options of its `add…`
method.

<LiveExample slug="react/geojson" height="420px" />

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
| `values` (a `Float32Array`, one per feature or node) | `setValues`: the layer colored by its `ramp` |
| `filter` | `setFilter`: the features shown, nothing else sent again |
| its style (`color`, `width`, `opacity`, `visible`…) | `setStyle`, the changed keys only |
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

<LiveExample slug="react/lambert93" height="420px" />

```tsx
<Layer type="fills" data={areas} order={0} />
{showRoutes && <Layer type="lines" data={routes} order={1} />} {/* above the fills, mounted after or before */}
```

## Types

The properties of the features are inferred from the data or from the style functions, and the handle from the
`type`:

```tsx
interface Building { hauteur: number | null }

<Layer type="extrusions" data={buildings} height={(p: Building) => p.hauteur ?? 6} onLayer={(layer) => …} />
// layer: ExtrusionLayer<Building>
```

## The handle

The ref, or `onLayer`, gives the layer's handle, typed by its `type`, for what is not a prop (`setValues` of a network, `featureAt` of 3D
Tiles, `ready`); a layer made anew gives its new handle:

<LiveExample slug="react/network" height="420px" />

```tsx
const network = useRef<NetworkLayer>(undefined);
<Layer ref={network} type="network" nodes={nodes} edges={edges} />
useMapEvent('hover', (e) => network.current?.setValues(times(e.lonLat)));
```

## Custom layers

`<CustomLayer>` draws your WebGL code or a three.js scene in the map's frame and depth (see
[custom layers](../layers/custom)), among the layers of the components (`order`); another `layer` object makes it
anew, so keep it in a memo:

```tsx
const scene = useMemo(() => threeLayer(), []);
<CustomLayer layer={scene} order={1} />
```
