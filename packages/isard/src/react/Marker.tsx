/** `<Marker>`: an element at a position, the default pin or JSX, its `<Popup>` opened by its click */

import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef } from 'react';
import { createPortal } from 'react-dom';
import type { LonLat, Point } from '../types.ts';
import { type Anchor, Marker as MarkerHandle } from '../ui/overlay.ts';
import { defined, MarkerContext, useMap } from './context.ts';

export interface MarkerProps {
  lonLat: LonLat;
  /** Metres above the ground, its stem down to `base` (what it stands on: a roof, a mesh) */
  height?: number;
  base?: number;
  draggable?: boolean;
  offset?: Point;
  /** Which part of the element sits on the position (default 'bottom' for the pin, 'center' with an icon) */
  anchor?: Anchor;
  className?: string;
  /** The marker's look, in place of the default pin; the element changes with it */
  icon?: ReactNode;
  onDragStart?: (lonLat: LonLat) => void;
  onDrag?: (lonLat: LonLat) => void;
  onDragEnd?: (lonLat: LonLat) => void;
  /** A `<Popup>` opened by a click on the marker */
  children?: ReactNode;
}

/** A marker at a position, its look the default pin or the `icon` given */
export const Marker = (props: MarkerProps): ReactNode => {
  const map = useMap();
  const latest = useRef(props);
  const { anchor, className, lonLat, height = 0, base = 0, draggable = false, offset } = props;
  const custom = props.icon !== undefined;
  const element = useMemo(() => (custom ? document.createElement('div') : undefined), [custom]);
  const marker = useMemo(
    () => new MarkerHandle({ ...defined({ anchor, className, element }) }),
    [anchor, className, element],
  );
  const context = useMemo(() => ({ marker, lonLat }), [marker, lonLat]);

  latest.current = props;

  // On the map once it is there, its drag events forwarded to the latest props; off the map when unmounted
  useEffect(() => {
    if (!map) return;

    const offs = [
      marker.on('dragstart', (e) => latest.current.onDragStart?.(e.lonLat)),
      marker.on('drag', (e) => latest.current.onDrag?.(e.lonLat)),
      marker.on('dragend', (e) => latest.current.onDragEnd?.(e.lonLat)),
    ];

    marker.addTo(map);

    return () => {
      offs.forEach((off) => {
        off();
      });
      marker.remove();
    };
  }, [map, marker]);

  // Its position, before the browser paints (no frame at the old place)
  useLayoutEffect(() => {
    marker.setLonLat(lonLat);
  }, [marker, lonLat[0], lonLat[1]]);
  // Its height above what it stands on
  useLayoutEffect(() => {
    marker.setHeight(height, base);
  }, [marker, height, base]);
  // Draggable or not
  useLayoutEffect(() => {
    marker.setDraggable(draggable);
  }, [marker, draggable]);
  // Its shift from the anchor
  useLayoutEffect(() => {
    if (offset) marker.setOffset(offset);
  }, [marker, offset?.[0], offset?.[1]]);

  return (
    <MarkerContext.Provider value={context}>
      {element ? createPortal(props.icon, element) : null}
      {props.children}
    </MarkerContext.Provider>
  );
};
