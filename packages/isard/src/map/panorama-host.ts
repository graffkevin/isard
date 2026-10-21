/** What the street-level panoramas need of the map: its engine, its street-level eye, its clicks, keys and wheel. */

import type { PanoramaHost } from '../layers/panoramas.ts';
import type { GeoMap } from '../map.ts';

/** The panoramas' host on a map; `geoidHeight` from the class of the map's engine */
export const panoramaHost = (map: GeoMap, geoidHeight: (lon: number, lat: number) => number): PanoramaHost => ({
  setPanorama: (...args) => {
    map.engine.setPanorama(...args);
    map.requestRender();
  },
  setPanoramaImage: (index, image) => map.engine.setPanoramaImage(index, image),
  clearPanorama: () => map.engine.clearPanorama(),
  geoidHeight,
  firstPerson: () => map.firstPerson,
  setFirstPerson: (view) => map.setFirstPerson(view),
  elevationAt: (lonLat) => map.elevationAt(lonLat),
  listen: (click, key, wheel) => {
    const off = map.on('click', ({ lonLat }) => click(lonLat));
    const container = map.getContainer();
    // Before the map's own wheel (its zoom, the eye's field of view)
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      wheel(e);
    };

    container.addEventListener('keydown', key);
    container.addEventListener('wheel', onWheel, { capture: true, passive: false });

    return () => {
      off();
      container.removeEventListener('keydown', key);
      container.removeEventListener('wheel', onWheel, { capture: true });
    };
  },
});
