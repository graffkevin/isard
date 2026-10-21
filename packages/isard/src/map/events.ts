/** The listeners of the map's events: added, called, cleared with the map. */

import type { MapEvents } from '../types.ts';
import { report } from './checks.ts';

type Listeners = { [K in keyof MapEvents]: Set<(event: MapEvents[K]) => void> };

/** The listeners of the map's events, by type */
export class MapListeners {
  private readonly listeners: Listeners = {
    movestart: new Set(),
    move: new Set(),
    moveend: new Set(),
    resize: new Set(),
    hover: new Set(),
    hoverfeature: new Set(),
    click: new Set(),
    render: new Set(),
    idle: new Set(),
    leave: new Set(),
  };

  /** Listens to an event; returns the function that stops listening */
  on<K extends keyof MapEvents>(type: K, listener: (event: MapEvents[K]) => void): () => void {
    const listeners: Set<(event: MapEvents[K]) => void> = this.listeners[type];

    listeners.add(listener);

    return () => listeners.delete(listener);
  }

  /** Calls the listeners; one that throws is reported and does not stop the others nor the frame */
  emit<K extends keyof MapEvents>(type: K, event: MapEvents[K]): void {
    const listeners: Set<(event: MapEvents[K]) => void> = this.listeners[type];

    listeners.forEach((listener) => {
      try {
        listener(event);
      } catch (error) {
        report(error);
      }
    });
  }

  /** Something listens to `type` */
  has(type: keyof MapEvents): boolean {
    return this.listeners[type].size > 0;
  }

  /** Forgets every listener */
  clear(): void {
    Object.values(this.listeners).forEach((listeners) => {
      listeners.clear();
    });
  }
}
