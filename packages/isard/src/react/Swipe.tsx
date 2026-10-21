/** `<Swipe>`: the `<Layer>` in it on one side of a vertical line only */

import { type ReactNode, useEffect, useMemo, useRef } from 'react';
import type { Layer as LayerHandle } from '../layers/layers.ts';
import type { SwipeOptions } from '../types.ts';
import { defined, SwipeContext, type SwipeGroup, useMap } from './context.ts';

export interface SwipeProps extends Omit<SwipeOptions, 'layers'> {
  /** The `<Layer>` shown on one side of the line only */
  children?: ReactNode;
}

/** The `<Layer>` in it shown on one side of a vertical line only, at `position` CSS pixels from the left */
export const Swipe = (props: SwipeProps): ReactNode => {
  const map = useMap();
  const latest = useRef(props);
  const { position, side } = props;
  const layers = useMemo(() => new Set<LayerHandle>(), []);

  latest.current = props;

  const group = useMemo<SwipeGroup | undefined>(() => {
    if (!map) return undefined;

    const apply = () => {
      const { position, side } = latest.current;

      map.setSwipe(layers.size > 0 ? { position, layers: [...layers], ...defined({ side }) } : undefined);
    };

    return {
      set: (layer) => {
        layers.add(layer);
        apply();
      },
      unset: (layer) => {
        layers.delete(layer);
        apply();
      },
    };
  }, [map, layers]);

  useEffect(() => {
    if (map && layers.size > 0) map.setSwipe({ position, layers: [...layers], ...defined({ side }) });
  }, [map, layers, position, side]);
  useEffect(
    () => () => {
      map?.setSwipe(undefined);
    },
    [map],
  );

  return <SwipeContext.Provider value={group}>{props.children}</SwipeContext.Provider>;
};
