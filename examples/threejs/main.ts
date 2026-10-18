/**
 * three.js in the map: a custom layer renders a three.js scene into the map's WebGL context at its place among the
 * layers, in the '3d' mode: the IGN's BD TOPO buildings (Géoplateforme WFS), raised to their height, hide the objects
 * passing behind them and are hidden by them. The scene is laid out in metres around an anchor (x east, y north, z
 * up); `frame.matrixAt(anchor)` is the camera's matrix for it, exact at any zoom.
 */
import { GeoMap, geoplateformeWfs, type LonLat } from 'isard';
import * as THREE from 'three';
import { element } from '../shared/page.ts';

const ANCHOR: LonLat = [2.4225, 48.8452];
// `?still`: the scene drawn at a fixed time, without animation (the visual tests)
const still = new URLSearchParams(location.search).has('still');

const map = await GeoMap.create(element('map'), {
  center: ANCHOR,
  zoom: 16.6,
  pitch: 60,
  bearing: -25,
  basemap: 'ign-plan-grey-vector',
  projection: 'mercator',
});

const buildings = await geoplateformeWfs<{ hauteur: number | null }>(
  'BDTOPO_V3:batiment',
  [2.414, 48.84, 2.432, 48.851],
);
map.addExtrusions({ data: buildings, height: (p) => p.hauteur ?? 6, color: '#d9d6cc' });

// The scene, in metres around the anchor: z up
const scene = new THREE.Scene();
scene.add(new THREE.HemisphereLight(0xffffff, 0x8899aa, 1.6));
const sun = new THREE.DirectionalLight(0xffffff, 2.2);
sun.position.set(0.5, -0.8, 1);
scene.add(sun);

// A knot hovering over the roofs
const knot = new THREE.Mesh(
  new THREE.TorusKnotGeometry(14, 4, 160, 20),
  new THREE.MeshLambertMaterial({ color: 0xe8553b }),
);
knot.position.set(0, 0, 60);
scene.add(knot);

// Cubes circling at street level: they pass behind the buildings
const ring = new THREE.Group();
const colors = [0x2f7de1, 0x2bb673, 0xf2b134, 0x8e5bd6, 0x1fb5c4, 0xe0457b];
colors.forEach((color, i) => {
  const cube = new THREE.Mesh(new THREE.BoxGeometry(14, 14, 14), new THREE.MeshLambertMaterial({ color }));
  const angle = (i / colors.length) * Math.PI * 2;
  cube.position.set(Math.cos(angle) * 140, Math.sin(angle) * 140, 12);
  ring.add(cube);
});
scene.add(ring);

// A balloon bobbing between the buildings, 200 m to the north-east
const balloon = new THREE.Mesh(
  new THREE.SphereGeometry(10, 32, 16),
  new THREE.MeshLambertMaterial({ color: 0xffd23f }),
);
balloon.position.set(140, 160, 25);
scene.add(balloon);

const camera = new THREE.Camera();
let renderer: THREE.WebGLRenderer | undefined;

map.addCustomLayer({
  id: 'three',
  renderingMode: '3d',
  onAdd(gl) {
    // On the map's canvas and context; the map clears the frame
    renderer = new THREE.WebGLRenderer({ canvas: map.getCanvas(), context: gl, antialias: true });
    renderer.autoClear = false;
  },
  render(gl, frame) {
    if (!renderer) return;
    const t = still ? 2 : performance.now() / 1000;
    knot.rotation.set(t * 0.6, 0, t * 0.9);
    ring.rotation.z = t * 0.25;
    for (const cube of ring.children) cube.rotation.set(t, t * 0.7, 0);
    balloon.position.z = 25 + Math.sin(t * 1.5) * 6;
    // The camera's matrix for the scene's frame; three.js forgets the WebGL state the map changed
    camera.projectionMatrix.fromArray(frame.matrixAt(ANCHOR));
    camera.projectionMatrixInverse.copy(camera.projectionMatrix).invert();
    renderer.resetState();
    renderer.setViewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
    renderer.render(scene, camera);
  },
  onRemove() {
    renderer?.dispose();
    renderer = undefined;
  },
});

// Animated: a frame asked for every frame
const tick = () => {
  map.requestRender();
  requestAnimationFrame(tick);
};
if (!still) requestAnimationFrame(tick);
