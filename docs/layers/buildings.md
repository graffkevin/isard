# Buildings

```ts
const buildings = map.addExtrusions({
  data,                          // polygons
  height: (p) => p.hauteur ?? 6, // metres over the ground
  base: 0,
  color: '#d9d6cc',
  ramp: [[0, '#fde0c5'], [25, '#d0587e'], [45, '#6c2167']],
  opacity: [[13, 0], [13.6, 0.92]],
});
buildings.setValues(waterDepths); // per building, through the ramp
```

Blocks with depth, lit by the side they face, darker towards their foot. In relief, the walls stand on the ground and
the roof at its height over the ground at the centroid. A translucent layer draws its depth first: each pixel shows
its nearest face only.

## Tiles

For a tiled source (BD TOPO vector tiles), the application loads the tiles and hands their buildings over; GeoJSON,
or typed arrays decoded in a worker:

```ts
buildings.setTile('15/16604/11274', { arrays: { points, ringStart, polygonStart, feature }, properties });
buildings.removeTile('15/16604/11274');
```

## Colored by a network

Each building is joined once to the two ends of its nearest street and the cost from each; the engine then reads the
network's node values on the graphics card. The buildings recolor with the network in the same frame, at no cost.

```ts
buildings.linkNetwork(network, maxSeconds);
buildings.setAnchors(Float32Array.from(anchors.flatMap((a) => [a.nodeA, a.nodeB, a.costA, a.costB])), tileKey);
```

<LiveExample slug="buildings" height="440px" />
