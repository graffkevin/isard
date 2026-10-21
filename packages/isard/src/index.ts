/**
 * isard: a WebGL2 map engine for large, interactive data layers, labels and 3D Tiles.
 *
 * ```ts
 * import { GeoMap } from 'isard';
 * const map = await GeoMap.create(container, { center: [2.4246, 48.8457], zoom: 13 });
 * const network = map.addNetwork({ nodes, edges, max: 1800, contour: 300 });
 * map.on('hover', ({ lonLat }) => lonLat && network.setValues(travelTimes(lonLat)));
 * map.addTiles3d({ url: 'https://…/tileset.json' });
 * map.addControl(new NavigationControl());
 * new Marker().setLonLat([2.4246, 48.8457]).setPopup(new Popup({ offset: [0, -34] }).setText('Here')).addTo(map);
 * ```
 */
export { OPENFREEMAP_ATTRIBUTION, TERRAIN_TILES_ATTRIBUTION } from './basemap/basemaps.ts';
export type { TileGrid, TileMatrix } from './basemap/grid.ts';
export {
  PMTiles,
  type PmtilesEntry,
  type PmtilesHeader,
  type PmtilesTileType,
  tileIdToZxy,
  zxyToTileId,
} from './basemap/pmtiles.ts';
export type {
  CandidateIcon,
  CandidateOverlap,
  Json,
  LabelCandidate,
  RasterSource,
  Style,
  StyleLayer,
} from './basemap/vectorstyle.ts';
export type { PmtilesOptions, VectorTilesOptions } from './basemap/vectortiles.ts';
export { basicStyle, OPENFREEMAP_STYLES, PLAN_STYLES, VectorTiles } from './basemap/vectortiles.ts';
export { parseWmtsGrid, wmtsGrid } from './basemap/wmts.ts';
export {
  type Crs,
  type CrsLike,
  getCrs,
  lambertConformalConic,
  registerCrs,
  reproject,
  transverseMercator,
} from './data/crs.ts';
export { parseGpx } from './data/gpx.ts';
export { parseKml } from './data/kml.ts';
export { parseTopoJson, type TopoGeometry, type Topology } from './data/topojson.ts';
export type { DecoderUrls } from './engine/decoders.ts';
export { setRequestHeaders } from './engine/requests.ts';
export {
  BATIMENTS_3D_URL,
  type BdTopoBuilding,
  batiments3d,
  bdtopoBuildings,
  geoplateformeWfs,
  geoplateformeWms,
  geoplateformeWmts,
  IGN_ATTRIBUTION,
  IGN_PRESETS,
  type IgnPreset,
} from './geoplateforme.ts';
export type { ExtrusionOptions, ExtrusionStyle, PolygonArrays, TileKey } from './layers/extrusions.ts';
export { ExtrusionLayer } from './layers/extrusions.ts';
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
} from './layers/features.ts';
export { CircleLayer, FeatureLayer, FillLayer, interpolate, LineLayer, parseColor } from './layers/features.ts';
export type { TextPart } from './layers/glyphs.ts';
export { DEFAULT_FONT, GLYPHS_URL } from './layers/glyphs.ts';
export type { ImageOptions, Pixels, SpriteIcon } from './layers/icons.ts';
export type {
  CustomFrame,
  CustomLayerInterface,
  GroundOptions,
  GroundTexture,
  ModelInstance,
  ModelOptions,
} from './layers/layers.ts';
export {
  CustomLayer,
  GroundLayer,
  Layer,
  ModelLayer,
  NetworkLayer,
  RasterLayer,
  Tiles3dLayer,
} from './layers/layers.ts';
export { type Panorama, type PanoramaOptions, Panoramas } from './layers/panoramas.ts';
export type { ClassStyle, PointCloudOptions, PointCloudStyle, PointColor } from './layers/pointclouds.ts';
export { CLASSIFICATIONS, ELEVATION_RAMP, PointCloudLayer } from './layers/pointclouds.ts';
export type { Overlap, SymbolOptions, SymbolStyle } from './layers/symbols.ts';
export { SymbolLayer } from './layers/symbols.ts';
export type { HoverPicking } from './map/hover-pick.ts';
export { GeoMap, type LayerSpec } from './map.ts';
export type {
  AnimationOptions,
  AtmosphereOptions,
  Basemap,
  BasemapName,
  BodyOptions,
  CelestialBody,
  FeatureHit,
  FirstPersonView,
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
  RasterStyle,
  SwipeOptions,
  TerrainEncoding,
  TerrainModel,
  TerrainOptions,
  Tiles3dOptions,
  Tiles3dStyle,
  ViewshedOptions,
} from './types.ts';
export {
  AttributionControl,
  type AttributionOptions,
  type Control,
  type ControlPosition,
  NavigationControl,
  type NavigationOptions,
} from './ui/controls.ts';
export {
  type Anchor,
  Marker,
  type MarkerEvent,
  type MarkerEvents,
  type MarkerOptions,
  Panel,
  type PanelOptions,
  Popup,
  type PopupEvents,
  type PopupOptions,
} from './ui/overlay.ts';
export type {
  Camera,
  NetworkStats,
  PickHit,
  PickResult,
  PointCloudInfo,
  PointCloudPoint,
  PointCloudStats,
  RasterLayerStats,
  Tiles3dStats,
} from './wasm/isard.js';
