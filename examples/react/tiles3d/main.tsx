/** 3D Tiles in React: the Lille photomesh (Métropole Européenne de Lille, 2020, 5 cm, open data) over the IGN orthophoto */
import type { Tiles3dLayer } from 'isard';
import { IsardMap, Layer } from 'isard/react';
import { useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const App = () => {
  const mesh = useRef<Tiles3dLayer>(undefined);
  const [stats, setStats] = useState('…');

  return (
    <>
      <IsardMap
        id="map"
        center={[3.0635, 50.6366]}
        zoom={17}
        pitch={55}
        bearing={30}
        basemap="ign-ortho"
        projection="mercator"
        // 3D Tiles at once: the engine that draws them from the start, not after the one without them
        tiles3d
        onRender={() => {
          const s = mesh.current?.stats();

          if (s) setStats(`${s.drawn} tiles drawn, ${s.inFlight} loading, ${s.gpuMb.toFixed(0)} MB on the GPU`);
        }}
      >
        <Layer
          ref={mesh}
          type="tiles3d"
          url="https://webimaging.lillemetropole.fr/externe/maillage/2020_mel_5cm/tileset.json"
          maxScreenSpaceError={8}
        />
      </IsardMap>
      <div className="panel">
        <h1>3D Tiles</h1>
        <p>The photomesh of the Métropole Européenne de Lille (2020, 5 cm, open data), streamed as 3D Tiles.</p>
        <p>
          <output>{stats}</output>
        </p>
      </div>
    </>
  );
};

createRoot(element('root')).render(<App />);
