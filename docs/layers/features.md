# Lines, circles, fills

## Lines

```ts
map.addLines({
  data,                         // LineString, MultiLineString; the rings of polygons are drawn as lines
  color: (p) => p.color,
  width: [[11, 1.5], [15, 4]],
  dash: [1.5, 1],               // dash and gap in line widths, as line-dasharray
  opacity: 0.9,
});
```

Round caps and joins. Pieces are sorted by square, only the squares in view are drawn; on the relief they stand on the
ground and hills hide them. A translucent layer is drawn through the stencil, so that its joins are not blended twice.
A casing is a wider line under another.

## Circles

```ts
map.addCircles({
  data,                         // Point, MultiPoint
  color: '#fff',
  radius: [[10, 1.5], [15, 5]],
  strokeColor: (p) => p.color,
  strokeWidth: 2,
  alignment: 'viewport',        // or 'map': lying on the ground
});
```

## Fills

```ts
map.addFills({ data, color: (p) => p.color, opacity: 0.5 });
```

Polygons with their holes (earcut), cut along a grid so that they follow the globe. Over the relief a fill is drawn as
a shadow volume: each polygon lifted into a prism, the ground inside it counted in the stencil, then colored. It lies
on hills and valleys at any size, exact to the pixel: a veil as large as the world with a hole for a region costs a few
full-screen passes at most.

<Example slug="geojson" height="440px" />
