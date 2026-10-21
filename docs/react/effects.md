# Viewsheds and swipe

## Viewshed

An observer's viewshed, with the options of `map.setViewshed` (`position`, `height`, `range`, `direction`, `tilt`,
`fov`, `verticalFov`, `visibleColor`, `hiddenColor`, `opacity`, `coneColor`; see the [viewshed](../examples/viewshed)
example); several `<Viewshed>` are drawn together, each one added and removed with its component:

```tsx
{observers.map((o) => (
  <Viewshed key={o.id} position={o.lonLat} height={o.height} range={800} direction={o.heading} fov={90} />
))}
```

The options are compared by value: a new position array at each render is no change.

## Swipe

The `<Layer>` in a `<Swipe>` show on one side of a vertical line only, `position` CSS pixels from the left of the map,
the layers beneath them on the other side (see [Swipe](../layers/#comparing-two-layers-swipe)):

```tsx
const [x, setX] = useState(400);
<IsardMap basemap="ign-ortho">
  <Swipe position={x}>
    <Layer {...geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS.1950-1965')} type="raster" />
  </Swipe>
</IsardMap>
<input type="range" min={0} max={800} value={x} onChange={(e) => setX(Number(e.target.value))} />
```

`side: 'right'` shows them right of the line.
