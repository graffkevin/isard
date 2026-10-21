/**
 * 3D panels in React: JSX standing in 3D (`<Panel>`, any content: a video, an iframe, a page): a photo 40 metres wide
 * on the forecourt of the Sacré-Cœur and a sign below it, both facing the camera's first view; markers raised 30 and
 * 60 metres above the ground, their stems down to it (`<Marker>`'s `height`). Placed after each frame by a CSS 3D
 * transform from the camera.
 */
import type { LonLat } from 'isard';
import { IsardMap, Marker, Panel } from 'isard/react';
import { createRoot } from 'react-dom/client';
import { element } from '../../shared/page.ts';

const RAISED: [LonLat, number][] = [
  [[2.3418, 48.8859], 30],
  [[2.3446, 48.8857], 60],
];

const App = () => (
  <>
    <IsardMap
      id="map"
      center={[2.3431, 48.8858]}
      zoom={17.2}
      pitch={62}
      bearing={160}
      basemap="ign-plan-grey-vector"
      terrain={{ model: 'ign' }}
      projection="mercator"
    >
      <Panel lonLat={[2.3431, 48.8861]} width={40} heading={340} height={4}>
        {/* Any JSX stands in 3D: a video, an iframe, a page; here a photo of a street of Paris */}
        <div className="screen">
          <img
            src="https://www.itowns-project.org/itowns-sample-data-small/images/140616/Paris-140616_0740-300-00001_0000482.jpg"
            alt="A street of Paris"
          />
        </div>
      </Panel>
      <Panel lonLat={[2.3433, 48.8848]} width={30} heading={340} height={2}>
        <div className="sign">Square Louise-Michel</div>
      </Panel>
      {RAISED.map(([lonLat, height]) => (
        <Marker key={height} lonLat={lonLat} height={height} />
      ))}
    </IsardMap>
    <div className="panel">
      <h1>3D panels</h1>
      <p>
        A photo and a sign standing in 3D on the forecourt of the Sacré-Cœur (<code>Panel</code>: any element, a video,
        an iframe), and markers raised above the ground with their stem down to it. Turn the map around them.
      </p>
    </div>
  </>
);

createRoot(element('root')).render(<App />);
