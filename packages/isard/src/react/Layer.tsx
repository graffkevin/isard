/** `<Layer>`: a layer added when it mounts, updated by the least its props need, removed when it unmounts, in its
 * declared order */

import {
  forwardRef,
  type ReactNode,
  type Ref,
  useContext,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from 'react';
import {
  addBoundLayer,
  applyLayerChange,
  type BoundLayerSpec,
  layerChange,
  rebuildBoundLayer,
} from '../bind/layers.ts';
import type { ExtrusionLayer } from '../layers/extrusions.ts';
import type { CircleLayer, FillLayer, LineLayer } from '../layers/features.ts';
import type {
  GroundLayer,
  Layer as LayerHandle,
  ModelLayer,
  NetworkLayer,
  RasterLayer,
  Tiles3dLayer,
} from '../layers/layers.ts';
import type { PointCloudLayer } from '../layers/pointclouds.ts';
import type { SymbolLayer } from '../layers/symbols.ts';
import type { LayerSpec } from '../map.ts';
import { MapContext, SwipeContext } from './context.ts';

/** The handle of a layer by its type */
export interface LayerHandles<P = Record<string, unknown>> {
  lines: LineLayer<P>;
  circles: CircleLayer<P>;
  fills: FillLayer<P>;
  symbols: SymbolLayer<P>;
  extrusions: ExtrusionLayer<P>;
  raster: RasterLayer;
  ground: GroundLayer;
  network: NetworkLayer;
  tiles3d: Tiles3dLayer;
  pointcloud: PointCloudLayer;
  models: ModelLayer;
}

export type LayerType = LayerSpec['type'];

export type LayerProps<P = Record<string, unknown>, T extends LayerType = LayerType> = Extract<
  BoundLayerSpec<P>,
  { type: T }
> & {
  /** Its place among the layers of the components: from the bottom up, the same order by mounting (default 0) */
  order?: number;
  /** The layer, once created and each time it is made anew */
  onLayer?: (layer: LayerHandles<P>[T]) => void;
  ref?: Ref<LayerHandles<P>[T] | undefined>;
};

interface Mounted {
  layer: LayerHandle;
  spec: BoundLayerSpec;
  order: number;
}

const LayerComponent = forwardRef<LayerHandle | undefined, Omit<LayerProps, 'ref'>>(function Layer(props, ref) {
  const binding = useContext(MapContext);
  const swipe = useContext(SwipeContext);
  const { order = 0, onLayer, ...rest } = props;
  const spec = rest as BoundLayerSpec;
  const mounted = useRef<Mounted>(undefined);
  const latest = useRef({ spec, order, onLayer });
  const [layer, setLayer] = useState<LayerHandle>();

  latest.current = { spec, order, onLayer };
  useImperativeHandle(ref, () => layer, [layer]);

  // Added once the map is there, removed when unmounted (or when the map goes)
  useEffect(() => {
    if (!binding) return;

    const { spec, order, onLayer } = latest.current;
    const created = addBoundLayer(binding.map, spec);

    binding.order.place(created, order);
    mounted.current = { layer: created, spec, order };
    swipe?.set(created);
    setLayer(created);
    onLayer?.(created as never);

    return () => {
      const current = mounted.current;

      mounted.current = undefined;
      if (!current) return;
      binding.order.remove(current.layer);
      swipe?.unset(current.layer);
      if (binding.map.hasLayer(current.layer)) current.layer.remove();
    };
  }, [binding, swipe]);

  // Each render: what changed sent, or the layer made anew in its place
  useEffect(() => {
    const current = mounted.current;

    if (!binding || !current) return;

    const change = layerChange(current.spec, spec);

    current.spec = spec;
    if (change?.rebuild) {
      const old = current.layer;
      const fresh = rebuildBoundLayer(binding.map, old, spec);

      binding.order.place(fresh, order, old);
      swipe?.unset(old);
      swipe?.set(fresh);
      current.layer = fresh;
      current.order = order;
      setLayer(fresh);
      latest.current.onLayer?.(fresh as never);

      return;
    }

    if (change) applyLayerChange(current.layer, spec, change);
    if (current.order !== order) {
      current.order = order;
      binding.order.place(current.layer, order, current.layer);
    }
  });

  return null;
});

/**
 * A layer: the options of its `add…` method with its `type` (`map.add`), and `values` for the layers colored by
 * values. A change of data, style, filter, values or instances updates it; a change of what is set at creation (its
 * URL, grid, nodes…) makes it anew, in its place. Data, functions and arrays are compared by identity: keep them in a
 * memo or a state. The ref and `onLayer` give its handle, typed by its `type`; the properties of its features are
 * inferred from its data or its style functions (`<Layer<Building> …>` to name them)
 */
export const Layer = LayerComponent as <P = Record<string, unknown>, T extends LayerType = LayerType>(
  props: LayerProps<P, T>,
) => ReactNode;
