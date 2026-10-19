# QGIS plugin

The **isard 3D view** plugin shows the QGIS map and its layers in isard's viewer: the globe, the relief, the
buildings, point clouds. It works with QGIS 3.28 and later, QGIS 4 (Qt 6) included.

## Install

1. Build the archive from a checkout of the repository (it builds the viewer first when needed):

   ```sh
   bun run qgis:package   # → integrations/qgis/dist/isard_viewer-<version>.zip
   ```

2. In QGIS: **Plugins › Manage and Install Plugins › Install from ZIP**, choose the archive, **Install Plugin**.
3. Click **isard 3D view** in the toolbar (or **Plugins › isard › isard 3D view**): a panel opens.

The plugin runs a small web server on `127.0.0.1` (a free port, stopped with the plugin): the viewer's page, the
link between QGIS and the viewer, and the local files the viewer reads. It only answers the machine itself.

## In the panel or in the browser

When the QGIS build has Qt WebEngine, the viewer is drawn in the panel, WebGL enabled. Otherwise (QGIS 4 on macOS,
for one, ships without it) the viewer opens in the web browser, at `http://127.0.0.1:<port>/?bridge=1`, and the panel
stays its control panel: everything below works the same, and **Open in browser** opens another view (several views
can be open at once, each gets the current state when it connects). `ISARD_QGIS_BROWSER=1` in QGIS's environment
forces the browser.

## What is synced

| | |
|---|---|
| Camera | the view follows the map canvas (**Follow the QGIS map**); with **Move the QGIS map with the 3D view**, the canvas follows the viewer too, without the two chasing each other |
| Basemap and relief | OpenStreetMap Positron or Liberty, PLAN IGN, IGN orthophotos or none; relief off, worldwide or IGN (France) |
| Vector layers | **Send selected layer**: the features in WGS84 as GeoJSON, up to **Max. features** (a warning says how many were left out), simplified by a tolerance in metres if asked; points as circles, lines, polygons as fills or extruded by a numeric field (**Extrude by**) |
| Their style | single symbol: its color, opacity, width or size; categorized and graduated renderers: each feature's color; simple labels: their field; other renderers: their first symbol |
| Raster layers | XYZ connections (their URL template), WMS (GetMap in Web Mercator) and WMTS (Web Mercator matrix sets named by zoom), MBTiles files (served as tiles) |
| Vector tiles | XYZ vector tile connections, MBTiles and PMTiles files |
| Point clouds | COPC files (`.copc.laz`, local or by URL), colored by RGB, class, altitude or intensity; a LAS / LAZ once QGIS has indexed it (QGIS writes the `.copc.laz` next to it) |
| Clicks | a click on a sent feature in the viewer selects it in QGIS and shows its attributes in the panel |

Layers removed from the project are removed from the viewer; **Remove** and **Clear** remove them by hand.

## Limits

- The GeoJSON of a vector layer goes through the page: hundreds of thousands of features are slow to send. Lower
  **Max. features** or simplify; for buildings at large, the viewer's own vector basemaps already draw them.
- Styles are simplified: rule-based renderers, data-defined properties, symbol layers past the first, label
  expressions are not translated.
- Local raster files (GeoTIFF and others) are not sent, only MBTiles; WMS and WMTS in another projection than Web
  Mercator are not drawn.
- The viewer's basemaps and relief come from the web (OpenFreeMap, the Géoplateforme): an internet connection is
  needed for them.

## Develop

The plugin is in `integrations/qgis/isard_viewer` (Python, `qgis.PyQt` only). The server (`server.py`) and the
conversions (`convert.py`) import nothing from QGIS and are tested with the standard library:

```sh
bun run test:qgis   # python3 -m unittest, no QGIS needed
```

To try it from a checkout, link `integrations/qgis/isard_viewer` into the QGIS profile's `python/plugins` folder: the
plugin serves `apps/viewer/dist` when no viewer is bundled.
