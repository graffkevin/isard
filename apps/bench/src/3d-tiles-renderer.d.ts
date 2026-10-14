/** GeneratedSurfacePlugin ships without typings in 3d-tiles-renderer 0.5.3: its constructor options, from its JSDoc */
import type { ImageOverlay } from '3d-tiles-renderer/plugins';

declare module '3d-tiles-renderer/plugins' {
  export class GeneratedSurfacePlugin {
    constructor(options?: {
      overlay?: ImageOverlay | null;
      projection?: 'ellipsoid' | 'source';
      endCaps?: boolean;
      center?: boolean;
      useRecommendedSettings?: boolean;
      applyOverlayTexture?: boolean;
    });
  }
}
