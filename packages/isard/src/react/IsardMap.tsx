/** `<IsardMap>`: the map, created when it mounts, destroyed when it unmounts, its options and events as props */

import {
  type CSSProperties,
  forwardRef,
  type ReactNode,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from 'react';
import { LayerOrder } from '../bind/layers.ts';
import { GeoMap } from '../map.ts';
import type { GeoMapOptions, LonLat, MapEvents, ViewshedOptions } from '../types.ts';
import type { Camera } from '../wasm/isard.js';
import { type Binding, defined, MapContext } from './context.ts';

/** The props of the map's events: `onClick` for 'click', `onMoveEnd` for 'moveend'… */
export interface MapEventProps {
  onMoveStart?: (camera: Camera) => void;
  onMove?: (camera: Camera) => void;
  onMoveEnd?: (camera: Camera) => void;
  onResize?: (event: MapEvents['resize']) => void;
  onHover?: (event: MapEvents['hover']) => void;
  onHoverFeature?: (event: MapEvents['hoverfeature']) => void;
  onClick?: (event: MapEvents['click']) => void;
  onRender?: (event: MapEvents['render']) => void;
  onIdle?: () => void;
  onLeave?: () => void;
}

const EVENT_PROPS = {
  onMoveStart: 'movestart',
  onMove: 'move',
  onMoveEnd: 'moveend',
  onResize: 'resize',
  onHover: 'hover',
  onHoverFeature: 'hoverfeature',
  onClick: 'click',
  onRender: 'render',
  onIdle: 'idle',
  onLeave: 'leave',
} as const satisfies Record<keyof MapEventProps, keyof MapEvents>;

export interface IsardMapProps extends Omit<GeoMapOptions, keyof Camera>, MapEventProps {
  /** The camera at creation, then left to the user's gestures */
  center?: LonLat;
  zoom?: number;
  pitch?: number;
  bearing?: number;
  /** A camera the map follows: set again whenever it changes (with `onMove` to keep it in a state) */
  camera?: Partial<Camera>;
  /** What `onHoverFeature` picks: within `radius` CSS pixels, among `layers` (default every layer) */
  hoverPicking?: GeoMap['hoverPicking'];
  /** The map, once created */
  onLoad?: (map: GeoMap) => void;
  /** The map's creation failed (no WebGL2, the engine not found) */
  onError?: (error: unknown) => void;
  id?: string;
  className?: string;
  style?: CSSProperties;
  /** Layers, markers, popups, panels, effects; drawn once the map is created */
  children?: ReactNode;
}

/** The props that are not options of the map */
const NOT_OPTIONS = [...Object.keys(EVENT_PROPS), 'onLoad', 'onError', 'hoverPicking', 'id', 'className', 'style'];

/** Options the map takes again after its creation, and how */
const LIVE_OPTIONS: { [K in keyof GeoMapOptions]?: (map: GeoMap, props: IsardMapProps) => void } = {
  basemap: (map, p) => {
    if (p.basemap) map.setBasemap(p.basemap, p.hillshade);
  },
  hillshade: (map, p) => {
    if (p.basemap) map.setBasemap(p.basemap, p.hillshade);
  },
  terrain: (map, p) => map.setTerrain(p.terrain ?? false),
  minZoom: (map, p) => map.setMinZoom(p.minZoom ?? 0),
  maxZoom: (map, p) => map.setMaxZoom(p.maxZoom ?? 22),
  maxPitch: (map, p) => map.setMaxPitch(p.maxPitch ?? 70),
  atmosphere: (map, p) => map.setAtmosphere(p.atmosphere),
  projection: (map, p) => map.setProjection(p.projection ?? 'globe'),
  loading: (map, p) => map.setLoading(p.loading ?? 'immediate'),
  sky: (map, p) => map.setSky(p.sky ?? true),
  background: (map, p) => {
    if (p.background) map.setBackground(p.background);
  },
};

const optionsOf = (props: Partial<IsardMapProps>): GeoMapOptions =>
  Object.fromEntries(Object.entries(props).filter(([key]) => !NOT_OPTIONS.includes(key))) as GeoMapOptions;

const sameCamera = (a: Camera, b: Partial<Camera>): boolean =>
  (b.center === undefined || (a.center[0] === b.center[0] && a.center[1] === b.center[1])) &&
  (b.zoom === undefined || a.zoom === b.zoom) &&
  (b.pitch === undefined || a.pitch === b.pitch) &&
  (b.bearing === undefined || a.bearing === b.bearing);

/** What the components of a new map share */
const bindingOf = (map: GeoMap): Binding => {
  const viewsheds = new Map<object, ViewshedOptions>();

  return {
    map,
    order: new LayerOrder(),
    viewsheds,
    applyViewsheds: () => map.setViewshed(viewsheds.size > 0 ? [...viewsheds.values()] : undefined),
  };
};

/**
 * A map filling its element (give it a height). Created once mounted, destroyed when unmounted; the options read at
 * creation only (`body`, `preset`, `interactive`, the URLs) need a new `key` to take another value
 */
export const IsardMap = forwardRef<GeoMap | undefined, IsardMapProps>(function IsardMap(props, ref) {
  const element = useRef<HTMLDivElement>(null);
  const latest = useRef(props);
  const applied = useRef<IsardMapProps>(props);
  const [binding, setBinding] = useState<Binding>();
  const map = binding?.map;
  const { camera, hoverPicking } = props;

  latest.current = props;
  useImperativeHandle(ref, () => map, [map]);

  // Created once: StrictMode's second mount and an unmount before the engine is loaded drop the map they started
  useEffect(() => {
    const container = element.current;

    if (!container) return;

    const created: { map?: GeoMap; cancelled: boolean } = { cancelled: false };
    const { camera, center, zoom, pitch, bearing, children: _, ...options } = latest.current;

    GeoMap.create(container, { ...optionsOf(options), ...defined({ center, zoom, pitch, bearing }), ...camera }).then(
      (map) => {
        if (created.cancelled) {
          map.destroy();

          return;
        }

        created.map = map;
        setBinding(bindingOf(map));
        latest.current.onLoad?.(map);
      },
      (error: unknown) => {
        if (created.cancelled) return;
        if (latest.current.onError) latest.current.onError(error);
        else console.error(error);
      },
    );

    return () => {
      created.cancelled = true;
      created.map?.destroy();
      setBinding(undefined);
    };
  }, []);

  // The events, each listened to once: the props may give another function at each render
  useEffect(() => {
    if (!map) return;

    const offs = Object.entries(EVENT_PROPS).map(([prop, type]) =>
      map.on(type, (event) =>
        (latest.current[prop as keyof MapEventProps] as ((e: unknown) => void) | undefined)?.(event),
      ),
    );

    return () => {
      offs.forEach((off) => {
        off();
      });
    };
  }, [map]);

  // The options taken again, compared with what the map was given last
  useEffect(() => {
    if (!map) return;

    const last = applied.current;
    const done = new Set<(map: GeoMap, props: IsardMapProps) => void>();

    applied.current = props;
    Object.entries(LIVE_OPTIONS).forEach(([key, apply]) => {
      const changed = !Object.is(last[key as keyof IsardMapProps], props[key as keyof IsardMapProps]);

      if (!apply || done.has(apply) || !changed) return;
      done.add(apply);
      apply(map, props);
    });
  });

  useEffect(() => {
    if (map && hoverPicking) map.hoverPicking = hoverPicking;
  }, [map, hoverPicking]);

  useEffect(() => {
    if (map && camera && !sameCamera(map.getCamera(), camera)) map.setCamera(camera);
  }, [map, camera]);

  return (
    <div ref={element} id={props.id} className={props.className} style={props.style}>
      {binding ? <MapContext.Provider value={binding}>{props.children}</MapContext.Provider> : null}
    </div>
  );
});
