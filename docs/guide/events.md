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
| `hoverfeature` | what is under the pointer, a frame or two after `hover`, picked without stalling the GPU | `{ point, lonLat, hit }` |
| `click` | a press and release without drag | `{ point, lonLat }` |
| `leave` | the pointer left the map | |
| `resize` | the container changed size | `{ width, height }` |
| `render` | a frame was drawn | `{ ms }`: the engine's time |
| `idle` | everything in view is loaded and drawn | |

`point` is in CSS pixels from the top left of the map; `lonLat` is the ground under it, on the relief itself, and
undefined above the horizon.

## Keyboard

The map is reached with Tab (the canvas is focusable, outlined when focused from the keyboard, named by `ariaLabel`,
'Map' by default); once it has the focus, as in MapLibre:

| Key | |
|---|---|
| arrows | pan by 100 pixels |
| Shift + left, right | rotate by 15° |
| Shift + up, down | tilt by 10° |
| `+` or `=`, `-` | zoom in, out (with Shift: two levels) |
| `0` | face north, level the view |

Keys held with Ctrl, Meta or Alt, and keys typed in a field over the map (a popup's input), are left alone. The moves
are short animations, instant under `prefers-reduced-motion`. `keyboard: false` turns the keys off (`interactive:
false` too, and the canvas is then not focusable). The navigation control's buttons are plain buttons, with labels.

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

`pickFeature` reads the pixels back at once: the main thread waits for the GPU to finish its frame. For hover effects,
`hoverfeature` asks the GPU during the frame and reads its answer on a later one, when it is ready, without waiting:

```ts
map.hoverPicking = { radius: 8, layers: [stations] };
map.on('hoverfeature', ({ hit }) => highlight(hit?.index));
```

It is only computed while it has listeners; `hit` has the shape `pickFeature` returns.
