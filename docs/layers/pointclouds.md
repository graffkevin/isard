# Point clouds

```ts
const lidar = map.addPointCloud({
  url: 'https://data.geopf.fr/telechargement/download/LiDARHD-NUALID/…/LHD_FXX_0652_6862_PTS_LAMB93_IGN69.copc.laz',
  colorBy: 'classification', // 'rgb' | 'classification' | 'elevation' | 'intensity'
  edl: true,
  pointBudget: 2_000_000,
});
lidar.setStyle({ colorBy: 'elevation', classifications: { 66: { visible: false } } });
lidar.stats(); // { points, nodes, loaded, inFlight, gpuMb, known }
lidar.info(); // { points, format, wkt, center, altitudes } once the head of the file is read
map.pickFeature(point, { layers: [lidar] }); // feature: a Point, properties { classification, intensity, altitude… }
```

Two formats:

- **COPC** (Cloud Optimized Point Cloud: a LAZ 1.4 file whose points are grouped in the nodes of an octree, each node a
  compressed chunk read by an HTTP range request): the IGN distributes [LiDAR HD](../guide/geoplateforme#lidar-hd) so,
  and PDAL, Entwine and untwine write it. Point formats 6, 7 and 8 (7 and 8 with colors), its hierarchy read page by
  page as the view reaches it. The server must answer range requests (`206`) and allow the `Range` header across
  origins (CORS).
- **3D Tiles point clouds**: `.pnts` (positions plain or quantized, RGB, RGBA, RGB565 or one color, normals plain or
  oct-encoded, Draco-compressed with `3DTILES_draco_point_compression`) and glTF points (mode 0, meshopt, quantized or
  Draco), in a `tileset.json` (`format: '3dtiles'`, the default for a `.json` URL). They are drawn by the
  [3D Tiles](./tiles3d) traversal, in their colors, lit by their normals; `pointSize` and `maxScreenSpaceError` apply.
  `addTiles3d` draws them too, with meshes in the same tileset.

## Loading

The octree is traversed by screen-space error, as Potree does: nodes taken by priority (the nearest, the coarsest and
those near the middle of the screen first), each refined while the spacing of its points covers more than
`maxScreenSpaceError` pixels (1.5), until `pointBudget` points are selected (3 000 000). Points refine additively: a
node's points stay under its children's. The nodes in view are requested in that order, their downloads aborted when
they leave the selection; past a GPU memory budget (twice the budget's vertices), the nodes least recently drawn are
unloaded.

The chunks are fetched and decoded in the decoding workers, by the point cloud module: the LAZ decoder (the `laz`
crate, Apache-2.0), 43 KB gzipped, downloaded with the first COPC chunk; the engines do not hold it. `workers: false`
leaves COPC point clouds out. A server that turns requests away for their number (`429`) is asked again after a
pause; the Géoplateforme, which allows 10 requests a second, is asked 8 times a second at most
(`requestsPerSecond`).

Point clouds are drawn by the engine with 3D Tiles: the first one loads it (`tiles3d: true` to start with it,
`'preload'` to load it ahead, see [3D Tiles](./tiles3d#the-engine-with-3d-tiles)); the layer shows once it is there.

## CRS and heights

The positions are reprojected as they are decoded: the CRS of the file comes from its WKT, its EPSG code taken from it
for the built-in CRSs (Lambert-93, the CC zones, UTM: see [projections](../guide/projections)) or those registered
with `registerCrs`; `crs` gives another (`proj4('EPSG:2992')`, or any object with `forward` and `inverse`). The page's
CRS is called on a grid of each node (9 × 9 positions at most, one every 125 m), every point interpolated in it: exact
to well under a millimetre, any projection, in the workers.

Heights: the engine's are ellipsoidal (those of 3D Tiles, and of the IGN's relief raised by the geoid). Altitudes in
NGF-IGN69, the default for Lambert-93 and the CC zones (`geoid`), are raised by the RAF20 geoid height; `verticalScale`
gives the metres of a unit of z when the WKT does not (its vertical unit, a US survey foot…).

## Style

| | |
|---|---|
| `colorBy` | `'rgb'` (the default for files with colors), `'classification'` (the default otherwise), `'elevation'`, `'intensity'` |
| `classifications` | `{ [code]: { color, visible } }` over `CLASSIFICATIONS`: ASPRS, then LiDAR HD's 64 (permanent structures), 65 (artefacts, hidden), 66 (virtual points), 67 (miscellaneous buildings); the noise 7 and 18 hidden |
| `elevationRange`, `elevationRamp` | altitudes (metres, the file's datum) mapped onto the ramp; the file's by default |
| `intensityRange` | intensities mapped from black to white; the bulk of the first node's by default |
| `pointSize`, `maxPointSize` | CSS pixels (2, 4) |
| `sizeMode` | `'adaptive'` (the spacing of the finest level drawn there, at the point's distance, between the two sizes) or `'fixed'` |
| `shape` | `'circle'` or `'square'` |
| `edl` | Eye-Dome Lighting: each point shaded by the depth of its neighbours on screen, the relief of a cloud without colors or normals read at once; `true`, or `{ strength, radius }` |
| `maxScreenSpaceError`, `pointBudget` | see [loading](#loading) |

Points are drawn with depth: the relief and the buildings hide them, they hide what is behind them. With EDL they are
drawn into a framebuffer of their own, then laid over the frame with their depth.

## Picking

`pickFeature` (and `'hoverfeature'`) gives the point under the cursor of a COPC cloud: a GeoJSON Point at its
longitude, latitude and ellipsoidal height, its properties `classification`, `intensity`, `altitude` (in the file's
datum), `returnNumber`, `numberOfReturns`, `rgb`. Its attributes are read back from the GPU: nothing more is kept in
memory for it.

<LiveExample slug="pointcloud" height="520px" />

EPT and Potree (1.x, 2.0) are not read: their trees are many files, which COPC puts in one.
