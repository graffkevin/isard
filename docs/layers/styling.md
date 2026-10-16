# Styling features

The style of the feature layers (lines, circles, fills, labels, buildings) is given once, evaluated per feature when
the data or the style change, and kept by the engine in textures: restyling, recoloring and filtering thousands of
features never touches their geometry.

## Style values

A value is a constant, zoom stops, or a function of the feature returning either.

```ts
map.addLines({
  data: rivers,
  color: '#2f7fc4',                                   // a constant
  width: [[8, 1], [13, 2], [17, 5]],                  // zoom stops: linear in between, constant outside
  opacity: (p) => (p.intermittent ? 0.6 : 1),         // a function of the properties (and the index)
});
```

Zoom stops are `[[zoom, value], …]` in increasing zooms, up to four (more are resampled), as MapLibre's
`["interpolate", ["linear"], ["zoom"], …]`. Colors are CSS strings (`#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`).

## Values and ramps

A ramp colors a value per feature, set at any time in one call: one texture upload, whatever the number of features.

```ts
const gauges = map.addCircles({
  data,
  radius: 7,
  ramp: [[0, '#3a9d5d'], [0.5, '#e8b729'], [0.75, '#f07d22'], [0.9, '#d7301f']],
});
gauges.setValues(Float32Array.from(readings)); // NaN: the feature keeps its own color
```

## Layer-wide settings

| Setting | |
|---|---|
| `opacity` | a number or zoom stops: a layer fading in past a zoom |
| `minZoom`, `maxZoom` | the zooms it shows in (from `minZoom` included to `maxZoom` excluded) |
| `visible` | shown or not (`layer.setVisible(b)`) |
| `filter` | a predicate on the properties: the others are hidden (`layer.setFilter(fn)`), one texture upload |

## Data

`setData(data)` replaces the features: a FeatureCollection, an array of features, or one feature. With the same
number of features only the geometry is sent again; the route of a trip can change at every mouse move.
