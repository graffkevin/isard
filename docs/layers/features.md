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

## Other CRSs and formats

```ts
map.addFills({ data: await geoplateformeWfs('BDTOPO_V3:batiment', box, { srsName: 'EPSG:2154' }) });
map.addLines({ data: reproject(data, 'EPSG:32631') });
map.addLines({ data: parseGpx(await (await fetch('walk.gpx')).text()) });
map.addFills({ data: parseKml(kmlText) });
map.addFills({ data: parseTopoJson(topology, 'counties') });
```

A collection whose legacy `crs` member names another CRS is reprojected by the layers; `reproject(data, crs)` does it
for data without one ([projections](../guide/projections#geojson-in-another-crs)). The parsers turn other formats into
GeoJSON, in the page or in a worker (their own XML reader: no `DOMParser` needed), each a module of its own, bundled
only when imported:

| | |
|---|---|
| `parseKml(text)` | the placemarks (in folders or not): points, lines, polygons with holes, `MultiGeometry` (a Multi… of one type, else a GeometryCollection), `gx:Track`; `name`, `description`, `ExtendedData` (`Data`, `SimpleData`) as properties, the colors of their style (inline, `styleUrl`, `StyleMap`) as `stroke`, `stroke-opacity`, `stroke-width`, `fill`, `fill-opacity`, `marker-color` (KML's `aabbggrr` turned into CSS colors) |
| `parseGpx(text)` | waypoints as points (with `time`), routes and tracks as lines (a track of several segments as a multi-line), altitudes as the third coordinate, `name`, `desc`, `type`… as properties, the times of the points as `times` |
| `parseTopoJson(topology, name)` | the features of an object (the first by default): arcs decoded (quantization, deltas) and stitched, ids and properties kept |

A KML style is a set of properties, for the layer's style functions:
`map.addLines({ data, color: (p) => String(p.stroke ?? '#333'), width: (p) => Number(p['stroke-width'] ?? 2) })`.

<LiveExample slug="geojson" height="440px" />
