/** `<CustomLayer>`: your WebGL code or a three.js scene among the layers of the components */

import { type ReactNode, useContext, useEffect, useRef } from 'react';
import type { CustomLayerInterface, Layer as LayerHandle } from '../layers/layers.ts';
import { MapContext } from './context.ts';

export interface CustomLayerProps {
  /** Your WebGL code: `onAdd`, `render`, `onRemove` (see `map.addCustomLayer`); another object makes it anew */
  layer: CustomLayerInterface;
  /** Its place among the layers of the components (as `<Layer order>`) */
  order?: number;
}

/** Your WebGL code or a three.js scene drawn in the map's frame and depth, among the layers of the components */
export const CustomLayer = ({ layer, order = 0 }: CustomLayerProps): ReactNode => {
  const binding = useContext(MapContext);
  const initialOrder = useRef(order);
  const handle = useRef<LayerHandle>(undefined);

  // Added once the map is there, in the declared order; removed when unmounted, made anew when the layer object changes
  useEffect(() => {
    if (!binding) return;

    const custom = binding.map.addCustomLayer(layer);

    handle.current = custom;
    binding.order.place(custom, initialOrder.current);

    return () => {
      handle.current = undefined;
      binding.order.remove(custom);
      if (binding.map.hasLayer(custom)) custom.remove();
    };
  }, [binding, layer]);

  // A new `order`: moved to its new place among the layers of the components
  useEffect(() => {
    if (binding && handle.current) binding.order.place(handle.current, order, handle.current);
  }, [binding, order]);

  return null;
};
