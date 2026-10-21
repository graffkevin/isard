/** A layer described by its type and options, as an application's configuration gives it, and the map adding it. */

import type { ExtrusionOptions } from '../layers/extrusions.ts';
import type { CircleOptions, FillOptions, LineOptions } from '../layers/features.ts';
import type { GroundOptions, Layer, ModelOptions } from '../layers/layers.ts';
import type { PointCloudOptions } from '../layers/pointclouds.ts';
import type { SymbolOptions } from '../layers/symbols.ts';
import type { GeoMap } from '../map.ts';
import type { NetworkOptions, RasterOptions, Tiles3dOptions } from '../types.ts';

/** A layer by its type and the options of its `add…` method (`GeoMap.add`) */
export type LayerSpec<P = Record<string, unknown>> =
  | ({ type: 'lines' } & LineOptions<P>)
  | ({ type: 'circles' } & CircleOptions<P>)
  | ({ type: 'fills' } & FillOptions<P>)
  | ({ type: 'symbols' } & SymbolOptions<P>)
  | ({ type: 'extrusions' } & ExtrusionOptions<P>)
  | ({ type: 'raster' } & RasterOptions & { before?: Layer })
  | ({ type: 'ground' } & GroundOptions & { before?: Layer })
  | ({ type: 'network' } & NetworkOptions)
  | ({ type: 'tiles3d' } & Tiles3dOptions)
  | ({ type: 'pointcloud' } & PointCloudOptions & { before?: Layer })
  | ({ type: 'models' } & ModelOptions & { before?: Layer });

/** Adds the layer a description gives with the `add…` method of its type; an unknown type throws */
export const addSpec = <P = Record<string, unknown>>(map: GeoMap, spec: LayerSpec<P>): Layer => {
  switch (spec.type) {
    case 'lines':
      return map.addLines<P>(spec);
    case 'circles':
      return map.addCircles<P>(spec);
    case 'fills':
      return map.addFills<P>(spec);
    case 'symbols':
      return map.addSymbols<P>(spec);
    case 'extrusions':
      return map.addExtrusions<P>(spec);
    case 'raster':
      return map.addRaster(spec);
    case 'ground':
      return map.addGround(spec);
    case 'network':
      return map.addNetwork(spec);
    case 'tiles3d':
      return map.addTiles3d(spec);
    case 'pointcloud':
      return map.addPointCloud(spec);
    case 'models':
      return map.addModels(spec);
    default: {
      // Only from an application's configuration, untyped: the cases above cover every type
      const { type }: { type?: unknown } = spec;

      throw new Error(`isard: no layer of type ${String(type)}`);
    }
  }
};
