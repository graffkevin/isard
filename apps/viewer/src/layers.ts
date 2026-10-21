/**
 * The viewer's layers, by id: each one the add message it came from (kept, to show it again once hidden) and the map
 * handles drawing it. GeoJSON as circles, lines, fills or extrusions and labels; rasters, PMTiles, vector tiles, point
 * clouds, 3D Tiles and models through the package.
 */
import {
  basicStyle,
  type FeatureCollection,
  type GeoMap,
  type Layer,
  type LonLat,
  PMTiles,
  type Style,
  VectorTiles,
} from 'isard';
import { collectionBounds } from './bounds.ts';
import {
  type AddMessage,
  type Bounds,
  type GeoJsonCollection,
  type GeoJsonStyle,
  type HostMessage,
  isRecord,
} from './protocol.ts';

type Handle = Layer | VectorTiles;

export interface Entry {
  id: string;
  message: AddMessage;
  handles: Handle[];
  visible: boolean;
  /** Where its data lies, when known (to fit the view to it) */
  bounds: Bounds | undefined;
  error: string | undefined;
  /** Each show creates its handles anew: an async add of an older show is dropped */
  generation: number;
  /** A `blob:` URL of a dropped file, revoked with the layer */
  blob?: string;
}

/** A MapLibre style given by a host, as far as the package reads it first: its sources and its layers */
const isStyle = (value: unknown): value is Style =>
  isRecord(value) && isRecord(value.sources) && Array.isArray(value.layers);

const COLOR = '#2f6fd1';
const HIGHLIGHT = '#ff8c00';

/** The names of a vector tile's layers (protobuf: field 3, a layer; its field 1, the name) */
export const tileLayerNames = (bytes: Uint8Array): string[] => {
  const names: string[] = [];
  let pos = 0;
  const varint = () => {
    let value = 0;
    let scale = 1;

    for (;;) {
      const b = bytes[pos++];

      if (b === undefined) throw new Error('truncated');
      value += (b & 0x7f) * scale;
      if (b < 0x80) return value;
      scale *= 128;
    }
  };
  const skip = (wire: number) => {
    if (wire === 0) varint();
    else if (wire === 1) pos += 8;
    else if (wire === 2) pos += varint();
    else if (wire === 5) pos += 4;
    else throw new Error(`wire type ${wire}`);
  };

  while (pos < bytes.length) {
    const key = varint();

    if (key >> 3 === 3 && (key & 7) === 2) {
      const end = varint() + pos;

      while (pos < end) {
        const k = varint();

        if (k >> 3 === 1 && (k & 7) === 2) {
          const length = varint();

          names.push(new TextDecoder().decode(bytes.subarray(pos, pos + length)));
          pos += length;
        } else skip(k & 7);
      }
      pos = end;
    } else skip(key & 7);
  }

  return names;
};

/** An absolute URL, its `{z}`-like placeholders kept as they are (the workers resolve nothing) */
export const absolute = (url: string): string => {
  return new URL(url, location.href).href.replace(/%7B/gi, '{').replace(/%7D/gi, '}');
};

const lon2x = (lon: number) => (lon + 180) / 360;
const lat2y = (lat: number) =>
  (1 - Math.log(Math.tan(Math.PI / 4 + (Math.max(-85, Math.min(85, lat)) * Math.PI) / 360)) / Math.PI) / 2;

export class Layers {
  readonly entries = new Map<string, Entry>();
  private highlight: Handle[] = [];
  /** Told when the list changes */
  onChange: () => void = () => undefined;

  constructor(
    private readonly map: GeoMap,
    private readonly report: (message: string, id?: string) => void,
  ) {}

  /** Adds (or replaces) a layer */
  add(message: AddMessage, extra: { blob?: string } = {}): Entry {
    const old = this.entries.get(message.id);

    if (old) this.drop(old, old.blob !== extra.blob);

    const entry: Entry = {
      id: message.id,
      message,
      handles: [],
      visible: true,
      bounds: undefined,
      error: undefined,
      generation: 0,
      ...(extra.blob ? { blob: extra.blob } : {}),
    };

    this.entries.set(message.id, entry);
    this.show(entry);

    return entry;
  }

  remove(id: string): void {
    const entry = this.entries.get(id);

    if (!entry) return;
    this.drop(entry, true);
    this.entries.delete(id);
    this.onChange();
  }

  clear(): void {
    [...this.entries.keys()].forEach((id) => {
      this.remove(id);
    });
  }

  setVisible(id: string, visible: boolean): void {
    const entry = this.entries.get(id);

    if (!entry || entry.visible === visible) return;
    entry.visible = visible;
    if (visible) this.show(entry);
    else this.hide(entry);
    this.onChange();
  }

  /** The layer and feature index of a handle picked */
  find(layer: Layer): Entry | undefined {
    return [...this.entries.values()].find((entry) => entry.handles.includes(layer));
  }

  /** Highlights a feature of a GeoJSON layer (null: none) */
  select(id: string, index: number | null): void {
    this.highlight.forEach((h) => {
      this.removeHandle(h);
    });
    this.highlight = [];

    const entry = this.entries.get(id);

    if (index === null || !entry || entry.message.type !== 'addGeoJSON') return;

    const feature = entry.message.data.features[index];

    if (!feature?.geometry) return;

    const data = { type: 'FeatureCollection', features: [feature] } as FeatureCollection;
    const width = (entry.message.style?.width ?? 2) + 3;

    this.highlight = [
      this.map.addLines({ data, color: HIGHLIGHT, width }),
      this.map.addCircles({
        data,
        color: HIGHLIGHT,
        radius: (entry.message.style?.radius ?? 5) + 2,
        strokeColor: '#fff',
        strokeWidth: 2,
      }),
    ];
  }

  private removeHandle(handle: Handle): void {
    if (handle instanceof VectorTiles) handle.destroy();
    else if (this.map.hasLayer(handle)) handle.remove();
  }

  private hide(entry: Entry): void {
    entry.generation += 1;
    entry.handles.forEach((h) => {
      this.removeHandle(h);
    });
    entry.handles = [];
  }

  private drop(entry: Entry, revoke: boolean): void {
    this.hide(entry);
    if (revoke && entry.blob) URL.revokeObjectURL(entry.blob);
  }

  private show(entry: Entry): void {
    const generation = ++entry.generation;

    entry.error = undefined;

    const done = (handles: Handle[]) => {
      // Hidden or replaced meanwhile
      if (generation !== entry.generation || this.entries.get(entry.id) !== entry) {
        handles.forEach((h) => {
          this.removeHandle(h);
        });

        return;
      }
      entry.handles.push(...handles);
      this.onChange();
    };
    const fail = (error: unknown) => {
      if (generation !== entry.generation) return;
      entry.error = error instanceof Error ? error.message : String(error);
      this.report(entry.error, entry.id);
      this.onChange();
    };

    try {
      const result = this.create(entry);

      if (Array.isArray(result)) done(result);
      else result.then(done, fail);
    } catch (error) {
      fail(error);
    }
  }

  private create(entry: Entry): Handle[] | Promise<Handle[]> {
    const m = entry.message;
    const map = this.map;

    switch (m.type) {
      case 'addGeoJSON':
        entry.bounds = collectionBounds(m.data);

        return this.geojson(m.data, m.style ?? {});
      case 'addRaster':
        return [
          map.addRaster({
            url: m.url,
            ...(m.attribution ? { attribution: m.attribution } : {}),
            ...(m.opacity === undefined ? {} : { opacity: m.opacity }),
            ...(m.minZoom === undefined ? {} : { minZoom: m.minZoom }),
            ...(m.maxZoom === undefined ? {} : { maxZoom: m.maxZoom }),
          }),
        ];
      case 'addPmtiles':
        return map
          .addPmtiles(m.url, { ...(m.kind ? { kind: m.kind } : {}), ...(isStyle(m.style) ? { style: m.style } : {}) })
          .then(async (handle) => {
            const header = await new PMTiles(absolute(m.url.replace(/^pmtiles:\/\//, '')))
              .header()
              .catch(() => undefined);

            if (header) entry.bounds = header.bounds;

            return [handle];
          });
      case 'addVectorTiles':
        return this.vectorTiles(m);
      case 'addPointCloud':
        return [map.addPointCloud({ url: m.url, ...(m.colorBy ? { colorBy: m.colorBy } : {}) })];
      case 'addTiles3d':
        return [map.addTiles3d({ url: m.url })];
      case 'addModels': {
        const { center } = map.getCamera();
        const instances = m.instances.length > 0 ? m.instances : [{ lonLat: center }];

        if (m.instances.length === 0) m.instances = instances;

        const lons = instances.map((i) => i.lonLat[0]);
        const lats = instances.map((i) => i.lonLat[1]);

        entry.bounds = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];

        return [map.addModels({ url: m.url, instances })];
      }
    }
  }

  private geojson(data: GeoJsonCollection, style: GeoJsonStyle): Handle[] {
    const map = this.map;
    const collection = data as unknown as FeatureCollection;
    const color = style.color ?? COLOR;
    const opacity = style.opacity ?? 1;
    const prop = style.colorProperty;
    const colorOf = prop
      ? (p: Record<string, unknown> | null) => {
          const c = p?.[prop];

          return typeof c === 'string' && c ? c : color;
        }
      : undefined;
    const fillColor = style.fill ?? color;
    const types = new Set<string>();
    const scan = (g: unknown): void => {
      if (!isRecord(g) || typeof g.type !== 'string') return;
      if (g.type !== 'GeometryCollection') types.add(g.type.replace('Multi', ''));
      else if (Array.isArray(g.geometries)) g.geometries.forEach(scan);
    };

    data.features.forEach((f) => {
      scan(f.geometry);
    });

    const handles: Handle[] = [];
    const height = (p: Record<string, unknown> | null, key: string | number | undefined) => {
      if (typeof key === 'number') return key;

      const v = key ? Number(p?.[key]) : 0;

      return Number.isFinite(v) ? v : 0;
    };

    if (types.has('Polygon')) {
      if (style.extrude) {
        const { height: h, base } = style.extrude;

        handles.push(
          map.addExtrusions({
            data: collection,
            color: colorOf ?? fillColor,
            height: (p) => height(p, h),
            base: (p) => height(p, base),
            opacity,
          }),
        );
      } else {
        handles.push(map.addFills({ data: collection, color: colorOf ?? fillColor, opacity: 0.35 * opacity }));
      }
    }
    if (types.has('LineString') || (types.has('Polygon') && !style.extrude)) {
      handles.push(map.addLines({ data: collection, color: colorOf ?? color, width: style.width ?? 2, opacity }));
    }
    if (types.has('Point')) {
      handles.push(
        map.addCircles({
          data: collection,
          color: colorOf ?? color,
          radius: style.radius ?? 5,
          strokeColor: '#fff',
          strokeWidth: 1,
          opacity,
        }),
      );
    }

    const label = style.label;

    if (label) {
      handles.push(
        map.addSymbols({
          data: collection,
          text: (p) => {
            const v = p?.[label];

            return v === undefined || v === null ? '' : String(v);
          },
          size: 12,
          color: '#1d2433',
          haloColor: '#fff',
          haloWidth: 1.5,
          offset: types.has('Point') ? [0, 1.2] : [0, 0],
          anchor: types.has('Point') ? 'top' : 'center',
        }),
      );
    }

    return handles;
  }

  /** XYZ vector tiles: their layers' names given, or read from the tile at the view's center */
  private async vectorTiles(m: Extract<HostMessage, { type: 'addVectorTiles' }>): Promise<Handle[]> {
    const url = absolute(m.url);
    const [minZoom, maxZoom] = [m.minZoom ?? 0, m.maxZoom ?? 14];
    const layerNames = async (): Promise<string[]> => {
      if (m.layers?.length) return m.layers;

      const camera = this.map.getCamera();
      const z = Math.round(Math.max(minZoom, Math.min(maxZoom, camera.zoom)));
      const n = 2 ** z;
      const [lon, lat] = camera.center;
      const x = Math.min(n - 1, Math.floor(lon2x(lon) * n));
      const y = Math.min(n - 1, Math.floor(lat2y(lat) * n));

      const candidates: [z: number, x: number, y: number][] = [
        [z, x, y],
        [minZoom, Math.floor(x / 2 ** (z - minZoom)), Math.floor(y / 2 ** (z - minZoom))],
      ];

      for (const [tz, tx, ty] of candidates) {
        const response = await fetch(url.replace('{z}', `${tz}`).replace('{x}', `${tx}`).replace('{y}', `${ty}`));

        if (!response.ok || response.status === 204) continue;

        const names = tileLayerNames(new Uint8Array(await response.arrayBuffer()));

        if (names.length) return names;
      }
      throw new Error('no vector tile here to read the layers from: give `layers`');
    };
    if (m.style !== undefined && typeof m.style !== 'string' && !isStyle(m.style)) {
      throw new Error('style: not a MapLibre style');
    }

    const style = m.style ?? basicStyle({ tiles: [url], minzoom: minZoom, maxzoom: maxZoom }, await layerNames());
    const tiles = this.map.vectorTiles({ style, ...(m.attribution ? { attribution: m.attribution } : {}) });

    tiles.raster();
    tiles.labels();

    return [tiles];
  }
}

/** The camera that shows bounds in a viewport (Web Mercator, 512 px tiles as the map's zooms) */
export const boundsCamera = (
  [west, south, east, north]: Bounds,
  [width, height]: [number, number],
  padding = 40,
): { center: LonLat; zoom: number } => {
  const dx = Math.max(1e-9, lon2x(east) - lon2x(west));
  const dy = Math.max(1e-9, lat2y(south) - lat2y(north));
  const w = Math.max(1, width - 2 * padding);
  const h = Math.max(1, height - 2 * padding);
  const zoom = Math.log2(Math.min(w / (dx * 512), h / (dy * 512)));
  const cy = (lat2y(south) + lat2y(north)) / 2;
  const lat = (Math.atan(Math.sinh(Math.PI * (1 - 2 * cy))) * 180) / Math.PI;

  return { center: [(west + east) / 2, lat], zoom: Math.max(0, Math.min(18, zoom)) };
};
