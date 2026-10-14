/** iTowns (2.46.1-next.72, the version of junn-apps) running the benchmark scenario */
import * as itowns from 'itowns';
import { PerspectiveCamera } from 'three';
import { benchmark, FOV, LILLE_MESH, ORTHO, type Pose, SSE, START } from './scenario.ts';

const container = document.getElementById('view');
if (!(container instanceof HTMLDivElement)) throw new Error('#view missing');

const view = new itowns.GlobeView(container, {
  coord: new itowns.Coordinates('EPSG:4326', START.lon, START.lat, 0),
  range: START.range,
  tilt: 90 - START.tilt,
  heading: START.heading,
});
// Same field of view as the other engines
if (view.camera3D instanceof PerspectiveCamera) {
  view.camera3D.fov = FOV;
  view.camera3D.updateProjectionMatrix();
}

const ortho = new itowns.WMTSSource({
  url: ORTHO.url,
  crs: 'EPSG:3857',
  name: ORTHO.layer,
  tileMatrixSet: ORTHO.tileMatrixSet,
  format: ORTHO.format,
});
view.addLayer(new itowns.ColorLayer('ortho', { source: ortho }));

const mesh = new itowns.OGC3DTilesLayer('lille', { source: new itowns.OGC3DTilesSource({ url: LILLE_MESH }) });
// The screen-space error target (iTowns' sseThreshold, not in its typings)
mesh.tilesRenderer.errorTarget = SSE;
itowns.View.prototype.addLayer.call(view, mesh);

const engine = {
  name: 'iTowns 2.46.1-next.72',
  setPose(pose: Pose): void {
    void view.controls?.lookAtCoordinate(
      {
        coord: new itowns.Coordinates('EPSG:4326', pose.lon, pose.lat, 0),
        range: pose.range,
        tilt: 90 - pose.tilt,
        heading: pose.heading,
      },
      false,
    );
  },
  idle() {
    const loop = view.mainLoop;
    return {
      // RENDERING_PAUSED (0), not exported by iTowns
      ground: loop.scheduler.commandsWaitingExecutionCount() === 0 && loop.renderingState === 0,
      mesh: mesh.tilesRenderer.loadProgress === 1,
    };
  },
  info(): string {
    const s = mesh.tilesRenderer.stats;
    return `3D tiles: ${s.visible} visible, ${s.downloading} downloading, ${s.parsing} parsing`;
  },
};

// For inspection from the console
Object.assign(window, { bench: { view, mesh } });

// The first phase waits for the view to be loaded: no need to wait for the globe initialization event
void benchmark(engine);
