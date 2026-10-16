# Concepts

## The camera

The camera has MapLibre's conventions, so settings carry over unchanged: a `center` (longitude, latitude), a `zoom`
(512 px tiles: zoom 0 shows the world in 512 pixels), a `pitch` (degrees from the vertical, up to 70 by default, 85
with `maxPitch`) and a `bearing` (degrees clockwise from the north).

```ts
map.getCamera(); // { center, zoom, pitch, bearing }
map.setCamera({ zoom: 16, pitch: 45 });
map.easeTo({ center: [2.35, 48.85] }, 600); // or { duration, easing }
map.flyTo({ center: [5.72, 45.19], zoom: 13 }, { duration: 2600 }); // zooms out on its way, as MapLibre
map.getBounds(); // [west, south, east, north] of the ground in view
```

Two projections: `'globe'` (a sphere up to zoom 5, blending into Web Mercator up to zoom 7) and `'mercator'`.

## Frames on demand

Nothing runs while the map is idle. A frame is drawn when the camera moves, a tile arrives, a layer changes. Pointer
events are coalesced to one per display frame, and the `hover` listener runs inside the frame, before it is drawn:
values set there show at once, a frame earlier than with a library drawing in its own animation loop.

`'idle'` fires once everything in view is loaded and drawn: the end of a load, as MapLibre's.

## Layers and their order

Layers are drawn bottom to top in the order they were added; `before` (or `layer.moveBefore(other)`) puts a layer
under another, `layer.moveBefore(undefined)` on top of all.

The labels of a vector basemap stay above the layers added after them, as MapLibre's `beforeId: firstSymbol` does:
your network, buildings and areas go between the basemap's areas and its labels. A layer moved to the top
(`moveBefore(undefined)`) goes over the labels too.

Consecutive raster layers are composited in one pass over the ground: the basemap, a veil, the hillshade cost one
draw call. A layer in between splits the stack, which is how a fill can sit between the basemap and the hillshade.

## Coordinates and precision

Positions are given in degrees. Everything is computed in f64 on the CPU; the GPU receives coordinates relative to the
camera, in pixels at the current zoom, so f32 keeps centimetres at any zoom. Fills are split into high and low f32
parts: a veil as large as the world keeps its edges exact.

## A WebGL context loss

The browser may take the GPU away (a driver reset, a background tab on mobile). The map waits for it, then rebuilds
its engine and every layer in their order, with the same handles: nothing to do on the application's side.
