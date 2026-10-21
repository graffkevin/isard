/**
 * Data in Lambert-93 on a Web Mercator map, in React: the Géoplateforme's PLAN IGN as served in its Lambert-93 tile
 * matrix set (2154_10cm_6_20), reprojected by the map's workers, over OpenFreeMap's basemap (or the same map in its Web
 * Mercator grid, to compare); the arrondissements of Paris from its WFS in EPSG:2154 (the collection names its CRS, the
 * layers reproject it); a GPX file: a boat trip down the Seine and its bridges.
 */
import type { FeatureCollection, Properties } from 'isard';
import { parseGpx } from 'isard/formats';
import { geoplateformeWfs, geoplateformeWmts, IGN_ATTRIBUTION } from 'isard/geoplateforme';
import { IsardMap, Layer } from 'isard/react';
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

/** PLAN IGN in each grid of the Géoplateforme: its layer, matrix set and format */
const GRIDS = {
  '2154_10cm_6_20': ['GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2.L93', 'image/jpeg'],
  PM: ['GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2', 'image/png'],
} as const;
type Grid = keyof typeof GRIDS;
const LABELS: Record<Grid, string> = { '2154_10cm_6_20': 'Lambert-93', PM: 'Web Mercator' };

/** PLAN IGN in a grid: Lambert-93 (reprojected) or Web Mercator (as the engine draws it); made once, by identity */
const PLANS = Object.fromEntries(
  (Object.keys(GRIDS) as Grid[]).map((matrixSet) => {
    const [layer, format] = GRIDS[matrixSet];

    return [matrixSet, geoplateformeWmts(layer, { matrixSet, format })];
  }),
) as Record<Grid, ReturnType<typeof geoplateformeWmts>>;

const ATTRIBUTION = { customAttribution: IGN_ATTRIBUTION };
const DASH: [number, number] = [2, 1.5];
// The waypoints' labels: above and below the river in turn, over the basemap's labels of the river
const name = (p: Properties) => String(p.name ?? '');
const timed = (p: Properties) => p.time !== undefined;
const anchor = (_: Properties, i: number): 'top' | 'bottom' => (i % 2 ? 'top' : 'bottom');
const offset = (_: Properties, i: number): [number, number] => [0, i % 2 ? 0.7 : -0.7];

const App = () => {
  const [grid, setGrid] = useState<Grid>('2154_10cm_6_20');
  const [opacity, setOpacity] = useState(0.6);
  const [arrondissements, setArrondissements] = useState<FeatureCollection>();
  const [trip, setTrip] = useState<FeatureCollection>();

  // The arrondissements in Lambert-93 (their `crs` member is enough), and the GPX file
  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    const failed = (error: unknown) => {
      if (!signal.aborted) console.error(error);
    };

    geoplateformeWfs('ADMINEXPRESS-COG.LATEST:arrondissement_municipal', [2.25, 48.83, 2.4, 48.89], {
      srsName: 'EPSG:2154',
      signal,
    }).then(setArrondissements, failed);
    fetch('../../lambert93/seine.gpx', { signal })
      .then((response) => response.text())
      .then((text) => setTrip(parseGpx(text)), failed);

    return () => controller.abort();
  }, []);

  return (
    <>
      <IsardMap id="map" center={[2.317, 48.8595]} zoom={13.2} projection="mercator" attributionControl={ATTRIBUTION}>
        {/* Another grid makes the layer anew, in its place under the others */}
        <Layer type="raster" {...PLANS[grid]} opacity={opacity} />
        {arrondissements && (
          <Layer type="lines" data={arrondissements} color="#7a2e8e" width={2.5} dash={DASH} order={1} />
        )}
        {/* The track as a line, the waypoints as discs and labels */}
        {trip && (
          <>
            <Layer type="lines" data={trip} color="#fff" width={7} opacity={0.9} order={2} />
            <Layer type="lines" data={trip} color="#1764c4" width={3.5} order={2} />
            <Layer
              type="circles"
              data={trip}
              color="#fff"
              radius={5}
              strokeColor="#1764c4"
              strokeWidth={2.5}
              order={2}
            />
            <Layer
              type="symbols"
              data={trip}
              text={name}
              filter={timed}
              size={12}
              color="#0f3d7a"
              haloColor="#fff"
              haloWidth={1.5}
              anchor={anchor}
              offset={offset}
              overlap
              order={2}
            />
          </>
        )}
      </IsardMap>
      <div className="panel">
        <h1>Lambert-93 data</h1>
        <p>
          PLAN IGN served in Lambert-93, reprojected over the Web Mercator basemap; the arrondissements from a WFS in
          EPSG:2154; a GPX trip down the Seine.
        </p>
        <p>
          {(Object.keys(GRIDS) as Grid[]).map((id) => (
            <button key={id} type="button" className={id === grid ? 'on' : ''} onClick={() => setGrid(id)}>
              {LABELS[id]}
            </button>
          ))}
        </p>
        <p>
          <input
            type="range"
            min={0}
            max={1}
            step={0.05}
            value={opacity}
            aria-label="Opacity of PLAN IGN"
            onChange={(e) => setOpacity(Number(e.target.value))}
          />
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
