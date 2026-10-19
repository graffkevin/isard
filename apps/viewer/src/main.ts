/**
 * The Isard viewer: a full-window map for local files (dropped, opened, or given to the local server) and for the
 * apps that drive it through its bridge (docs/guide/viewer.md). Query: `?bridge=1` (the local server's events),
 * `?basemap=` (a basemap name or `none`), `?data=` (a URL to add, repeatable; relative to the page), `?center=lon,lat`,
 * `?zoom=`.
 */
import {
  type Basemap,
  GeoMap,
  NavigationControl,
  PMTiles,
  PointCloudLayer,
  parseGpx,
  parseKml,
  parseTopoJson,
  type Topology,
} from 'isard';
import pkg from 'isard/package.json' with { type: 'json' };
import { Bridge } from './bridge.ts';
import { boundsCamera, Layers } from './layers.ts';
import type { AddMessage, Bounds, GeoJsonCollection, HostMessage, LonLat, ViewerMessage } from './protocol.ts';

const BASEMAPS = ['osm-positron', 'osm-liberty', 'osm-bright', 'ign-plan', 'ign-plan-grey', 'ign-ortho', 'none'];

const query = new URLSearchParams(location.search);
const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const center = query.get('center')?.split(',').map(Number);
const zoom = Number(query.get('zoom'));
const map = await GeoMap.create(element('map'), {
  basemap: (query.get('basemap') ?? 'osm-positron') as Basemap,
  ...(center?.length === 2 && center.every(Number.isFinite) ? { center: center as LonLat } : { center: [2.35, 48.85] }),
  zoom: Number.isFinite(zoom) && query.has('zoom') ? zoom : 3,
  attributionControl: { compact: true },
});
map.addControl(new NavigationControl({ visualizePitch: true }), 'top-right');

// ── Messages ──

let bridge: Bridge | undefined;
const post = (message: ViewerMessage) => bridge?.send(message);
const toast = element<HTMLDivElement>('toast');
let toastTimer = 0;
function report(message: string, id?: string): void {
  console.warn('isard viewer:', id ?? '', message);
  toast.textContent = id ? `${id}: ${message}` : message;
  toast.style.display = 'block';
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.style.display = 'none';
  }, 5000);
  post({ type: 'error', message, ...(id ? { id } : {}) });
}

const layers = new Layers(map, report);

function size(): [number, number] {
  const canvas = map.getCanvas();
  return [canvas.clientWidth, canvas.clientHeight];
}

function fit(bounds: Bounds, padding = 40, duration = 0): void {
  const camera = boundsCamera(bounds, size(), padding);
  if (duration > 0) map.easeTo({ ...camera, pitch: 0, bearing: 0 }, duration);
  else map.setCamera({ ...camera, pitch: 0, bearing: 0 });
}

/** Applies a host message */
function apply(message: HostMessage): void {
  try {
    switch (message.type) {
      case 'camera': {
        const camera = {
          center: message.center,
          zoom: message.zoom,
          ...(message.pitch === undefined ? {} : { pitch: message.pitch }),
          ...(message.bearing === undefined ? {} : { bearing: message.bearing }),
        };
        if (message.duration) map.easeTo(camera, message.duration);
        else map.setCamera(camera);
        break;
      }
      case 'fitBounds':
        fit(message.bounds, message.padding ?? 40, message.duration ?? 0);
        break;
      case 'basemap':
        map.setBasemap(message.name as Basemap);
        basemapSelect.value = message.name;
        break;
      case 'terrain':
        map.setTerrain(message.value);
        break;
      case 'remove':
        layers.remove(message.id);
        break;
      case 'clear':
        layers.clear();
        break;
      case 'select':
        layers.select(message.id, message.index);
        break;
      default:
        if (typeof message.type === 'string' && message.type.startsWith('add') && 'id' in message) {
          if (typeof message.id !== 'string' || !message.id) throw new Error(`${message.type} without an id`);
          layers.add(message as AddMessage);
        }
      // Other types are ignored: later versions of the protocol
    }
  } catch (error) {
    report(error instanceof Error ? error.message : String(error), 'id' in message ? String(message.id) : undefined);
  }
}

bridge = new Bridge(apply, { http: query.get('bridge') === '1' });

// The view, once it stops (a little after: an animation's last moves come together)
let moveTimer = 0;
map.on('moveend', () => {
  clearTimeout(moveTimer);
  moveTimer = window.setTimeout(() => {
    const { center: c, zoom: z, pitch, bearing } = map.getCamera();
    post({
      type: 'moveend',
      camera: { center: [c[0], c[1]], zoom: z, pitch, bearing },
      bounds: map.getBounds(),
    });
  }, 150);
});

map.on('click', ({ point, lonLat }) => {
  const picked = map.pickFeature(point, { radius: 4 });
  const entry = picked && layers.find(picked.layer);
  let hit: Extract<ViewerMessage, { type: 'click' }>['hit'] = null;
  if (picked && entry) {
    const m = entry.message;
    const properties =
      m.type === 'addGeoJSON'
        ? (m.data.features[picked.index]?.properties ?? null)
        : ((picked.feature?.properties as Record<string, unknown> | undefined) ?? null);
    hit = { id: entry.id, index: picked.index, properties };
  }
  post({ type: 'click', lonLat: lonLat ? [lonLat[0], lonLat[1]] : null, hit });
});

// ── Files ──

/** A layer id from a file name, unique */
function idOf(name: string): string {
  let id = name;
  for (let n = 2; layers.entries.has(id); n += 1) id = `${name}-${n}`;
  return id;
}

/** A feature collection from parsed JSON: a collection, a feature, a geometry, or TopoJSON */
function collection(json: unknown): GeoJsonCollection {
  const value = json as { type?: string; objects?: Record<string, unknown> };
  if (value?.type === 'Topology' && value.objects) {
    const name = Object.keys(value.objects)[0];
    return parseTopoJson(value as Topology, name) as unknown as GeoJsonCollection;
  }
  if (value?.type === 'FeatureCollection') return value as GeoJsonCollection;
  if (value?.type === 'Feature')
    return { type: 'FeatureCollection', features: [value as GeoJsonCollection['features'][number]] };
  if (typeof value?.type === 'string') {
    return { type: 'FeatureCollection', features: [{ type: 'Feature', geometry: value as never, properties: {} }] };
  }
  throw new Error('not GeoJSON');
}

const kindOf = (name: string) => {
  const lower = name.toLowerCase().replace(/[?#].*$/, '');
  if (lower.endsWith('.laz') || lower.endsWith('.las')) return 'pointcloud';
  if (lower.endsWith('.pmtiles')) return 'pmtiles';
  if (lower.endsWith('.glb') || lower.endsWith('.gltf')) return 'model';
  if (lower.endsWith('.kml')) return 'kml';
  if (lower.endsWith('.gpx')) return 'gpx';
  if (lower.endsWith('.mbtiles')) return 'mbtiles';
  if (lower.endsWith('tileset.json')) return 'tiles3d';
  return 'json';
};

/** Adds a file (dropped or opened: read in the page) or a URL; `at`: where a model goes */
async function addSource(name: string, source: File | string, at?: LonLat): Promise<void> {
  const id = idOf(name.replace(/^.*\//, '').replace(/[?#].*$/, ''));
  const kind = kindOf(name);
  const text = async () => (typeof source === 'string' ? (await fetchOk(source)).text() : source.text());
  const blobUrl = () => (typeof source === 'string' ? undefined : URL.createObjectURL(source));
  const url = () => (typeof source === 'string' ? source : (blobUrl() as string));
  try {
    switch (kind) {
      case 'json':
      case 'kml':
      case 'gpx': {
        const body = await text();
        let data: GeoJsonCollection;
        if (kind === 'kml') data = parseKml(body) as unknown as GeoJsonCollection;
        else if (kind === 'gpx') data = parseGpx(body) as unknown as GeoJsonCollection;
        else data = collection(JSON.parse(body));
        const entry = layers.add({ type: 'addGeoJSON', id, name, data });
        if (entry.bounds) fit(entry.bounds);
        break;
      }
      case 'pmtiles': {
        const u = url();
        const header = await new PMTiles(u).header();
        layers.add({ type: 'addPmtiles', id, name, url: u }, typeof source === 'string' ? {} : { blob: u });
        fit(header.bounds);
        break;
      }
      case 'pointcloud': {
        const u = url();
        layers.add({ type: 'addPointCloud', id, name, url: u }, typeof source === 'string' ? {} : { blob: u });
        flyToCloud(id);
        break;
      }
      case 'model': {
        const u = url();
        const lonLat = at ?? (map.getCamera().center as LonLat);
        layers.add(
          { type: 'addModels', id, name, url: u, instances: [{ lonLat }] },
          typeof source === 'string' ? {} : { blob: u },
        );
        break;
      }
      case 'tiles3d':
        layers.add({ type: 'addTiles3d', id, name, url: url() });
        break;
      case 'mbtiles':
        throw new Error('MBTiles are read by the local server: bun scripts/viewer.ts file.mbtiles');
    }
  } catch (error) {
    report(error instanceof Error ? error.message : String(error), id);
  }
}

async function fetchOk(url: string): Promise<Response> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return response;
}

/** Goes to a point cloud once its head is read */
function flyToCloud(id: string): void {
  const start = Date.now();
  const poll = () => {
    const entry = layers.entries.get(id);
    const cloud = entry?.handles[0];
    const info = cloud instanceof PointCloudLayer ? cloud.info() : undefined;
    if (info) {
      map.easeTo({ center: info.center, zoom: 16, pitch: 50 }, 800);
      const [lon, lat] = info.center;
      if (entry) entry.bounds = [lon - 0.005, lat - 0.005, lon + 0.005, lat + 0.005];
      return;
    }
    if (entry && !entry.error && Date.now() - start < 30_000) setTimeout(poll, 300);
  };
  poll();
}

const addFiles = (files: Iterable<File>, at?: LonLat) => {
  for (const file of files) void addSource(file.name, file, at);
};

let depth = 0;
window.addEventListener('dragenter', (event) => {
  if (!event.dataTransfer?.types.includes('Files')) return;
  depth += 1;
  document.body.classList.add('dragging');
});
window.addEventListener('dragleave', () => {
  depth = Math.max(0, depth - 1);
  if (depth === 0) document.body.classList.remove('dragging');
});
window.addEventListener('dragover', (event) => event.preventDefault());
window.addEventListener('drop', (event) => {
  event.preventDefault();
  depth = 0;
  document.body.classList.remove('dragging');
  const rect = map.getCanvas().getBoundingClientRect();
  const at = map.unproject([event.clientX - rect.left, event.clientY - rect.top]);
  addFiles(event.dataTransfer?.files ?? [], at ? [at[0], at[1]] : undefined);
});
const open = element<HTMLInputElement>('open');
open.addEventListener('change', () => {
  addFiles(open.files ?? []);
  open.value = '';
});

// ── Panel ──

const panel = element<HTMLElement>('panel');
const collapse = element<HTMLButtonElement>('collapse');
collapse.addEventListener('click', () => {
  const collapsed = panel.classList.toggle('collapsed');
  collapse.textContent = collapsed ? '+' : '–';
  collapse.setAttribute('aria-expanded', String(!collapsed));
  collapse.setAttribute('aria-label', collapsed ? 'Expand the panel' : 'Collapse the panel');
});

const basemapSelect = element<HTMLSelectElement>('basemap');
const initial = query.get('basemap') ?? 'osm-positron';
for (const name of BASEMAPS.includes(initial) ? BASEMAPS : [...BASEMAPS, initial]) {
  basemapSelect.append(new Option(name, name, name === initial, name === initial));
}
basemapSelect.addEventListener('change', () => apply({ type: 'basemap', name: basemapSelect.value }));

const KINDS: Record<AddMessage['type'], string> = {
  addGeoJSON: 'features',
  addRaster: 'raster',
  addPmtiles: 'PMTiles',
  addVectorTiles: 'vector tiles',
  addPointCloud: 'points',
  addTiles3d: '3D Tiles',
  addModels: 'model',
};

const list = element<HTMLUListElement>('layers');
const empty = element<HTMLParagraphElement>('empty');
layers.onChange = () => {
  list.replaceChildren(
    ...[...layers.entries.values()].reverse().map((entry) => {
      const li = document.createElement('li');
      if (entry.error) li.className = 'error';
      const check = document.createElement('input');
      check.type = 'checkbox';
      check.checked = entry.visible;
      check.setAttribute('aria-label', `Show ${entry.id}`);
      check.addEventListener('change', () => layers.setVisible(entry.id, check.checked));
      const name = document.createElement('span');
      name.className = 'name';
      name.textContent = entry.message.name ?? entry.id;
      name.title = entry.error ?? 'Zoom to the layer';
      name.addEventListener('click', () => {
        if (entry.bounds) fit(entry.bounds, 40, 600);
      });
      const kind = document.createElement('span');
      kind.className = 'kind';
      kind.textContent = KINDS[entry.message.type];
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.className = 'icon';
      remove.textContent = '×';
      remove.setAttribute('aria-label', `Remove ${entry.id}`);
      remove.addEventListener('click', () => layers.remove(entry.id));
      li.append(check, name, kind, remove);
      return li;
    }),
  );
  empty.style.display = layers.entries.size ? 'none' : '';
};

// URLs of the query (relative to the page)
for (const url of query.getAll('data')) void addSource(url, url);

post({ type: 'ready', version: pkg.version });
