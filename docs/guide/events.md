# Events and picking

## Events

```ts
const off = map.on('hover', ({ point, lonLat }) => { /* … */ });
off(); // stops listening
```

| Event | When | Payload |
|---|---|---|
| `movestart`, `move`, `moveend` | the camera starts moving, moves, stops (no gesture held, no animation) | the camera |
| `hover` | the pointer moved over the map, at most once per frame, inside the frame | `{ point, lonLat }` |
| `click` | a press and release without drag | `{ point, lonLat }` |
| `leave` | the pointer left the map | |
| `resize` | the container changed size | `{ width, height }` |
| `render` | a frame was drawn | `{ ms }`: the engine's time |
| `idle` | everything in view is loaded and drawn | |

`point` is in CSS pixels from the top left of the map; `lonLat` is the ground under it, on the relief itself, and
undefined above the horizon.

## Projecting

```ts
map.project([2.35, 48.85]);   // [x, y] CSS pixels, undefined behind the camera or past the globe's horizon
map.unproject([400, 300]);    // [lon, lat] on the relief, undefined above the horizon
```

## Picking features

```ts
const hit = map.pickFeature(point, { radius: 8, layers: [stations] });
if (hit) console.log(hit.layer, hit.index, hit.feature?.properties);
```

The pickable layers (networks, lines, circles, buildings) are drawn once more into an offscreen target, each feature
colored by its number, clipped to the pixels around the point, which are read back: the cost does not depend on the
number of features, and picking on every `hover` is fine. Within `radius`, the feature nearest to the point wins;
`layers` restricts the search (the others hide nothing). Fills are tested on the CPU against the ground position.

`map.pick(point, radius)` is the raw form: `{ layer, index }` with the layer's id.
