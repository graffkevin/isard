/**
 * Relief and a ground shader in React: the IGN terrain model (RGE ALTI) and PLAN.IGN with its hillshade under the
 * Mont Blanc, from the Géoplateforme, a sky and fog, and a fragment shader of the page run over the ground itself
 * (rings every 500 m around the cursor), so that it lies on the mountains exactly as the basemap does. The shader gets
 * the Mercator offset of each pixel from its anchor, exact at any zoom.
 */
import type { GroundLayer, LonLat } from 'isard';
import { IsardMap, Layer, useMapEvent } from 'isard/react';
import { useRef } from 'react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const MONT_BLANC: LonLat = [6.8652, 45.8326];

const CODE = `
    uniform vec2 u_cursor;
    uniform float u_metres;
    vec4 ground(vec2 offset) {
      // Mercator metres from the cursor, then ground metres (Web Mercator stretches by 1 / cos(latitude))
      float d = length((offset - u_cursor) * ISARD_CIRCUMFERENCE) * u_metres;
      if (d > 3000.0) return vec4(0.0);
      // Distance to the nearest ring, metres; lines two pixels wide whatever the zoom (fwidth: metres per pixel)
      float ring = abs(fract(d / 500.0 + 0.5) - 0.5) * 500.0;
      float line = 1.0 - smoothstep(fwidth(d), 2.0 * fwidth(d), ring);
      float fade = 1.0 - d / 3000.0;
      vec3 c = vec3(0.95, 0.35, 0.15);
      float a = max(line * 0.95, 0.18) * fade;
      return vec4(c * a, a);
    }`;
// The uniforms at creation; the cursor's is then set on the handle, at each move
const UNIFORMS = { u_cursor: [0, 0], u_metres: Math.cos((MONT_BLANC[1] * Math.PI) / 180) };
const TERRAIN = { model: 'ign', exaggeration: 1.5 } as const;
const ATMOSPHERE = {};

const merc = ([lon, lat]: LonLat) => [
  (lon + 180) / 360,
  0.5 - Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360)) / (2 * Math.PI),
];
const [ax, ay] = merc(MONT_BLANC);

/** The rings, following the cursor: the uniform set in the 'hover' of the frame drawn, not through a state */
const Rings = () => {
  const rings = useRef<GroundLayer>(undefined);

  useMapEvent('hover', ({ lonLat }) => {
    if (!lonLat) return;

    const [x, y] = merc(lonLat);

    rings.current?.setUniform('u_cursor', [(x ?? 0) - (ax ?? 0), (y ?? 0) - (ay ?? 0)]);
  });

  return <Layer ref={rings} type="ground" anchor={MONT_BLANC} code={CODE} uniforms={UNIFORMS} />;
};

const App = () => (
  <>
    <IsardMap
      id="map"
      center={[6.92, 45.87]}
      zoom={12.2}
      pitch={68}
      bearing={-140}
      maxPitch={80}
      basemap="ign-plan"
      hillshade={0.5}
      terrain={TERRAIN}
      atmosphere={ATMOSPHERE}
      projection="mercator"
    >
      <Rings />
    </IsardMap>
    <div className="panel">
      <h1>Relief and a ground shader</h1>
      <p>
        The IGN relief (RGE ALTI, ×1.5) under the Mont Blanc, a sky, and the application's GLSL drawn over the ground:
        rings every 500 m around the cursor.
      </p>
    </div>
  </>
);

createRoot(element('root')).render(<App />);
