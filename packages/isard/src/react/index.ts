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

export type { Layer as LayerHandle } from '../layers/layers.ts';
export type { GeoMap, LayerSpec } from '../map.ts';
export type { LonLat } from '../types.ts';
export type { Marker as MarkerHandle, Panel as PanelHandle, Popup as PopupHandle } from '../ui/overlay.ts';
export type { Camera } from '../wasm/isard.js';
export { Control, type ControlProps } from './Control.tsx';
export { CustomLayer, type CustomLayerProps } from './CustomLayer.tsx';
export { useMap, useMapEvent } from './context.ts';
export { IsardMap, type IsardMapProps, type MapEventProps } from './IsardMap.tsx';
export { Layer, type LayerHandles, type LayerProps, type LayerType } from './Layer.tsx';
export { Marker, type MarkerProps } from './Marker.tsx';
export { Panel, type PanelProps } from './Panel.tsx';
export { Popup, type PopupProps } from './Popup.tsx';
export { Swipe, type SwipeProps } from './Swipe.tsx';
export { Viewshed } from './Viewshed.tsx';
