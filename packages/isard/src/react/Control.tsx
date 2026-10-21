/** `<Control>`: a control in a corner of the map */

import { type ReactNode, useEffect } from 'react';
import type { Control as ControlHandle, ControlPosition } from '../ui/controls.ts';
import { useMap } from './context.ts';

export interface ControlProps {
  /** `new NavigationControl()`, `new AttributionControl()` or yours: keep it in a memo (another one replaces it) */
  control: ControlHandle;
  /** Default 'top-right' */
  position?: ControlPosition;
}

/** A control in a corner of the map */
export const Control = ({ control, position = 'top-right' }: ControlProps): ReactNode => {
  const map = useMap();

  useEffect(() => {
    if (!map) return;
    map.addControl(control, position);

    return () => {
      map.removeControl(control);
    };
  }, [map, control, position]);

  return null;
};
