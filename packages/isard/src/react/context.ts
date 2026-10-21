/** What the components of a map share: the map, the order of their layers, the viewsheds, the swipe and the marker
 * around; the hooks */

import { createContext, useContext, useEffect, useRef } from 'react';
import type { LayerOrder } from '../bind/layers.ts';
import type { Layer as LayerHandle } from '../layers/layers.ts';
import type { GeoMap } from '../map.ts';
import type { LonLat, MapEvents, ViewshedOptions } from '../types.ts';
import type { Marker as MarkerHandle } from '../ui/overlay.ts';

export interface Binding {
  map: GeoMap;
  order: LayerOrder;
  /** The observers of the `<Viewshed>` components, set on the map together */
  viewsheds: Map<object, ViewshedOptions>;
  applyViewsheds(): void;
}

export const MapContext = createContext<Binding | undefined>(undefined);

/** The layers of a `<Swipe>` around, its `<Layer>` children joining it */
export interface SwipeGroup {
  set(layer: LayerHandle): void;
  unset(layer: LayerHandle): void;
}

export const SwipeContext = createContext<SwipeGroup | undefined>(undefined);

/** The marker of a `<Marker>` around and its position (a `<Popup>` in it opens on its click, at that position) */
export const MarkerContext = createContext<{ marker: MarkerHandle; lonLat: LonLat } | undefined>(undefined);

/** The map of the `<IsardMap>` around, once created (undefined before) */
export const useMap = (): GeoMap | undefined => useContext(MapContext)?.map;

/** Listens to an event of the map around while the component is mounted; the listener may change at each render */
export const useMapEvent = <K extends keyof MapEvents>(type: K, listener: (event: MapEvents[K]) => void): void => {
  const map = useMap();
  const latest = useRef(listener);

  latest.current = listener;
  useEffect(() => map?.on(type, (event) => latest.current(event)), [map, type]);
};

/** The entries of an object whose value is defined (`exactOptionalPropertyTypes`) */
export const defined = <T extends object>(object: T): { [K in keyof T]?: Exclude<T[K], undefined> } =>
  Object.fromEntries(Object.entries(object).filter(([, value]) => value !== undefined)) as {
    [K in keyof T]?: Exclude<T[K], undefined>;
  };
