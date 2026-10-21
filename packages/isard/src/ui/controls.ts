/**
 * Controls: elements in a corner of the map. A control is anything with `onAdd` and `onRemove`, as in MapLibre, so
 * applications write their own; the navigation buttons and the attribution are built in.
 */
import type { GeoMap } from '../map.ts';

export type ControlPosition = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';

export interface Control {
  /** Builds the control's element; the map puts it in its corner */
  onAdd(map: GeoMap): HTMLElement;
  /** Cleans up (listeners); the map removes the element */
  onRemove(map: GeoMap): void;
}

const svg = (body: string) =>
  `<svg width="29" height="29" viewBox="0 0 29 29" fill="currentColor" aria-hidden="true">${body}</svg>`;
const PLUS = svg(
  '<path d="M14.5 8.5a1 1 0 0 1 1 1v4h4a1 1 0 0 1 0 2h-4v4a1 1 0 0 1-2 0v-4h-4a1 1 0 0 1 0-2h4v-4a1 1 0 0 1 1-1z"/>',
);
const MINUS = svg('<path d="M9.5 13.5h10a1 1 0 0 1 0 2h-10a1 1 0 0 1 0-2z"/>');
/** Needle: red half to the north, grey half to the south */
const COMPASS = svg(
  '<path d="M10.5 14 14.5 6.5 18.5 14z" fill="#e3342f"/><path d="M10.5 15 14.5 22.5 18.5 15z" fill="currentColor" opacity="0.5"/>',
);
const INFO =
  '<svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true"><path fill-rule="evenodd" ' +
  'd="M10 3a7 7 0 1 0 0 14 7 7 0 0 0 0-14zm-1 5.5h2V14H9zM10 5.5a1.1 1.1 0 1 1 0 2.2 1.1 1.1 0 0 1 0-2.2z"/></svg>';

const button = (className: string, label: string, icon: string, onClick: () => void): HTMLButtonElement => {
  const element = document.createElement('button');

  element.type = 'button';
  element.className = className;
  element.title = label;
  element.setAttribute('aria-label', label);
  element.innerHTML = icon;
  element.addEventListener('click', onClick);

  return element;
};

export interface NavigationOptions {
  /** Zoom in and out buttons (default true) */
  showZoom?: boolean;
  /** Compass turning with the bearing; a click faces north and levels the view (default true) */
  showCompass?: boolean;
  /** The compass also tilts with the pitch (default false) */
  visualizePitch?: boolean;
}

/** Zoom buttons and compass */
export class NavigationControl implements Control {
  private readonly options: Required<NavigationOptions>;
  private stopListening: (() => void) | undefined;

  constructor(options: NavigationOptions = {}) {
    this.options = { showZoom: true, showCompass: true, visualizePitch: false, ...options };
  }

  onAdd(map: GeoMap): HTMLElement {
    const group = document.createElement('div');

    group.className = 'isard-ctrl-group';

    const zoomBy = (delta: number) => map.easeTo({ zoom: map.getCamera().zoom + delta }, 300);

    if (this.options.showZoom) {
      group.append(
        button('isard-ctrl-zoom-in', 'Zoom in', PLUS, () => zoomBy(1)),
        button('isard-ctrl-zoom-out', 'Zoom out', MINUS, () => zoomBy(-1)),
      );
    }
    if (this.options.showCompass) {
      const compass = button('isard-ctrl-compass', 'Reset bearing to north', COMPASS, () =>
        map.easeTo({ bearing: 0, pitch: 0 }, 300),
      );
      const needle = compass.firstElementChild;
      const turn = () => {
        if (!(needle instanceof SVGElement)) return;

        const { bearing, pitch } = map.getCamera();

        // Seen from above when tilted: the needle lies back with the ground, squashed by cos(pitch)
        needle.style.transform = this.options.visualizePitch
          ? `scale(${1 / Math.cos((pitch * Math.PI) / 180) ** 0.5}) rotateX(${pitch}deg) rotateZ(${-bearing}deg)`
          : `rotate(${-bearing}deg)`;
      };

      turn();
      this.stopListening = map.on('move', turn);
      group.append(compass);
    }

    return group;
  }

  onRemove(): void {
    this.stopListening?.();
    this.stopListening = undefined;
  }
}

export interface AttributionOptions {
  /** Folded into a small (i) button that shows the text on click (default: when the map is narrower than 640 px) */
  compact?: boolean;
  /** Attributions shown after those of the map's sources, HTML allowed (links open in a new tab) */
  customAttribution?: string | string[];
}

/** Credits of the data shown: those of the map's basemap, hillshade and relief (OpenFreeMap and OpenStreetMap, IGN,
 * the Terrain Tiles…, followed as they change), then the page's own */
export class AttributionControl implements Control {
  private observer: ResizeObserver | undefined;
  private stopListening: (() => void) | undefined;

  constructor(private readonly options: AttributionOptions = {}) {}

  onAdd(map: GeoMap): HTMLElement {
    const element = document.createElement('div');

    element.className = 'isard-attrib';

    const inner = document.createElement('div');

    inner.className = 'isard-attrib-inner';

    const custom = this.options.customAttribution ?? [];
    const render = () => {
      inner.innerHTML = [...map.attributions(), ...(typeof custom === 'string' ? [custom] : custom)].join(' | ');
      inner.querySelectorAll('a').forEach((link) => {
        link.target = '_blank';
        link.rel = 'noopener noreferrer';
      });
    };

    render();
    this.stopListening = map.onAttributionChange(render);

    const toggle = button('isard-attrib-button', 'Toggle attribution', INFO, () =>
      toggle.setAttribute('aria-expanded', String(element.classList.toggle('isard-attrib-open'))),
    );

    toggle.setAttribute('aria-expanded', 'false');
    element.append(inner, toggle);

    const { compact } = this.options;

    if (compact !== undefined) {
      element.classList.toggle('isard-attrib-compact', compact);
    } else {
      // Follows the map's width; a narrowed map folds the text, starting closed
      const fold = () => {
        const narrow = map.getContainer().clientWidth < 640;

        if (narrow !== element.classList.contains('isard-attrib-compact')) {
          element.classList.remove('isard-attrib-open');
          toggle.setAttribute('aria-expanded', 'false');
        }
        element.classList.toggle('isard-attrib-compact', narrow);
      };

      fold();
      this.observer = new ResizeObserver(fold);
      this.observer.observe(map.getContainer());
    }

    return element;
  }

  onRemove(): void {
    this.observer?.disconnect();
    this.observer = undefined;
    this.stopListening?.();
    this.stopListening = undefined;
  }
}
