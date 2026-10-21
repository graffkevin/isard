/**
 * GeoJSON layers in React: the communes around Paris (the IGN's ADMIN EXPRESS, from the Géoplateforme's WFS) as fills
 * colored by their population through a ramp, their outlines and their names; the commune under the cursor outlined
 * and described.
 */
import { type FeatureCollection, type FillLayer, geoplateformeWfs, IGN_ATTRIBUTION } from 'isard';
import { IsardMap, Layer } from 'isard/react';
import { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

type Commune = { nom_officiel: string; population: number; code_insee: string };

// The communes are the IGN's data
const ATTRIBUTION = { customAttribution: IGN_ATTRIBUTION };
const RAMP: [number, string][] = [
  [0, '#f7fbff'],
  [20_000, '#9ecae1'],
  [60_000, '#3182bd'],
  [150_000, '#08306b'],
];
const OUTLINE: [number, number][] = [
  [9, 0.5],
  [14, 2],
];
const LABEL_SIZE: [number, number][] = [
  [10, 10],
  [14, 14],
];
const label = (p: Commune) => p.nom_officiel;

/** The layers of the communes; `hovered`: the index of the commune under the cursor; `onFills`: the fills' layer */
const Communes = ({
  communes,
  hovered,
  onFills,
}: {
  communes: FeatureCollection<Commune>;
  hovered: number | undefined;
  onFills: (layer: FillLayer<Commune>) => void;
}) => {
  // One value per feature, colored by the ramp: one texture upload
  const populations = useMemo(() => Float32Array.from(communes.features, (f) => f.properties.population), [communes]);
  const outlined = useMemo(() => (_: Commune, i: number) => i === hovered, [hovered]);

  return (
    <>
      <Layer
        type="fills"
        data={communes}
        color="#999"
        opacity={0.55}
        ramp={RAMP}
        values={populations}
        onLayer={onFills}
      />
      <Layer type="lines" data={communes} color="#fff" width={OUTLINE} />
      <Layer type="lines" data={communes} color="#e8590c" width={3} filter={outlined} />
      <Layer
        type="symbols"
        data={communes}
        text={label}
        size={LABEL_SIZE}
        color="#1d2433"
        haloColor="#fff"
        haloWidth={1.5}
        minZoom={10.5}
      />
    </>
  );
};

const App = () => {
  const [communes, setCommunes] = useState<FeatureCollection<Commune>>();
  const [hovered, setHovered] = useState<{ index: number | undefined; commune: Commune | undefined }>();
  const [fills, setFills] = useState<FillLayer<Commune>>();
  // The fills picked, without stalling the GPU: a frame after the pointer moved
  const hoverPicking = useMemo(() => ({ layers: fills ? [fills] : [] }), [fills]);

  // The communes around Paris, fetched once; the request dropped when unmounted
  useEffect(() => {
    const controller = new AbortController();

    geoplateformeWfs<Commune>('ADMINEXPRESS-COG.LATEST:commune', [2.15, 48.72, 2.62, 48.95], {
      signal: controller.signal,
    }).then(setCommunes, (error: unknown) => {
      if (!controller.signal.aborted) console.error(error);
    });

    return () => controller.abort();
  }, []);

  const commune = hovered?.commune;

  return (
    <>
      <IsardMap
        id="map"
        center={[2.39, 48.84]}
        zoom={10.6}
        projection="mercator"
        attributionControl={ATTRIBUTION}
        hoverPicking={hoverPicking}
        onHoverFeature={({ hit }) =>
          setHovered({ index: hit?.index, commune: hit && fills?.featureAt(hit.index)?.properties })
        }
      >
        {communes && <Communes communes={communes} hovered={hovered?.index} onFills={setFills} />}
      </IsardMap>
      <div className="panel">
        <h1>GeoJSON layers</h1>
        <p>The communes around Paris (ADMIN EXPRESS), colored by population, outlined, labelled. Hover a commune.</p>
        <p>{commune ? `${commune.nom_officiel}: ${commune.population.toLocaleString('en')} inhabitants` : ' '}</p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
