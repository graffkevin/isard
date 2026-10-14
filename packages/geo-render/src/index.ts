/**
 * geo-render: WebGL2 map engine for large, interactive data layers and 3D Tiles on Géoplateforme basemaps.
 *
 * ```ts
 * import { GeoMap } from 'geo-render';
 * const map = await GeoMap.create(container, { center: [2.4246, 48.8457], zoom: 13 });
 * const network = map.addNetwork({ nodes, edges, max: 1800, contour: 300 });
 * map.on('hover', ({ lonLat }) => lonLat && network.setValues(travelTimes(lonLat)));
 * map.addTiles3d({ url: 'https://…/tileset.json' });
 * ```
 */
export { NetworkLayer, RasterLayer, Tiles3dLayer } from './layers.ts';
export { GeoMap } from './map.ts';
export type {
  Basemap,
  GeoMapOptions,
  LoadingMode,
  LonLat,
  MapEvents,
  MapPointerEvent,
  NetworkOptions,
  NetworkStyle,
  Point,
  Projection,
  RasterOptions,
  Tiles3dOptions,
  Tiles3dStyle,
} from './types.ts';
export type { Camera, PickHit, RasterLayerStats, Tiles3dStats } from './wasm/geo_render.js';
