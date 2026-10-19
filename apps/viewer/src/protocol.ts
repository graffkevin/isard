/**
 * The viewer's bridge protocol (v1), as docs/guide/viewer.md describes it: the messages a host (the local server, the
 * QGIS plugin, a Tauri or any other app) sends the viewer, and those the viewer sends back. Plain JSON, the same over
 * every transport: Server-Sent Events and POST (`?bridge=1`), `postMessage` (`{ isard: 1, ...message }`), or
 * `window.isardViewer`. Types only: imported by the viewer and by scripts/viewer.ts.
 */

export const PROTOCOL_VERSION = 1;

export type LonLat = [lon: number, lat: number];
export type Bounds = [west: number, south: number, east: number, north: number];

/** How a GeoJSON layer is drawn: points as circles, lines as lines, polygons filled (or raised with `extrude`) */
export interface GeoJsonStyle {
  /** CSS color (default a blue) */
  color?: string;
  /** Line and outline width, CSS pixels (default 2) */
  width?: number;
  /** Fill color of the polygons (default `color`) */
  fill?: string;
  /** 0 to 1 (default 1; the fills 0.35 of it) */
  opacity?: number;
  /** Circle radius, CSS pixels (default 5) */
  radius?: number;
  /** Polygons raised: the property with each feature's height in metres (or one height for all), and that of its base */
  extrude?: { height: string | number; base?: string | number };
  /** A property shown as a label */
  label?: string;
  /** A property holding each feature's CSS color (QGIS categorized and graduated renderers); `color` otherwise */
  colorProperty?: string;
}

export interface GeoJsonFeature {
  type: 'Feature';
  geometry: { type: string; coordinates?: unknown; geometries?: unknown[] } | null;
  properties: Record<string, unknown> | null;
  id?: string | number;
}

export interface GeoJsonCollection {
  type: 'FeatureCollection';
  features: GeoJsonFeature[];
}

export interface CameraMessage {
  type: 'camera';
  center: LonLat;
  zoom: number;
  pitch?: number;
  bearing?: number;
  /** Milliseconds of animation (default none) */
  duration?: number;
}

/** The basemaps of isard, or `'none'` */
export type BasemapMessage = { type: 'basemap'; name: string };

export type HostMessage =
  | CameraMessage
  | { type: 'fitBounds'; bounds: Bounds; padding?: number; duration?: number }
  | BasemapMessage
  | { type: 'terrain'; value: boolean | 'ign' }
  | { type: 'addGeoJSON'; id: string; data: GeoJsonCollection; style?: GeoJsonStyle; name?: string }
  | {
      type: 'addRaster';
      id: string;
      /** XYZ template, `{bbox}` WMS, or `pmtiles://` */
      url: string;
      attribution?: string;
      opacity?: number;
      minZoom?: number;
      maxZoom?: number;
      name?: string;
    }
  | {
      type: 'addPmtiles';
      id: string;
      url: string;
      kind?: 'raster' | 'vector';
      /** A MapLibre style for vector archives (its source `pmtiles://…`); a plain style of the layers otherwise */
      style?: unknown;
      name?: string;
    }
  | {
      type: 'addVectorTiles';
      id: string;
      /** XYZ template of Mapbox Vector Tiles */
      url: string;
      minZoom?: number;
      maxZoom?: number;
      attribution?: string;
      style?: unknown;
      /** The source layers' names (else read from a tile) */
      layers?: string[];
      name?: string;
    }
  | {
      type: 'addPointCloud';
      id: string;
      url: string;
      colorBy?: 'rgb' | 'classification' | 'elevation' | 'intensity';
      name?: string;
    }
  | { type: 'addTiles3d'; id: string; url: string; name?: string }
  | {
      type: 'addModels';
      id: string;
      url: string;
      instances: { lonLat: LonLat; altitude?: number; heading?: number; scale?: number }[];
      name?: string;
    }
  | { type: 'remove'; id: string }
  | { type: 'clear' }
  | { type: 'select'; id: string; index: number | null };

/** The messages adding a layer */
export type AddMessage = Extract<HostMessage, { id: string; type: `add${string}` }>;

export interface ViewCamera {
  center: LonLat;
  zoom: number;
  pitch: number;
  bearing: number;
}

export type ViewerMessage =
  | { type: 'ready'; version: string }
  | { type: 'moveend'; camera: ViewCamera; bounds: Bounds }
  | {
      type: 'click';
      lonLat: LonLat | null;
      hit: { id: string; index: number; properties: Record<string, unknown> | null } | null;
    }
  | { type: 'error'; message: string; id?: string };

/** A message over `postMessage`: tagged, so that other messages of the page are left alone */
export type PostedMessage = (HostMessage | ViewerMessage) & { isard: 1 };
