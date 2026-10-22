<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';

/** The architecture of isard as a diagram: its parts in lanes (the page, its workers, the engine, the GPU, the
 * network), a part's role and files on click, and scenarios played step by step along the arrows */

interface Part {
  id: string;
  label: string;
  sub?: string;
  x: number;
  y: number;
  w: number;
  h: number;
  info: string;
  files: string[];
}

interface Arrow {
  id: string;
  /** Its route, corner by corner (in the diagram's units), the head at the last point */
  points: [number, number][];
}

interface Step {
  arrows: string[];
  parts: string[];
  text: string;
}

const REPO = 'https://github.com/graffkevin/isard/tree/main/';

const LANES = [
  { label: 'Your application', y: 10, h: 84 },
  { label: 'The page (main thread) · packages/isard', y: 106, h: 132 },
  { label: 'Workers', y: 250, h: 104 },
  { label: 'Engine (Rust → WebAssembly) · crates/', y: 366, h: 210 },
  { label: 'GPU', y: 588, h: 92 },
];

const PARTS: Part[] = [
  {
    id: 'app',
    label: 'Your code',
    sub: 'GeoMap.create, map.add…',
    x: 230,
    y: 30,
    w: 210,
    h: 52,
    info: 'The application: it creates the map, adds layers, markers and controls, and listens to the events.',
    files: ['examples/'],
  },
  {
    id: 'react',
    label: 'isard/react',
    sub: '<IsardMap>, <Layer>…',
    x: 470,
    y: 30,
    w: 210,
    h: 52,
    info: 'Components over the same handles: each one adds what it shows when it mounts, sends the least its props need, and removes it when it unmounts. The logic free of React is in src/bind/.',
    files: ['packages/isard/src/react/', 'packages/isard/src/bind/'],
  },
  {
    id: 'geomap',
    label: 'GeoMap',
    sub: 'the map',
    x: 40,
    y: 140,
    w: 170,
    h: 64,
    info: 'What the application holds: the canvas, the options, the events; it creates the engine and moves to the one with 3D Tiles when needed. Its parts are in src/map/.',
    files: ['packages/isard/src/map.ts', 'packages/isard/src/map/'],
  },
  {
    id: 'handles',
    label: 'Layer handles',
    sub: 'setData, setStyle…',
    x: 610,
    y: 140,
    w: 170,
    h: 64,
    info: 'A thin object per layer: its id in the engine, its data and style sent to it. Handles survive an engine change: they recreate their layer in the new one.',
    files: ['packages/isard/src/layers/'],
  },
  {
    id: 'frame',
    label: 'Frame loop',
    sub: 'frames on demand',
    x: 230,
    y: 140,
    w: 170,
    h: 64,
    info: 'A frame is drawn only when something asked for one (a move, an arrived tile, an animation), at most once per animation frame.',
    files: ['packages/isard/src/map/frame-loop.ts'],
  },
  {
    id: 'overlays',
    label: 'Overlays',
    sub: 'markers, popups, panels',
    x: 420,
    y: 140,
    w: 170,
    h: 64,
    info: 'HTML over the canvas, positioned after each frame from the camera: markers, popups, 3D panels, controls.',
    files: ['packages/isard/src/ui/', 'packages/isard/src/map/overlay-host.ts'],
  },
  {
    id: 'loader',
    label: 'Tile loader',
    sub: 'pool of workers',
    x: 800,
    y: 140,
    w: 170,
    h: 64,
    info: 'What the engine calls to load 3D Tiles content and point cloud chunks: it hands them to the decoding workers by priority and gives their results back.',
    files: ['packages/isard/src/engine/loader.ts', 'packages/isard/src/engine/protocol.ts'],
  },
  {
    id: 'bworker',
    label: 'basemap.worker',
    sub: 'vector basemaps, relief',
    x: 420,
    y: 276,
    w: 210,
    h: 60,
    info: 'Fetches and decodes vector tiles (MVT, PMTiles), draws them with the MapLibre style into images (Canvas 2D) and lists their labels; decodes the relief and reprojects rasters.',
    files: ['packages/isard/src/workers/basemap.worker.ts', 'packages/isard/src/basemap/'],
  },
  {
    id: 'dworker',
    label: 'decoder.worker',
    sub: '3D Tiles, points',
    x: 800,
    y: 276,
    w: 170,
    h: 60,
    info: 'Decodes 3D Tiles with the engine’s own module (Draco, KTX2, meshopt) and COPC chunks with the point cloud module (LAZ); transfers GPU-ready arrays back.',
    files: ['packages/isard/src/workers/decoder.worker.ts', 'crates/engine/src/worker.rs'],
  },
  {
    id: 'engine',
    label: 'Engine',
    sub: 'isard_core / isard_bg.wasm',
    x: 40,
    y: 396,
    w: 170,
    h: 150,
    info: 'The engine exported to JavaScript: it owns the GPU, the camera and every layer’s state. Two builds: without 3D Tiles (loaded first) and with them (once needed).',
    files: ['crates/engine/src/engine.rs', 'crates/engine/src/lib.rs'],
  },
  {
    id: 'camera',
    label: 'Camera & tiles',
    sub: 'crates/geo',
    x: 230,
    y: 396,
    w: 170,
    h: 64,
    info: 'The camera and its projections (globe, Mercator, first person), the bodies and the geoid, and which tiles are in view.',
    files: ['crates/geo/src/camera.rs', 'crates/geo/src/tiles.rs'],
  },
  {
    id: 'ground',
    label: 'Ground',
    sub: 'rasters over the relief',
    x: 420,
    y: 396,
    w: 170,
    h: 64,
    info: 'The raster layers composited in one pass over the relief, the globe, the sky; the raster cache and its downloads.',
    files: ['crates/engine/src/render/raster.rs', 'crates/engine/src/render/backdrop.rs'],
  },
  {
    id: 'layers',
    label: 'Layers',
    sub: 'features, buildings, networks',
    x: 610,
    y: 396,
    w: 170,
    h: 64,
    info: 'Everything drawn over the ground, in its order: lines, circles, fills, extrusions, networks, models, custom layers; then the viewsheds and the swipe.',
    files: ['crates/engine/src/render/', 'crates/engine/src/layer.rs'],
  },
  {
    id: 'tiles3d',
    label: '3D Tiles & points',
    sub: 'crates/tiles3d, pointcloud',
    x: 800,
    y: 396,
    w: 170,
    h: 64,
    info: 'The 3D Tiles tree and the COPC octree traversed each frame: what is drawn, what is wanted by priority, what is dropped over the budgets.',
    files: ['crates/tiles3d/src/', 'crates/pointcloud/src/', 'crates/engine/src/render/tiles3d.rs'],
  },
  {
    id: 'labels',
    label: 'Labels',
    sub: 'placed, drawn last',
    x: 420,
    y: 482,
    w: 170,
    h: 64,
    info: 'Text and icons at points and along lines, placed without overlap (greedy, by priority), drawn from signed distance field glyphs.',
    files: ['crates/engine/src/render/symbols.rs', 'crates/geo/src/collision.rs'],
  },
  {
    id: 'gpu',
    label: 'WebGL2',
    sub: 'the canvas',
    x: 230,
    y: 612,
    w: 740,
    h: 50,
    info: 'The canvas the engine draws into; nothing is drawn while nothing changes.',
    files: ['crates/engine/src/render/gl.rs'],
  },
  {
    id: 'net',
    label: 'Servers',
    sub: 'tiles, 3D Tiles, COPC',
    x: 1010,
    y: 106,
    w: 170,
    h: 470,
    info: 'Where the data comes from: raster tiles (WMTS, XYZ), vector tiles and their styles, 3D Tiles, point clouds (COPC, by range requests). setRequestHeaders adds an access token to every request.',
    files: ['packages/isard/src/engine/requests.ts'],
  },
];

// The routes run in the gaps between the lanes and the parts, so that no arrow crosses a part
const ARROWS: Arrow[] = [
  {
    id: 'app-geomap',
    points: [
      [230, 56],
      [125, 56],
      [125, 140],
    ],
  },
  {
    id: 'react-handles',
    points: [
      [575, 82],
      [575, 100],
      [695, 100],
      [695, 140],
    ],
  },
  {
    id: 'geomap-frame',
    points: [
      [210, 172],
      [230, 172],
    ],
  },
  {
    id: 'frame-overlays',
    points: [
      [400, 172],
      [420, 172],
    ],
  },
  {
    id: 'frame-engine',
    points: [
      [290, 204],
      [290, 362],
      [115, 362],
      [115, 396],
    ],
  },
  {
    id: 'engine-frame',
    points: [
      [165, 396],
      [165, 370],
      [340, 370],
      [340, 204],
    ],
  },
  {
    id: 'handles-engine',
    points: [
      [695, 204],
      [695, 380],
      [190, 380],
      [190, 396],
    ],
  },
  {
    id: 'geomap-engine',
    points: [
      [60, 204],
      [60, 396],
    ],
  },
  {
    id: 'engine-camera',
    points: [
      [210, 428],
      [230, 428],
    ],
  },
  {
    id: 'camera-ground',
    points: [
      [400, 428],
      [420, 428],
    ],
  },
  {
    id: 'ground-layers',
    points: [
      [590, 428],
      [610, 428],
    ],
  },
  {
    id: 'layers-labels',
    points: [
      [695, 460],
      [695, 514],
      [590, 514],
    ],
  },
  {
    id: 'engine-gpu',
    points: [
      [125, 546],
      [125, 637],
      [230, 637],
    ],
  },
  {
    id: 'ground-net',
    points: [
      [480, 396],
      [480, 358],
      [1010, 358],
    ],
  },
  {
    id: 'net-ground',
    points: [
      [1010, 348],
      [530, 348],
      [530, 396],
    ],
  },
  {
    id: 'geomap-bworker',
    points: [
      [100, 204],
      [100, 300],
      [420, 300],
    ],
  },
  {
    id: 'bworker-geomap',
    points: [
      [420, 316],
      [145, 316],
      [145, 204],
    ],
  },
  {
    id: 'bworker-net',
    points: [
      [525, 276],
      [525, 244],
      [1010, 244],
    ],
  },
  {
    id: 'loader-dworker',
    points: [
      [865, 204],
      [865, 276],
    ],
  },
  {
    id: 'dworker-loader',
    points: [
      [905, 276],
      [905, 204],
    ],
  },
  {
    id: 'dworker-net',
    points: [
      [970, 306],
      [1010, 306],
    ],
  },
  {
    id: 'tiles3d-loader',
    points: [
      [970, 420],
      [990, 420],
      [990, 180],
      [970, 180],
    ],
  },
  {
    id: 'loader-tiles3d',
    points: [
      [970, 164],
      [1000, 164],
      [1000, 436],
      [970, 436],
    ],
  },
];

const SCENARIOS: Record<string, { label: string; steps: Step[] }> = {
  frame: {
    label: 'A frame',
    steps: [
      {
        arrows: ['app-geomap', 'react-handles'],
        parts: ['app', 'react', 'geomap'],
        text: 'A gesture, a camera move or a layer change asks for a frame (requestRender); nothing is drawn otherwise.',
      },
      { arrows: ['geomap-frame'], parts: ['frame'], text: 'The frame loop draws at most once per animation frame.' },
      { arrows: ['frame-engine'], parts: ['engine'], text: 'One call into the WebAssembly: Engine.render().' },
      {
        arrows: ['engine-camera'],
        parts: ['camera'],
        text: 'The camera settles on the relief; the tiles in view are selected.',
      },
      {
        arrows: ['camera-ground', 'engine-gpu'],
        parts: ['ground', 'gpu'],
        text: 'The ground first: the rasters draped over the relief, the globe, the sky.',
      },
      {
        arrows: ['ground-layers', 'engine-gpu'],
        parts: ['layers', 'tiles3d', 'gpu'],
        text: 'Then the layers in their order: features, buildings, networks, 3D Tiles, point clouds; the viewsheds and the swipe.',
      },
      {
        arrows: ['layers-labels', 'engine-gpu'],
        parts: ['labels', 'gpu'],
        text: 'The labels are placed without overlap and drawn last.',
      },
      {
        arrows: ['engine-frame', 'frame-overlays'],
        parts: ['frame', 'overlays'],
        text: 'Back in the page: the markers, popups and panels follow the camera; the move, render and idle events fire.',
      },
    ],
  },
  raster: {
    label: 'A raster tile',
    steps: [
      {
        arrows: ['engine-camera', 'camera-ground'],
        parts: ['camera', 'ground'],
        text: 'A tile in view is missing from the raster cache: a coarser one stands in meanwhile.',
      },
      {
        arrows: ['ground-net'],
        parts: ['net'],
        text: 'The engine downloads it itself (WMTS, XYZ, the relief), the page’s request headers added.',
      },
      {
        arrows: ['net-ground'],
        parts: ['ground'],
        text: 'The browser decodes the image (createImageBitmap); the engine uploads it to its tile pool.',
      },
      {
        arrows: ['engine-frame'],
        parts: ['frame'],
        text: 'A frame is asked: the tile fades in over the one that stood in.',
      },
    ],
  },
  vector: {
    label: 'A vector basemap tile',
    steps: [
      {
        arrows: ['geomap-bworker'],
        parts: ['geomap', 'bworker'],
        text: 'The page asks the basemap worker for the tiles of the vector basemap in view.',
      },
      {
        arrows: ['bworker-net'],
        parts: ['net'],
        text: 'The worker fetches the vector tiles (MVT, PMTiles) and keeps them for the tiles that share them.',
      },
      {
        arrows: ['bworker-geomap'],
        parts: ['bworker', 'geomap'],
        text: 'It draws them with the MapLibre style into images (Canvas 2D) and lists their label candidates.',
      },
      {
        arrows: ['geomap-engine'],
        parts: ['engine', 'ground', 'labels'],
        text: 'The images go to the raster pool; the page shapes the labels (glyphs), the engine places them.',
      },
    ],
  },
  tiles3d: {
    label: '3D Tiles, point clouds',
    steps: [
      {
        arrows: [],
        parts: ['engine', 'tiles3d'],
        text: 'Each frame, the tree (or the COPC octree) is traversed: what is drawn, what is wanted, by priority.',
      },
      {
        arrows: ['tiles3d-loader'],
        parts: ['loader'],
        text: 'The engine asks the tile loader for the content it wants.',
      },
      {
        arrows: ['loader-dworker'],
        parts: ['dworker'],
        text: 'The loader hands it to a decoding worker, the most urgent first.',
      },
      { arrows: ['dworker-net'], parts: ['net'], text: 'The worker fetches it (range requests for the point clouds).' },
      {
        arrows: ['dworker-loader'],
        parts: ['dworker', 'loader'],
        text: 'It decodes it with the engine’s module (Draco, KTX2, meshopt, LAZ) and transfers GPU-ready arrays back.',
      },
      {
        arrows: ['loader-tiles3d'],
        parts: ['tiles3d', 'gpu'],
        text: 'The main thread only uploads them; what fell out of view is dropped over the budgets.',
      },
    ],
  },
};

const scenario = ref<keyof typeof SCENARIOS | 'parts'>('frame');
const step = ref(0);
const selected = ref<Part | undefined>(PARTS.find((p) => p.id === 'engine'));
const playing = ref(false);

const steps = computed(() => (scenario.value === 'parts' ? [] : SCENARIOS[scenario.value].steps));
const current = computed(() => steps.value[step.value]);
const activeArrows = computed(() => new Set(current.value?.arrows ?? []));
const activeParts = computed(() => new Set(current.value?.parts ?? []));

// A new scenario starts from its first step
watch(scenario, () => {
  step.value = 0;
});

const timer = ref<ReturnType<typeof setInterval>>();
const stop = () => {
  playing.value = false;
  if (timer.value) clearInterval(timer.value);
};
const play = () => {
  stop();
  playing.value = true;
  step.value = 0;
  timer.value = setInterval(() => {
    if (step.value + 1 >= steps.value.length) stop();
    else step.value += 1;
  }, 2200);
};
onBeforeUnmount(stop);

/** The SVG path of an arrow's route */
const path = (arrow: Arrow): string => arrow.points.map(([x, y], i) => `${i === 0 ? 'M' : 'L'}${x},${y}`).join(' ');

const select = (part: Part) => {
  selected.value = part;
};
</script>

<template>
  <div class="architecture">
    <div class="bar">
      <button
        v-for="(s, key) in SCENARIOS"
        :key="key"
        type="button"
        :class="{ on: scenario === key }"
        @click="
          stop();
          scenario = key;
        "
      >
        {{ s.label }}
      </button>
      <button type="button" :class="{ on: scenario === 'parts' }" @click="stop(); scenario = 'parts'">All the parts</button>
    </div>

    <div class="scroll">
      <svg viewBox="0 0 1200 690" role="img" aria-label="Architecture of isard">
        <defs>
          <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0,0 L10,5 L0,10 z" class="head" />
          </marker>
          <marker id="arrow-on" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M0,0 L10,5 L0,10 z" class="head on" />
          </marker>
        </defs>

        <rect v-for="lane in LANES" :key="lane.label" class="lane" x="20" :y="lane.y" width="970" :height="lane.h" rx="12" />

        <path
          v-for="a in ARROWS"
          :key="a.id"
          :d="path(a)"
          :class="['link', { on: activeArrows.has(a.id), faint: scenario !== 'parts' && !activeArrows.has(a.id) }]"
          :marker-end="activeArrows.has(a.id) ? 'url(#arrow-on)' : 'url(#arrow)'"
        />

        <!-- The lanes' titles over the arrows that cross them -->
        <text v-for="lane in LANES" :key="`label-${lane.label}`" class="lane-label" x="34" :y="lane.y + 18">{{ lane.label }}</text>
        <text class="lane-label" x="1014" y="98">Network</text>

        <g
          v-for="p in PARTS"
          :key="p.id"
          :class="['part', { on: activeParts.has(p.id), picked: selected?.id === p.id }]"
          tabindex="0"
          role="button"
          :aria-label="p.label"
          @click="select(p)"
          @keydown.enter="select(p)"
        >
          <rect :x="p.x" :y="p.y" :width="p.w" :height="p.h" rx="10" />
          <text class="title" :x="p.x + p.w / 2" :y="p.y + (p.sub ? p.h / 2 - 3 : p.h / 2 + 5)">{{ p.label }}</text>
          <text v-if="p.sub" class="sub" :x="p.x + p.w / 2" :y="p.y + p.h / 2 + 15">{{ p.sub }}</text>
        </g>
      </svg>
    </div>

    <div v-if="current" class="step">
      <div class="controls">
        <button type="button" :disabled="step === 0" aria-label="Previous step" @click="stop(); step -= 1">←</button>
        <span>{{ step + 1 }} / {{ steps.length }}</span>
        <button type="button" :disabled="step + 1 >= steps.length" aria-label="Next step" @click="stop(); step += 1">→</button>
        <button type="button" @click="playing ? stop() : play()">{{ playing ? 'Pause' : 'Play' }}</button>
      </div>
      <p>{{ current.text }}</p>
    </div>

    <div v-if="selected" class="info">
      <strong>{{ selected.label }}</strong>
      <p>{{ selected.info }}</p>
      <p class="files">
        <a v-for="f in selected.files" :key="f" :href="REPO + f" target="_blank" rel="noopener">{{ f }}</a>
      </p>
      <p class="hint">Click a part of the diagram for its role and its files.</p>
    </div>
  </div>
</template>

<style scoped>
.architecture {
  margin: 16px 0 32px;
}
.bar {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 12px;
}
button {
  padding: 4px 12px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 999px;
  background: var(--vp-c-bg);
  color: var(--vp-c-text-1);
  font-size: 14px;
  cursor: pointer;
}
button.on {
  border-color: var(--vp-c-brand-1);
  background: var(--vp-c-brand-1);
  color: var(--vp-c-white);
}
button:disabled {
  opacity: 0.4;
  cursor: default;
}
.scroll {
  overflow-x: auto;
  border: 1px solid var(--vp-c-divider);
  border-radius: 12px;
  background: var(--vp-c-bg-alt);
}
svg {
  display: block;
  min-width: 860px;
  width: 100%;
  height: auto;
}
.lane {
  fill: var(--vp-c-bg-soft);
  stroke: var(--vp-c-divider);
}
.lane-label {
  /* Readable over the arrows that cross it */
  paint-order: stroke;
  stroke: var(--vp-c-bg-soft);
  stroke-width: 5px;
  stroke-linejoin: round;
  fill: var(--vp-c-text-2);
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.02em;
}
.link {
  fill: none;
  stroke: var(--vp-c-text-3);
  stroke-width: 1.5;
  transition: opacity 0.3s, stroke 0.3s;
}
.link.faint {
  opacity: 0.25;
}
.link.on {
  stroke: var(--vp-c-brand-1);
  stroke-width: 3;
  stroke-dasharray: 8 6;
  animation: flow 0.8s linear infinite;
  opacity: 1;
}
@keyframes flow {
  to {
    stroke-dashoffset: -14;
  }
}
.head {
  fill: var(--vp-c-text-3);
}
.head.on {
  fill: var(--vp-c-brand-1);
}
.part {
  cursor: pointer;
  outline: none;
}
.part rect {
  fill: var(--vp-c-bg);
  stroke: var(--vp-c-divider);
  stroke-width: 1.5;
  transition: stroke 0.3s, fill 0.3s;
}
.part:hover rect,
.part:focus-visible rect {
  stroke: var(--vp-c-brand-2);
}
.part.on rect {
  stroke: var(--vp-c-brand-1);
  stroke-width: 3;
  fill: var(--vp-c-brand-soft);
}
.part.picked rect {
  stroke-dasharray: 5 3;
}
.title {
  fill: var(--vp-c-text-1);
  font-size: 15px;
  font-weight: 600;
  text-anchor: middle;
}
.sub {
  fill: var(--vp-c-text-2);
  font-size: 11.5px;
  text-anchor: middle;
}
.step {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
  margin-top: 12px;
}
.step p {
  flex: 1 1 320px;
  margin: 0;
}
.controls {
  display: flex;
  align-items: center;
  gap: 8px;
  font-variant-numeric: tabular-nums;
}
.info {
  margin-top: 16px;
  padding: 12px 16px;
  border-left: 3px solid var(--vp-c-brand-1);
  border-radius: 8px;
  background: var(--vp-c-bg-soft);
}
.info p {
  margin: 6px 0 0;
}
.files {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 14px;
  font-size: 13px;
}
.hint {
  color: var(--vp-c-text-3);
  font-size: 12px;
}
</style>
