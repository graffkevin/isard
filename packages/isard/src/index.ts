/**
 * @ign-junn/isard: WebGL2 map engine for large, interactive data layers and 3D Tiles on Géoplateforme basemaps.
 *
 * ```ts
 * import { GeoMap } from '@ign-junn/isard';
 * const map = await GeoMap.create(container, { center: [2.4246, 48.8457], zoom: 13 });
 * const network = map.addNetwork({ nodes, edges, max: 1800, contour: 300 });
 * map.on('hover', ({ lonLat }) => lonLat && network.setValues(travelTimes(lonLat)));
 * map.addTiles3d({ url: 'https://…/tileset.json' });
 * map.addControl(new NavigationControl());
 * new Marker().setLonLat([2.4246, 48.8457]).setPopup(new Popup({ offset: [0, -34] }).setText('IGN')).addTo(map);
 * ```
 */
export {
  AttributionControl,
  type AttributionOptions,
  type Control,
  type ControlPosition,
  DEFAULT_ATTRIBUTION,
  NavigationControl,
  type NavigationOptions,
} from './controls.ts';
export type { DecoderUrls } from './decoders.ts';
export type { ExtrusionOptions, ExtrusionStyle, PolygonArrays, TileKey } from './extrusions.ts';
export { ExtrusionLayer } from './extrusions.ts';
export type {
  CircleOptions,
  CircleStyle,
  Feature,
  FeatureCollection,
  FeatureData,
  FeatureLayerStyle,
  FillOptions,
  FillStyle,
  Geometry,
  LineOptions,
  LineStyle,
  Placement,
  Position,
  Properties,
  Ramp,
  Size,
  Stops,
  Styled,
} from './features.ts';
export { CircleLayer, FeatureLayer, FillLayer, interpolate, LineLayer, parseColor } from './features.ts';
export type { TextPart } from './glyphs.ts';
export { GLYPHS_URL } from './glyphs.ts';
export type { GroundOptions, GroundTexture } from './layers.ts';
export { GroundLayer, Layer, NetworkLayer, RasterLayer, Tiles3dLayer } from './layers.ts';
export { GeoMap } from './map.ts';
export {
  type Anchor,
  Marker,
  type MarkerEvent,
  type MarkerEvents,
  type MarkerOptions,
  Popup,
  type PopupEvents,
  type PopupOptions,
} from './overlay.ts';
export type { SymbolOptions, SymbolStyle } from './symbols.ts';
export { SymbolLayer } from './symbols.ts';
export type {
  AnimationOptions,
  AtmosphereOptions,
  Basemap,
  BodyOptions,
  CelestialBody,
  FeatureHit,
  FlyToOptions,
  GeoMapOptions,
  LoadingMode,
  LonLat,
  MapEvents,
  MapHoverFeatureEvent,
  MapPointerEvent,
  NetworkOptions,
  NetworkStyle,
  Point,
  Projection,
  RasterGrid,
  RasterOptions,
  TerrainOptions,
  Tiles3dOptions,
  Tiles3dStyle,
} from './types.ts';
export type { Json, LabelCandidate, Style, StyleLayer } from './vectorstyle.ts';
export type { VectorTilesOptions } from './vectortiles.ts';
export { PLAN_STYLES, VectorTiles } from './vectortiles.ts';
export type { Camera, NetworkStats, PickHit, PickResult, RasterLayerStats, Tiles3dStats } from './wasm/isard.js';
