/** `<Popup>`: a box of JSX content at a position or on a `<Marker>` */

import { type ReactNode, useContext, useEffect, useLayoutEffect, useMemo, useRef } from 'react';
import { createPortal } from 'react-dom';
import type { LonLat } from '../types.ts';
import { Popup as PopupHandle, type PopupOptions } from '../ui/overlay.ts';
import { defined, MarkerContext, useMap } from './context.ts';

export interface PopupProps extends PopupOptions {
  /** Its position; in a `<Marker>`, the marker's */
  lonLat?: LonLat;
  /** Metres above the ground (in a `<Marker>`, the marker's) */
  height?: number;
  /** Open or closed, when the application decides; else open when mounted alone, on the marker's click in a marker */
  open?: boolean;
  onOpen?: () => void;
  /** Closed by its button, a click on the map, or the marker's click */
  onClose?: () => void;
  children?: ReactNode;
}

/** A popup of JSX content, at a position or on a `<Marker>` */
export const Popup = (props: PopupProps): ReactNode => {
  const map = useMap();
  const inMarker = useContext(MarkerContext);
  const latest = useRef(props);
  const { anchor, closeButton, closeOnClick, maxWidth, className, height, open, offset } = props;
  const marker = inMarker?.marker;
  const lonLat = props.lonLat ?? inMarker?.lonLat;
  const opened = open ?? !marker;
  const content = useMemo(() => document.createElement('div'), []);
  const popup = useMemo(
    () => new PopupHandle(defined({ anchor, closeButton, closeOnClick, maxWidth, className })).setDOMContent(content),
    [anchor, closeButton, closeOnClick, maxWidth, className, content],
  );

  latest.current = props;

  // Its open and close events forwarded to the latest props, and tied to the marker around (its click opens it); removed when unmounted
  useEffect(() => {
    if (!map) return;

    const offs = [
      popup.on('open', () => latest.current.onOpen?.()),
      popup.on('close', () => latest.current.onClose?.()),
    ];

    if (marker) marker.setPopup(popup);

    return () => {
      offs.forEach((off) => {
        off();
      });
      if (marker?.getPopup() === popup) marker.setPopup(undefined);
      popup.remove();
    };
  }, [map, marker, popup]);

  // Its shift from the anchor
  useLayoutEffect(() => {
    if (offset) popup.setOffset(offset);
  }, [popup, offset?.[0], offset?.[1]]);
  // Its position, height and, when `open` decides or it stands alone, open or closed; before the browser paints
  useLayoutEffect(() => {
    if (!map) return;
    if (lonLat) popup.setLonLat(lonLat);
    if (height !== undefined) popup.setHeight(height);
    // In a marker without `open`: its click decides
    if (open === undefined && marker) return;
    if (opened && !popup.isOpen()) popup.addTo(map);
    if (!opened && popup.isOpen()) popup.remove();
  }, [map, marker, popup, lonLat?.[0], lonLat?.[1], height, open, opened]);

  return createPortal(props.children, content);
};
