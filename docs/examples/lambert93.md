# Lambert-93 data

Data in another projection on a Web Mercator map: the Géoplateforme's PLAN IGN as served in its Lambert-93 tile
matrix set (`2154_10cm_6_20`), reprojected by the map's workers over OpenFreeMap's basemap (or the same map in its Web
Mercator grid, to compare: the two match to a twentieth of a pixel); the arrondissements of Paris from the WFS in
EPSG:2154, reprojected by the layer since the collection names its CRS; a GPX file, a boat trip down the Seine with
its bridges, parsed by `parseGpx`. See [Projections](../guide/projections).

<LiveExample slug="lambert93" height="520px" />

<<< @/../examples/lambert93/main.ts
