/**
 * What the map shows over its canvas, in its container: the markers and popups (placed after every frame) and the
 * controls in the four corners.
 */

import type { GeoMap } from '../map.ts';
import type { Point } from '../types.ts';
import type { Control, ControlPosition } from '../ui/controls.ts';
import type { Positioned } from '../ui/overlay.ts';
import { injectStyle } from '../ui/style.ts';

const CORNERS: readonly ControlPosition[] = ['top-left', 'top-right', 'bottom-left', 'bottom-right'];

/** The map's container: its canvas, the element of the markers and popups over it, the corners of the controls */
export class OverlayHost {
  /** Canvas size in CSS pixels, kept on resize so placing overlays never reads the layout */
  size: Point = [0, 0];
  private readonly container: HTMLElement;
  private readonly canvas: HTMLCanvasElement;
  /** The map the controls are added to */
  private readonly map: GeoMap;
  /** Markers and popups, over the canvas */
  private readonly overlay: HTMLDivElement;
  private readonly corners: Record<ControlPosition, HTMLDivElement>;
  private readonly controls = new Map<Control, HTMLElement>();
  private readonly positioned = new Set<Positioned>();

  /** Puts the canvas, the overlay and the corners in `container` */
  constructor(container: HTMLElement, canvas: HTMLCanvasElement, map: GeoMap) {
    this.container = container;
    this.canvas = canvas;
    this.map = map;
    // Overlays and controls are positioned against the container
    injectStyle(container.ownerDocument);
    if (getComputedStyle(container).position === 'static') container.style.position = 'relative';
    container.classList.add('isard-map');
    this.overlay = document.createElement('div');
    this.overlay.className = 'isard-overlay';

    const corner = (position: ControlPosition) => {
      const element = document.createElement('div');

      element.className = `isard-ctrl-${position}`;

      return element;
    };

    this.corners = {
      'top-left': corner('top-left'),
      'top-right': corner('top-right'),
      'bottom-left': corner('bottom-left'),
      'bottom-right': corner('bottom-right'),
    };
    container.append(canvas, this.overlay, ...CORNERS.map((position) => this.corners[position]));
  }

  /** Adds a control in a corner; controls stack from the corner inward */
  addControl(control: Control, position: ControlPosition): void {
    if (this.controls.has(control)) return;

    const element = control.onAdd(this.map);

    element.classList.add('isard-ctrl');
    // In the bottom corners the first control added stays lowest, as in MapLibre
    if (position.startsWith('bottom')) this.corners[position].prepend(element);
    else this.corners[position].append(element);
    this.controls.set(control, element);
  }

  removeControl(control: Control): void {
    const element = this.controls.get(control);

    if (!element) return;
    this.controls.delete(control);
    control.onRemove(this.map);
    element.remove();
  }

  /** Shows a marker or popup, placed after every frame from now on */
  attach(item: Positioned, element: HTMLElement): void {
    this.overlay.append(element);
    this.positioned.add(item);
    item.position(this.size);
  }

  detach(item: Positioned, element: HTMLElement): void {
    this.positioned.delete(item);
    element.remove();
  }

  /** Places one overlay now (it moved) */
  position(item: Positioned): void {
    if (this.positioned.has(item)) item.position(this.size);
  }

  /** Places every overlay for the frame just drawn (camera, relief altitudes): all in one pass, transforms only */
  positionAll(): void {
    this.positioned.forEach((item) => {
      item.position(this.size);
    });
  }

  /** Removes the controls, then the canvas, the overlay and the corners from the container */
  destroy(): void {
    [...this.controls.keys()].forEach((control) => {
      this.removeControl(control);
    });
    this.positioned.clear();
    this.canvas.remove();
    this.overlay.remove();
    CORNERS.forEach((position) => {
      this.corners[position].remove();
    });
    this.container.classList.remove('isard-map');
  }
}
