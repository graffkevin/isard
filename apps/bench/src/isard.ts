/** isard running the benchmark scenario */
import { GeoMap } from 'isard';
import { benchmark, FOV, LILLE_MESH, type Pose, SSE, START } from './scenario.ts';

const found = document.getElementById('view');
if (!(found instanceof HTMLDivElement)) throw new Error('#view missing');
const container: HTMLDivElement = found;

/** Zoom whose camera distance is `range` metres at latitude `lat` (`MapLibre` conventions, same field of view) */
const zoom = (range: number, lat: number): number => {
  const centerDistance = (0.5 * container.clientHeight) / Math.tan(((FOV / 2) * Math.PI) / 180);
  const metresAround = 2 * Math.PI * 6_378_137 * Math.cos((lat * Math.PI) / 180);
  return Math.log2(((centerDistance / range) * metresAround) / 512);
};

const map = await GeoMap.create(container, {
  basemap: 'ign-ortho',
  center: [START.lon, START.lat],
  zoom: zoom(START.range, START.lat),
  interactive: false,
  tiles3d: true,
});
const mesh = map.addTiles3d({ url: LILLE_MESH, maxScreenSpaceError: SSE });
// For inspection from the console
Object.assign(window, { benchMap: map, benchMesh: mesh });

await benchmark({
  name: 'isard',
  setPose(pose: Pose): void {
    map.setCamera({
      center: [pose.lon, pose.lat],
      zoom: zoom(pose.range, pose.lat),
      pitch: pose.tilt,
      bearing: pose.heading,
    });
  },
  idle() {
    return {
      // Same criterion as the others: nothing loading or waiting (a tile missing on the server keeps its ancestor as a
      // stand-in for good)
      ground: map.stats().every((l) => l.inFlight === 0 && l.queued === 0),
      mesh: (mesh.stats()?.inFlight ?? 0) === 0,
    };
  },
  meshTiles: () => mesh.stats()?.drawn ?? 0,
  info(): string {
    const s = mesh.stats();
    return s ? `3D tiles: ${s.drawn} drawn, ${s.loaded} kept (${s.gpuMb.toFixed(0)} MB)` : '';
  },
});
