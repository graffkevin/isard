/**
 * DOM overlays tied to a position: markers and popups. The map places them all in one pass after each frame it draws
 * (camera moves, finer relief arriving), with a CSS transform from `map.project`: the browser only composites them,
 * nothing is laid out again.
 */
import type { GeoMap } from './map.ts';
import type { LonLat, Point } from './types.ts';

/** Which part of the element sits on the position */
export type Anchor =
  | 'center'
  | 'top'
  | 'bottom'
  | 'left'
  | 'right'
  | 'top-left'
  | 'top-right'
  | 'bottom-left'
  | 'bottom-right';

const ANCHOR_TRANSLATE: Record<Anchor, string> = {
  center: 'translate(-50%, -50%)',
  top: 'translate(-50%, 0)',
  bottom: 'translate(-50%, -100%)',
  left: 'translate(0, -50%)',
  right: 'translate(-100%, -50%)',
  'top-left': 'translate(0, 0)',
  'top-right': 'translate(-100%, 0)',
  'bottom-left': 'translate(0, -100%)',
  'bottom-right': 'translate(-100%, -100%)',
};

/** Overlays further than this out of the canvas (CSS pixels) are hidden: no part of them can show, and huge
 * transforms near the horizon would only cost the compositor */
const MARGIN = 500;
/** A press on a draggable marker that moves less than this (CSS pixels) is a click */
const DRAG_TOLERANCE = 3;

/** @internal What the map places after each frame */
export interface Positioned {
  /** Places the overlay for the current camera; `size` is the canvas size in CSS pixels */
  position(size: Point): void;
}

/** Typed listeners of an overlay */
class Emitter<E> {
  private readonly listeners: { [K in keyof E]?: Set<(event: E[K]) => void> } = {};

  on<K extends keyof E>(type: K, listener: (event: E[K]) => void): () => void {
    const set = this.listeners[type] ?? new Set();
    this.listeners[type] = set;
    set.add(listener);
    return () => set.delete(listener);
  }

  emit<K extends keyof E>(type: K, event: E[K]): void {
    for (const listener of this.listeners[type] ?? []) listener(event);
  }
}

/** Moves `element` so that its anchor sits on `lonLat`, or hides it when the position is out of sight */
function place(map: GeoMap, element: HTMLElement, lonLat: LonLat, anchor: Anchor, offset: Point, size: Point): void {
  const point = map.project(lonLat);
  const visible =
    point !== undefined &&
    point[0] > -MARGIN &&
    point[0] < size[0] + MARGIN &&
    point[1] > -MARGIN &&
    point[1] < size[1] + MARGIN;
  if (!visible) {
    element.style.display = 'none';
    return;
  }
  element.style.display = '';
  // Whole pixels: a fractional translation blurs text and icons
  const x = Math.round(point[0] + offset[0]);
  const y = Math.round(point[1] + offset[1]);
  element.style.transform = `translate(${x}px, ${y}px) ${ANCHOR_TRANSLATE[anchor]}`;
}

/** Round pin, its tip at the bottom: the default marker */
function pin(): HTMLElement {
  const element = document.createElement('div');
  element.innerHTML =
    '<svg class="isard-marker-pin" width="25" height="34" viewBox="0 0 25 34" aria-hidden="true">' +
    '<path fill="currentColor" stroke="rgb(0 0 0 / 0.25)" d="M12.5 33.5C12.5 33.5 .5 20.6 .5 12.5a12 12 0 0 1 24 0' +
    'c0 8.1-12 21-12 21z"/><circle cx="12.5" cy="12.5" r="4.5" fill="#fff"/></svg>';
  return element;
}

export interface MarkerOptions {
  /** The element to show (default: a round pin) */
  element?: HTMLElement;
  /** Which part of the element sits on the position (default 'center' with an element of your own, 'bottom' for the
   * default pin) */
  anchor?: Anchor;
  /** Shift from the anchor, CSS pixels (default [0, 0]) */
  offset?: Point;
  /** The marker can be dragged over the map, on the relief (default false) */
  draggable?: boolean;
  /** Classes added to the element, space separated */
  className?: string;
}

export interface MarkerEvent {
  target: Marker;
  lonLat: LonLat;
}

export interface MarkerEvents {
  dragstart: MarkerEvent;
  drag: MarkerEvent;
  dragend: MarkerEvent;
}

/** An element at a position on the map, optionally draggable */
export class Marker implements Positioned {
  private readonly element: HTMLElement;
  private readonly anchor: Anchor;
  private readonly events = new Emitter<MarkerEvents>();
  private offset: Point;
  private draggable: boolean;
  private lonLat: LonLat = [0, 0];
  private map: GeoMap | undefined;
  private popup: Popup | undefined;
  /** While pressed: the press point and the gap between the pointer and the marker's position */
  private grab: { start: Point; gap: Point } | undefined;
  private dragging = false;
  /** The click that ends a drag is not a click on the marker */
  private swallowClick = false;

  constructor(options: MarkerOptions = {}) {
    this.element = options.element ?? pin();
    this.anchor = options.anchor ?? (options.element ? 'center' : 'bottom');
    this.offset = options.offset ?? [0, 0];
    this.draggable = options.draggable ?? false;
    this.element.classList.add('isard-marker');
    if (options.className) this.element.classList.add(...options.className.split(/\s+/).filter(Boolean));
    this.element.classList.toggle('isard-marker-draggable', this.draggable);
    this.element.addEventListener('pointerdown', (e) => this.down(e));
    this.element.addEventListener('pointermove', (e) => this.move(e));
    this.element.addEventListener('pointerup', (e) => this.up(e));
    this.element.addEventListener('pointercancel', (e) => this.up(e));
    this.element.addEventListener('click', (e) => this.click(e), { capture: true });
  }

  getElement(): HTMLElement {
    return this.element;
  }

  getLonLat(): LonLat {
    return [...this.lonLat];
  }

  setLonLat(lonLat: LonLat): this {
    this.lonLat = [lonLat[0], lonLat[1]];
    this.map?.positionOverlay(this);
    // An open popup of the marker goes with it
    if (this.popup?.isOpen()) this.popup.setLonLat(this.lonLat);
    return this;
  }

  setOffset(offset: Point): this {
    this.offset = [offset[0], offset[1]];
    this.map?.positionOverlay(this);
    return this;
  }

  setDraggable(draggable: boolean): this {
    this.draggable = draggable;
    this.element.classList.toggle('isard-marker-draggable', draggable);
    return this;
  }

  isDraggable(): boolean {
    return this.draggable;
  }

  /** A popup that a click on the marker opens and closes, at the marker's position; give it an offset clearing the
   * marker (with the default pin, [0, -34]) */
  setPopup(popup: Popup | undefined): this {
    this.popup?.remove();
    this.popup = popup;
    return this;
  }

  getPopup(): Popup | undefined {
    return this.popup;
  }

  /** Opens the marker's popup, or closes it if open */
  togglePopup(): this {
    if (!this.popup || !this.map) return this;
    if (this.popup.isOpen()) this.popup.remove();
    else this.popup.setLonLat(this.lonLat).addTo(this.map);
    return this;
  }

  /** Listens to the drag of the marker; returns the function that stops listening */
  on<K extends keyof MarkerEvents>(type: K, listener: (event: MarkerEvents[K]) => void): () => void {
    return this.events.on(type, listener);
  }

  addTo(map: GeoMap): this {
    if (this.map === map) return this;
    this.remove();
    this.map = map;
    map.attachOverlay(this, this.element);
    return this;
  }

  remove(): this {
    this.popup?.remove();
    this.map?.detachOverlay(this, this.element);
    this.map = undefined;
    this.grab = undefined;
    this.dragging = false;
    this.element.classList.remove('isard-marker-dragging');
    return this;
  }

  /** @internal */
  position(size: Point): void {
    if (this.map) place(this.map, this.element, this.lonLat, this.anchor, this.offset, size);
  }

  private local(e: PointerEvent): Point | undefined {
    const rect = this.map?.getCanvas().getBoundingClientRect();
    return rect && [e.clientX - rect.left, e.clientY - rect.top];
  }

  private down(e: PointerEvent): void {
    this.swallowClick = false;
    if (!this.draggable || !this.map || e.button !== 0) return;
    const point = this.map.project(this.lonLat);
    const at = this.local(e);
    if (!point || !at) return;
    // The map must not start panning under the marker
    e.stopPropagation();
    e.preventDefault();
    this.element.setPointerCapture(e.pointerId);
    // The grabbed spot of the marker stays under the pointer, wherever the element was pressed
    this.grab = { start: at, gap: [at[0] - point[0], at[1] - point[1]] };
    this.dragging = false;
  }

  private move(e: PointerEvent): void {
    const at = this.local(e);
    if (!this.grab || !this.map || !at) return;
    e.stopPropagation();
    const { start, gap } = this.grab;
    if (!this.dragging && Math.hypot(at[0] - start[0], at[1] - start[1]) < DRAG_TOLERANCE) return;
    // On the relief: the ground under the point, read back from the picking pass
    const lonLat = this.map.unproject([at[0] - gap[0], at[1] - gap[1]]);
    if (!lonLat) return;
    if (!this.dragging) {
      this.dragging = true;
      this.element.classList.add('isard-marker-dragging');
      this.events.emit('dragstart', { target: this, lonLat: this.getLonLat() });
    }
    this.setLonLat(lonLat);
    this.events.emit('drag', { target: this, lonLat: this.getLonLat() });
  }

  private up(e: PointerEvent): void {
    if (!this.grab) return;
    e.stopPropagation();
    this.grab = undefined;
    if (this.element.hasPointerCapture(e.pointerId)) this.element.releasePointerCapture(e.pointerId);
    if (!this.dragging) return;
    this.dragging = false;
    this.swallowClick = true;
    this.element.classList.remove('isard-marker-dragging');
    this.events.emit('dragend', { target: this, lonLat: this.getLonLat() });
  }

  private click(e: MouseEvent): void {
    if (this.swallowClick) {
      this.swallowClick = false;
      e.stopImmediatePropagation();
      e.preventDefault();
      return;
    }
    if (this.popup) this.togglePopup();
  }
}

export interface PopupOptions {
  /** A × button in the corner (default true) */
  closeButton?: boolean;
  /** A click on the map elsewhere closes the popup (default true) */
  closeOnClick?: boolean;
  /** CSS max-width of the popup (default '240px') */
  maxWidth?: string;
  /** Classes added to the popup, space separated */
  className?: string;
  /** Which part of the popup sits on the position, its tip there (default 'bottom': the popup is above the point) */
  anchor?: Anchor;
  /** Shift from the anchor, CSS pixels (default [0, 0]) */
  offset?: Point;
}

export interface PopupEvents {
  open: { target: Popup };
  close: { target: Popup };
}

/** A box of content at a position, its tip pointing at it */
export class Popup implements Positioned {
  private readonly container: HTMLElement;
  private readonly content: HTMLElement;
  private readonly closeButton: HTMLButtonElement | undefined;
  private readonly anchor: Anchor;
  private readonly closeOnClick: boolean;
  private readonly events = new Emitter<PopupEvents>();
  private offset: Point;
  private lonLat: LonLat = [0, 0];
  private map: GeoMap | undefined;
  private stopListening: (() => void) | undefined;

  constructor(options: PopupOptions = {}) {
    this.anchor = options.anchor ?? 'bottom';
    this.offset = options.offset ?? [0, 0];
    this.closeOnClick = options.closeOnClick ?? true;
    this.container = document.createElement('div');
    this.container.className = `isard-popup isard-popup-anchor-${this.anchor}`;
    if (options.className) this.container.classList.add(...options.className.split(/\s+/).filter(Boolean));
    this.container.style.maxWidth = options.maxWidth ?? '240px';
    const tip = document.createElement('div');
    tip.className = 'isard-popup-tip';
    this.content = document.createElement('div');
    this.content.className = 'isard-popup-content';
    if (options.closeButton ?? true) {
      this.container.classList.add('isard-popup-has-close');
      this.closeButton = document.createElement('button');
      this.closeButton.type = 'button';
      this.closeButton.className = 'isard-popup-close';
      this.closeButton.ariaLabel = 'Close popup';
      this.closeButton.textContent = '×';
      this.closeButton.addEventListener('click', () => this.remove());
      this.content.append(this.closeButton);
    }
    this.container.append(tip, this.content);
  }

  getElement(): HTMLElement {
    return this.container;
  }

  getLonLat(): LonLat {
    return [...this.lonLat];
  }

  setLonLat(lonLat: LonLat): this {
    this.lonLat = [lonLat[0], lonLat[1]];
    this.map?.positionOverlay(this);
    return this;
  }

  setOffset(offset: Point): this {
    this.offset = [offset[0], offset[1]];
    this.map?.positionOverlay(this);
    return this;
  }

  /** Content from an HTML string: only for trusted HTML (use `setText` for user or remote data) */
  setHTML(html: string): this {
    const template = document.createElement('template');
    template.innerHTML = html;
    return this.setDOMContent(template.content);
  }

  setText(text: string): this {
    return this.setDOMContent(document.createTextNode(text));
  }

  setDOMContent(node: Node): this {
    this.content.replaceChildren(node);
    if (this.closeButton) this.content.append(this.closeButton);
    this.map?.positionOverlay(this);
    return this;
  }

  isOpen(): boolean {
    return this.map !== undefined;
  }

  /** Listens to the popup opening or closing; returns the function that stops listening */
  on<K extends keyof PopupEvents>(type: K, listener: (event: PopupEvents[K]) => void): () => void {
    return this.events.on(type, listener);
  }

  addTo(map: GeoMap): this {
    if (this.map === map) return this;
    this.remove();
    this.map = map;
    map.attachOverlay(this, this.container);
    if (this.closeOnClick) {
      // From the next task on: a popup opened by a map click must not be closed by that same click
      queueMicrotask(() => {
        if (this.map === map && !this.stopListening) this.stopListening = map.on('click', () => this.remove());
      });
    }
    this.events.emit('open', { target: this });
    return this;
  }

  remove(): this {
    if (!this.map) return this;
    this.stopListening?.();
    this.stopListening = undefined;
    this.map.detachOverlay(this, this.container);
    this.map = undefined;
    this.events.emit('close', { target: this });
    return this;
  }

  /** @internal */
  position(size: Point): void {
    if (this.map) place(this.map, this.container, this.lonLat, this.anchor, this.offset, size);
  }
}
