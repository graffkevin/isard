# Markers, popups, panels

Their content is JSX, rendered by a portal into the element the map positions: state, events and context of the
application work in it as anywhere.

## Marker

```tsx
<Marker lonLat={stop.lonLat} draggable onDragEnd={(lonLat) => move(stop.id, lonLat)} />
<Marker lonLat={[2.35, 48.85]} icon={<img src="/bike.svg" width={24} alt="" />} />
<Marker lonLat={roof} height={1.7} base={roofHeight} /> {/* raised, its stem down to the roof */}
```

The default pin, or `icon`: its look, any JSX. `lonLat`, `height`, `base`, `draggable` and `offset` change it in place;
`anchor` and `className` make it anew. `onDragStart`, `onDrag`, `onDragEnd` give its position.

## Popup

In a `<Marker>`, a popup opened and closed by a click on the marker; alone, a popup at `lonLat`, open while mounted:

```tsx
<Marker lonLat={place.lonLat}>
  <Popup offset={[0, -34]}>
    <h3>{place.name}</h3>
    <button type="button" onClick={() => remove(place.id)}>Remove</button>
  </Popup>
</Marker>

{selected && (
  <Popup lonLat={selected.lonLat} onClose={() => setSelected(undefined)}>
    {selected.name}
  </Popup>
)}
```

`open` decides it from the application (with `onOpen`, `onClose`). The [options](../guide/overlays) of `Popup`
(`closeButton`, `closeOnClick`, `maxWidth`, `anchor`, `className`) make it anew when they change.

## Panel

JSX standing in 3D at a position, `width` metres wide, facing `heading` (see [Panels](../guide/overlays#panels)):

```tsx
<Panel lonLat={[2.2945, 48.8584]} width={40} heading={120} height={20}>
  <video src="/tour.mp4" autoPlay muted loop width={640} />
</Panel>
```
