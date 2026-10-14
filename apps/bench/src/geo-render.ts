/** geo-render running the benchmark scenario */
import { GeoMap } from 'geo-render';
import { benchmark, FOV, LILLE_MESH, type Pose, SSE, START } from './scenario.ts';

const found = document.getElementById('view');
if (!(found instanceof HTMLDivElement)) throw new Error('#view missing');
const container: HTMLDivElement = found;

/** Zoom whose camera distance is `range` metres at latitude `lat` (`MapLibre` conventions, same field of view) */
function zoom(range: number, lat: number): number {
  const centerDistance = (0.5 * container.clientHeight) / Math.tan(((FOV / 2) * Math.PI) / 180);
  const metresAround = 2 * Math.PI * 6_378_137 * Math.cos((lat * Math.PI) / 180);
  return Math.log2(((centerDistance / range) * metresAround) / 512);
}

const map = await GeoMap.create(container, {
  basemap: 'ortho',
  hillshade: 0,
  center: [START.lon, START.lat],
  zoom: zoom(START.range, START.lat),
  interactive: false,
});
const mesh = map.addTiles3d({ url: LILLE_MESH, maxScreenSpaceError: SSE });

await benchmark({
  name: 'geo-render',
  setPose(pose: Pose): void {
    map.setCamera({
      center: [pose.lon, pose.lat],
      zoom: zoom(pose.range, pose.lat),
      pitch: pose.tilt,
      bearing: pose.heading,
    });
  },
  idle(): boolean {
    const tiles = mesh.stats();
    return map.stats().every((l) => l.inFlight === 0 && l.standIns === 0) && (tiles?.inFlight ?? 0) === 0;
  },
  info(): string {
    const s = mesh.stats();
    return s ? `3D tiles: ${s.drawn} drawn, ${s.loaded} kept (${s.gpuMb.toFixed(0)} MB)` : '';
  },
});
