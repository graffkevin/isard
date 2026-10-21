/**
 * The settings of the map's engine: made from the options, given to each new engine (at creation, and again after a
 * context loss or once the engine with 3D Tiles is loaded), with the page's request headers.
 */

import { SEA } from '../basemap/basemaps.ts';
import type { PngEncoding } from '../basemap/terrain.ts';
import { type HeaderRules, headerRules } from '../engine/requests.ts';
import {
  atmosphereSettings,
  bodySettings,
  haloSettings,
  rgb,
  type Settings,
  type TerrainLoader,
  terrainSettings,
} from '../settings.ts';
import type { GeoMapOptions } from '../types.ts';
import type { Engine } from '../wasm/isard.js';

/** The settings of a map from its options; `earth`: the map is of the Earth */
export const mapSettings = (
  options: GeoMapOptions,
  earth: boolean,
  terrainLoader: (url: string, encoding: PngEncoding, floor?: number) => TerrainLoader,
): Settings => {
  const body = bodySettings(options.body);

  return {
    body,
    zoomRange: [options.minZoom ?? 0, options.maxZoom ?? 22],
    maxPitch: options.maxPitch ?? 70,
    globe: (options.projection ?? 'globe') === 'globe',
    sky: options.sky ?? true,
    atmosphere: atmosphereSettings(options.atmosphere),
    terrain: terrainSettings(options.terrain, options.body, body.radius, terrainLoader),
    loading: options.loading ?? 'immediate',
    background: rgb(options.background ?? (earth ? '#f2f2f0' : '#000000')),
    globeColor: earth ? SEA['osm-positron'] : rgb(body.color),
  };
};

/** Gives a new engine the settings and the page's request headers */
export const applySettings = (engine: Engine, settings: Settings): void => {
  // The body first: the layers' shaders may take its size
  const { radius, flattening, atmosphere: air, relief } = settings.body;

  engine.setBody(radius, flattening, haloSettings(air), ...relief);
  sendHeaders(engine, headerRules());

  const { zoomRange, maxPitch, globe, sky, atmosphere, terrain, loading, background, globeColor } = settings;

  engine.setZoomRange(...zoomRange);
  engine.setMaxPitch(maxPitch);
  engine.setGlobe(globe);
  engine.setSky(sky);
  engine.setAtmosphere(atmosphere);
  engine.setTerrain(terrain?.url, terrain?.exaggeration ?? 1, terrain?.maxZoom, terrain?.geoid, terrain?.loader);
  engine.setImmediateLoading(loading === 'immediate');
  engine.setBackground(...background);
  engine.setGlobeColor(...globeColor);
};

/** The page's request headers to an engine (its own requests: raster tiles, tilesets, point clouds) */
export const sendHeaders = (engine: Engine, rules: HeaderRules): void => {
  engine.setRequestHeaders(
    rules.map(([prefix]) => prefix),
    Uint32Array.from(rules, ([, headers]) => Object.keys(headers).length),
    rules.flatMap(([, headers]) => Object.entries(headers).flat()),
  );
};
