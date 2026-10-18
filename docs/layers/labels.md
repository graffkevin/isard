# Labels

```ts
map.addSymbols({
  data,                                  // points; lines are labelled at their middle, polygons at their centroid
  text: (p) => `${p.name}\n${p.level} m`,
  font: 'Noto Sans Bold',                // the default glyphs' fonts: Noto Sans Regular, Bold, Italic
  size: [[12, 11], [16, 14]],
  color: '#16324f',
  haloColor: '#fff',
  haloWidth: 1.6,
  anchor: 'top',
  offset: [0, 1.3],                      // ems
  maxWidth: 10,                          // ems, then the text wraps
  placement: 'point',                    // or 'line': along the line, kept upright
  overlap: false,                        // true: always shown, hiding nothing
});
```

Text is drawn from signed distance fields (MapLibre's glyph format), loaded on first use from OpenFreeMap's fonts (the
`glyphs` option for others; a vector basemap's labels use the glyphs of its style). Lines break at `\n` and wrap at
`maxWidth`; runs can have their own scale and font, as MapLibre's `format`:

```ts
text: (p) => [{ text: p.name }, { text: '\nno data', scale: 0.85, font: 'Noto Sans Italic' }],
```

Labels are placed at every frame, every symbol layer sharing one collision grid from the top layer down, in data
order within a layer: a label shows only where no label placed before it does. Far labels of a tilted view are
smaller (MapLibre's perspective ratio).

## Icons

```ts
const pin = new Image();
pin.src = '/pin@2x.png';
await pin.decode();
map.addImage('pin', pin, { pixelRatio: 2 });  // an image, a canvas, ImageData, or { width, height, data }

map.addSymbols({
  data,
  text: (p) => p.name,
  icon: (p) => (p.open ? 'pin' : 'pin-grey'),  // an image of the map; empty: none
  iconSize: 0.8,                          // or zoom stops
  iconAnchor: 'bottom',
  anchor: 'top',                          // the text under the anchor
  textOptional: true,                     // the icon alone where its text does not fit
});
```

Icons take their place beside their text: a label's text and icon each have their box in the collision grid, and both
show or neither, unless one is optional (`textOptional`, `iconOptional`, as MapLibre's `text-optional` and
`icon-optional`). `overlap` and `iconOverlap` take `true` (shown wherever, hiding nothing), `'allow'` (shown wherever,
still hiding the labels after it) or `'ignore'` (shown only where it fits, hiding nothing). An icon turns by
`iconRotate` (degrees), and with the map when `iconRotationAlignment` is `'map'` (along its line with the `'line'`
placement: a one-way arrow). A signed distance field image (`sdf: true`) is drawn in the `iconColor`.

`iconTextFit` stretches an icon around its text (`'width'`, `'height'` or `'both'`, with `iconTextFitPadding`): a road
shield. An image with stretchable zones (`stretchX`, `stretchY`, in image pixels) keeps its other parts at their size,
its rounded ends for instance, and frames the text with its `content` box; without them, the whole image stretches.

The images are packed on first use into one atlas, shared by the symbol layers. A vector basemap's icons come from the
sprite of its style (see [basemaps](../guide/basemaps#vector-tiles-of-your-own-style)).
