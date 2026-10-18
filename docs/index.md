---
layout: home

hero:
  name: Isard
  text: Maps that recolor at every mouse move
  tagline: A WebGL2 map engine for large, interactive data layers, labels and 3D Tiles. Rust core in WebAssembly, a small TypeScript API.
  image:
    src: /logo.svg
    alt: Isard
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
    - theme: alt
      text: Examples
      link: /examples/
    - theme: alt
      text: API
      link: /api

features:
  - icon: ⚡
    title: Millions of features, one upload
    details: A network of millions of street pieces recolors from a single texture upload of 4 bytes per node, drawn in the frame of the mouse move. Only the squares in view are drawn.
  - icon: 🗺️
    title: Basemaps and relief, worldwide
    details: OpenFreeMap's vector basemaps by default (OpenStreetMap data, no key), any MapLibre style drawn in workers with its labels above your data, a worldwide relief, on a globe that turns into Web Mercator. The Géoplateforme's PLAN.IGN, orthophotos and relief for France.
  - icon: ⛰️
    title: Everything lies on the relief
    details: Lines, circles, labels and buildings stand on the ground; fills of any size are exact to the pixel over hills and valleys; your own GLSL is drawn over the ground like the basemap.
  - icon: 🏙️
    title: Buildings and 3D Tiles
    details: Buildings raised to their height, colored by a value or by a network's travel times. Photomeshes and city models in 3D Tiles, compressed or not.
  - icon: 🏷️
    title: Labels and overlays
    details: Text from signed distance fields with halos and shared collisions, markers, popups, navigation controls.
  - icon: 🦀
    title: Small and exact
    details: f64 on the CPU, f32 relative to the camera on the GPU, centimetres at any zoom. Frames on demand only. Strict TypeScript types end to end.
---

<div class="vp-doc" style="max-width: 860px; margin: 48px auto 0; padding: 0 24px">

## Five lines

```ts
import { GeoMap } from 'isard';

const map = await GeoMap.create(container, { center: [2.4246, 48.8457], zoom: 14 });
const network = map.addNetwork({ nodes, edges, max: 1800 }); // lon/lat pairs, node index pairs
map.on('hover', ({ lonLat }) => lonLat && network.setValues(travelTimes(lonLat))); // one value per node
```

The map fills its container, follows its size, handles mouse and touch, loads the basemap. The values computed in a
`hover` listener are drawn by the very frame of the move. Below, the globe: click anywhere to fly there.

<LiveExample slug="globe" height="460px" />

</div>
