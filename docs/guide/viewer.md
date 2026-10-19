# The viewer

A standalone page for looking at local files on an Isard map, and for other applications (a QGIS plugin, a Tauri
or Electron app, any program) to show their data in: a full-window map with the navigation and attribution
controls, a small list of layers (shown, hidden, removed, zoomed to) and a bridge that takes messages from the
application and sends back what happens on the map.

Drop files on it, or open them: GeoJSON, KML, GPX and TopoJSON (as circles, lines and fills), COPC point clouds
(`.laz`, `.copc.laz`), PMTiles archives (raster or vector) and glTF models (`.glb`, placed where they are dropped).
Dropped files are read in the page and never leave it: point clouds and archives through `blob:` URLs, by range
requests as on a server.

It lives in `apps/viewer`, apart from the npm package: `bun run build` puts it in `apps/viewer/dist`, a folder that
stands alone (the page, its module, the workers and the engines side by side, every URL relative), served as it is
under any path.

## Running it locally

```sh
bun run build
bun scripts/viewer.ts parcels.geojson lidar.copc.laz basemap.pmtiles --open   # or: bun run viewer -- …
```

The local server serves the viewer on `http://127.0.0.1:5190` (`--port` for another, `--open` opens the browser) and
adds the files it is given as layers, the view fitted to them:

| File | Layer |
|---|---|
| `.geojson`, `.json`, `.kml`, `.gpx`, `.topojson` | read by the server, sent as GeoJSON (in another CRS: reprojected) |
| `.laz`, `.copc.laz` | a point cloud, read by ranges from the server |
| `.pmtiles` | raster or vector tiles, as the archive's header says |
| `.mbtiles` | XYZ tiles read from the SQLite file (rows flipped from TMS), raster or vector as its metadata says |
| `.glb`, `.gltf` | a model at the center of the view |

It listens on 127.0.0.1 only, answers only requests addressed to `127.0.0.1:<port>` or `localhost:<port>`, and
refuses posts from another origin: a page opened elsewhere in the browser cannot drive it. The viewer's messages are
printed on its output, one JSON object per line.

| Endpoint | |
|---|---|
| `GET /` | the viewer; `/?bridge=1` connects it to the server (the URL printed) |
| `GET /bridge/events` | messages to the viewer (Server-Sent Events); the current state is replayed to each new connection |
| `POST /bridge/message` | a message from the viewer (204) |
| `POST /bridge/send` | a message, or an array of them, to the viewer: for other local tools (`curl -d '{"type":"basemap","name":"ign-ortho"}' …`) |
| `GET /bridge/host-events` | the viewer's messages, for other local tools (Server-Sent Events) |
| `GET /files/<token>/<name>` | the files given, with HTTP ranges |
| `GET /mbtiles/<token>/{z}/{x}/{y}`, `…/metadata.json` | MBTiles as XYZ tiles (their stored `Content-Encoding`), their metadata |

## The page's query

| Parameter | |
|---|---|
| `bridge=1` | messages from the server that serves the page (`bridge/events`, `bridge/message`, relative to it) |
| `basemap=` | an Isard basemap name, or `none` (default `osm-positron`) |
| `data=` | a URL to add (GeoJSON, KML, GPX, TopoJSON, COPC, PMTiles, glTF, `tileset.json`), relative to the page; repeatable |
| `center=lon,lat`, `zoom=` | the first view |

## The protocol

Plain JSON messages, the same over every transport. Their TypeScript types are in
[`apps/viewer/src/protocol.ts`](https://github.com/graffkevin/isard/blob/main/apps/viewer/src/protocol.ts) (version 1).
Unknown message types and fields are ignored: hosts and viewers may add some.

### Transports

- **HTTP**, the viewer served by the host (any `127.0.0.1` server) and opened with `?bridge=1`: the host's messages
  as Server-Sent Events at `bridge/events` (each event `data: <json>\n\n`, one message or an array), the viewer's by
  `POST bridge/message` (a JSON body, answered 204). The host replays its state on each connection (the last
  basemap, relief and camera, then every layer still there, in order): a reloaded page comes back as it was.
- **`postMessage`**, the viewer in an iframe or a webview: the same objects tagged `{ isard: 1, ...message }`, to and
  from the parent window (or the window that opened the viewer); other windows are not listened to.
- **JavaScript**, for QWebChannel, Tauri or Electron glue: `window.isardViewer.send(message)` applies a host message,
  `window.isardViewer.on(type, listener)` (`'*'` for all) listens to the viewer's messages and returns the function
  that stops listening.

### To the viewer

| Message | |
|---|---|
| `{ type: 'camera', center: [lon, lat], zoom, pitch?, bearing?, duration? }` | moves the view (eased over `duration` milliseconds) |
| `{ type: 'fitBounds', bounds: [west, south, east, north], padding?, duration? }` | shows a box |
| `{ type: 'basemap', name }` | an Isard basemap name, or `'none'` |
| `{ type: 'terrain', value: true \| false \| 'ign' }` | the relief |
| `{ type: 'addGeoJSON', id, data, style? }` | a FeatureCollection in longitudes and latitudes: points as circles, lines as lines, polygons filled with their outlines |
| `{ type: 'addRaster', id, url, attribution?, opacity?, minZoom?, maxZoom? }` | XYZ tiles, a WMS with `{bbox}`, or `pmtiles://…` |
| `{ type: 'addPmtiles', id, url, kind?, style? }` | a PMTiles archive: raster, or vector drawn with `style` (a MapLibre style) or a plain style of its layers |
| `{ type: 'addVectorTiles', id, url, minZoom?, maxZoom?, attribution?, style?, layers? }` | XYZ Mapbox Vector Tiles (vector MBTiles, QGIS vector tile layers), drawn like a vector archive; `layers`: the source layers' names, else read from a tile |
| `{ type: 'addPointCloud', id, url, colorBy? }` | a COPC file; `colorBy`: `'rgb'`, `'classification'`, `'elevation'`, `'intensity'` |
| `{ type: 'addTiles3d', id, url }` | a 3D Tiles `tileset.json` |
| `{ type: 'addModels', id, url, instances }` | a glTF model at `instances` (`{ lonLat, altitude?, heading?, scale? }`); none: at the center of the view |
| `{ type: 'remove', id }`, `{ type: 'clear' }` | removes a layer, or all of them |
| `{ type: 'select', id, index }` | highlights a feature of a GeoJSON layer (its index in the collection sent), `null` for none |

An `add…` message with the `id` of a layer already there replaces it. Every one takes a `name`, shown in the list of
layers instead of the id. Relative URLs are relative to the page (the local server's `files/…` and `mbtiles/…`).

The `style` of `addGeoJSON`: `color` (CSS), `width` (lines, CSS pixels), `fill` (the polygons' color, `color` by
default), `opacity` (0 to 1; the fills are drawn at 0.35 of it), `radius` (circles), `extrude: { height, base? }`
(the properties holding each polygon's height and base, in metres, or one number for all: raised as blocks), `label` (a property shown as
text), `colorProperty` (a property holding each feature's CSS color, as QGIS's categorized and graduated renderers
give them; the features without one keep `color`).

### From the viewer

| Message | |
|---|---|
| `{ type: 'ready', version }` | the map is created (`version`: Isard's) |
| `{ type: 'moveend', camera: { center, zoom, pitch, bearing }, bounds }` | the view stopped (sent 150 ms after) |
| `{ type: 'click', lonLat, hit }` | a click; `hit`: `{ id, index, properties }` of the feature under it (`index` in the collection sent, for GeoJSON; a point cloud's point with its attributes), or `null` |
| `{ type: 'error', message, id? }` | a message that failed (a layer's `id`), a file that could not be read |

## Embedding it

In an iframe, the viewer's folder served with your page:

```html
<iframe id="map" src="/isard-viewer/index.html?basemap=osm-liberty"></iframe>
<script>
  const frame = document.getElementById('map');
  addEventListener('message', ({ source, data }) => {
    if (source !== frame.contentWindow || data?.isard !== 1) return;
    if (data.type === 'ready') {
      frame.contentWindow.postMessage({ isard: 1, type: 'addGeoJSON', id: 'sites', data: sites }, '*');
    }
    if (data.type === 'click' && data.hit) showDetails(data.hit.properties);
  });
</script>
```

In a webview (Qt WebEngine, Tauri, Electron), serve the folder from a local server bound to 127.0.0.1 (the engine
loads its WebAssembly and workers by URL: `file://` does not work), then drive it with `window.isardViewer` through
the webview's script calls, or with the HTTP bridge as the local server does. The QGIS plugin in `integrations/qgis`
does the latter: it serves the viewer and its layers from QGIS and sends them over the same protocol.

## Limits

- MBTiles need the local server (SQLite): a dropped `.mbtiles` is refused with a message.
- PMTiles archives compressed with brotli or zstd are not read (gzip and none are); the archives of Protomaps and
  `pmtiles convert` are gzipped.
- Dropped COPC files and archives rely on range requests over `blob:` URLs, which Chrome, Firefox and Safari answer
  (206); a browser that did not would fail a large point cloud with "the server ignores range requests".
- A vector archive or vector tiles without a style are drawn with a plain style: each layer in a color, the `name`
  of points as labels.
