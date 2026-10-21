/**
 * Isard in React: the map, its layers, markers, popups, panels and effects as components, created when they mount,
 * updated by the least their props need and removed when they unmount.
 *
 * ```tsx
 * <IsardMap preset="ign-3d" center={[2.35, 48.85]} zoom={15} style={{ height: 400 }}>
 *   <Layer type="lines" data={routes} color="#0057b8" width={3} />
 *   <Marker lonLat={[2.35, 48.85]}><Popup>Here</Popup></Marker>
 * </IsardMap>
 * ```
 */

import {
  type CSSProperties,
  createContext,
  createElement,
  forwardRef,
  type ReactNode,
  useContext,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { createPortal } from 'react-dom';
import { applyLayerChange, LayerOrder, layerChange } from '../bind/layers.ts';
import type { Layer as LayerHandle } from '../layers/layers.ts';
import { GeoMap, type LayerSpec } from '../map.ts';
import type { GeoMapOptions, LonLat, MapEvents, Point, SwipeOptions, ViewshedOptions } from '../types.ts';
import {
  type Anchor,
  Marker as MarkerHandle,
  Panel as PanelHandle,
  type PanelOptions,
  Popup as PopupHandle,
  type PopupOptions,
} from '../ui/overlay.ts';
import type { Camera } from '../wasm/isard.js';

// ── The map ──

interface Binding {
  map: GeoMap;
  order: LayerOrder;
  /** The observers of the `<Viewshed>` components, set on the map together */
  viewsheds: Map<object, ViewshedOptions>;
  applyViewsheds(): void;
}

const MapContext = createContext<Binding | undefined>(undefined);

/** The map of the `<IsardMap>` around, once created (undefined before) */
export const useMap = (): GeoMap | undefined => {
  return useContext(MapContext)?.map;
};

/** Listens to an event of the map around while the component is mounted; the listener may change at each render */
export const useMapEvent = <K extends keyof MapEvents>(type: K, listener: (event: MapEvents[K]) => void): void => {
  const map = useMap();
  const latest = useRef(listener);
  latest.current = listener;
  useEffect(() => map?.on(type, (event) => latest.current(event)), [map, type]);
};

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

const sameCamera = (a: Camera, b: Partial<Camera>): boolean => {
  return (
    (b.center === undefined || (a.center[0] === b.center[0] && a.center[1] === b.center[1])) &&
    (b.zoom === undefined || a.zoom === b.zoom) &&
    (b.pitch === undefined || a.pitch === b.pitch) &&
    (b.bearing === undefined || a.bearing === b.bearing)
  );
};

/**
 * A map filling its element (give it a height). Created once mounted, destroyed when unmounted; the options read at
 * creation only (`body`, `preset`, `interactive`, the URLs) need a new `key` to take another value
 */
export const IsardMap = forwardRef<GeoMap | undefined, IsardMapProps>(function IsardMap(props, ref) {
  const element = useRef<HTMLDivElement>(null);
  const [binding, setBinding] = useState<Binding>();
  const latest = useRef(props);
  latest.current = props;
  useImperativeHandle(ref, () => binding?.map, [binding]);

  useEffect(() => {
    const container = element.current;
    if (!container) return;
    let cancelled = false;
    let created: GeoMap | undefined;
    const { camera, center, zoom, pitch, bearing, children: _, ...options } = latest.current;
    GeoMap.create(container, {
      ...stripProps(options),
      ...defined({ center, zoom, pitch, bearing }),
      ...camera,
    }).then(
      (map) => {
        // Unmounted while the engine was loading (or StrictMode's second mount): that map is not wanted
        if (cancelled) {
          map.destroy();
          return;
        }
        created = map;
        const viewsheds = new Map<object, ViewshedOptions>();
        setBinding({
          map,
          order: new LayerOrder(),
          viewsheds,
          applyViewsheds: () => map.setViewshed(viewsheds.size > 0 ? [...viewsheds.values()] : undefined),
        });
        latest.current.onLoad?.(map);
      },
      (error: unknown) => {
        if (cancelled) return;
        if (latest.current.onError) latest.current.onError(error);
        else console.error(error);
      },
    );
    return () => {
      cancelled = true;
      created?.destroy();
      setBinding(undefined);
    };
  }, []);

  // The events, each listened to once: the props may give another function at each render
  const map = binding?.map;
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
  const applied = useRef<IsardMapProps>(props);
  useEffect(() => {
    if (!map) return;
    const last = applied.current;
    applied.current = props;
    const done = new Set<(map: GeoMap, props: IsardMapProps) => void>();
    Object.entries(LIVE_OPTIONS).forEach(([key, apply]) => {
      if (!apply || done.has(apply) || Object.is(last[key as keyof IsardMapProps], props[key as keyof IsardMapProps])) {
        return;
      }
      done.add(apply);
      apply(map, props);
    });
  });

  const { camera } = props;
  useEffect(() => {
    if (map && camera && !sameCamera(map.getCamera(), camera)) map.setCamera(camera);
  }, [map, camera]);

  return createElement(
    'div',
    { ref: element, id: props.id, className: props.className, style: props.style },
    binding ? createElement(MapContext.Provider, { value: binding }, props.children) : null,
  );
});

/** The props that are not options of the map */
const stripProps = (props: Partial<IsardMapProps>): GeoMapOptions => {
  const options: Record<string, unknown> = { ...props };
  [...Object.keys(EVENT_PROPS), 'onLoad', 'onError', 'id', 'className', 'style'].forEach((key) => {
    delete options[key];
  });
  return options as GeoMapOptions;
};

/** The entries of an object whose value is defined (`exactOptionalPropertyTypes`) */
const defined = <T extends object>(object: T): { [K in keyof T]?: Exclude<T[K], undefined> } => {
  return Object.fromEntries(Object.entries(object).filter(([, value]) => value !== undefined)) as {
    [K in keyof T]?: Exclude<T[K], undefined>;
  };
};

// ── Layers ──

const SwipeContext = createContext<SwipeGroup | undefined>(undefined);

export type LayerProps<P = Record<string, unknown>> = LayerSpec<P> & {
  /** Its place among the layers of the components: from the bottom up, the same order by mounting (default 0) */
  order?: number;
  /** The layer, once created and each time it is made anew */
  onLayer?: (layer: LayerHandle) => void;
};

/**
 * A layer: the options of its `add…` method with its `type` (`map.add`). A change of data, style or instances updates
 * it; a change of what is set at creation (its URL, grid, nodes…) makes it anew, in its place. Data, functions and
 * arrays are compared by identity: keep them in a memo or a state. The ref gives the layer's handle
 */
export const Layer = forwardRef<LayerHandle | undefined, LayerProps>(function Layer(props, ref) {
  const binding = useContext(MapContext);
  const swipe = useContext(SwipeContext);
  const { order = 0, onLayer, ...spec } = props;
  const state = useRef<{ layer: LayerHandle; spec: LayerSpec; order: number }>(undefined);
  const [layer, setLayer] = useState<LayerHandle>();
  useImperativeHandle(ref, () => layer, [layer]);
  const latest = useRef({ spec, order, onLayer });
  latest.current = { spec, order, onLayer };

  useEffect(() => {
    if (!binding) return;
    const { spec, order, onLayer } = latest.current;
    const created = binding.map.add(spec as LayerSpec);
    binding.order.place(created, order);
    state.current = { layer: created, spec, order };
    swipe?.set(created);
    setLayer(created);
    onLayer?.(created);
    return () => {
      const current = state.current;
      state.current = undefined;
      if (!current) return;
      binding.order.remove(current.layer);
      swipe?.unset(current.layer);
      if (binding.map.hasLayer(current.layer)) current.layer.remove();
    };
  }, [binding, swipe]);

  useEffect(() => {
    const current = state.current;
    if (!binding || !current) return;
    const next = spec as LayerSpec;
    const change = layerChange(current.spec, next);
    current.spec = next;
    if (change?.rebuild) {
      const old = current.layer;
      const fresh = binding.map.rebuildLayer(old, next);
      binding.order.place(fresh, order, old);
      swipe?.unset(old);
      swipe?.set(fresh);
      current.layer = fresh;
      current.order = order;
      setLayer(fresh);
      latest.current.onLayer?.(fresh);
      return;
    }
    if (change) applyLayerChange(current.layer, next, change);
    if (current.order !== order) {
      current.order = order;
      binding.order.place(current.layer, order, current.layer);
    }
  });

  return null;
});

// ── Markers, popups, panels ──

/** The marker of a `<Marker>` around (a `<Popup>` in it opens on its click) */
const MarkerContext = createContext<MarkerHandle | undefined>(undefined);

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
  const custom = props.icon !== undefined;
  const { anchor, className } = props;
  const element = useMemo(() => (custom ? document.createElement('div') : undefined), [custom]);
  const marker = useMemo(
    () => new MarkerHandle({ ...defined({ anchor, className, element }) }),
    [anchor, className, element],
  );
  const latest = useRef(props);
  latest.current = props;

  useEffect(() => {
    if (!map) return;
    marker.addTo(map);
    const offs = [
      marker.on('dragstart', (e) => latest.current.onDragStart?.(e.lonLat)),
      marker.on('drag', (e) => latest.current.onDrag?.(e.lonLat)),
      marker.on('dragend', (e) => latest.current.onDragEnd?.(e.lonLat)),
    ];
    return () => {
      offs.forEach((off) => {
        off();
      });
      marker.remove();
    };
  }, [map, marker]);

  const { lonLat, height = 0, base = 0, draggable = false, offset } = props;
  useLayoutEffect(() => {
    marker.setLonLat(lonLat);
  }, [marker, lonLat[0], lonLat[1]]);
  useLayoutEffect(() => {
    marker.setHeight(height, base);
  }, [marker, height, base]);
  useLayoutEffect(() => {
    marker.setDraggable(draggable);
  }, [marker, draggable]);
  useLayoutEffect(() => {
    if (offset) marker.setOffset(offset);
  }, [marker, offset?.[0], offset?.[1]]);

  return createElement(
    MarkerContext.Provider,
    { value: marker },
    element ? createPortal(props.icon, element) : null,
    props.children,
  );
};

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
  const marker = useContext(MarkerContext);
  const { anchor, closeButton, closeOnClick, maxWidth, className } = props;
  const content = useMemo(() => document.createElement('div'), []);
  const popup = useMemo(
    () => new PopupHandle(defined({ anchor, closeButton, closeOnClick, maxWidth, className })).setDOMContent(content),
    [anchor, closeButton, closeOnClick, maxWidth, className, content],
  );
  const latest = useRef(props);
  latest.current = props;

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

  const { lonLat, height, open, offset } = props;
  const opened = open ?? !marker;
  useLayoutEffect(() => {
    if (offset) popup.setOffset(offset);
  }, [popup, offset?.[0], offset?.[1]]);
  useLayoutEffect(() => {
    if (!map) return;
    if (lonLat) popup.setLonLat(lonLat);
    else if (marker) popup.setLonLat(marker.getLonLat());
    if (height !== undefined) popup.setHeight(height);
    if (open === undefined && marker) return;
    if (opened && !popup.isOpen()) popup.addTo(map);
    if (!opened && popup.isOpen()) popup.remove();
  }, [map, marker, popup, lonLat?.[0], lonLat?.[1], height, open, opened]);

  return createPortal(props.children, content);
};

export interface PanelProps extends Omit<PanelOptions, 'element'> {
  lonLat: LonLat;
  /** What stands in 3D: a video, an iframe, a sign */
  children?: ReactNode;
}

/** JSX standing in 3D at a position, `width` metres wide, facing `heading` */
export const Panel = (props: PanelProps): ReactNode => {
  const map = useMap();
  const { className } = props;
  const element = useMemo(() => document.createElement('div'), []);
  // The width at creation: the placement is set again by the effect below
  const initialWidth = useRef(props.width);
  const panel = useMemo(
    () => new PanelHandle({ element, width: initialWidth.current, ...defined({ className }) }),
    [element, className],
  );

  useEffect(() => {
    if (!map) return;
    panel.addTo(map);
    return () => {
      panel.remove();
    };
  }, [map, panel]);

  const { lonLat, width, heading, pitch, height } = props;
  useLayoutEffect(() => {
    panel.setLonLat(lonLat);
  }, [panel, lonLat[0], lonLat[1]]);
  useLayoutEffect(() => {
    panel.setPlacement(defined({ width, heading, pitch, height }));
  }, [panel, width, heading, pitch, height]);
  // Its content may change its CSS size
  useLayoutEffect(() => {
    panel.resize();
  });

  return createPortal(props.children, element);
};

// ── Effects ──

/** An observer's viewshed; several `<Viewshed>` are drawn together */
export const Viewshed = (props: ViewshedOptions): ReactNode => {
  const binding = useContext(MapContext);
  const key = useMemo(() => ({}), []);
  useEffect(() => {
    if (!binding) return;
    return () => {
      binding.viewsheds.delete(key);
      binding.applyViewsheds();
    };
  }, [binding, key]);
  // Numbers, strings and the position: compared by value
  const options = JSON.stringify(props);
  useEffect(() => {
    if (!binding) return;
    binding.viewsheds.set(key, JSON.parse(options) as ViewshedOptions);
    binding.applyViewsheds();
  }, [binding, key, options]);
  return null;
};

interface SwipeGroup {
  set(layer: LayerHandle): void;
  unset(layer: LayerHandle): void;
}

export interface SwipeProps extends Omit<SwipeOptions, 'layers'> {
  /** The `<Layer>` shown on one side of the line only */
  children?: ReactNode;
}

/** The `<Layer>` in it shown on one side of a vertical line only, at `position` CSS pixels from the left */
export const Swipe = (props: SwipeProps): ReactNode => {
  const map = useMap();
  const layers = useMemo(() => new Set<LayerHandle>(), []);
  const latest = useRef(props);
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

  const { position, side } = props;
  useEffect(() => {
    if (map && layers.size > 0) map.setSwipe({ position, layers: [...layers], ...defined({ side }) });
  }, [map, layers, position, side]);
  useEffect(
    () => () => {
      map?.setSwipe(undefined);
    },
    [map],
  );

  return createElement(SwipeContext.Provider, { value: group }, props.children);
};

export type { Camera, GeoMap, LayerHandle, LayerSpec, LonLat, MarkerHandle, PanelHandle, PopupHandle };
