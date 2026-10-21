/** `<Panel>`: JSX standing in 3D at a position */

import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef } from 'react';
import { createPortal } from 'react-dom';
import type { LonLat } from '../types.ts';
import { Panel as PanelHandle, type PanelOptions } from '../ui/overlay.ts';
import { defined, useMap } from './context.ts';

export interface PanelProps extends Omit<PanelOptions, 'element'> {
  lonLat: LonLat;
  /** What stands in 3D: a video, an iframe, a sign */
  children?: ReactNode;
}

/** JSX standing in 3D at a position, `width` metres wide, facing `heading` */
export const Panel = (props: PanelProps): ReactNode => {
  const map = useMap();
  const { className, lonLat, width, heading, pitch, height } = props;
  // The width at creation: the placement is set again by the effect below
  const initialWidth = useRef(width);
  const element = useMemo(() => document.createElement('div'), []);
  const panel = useMemo(
    () => new PanelHandle({ element, width: initialWidth.current, ...defined({ className }) }),
    [element, className],
  );

  // On the map once it is there; off it when unmounted
  useEffect(() => {
    if (!map) return;
    panel.addTo(map);

    return () => {
      panel.remove();
    };
  }, [map, panel]);

  // Its position, before the browser paints
  useLayoutEffect(() => {
    panel.setLonLat(lonLat);
  }, [panel, lonLat[0], lonLat[1]]);
  // Its size and orientation: width, heading, pitch, height
  useLayoutEffect(() => {
    panel.setPlacement(defined({ width, heading, pitch, height }));
  }, [panel, width, heading, pitch, height]);
  // Its content may change its CSS size
  useLayoutEffect(() => {
    panel.resize();
  });

  return createPortal(props.children, element);
};
