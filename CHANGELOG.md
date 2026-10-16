# Changelog

Every notable change of `@ign-junn/isard`. The format follows [Keep a Changelog](https://keepachangelog.com), the
versions [Semantic Versioning](https://semver.org) (before 1.0.0, a minor version may change an API).

## [0.1.0] - 2026-10-16

First published version.

### Added

- `GeoMap`: Géoplateforme basemaps (PLAN.IGN raster or vector, ortho), hillshade and relief (RGE ALTI, exaggeration,
  geoid optional), globe and Web Mercator, camera of MapLibre's conventions (`easeTo`, `flyTo`, pitch up to 85 with a
  sky and ground fog), events (`move`, `moveend`, `hover` once per frame, `click`, `idle`…), picking within a radius.
- Layers: rasters (fetched or drawn by the page), networks colored by a value per node, GeoJSON lines, circles, fills
  (on the relief at any size) and labels (Géoplateforme fonts, shared collisions), buildings (values, or a network's
  times), ground shaders (the application's GLSL over the ground), 3D Tiles (b3dm, glb, Draco, meshopt, KTX2).
- Vector basemaps from MapLibre styles, drawn in workers into the raster pool, their labels above the other layers.
- Markers, popups, navigation and attribution controls.
