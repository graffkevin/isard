/** 3DTilesRendererJS (0.5.3, three.js 0.182) running the benchmark scenario: ortho on a generated ellipsoid surface */
import { TilesRenderer, WGS84_ELLIPSOID } from '3d-tiles-renderer';
import { GeneratedSurfacePlugin, XYZTilesOverlay } from '3d-tiles-renderer/plugins';
import {
  MathUtils,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  PerspectiveCamera,
  Scene,
  Vector3,
  WebGLRenderer,
} from 'three';
import { benchmark, FOV, LILLE_MESH, ORTHO, type Pose, SSE } from './scenario.ts';

const found = document.getElementById('view');
if (!(found instanceof HTMLDivElement)) throw new Error('#view missing');
const container: HTMLDivElement = found;

// ?logdepth=1: logarithmic depth buffer, as in the library's globe examples (black at close range with software
// rendering, hence off by default)
const logarithmicDepthBuffer = new URLSearchParams(location.search).get('logdepth') === '1';
const renderer = new WebGLRenderer({ antialias: true, logarithmicDepthBuffer });
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(container.clientWidth, container.clientHeight);
renderer.setClearColor(0x000000);
container.append(renderer.domElement);
const camera = new PerspectiveCamera(FOV, container.clientWidth / container.clientHeight, 1, 1e8);
const scene = new Scene();

const ortho = new XYZTilesOverlay({
  url: `${ORTHO.url}?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&LAYER=${ORTHO.layer}&STYLE=normal&TILEMATRIXSET=${ORTHO.tileMatrixSet}&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}&FORMAT=${ORTHO.format}`,
  levels: 20,
});
const ground = new TilesRenderer();
ground.registerPlugin(
  new GeneratedSurfacePlugin({ overlay: ortho, projection: 'ellipsoid', applyOverlayTexture: true }),
);
const mesh = new TilesRenderer(LILLE_MESH);
mesh.errorTarget = SSE;
// The photomesh declares PBR materials and the scene has no light: they would render black. Like iTowns and
// isard, its textures are shown unlit
mesh.addEventListener('load-model', ({ scene }) => {
  scene.traverse((object) => {
    if (object instanceof Mesh && object.material instanceof MeshStandardMaterial) {
      const lit = object.material;
      object.material = new MeshBasicMaterial({ map: lit.map, color: lit.color, side: lit.side });
      lit.dispose();
    }
  });
});
for (const tiles of [ground, mesh]) {
  tiles.setCamera(camera);
  tiles.setResolutionFromRenderer(camera, renderer);
  scene.add(tiles.group);
}

const east = new Vector3();
const north = new Vector3();
const up = new Vector3();
const target = new Vector3();

function setPose(pose: Pose): void {
  const lat = MathUtils.degToRad(pose.lat);
  const lon = MathUtils.degToRad(pose.lon);
  WGS84_ELLIPSOID.getCartographicToPosition(lat, lon, 0, target);
  WGS84_ELLIPSOID.getEastNorthUpAxes(lat, lon, east, north, up);
  const tilt = MathUtils.degToRad(pose.tilt);
  const heading = MathUtils.degToRad(pose.heading);
  // Horizontal direction the camera looks to, then the camera behind the target, raised by the tilt
  const forward = east
    .clone()
    .multiplyScalar(Math.sin(heading))
    .add(north.clone().multiplyScalar(Math.cos(heading)));
  const offset = up
    .clone()
    .multiplyScalar(Math.cos(tilt))
    .add(forward.clone().multiplyScalar(-Math.sin(tilt)));
  camera.position.copy(target).addScaledVector(offset, pose.range);
  // Screen up: perpendicular to the view, towards the heading
  camera.up.copy(forward.multiplyScalar(Math.cos(tilt)).add(up.clone().multiplyScalar(Math.sin(tilt)))).normalize();
  camera.near = Math.max(1, pose.range * 0.01);
  camera.far = pose.range * 10 + 2e7;
  camera.updateProjectionMatrix();
  camera.lookAt(target);
}

// For inspection from the console
Object.assign(window, { bench: { camera, ground, mesh, renderer } });

// Continuous rendering, the usual way of the library
renderer.setAnimationLoop(() => {
  camera.updateMatrixWorld();
  ground.update();
  mesh.update();
  renderer.render(scene, camera);
});

await benchmark({
  name: '3DTilesRendererJS 0.5.3',
  setPose,
  idle: () => ({ ground: ground.loadProgress === 1, mesh: mesh.loadProgress === 1 }),
  meshTiles: () => mesh.visibleTiles.size,
  info: () => `3D tiles: ${mesh.visibleTiles.size} visible, load ${Math.round(mesh.loadProgress * 100)} %`,
});
