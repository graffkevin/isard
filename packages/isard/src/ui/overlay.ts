/**
 * DOM overlays tied to a position: markers and popups. The map places them all in one pass after each frame it draws
 * (camera moves, finer relief arriving), with a CSS transform from `map.project`: the browser only composites them,
 * nothing is laid out again.
 */
import type { GeoMap } from '../map.ts';
import type { LonLat, Point } from '../types.ts';

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

/** Where the anchor lies within the element, CSS percentages: the stem of a raised marker starts there */
const ANCHOR_POINT: Record<Anchor, [string, string]> = {
  center: ['50%', '50%'],
  top: ['50%', '0'],
  bottom: ['50%', '100%'],
  left: ['0', '50%'],
  right: ['100%', '50%'],
  'top-left': ['0', '0'],
  'top-right': ['100%', '0'],
  'bottom-left': ['0', '100%'],
  'bottom-right': ['100%', '100%'],
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
    this.listeners[type]?.forEach((listener) => {
      listener(event);
    });
  }
}

/** Moves `element` so that its anchor sits on `lonLat`, `height` metres above the ground, or hides it when the
 * position is out of sight; returns the screen point */
const place = (
  map: GeoMap,
  element: HTMLElement,
  lonLat: LonLat,
  height: number,
  anchor: Anchor,
  offset: Point,
  size: Point,
): Point | undefined => {
  const point = map.project(lonLat, height);
  const visible =
    point !== undefined &&
    point[0] > -MARGIN &&
    point[0] < size[0] + MARGIN &&
    point[1] > -MARGIN &&
    point[1] < size[1] + MARGIN;

  if (!visible) {
    element.style.display = 'none';

    return undefined;
  }
  element.style.display = '';

  // Whole pixels: a fractional translation blurs text and icons
  const x = Math.round(point[0] + offset[0]);
  const y = Math.round(point[1] + offset[1]);

  element.style.transform = `translate(${x}px, ${y}px) ${ANCHOR_TRANSLATE[anchor]}`;

  return point;
};

/** Round pin, its tip at the bottom: the default marker */
const pin = (): HTMLElement => {
  const element = document.createElement('div');

  element.innerHTML =
    '<svg class="isard-marker-pin" width="25" height="34" viewBox="0 0 25 34" aria-hidden="true">' +
    '<path fill="currentColor" stroke="rgb(0 0 0 / 0.25)" d="M12.5 33.5C12.5 33.5 .5 20.6 .5 12.5a12 12 0 0 1 24 0' +
    'c0 8.1-12 21-12 21z"/><circle cx="12.5" cy="12.5" r="4.5" fill="#fff"/></svg>';

  return element;
};

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
  /** Metres above the ground (default 0): a raised marker has a stem down to its position on the ground */
  height?: number;
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
export class Marker {
  private readonly element: HTMLElement;
  private readonly anchor: Anchor;
  private readonly events = new Emitter<MarkerEvents>();
  private offset: Point;
  private draggable: boolean;
  private lonLat: LonLat = [0, 0];
  private height: number;
  /** Metres above the ground of what the marker stands on, where its stem ends */
  private base = 0;
  /** The line from a raised marker down to the ground */
  private readonly stem: HTMLElement;
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
    this.height = options.height ?? 0;
    this.stem = document.createElement('div');
    this.stem.className = 'isard-marker-stem';
    [this.stem.style.left, this.stem.style.top] = ANCHOR_POINT[this.anchor];
    this.element.append(this.stem);
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

  getHeight(): number {
    return this.height;
  }

  /** Metres above the ground: a raised marker has a stem down to `base` metres, what it stands on (a roof, a mesh;
   * the ground by default); its popup is raised too */
  setHeight(height: number, base = 0): this {
    this.height = Number.isFinite(height) ? height : 0;
    this.base = Number.isFinite(base) ? Math.min(base, this.height) : 0;
    this.popup?.setHeight(this.height);
    this.map?.positionOverlay(this);

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
    popup?.setHeight(this.height);

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
    if (!this.map) return;

    const top = place(this.map, this.element, this.lonLat, this.height, this.anchor, this.offset, size);
    const ground = top && this.height !== this.base ? this.map.project(this.lonLat, this.base) : undefined;

    if (!top || !ground) {
      this.stem.style.display = 'none';

      return;
    }

    // From the anchor down to what it stands on, a line turned toward it
    const [dx, dy] = [ground[0] - top[0], ground[1] - top[1]];

    this.stem.style.display = '';
    this.stem.style.height = `${Math.hypot(dx, dy)}px`;
    this.stem.style.transform = `rotate(${Math.atan2(dy, dx) - Math.PI / 2}rad)`;
  }

  private local(e: PointerEvent): Point | undefined {
    const rect = this.map?.getCanvas().getBoundingClientRect();

    return rect && [e.clientX - rect.left, e.clientY - rect.top];
  }

  private down(e: PointerEvent): void {
    this.swallowClick = false;
    if (!this.draggable || !this.map || e.button !== 0) return;

    // The gap to the position on the ground, below a raised marker
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
export class Popup {
  private readonly container: HTMLElement;
  private readonly content: HTMLElement;
  private readonly closeButton: HTMLButtonElement | undefined;
  private readonly anchor: Anchor;
  private readonly closeOnClick: boolean;
  private readonly events = new Emitter<PopupEvents>();
  private offset: Point;
  private lonLat: LonLat = [0, 0];
  private height = 0;
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
      this.closeButton.setAttribute('aria-label', 'Close popup');
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

  getHeight(): number {
    return this.height;
  }

  /** Metres above the ground (default 0); a marker's popup follows the marker's height */
  setHeight(height: number): this {
    this.height = Number.isFinite(height) ? height : 0;
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
    if (this.map) place(this.map, this.container, this.lonLat, this.height, this.anchor, this.offset, size);
  }
}

export interface PanelOptions {
  /** The element shown in 3D (a video, an iframe, an image…), laid out at its CSS size */
  element: HTMLElement;
  /** Its width on the ground, metres; its height follows its CSS aspect */
  width: number;
  /** Degrees clockwise from north the side it faces (default 0: it faces north, read from the north) */
  heading?: number;
  /** Degrees it leans back from upright (default 0: standing; 90: lying flat) */
  pitch?: number;
  /** Metres of its bottom edge above the ground (the relief; default 0) */
  height?: number;
  /** Classes added to the element, space separated */
  className?: string;
}

/** `a × b`, 4 × 4 column-major */
const multiply = (a: ArrayLike<number>, b: ArrayLike<number>): number[] =>
  Array.from({ length: 16 }, (_, i) => {
    const [c, r] = [Math.floor(i / 4), i % 4];

    return [0, 1, 2, 3].reduce((sum, k) => sum + (a[k * 4 + r] ?? 0) * (b[c * 4 + k] ?? 0), 0);
  });

/**
 * An element standing in 3D at a position: a video, a web page, a sign, `width` metres wide on the ground, facing
 * `heading`. Placed after each frame by a CSS 3D transform (the element is not drawn by WebGL: the scene does not
 * hide it); hidden when its position is behind the camera
 */
export class Panel {
  private readonly element: HTMLElement;
  private lonLat: LonLat = [0, 0];
  private width: number;
  private heading: number;
  private pitch: number;
  private height: number;
  private map: GeoMap | undefined;
  /** Its CSS size, read once it is on the page (`resize` reads it again) */
  private size: Point | undefined;

  constructor(options: PanelOptions) {
    this.element = options.element;
    this.width = options.width;
    this.heading = options.heading ?? 0;
    this.pitch = options.pitch ?? 0;
    this.height = options.height ?? 0;
    this.element.classList.add('isard-panel');
    if (options.className) this.element.classList.add(...options.className.split(/\s+/).filter(Boolean));
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

    return this;
  }

  /** Width (metres), heading and pitch (degrees), height of its bottom edge (metres), as in `PanelOptions` */
  setPlacement(placement: Partial<Pick<PanelOptions, 'width' | 'heading' | 'pitch' | 'height'>>): this {
    this.width = placement.width ?? this.width;
    this.heading = placement.heading ?? this.heading;
    this.pitch = placement.pitch ?? this.pitch;
    this.height = placement.height ?? this.height;
    this.map?.positionOverlay(this);

    return this;
  }

  /** Reads the element's CSS size again (after it changed) */
  resize(): this {
    this.size = undefined;
    this.map?.positionOverlay(this);

    return this;
  }

  addTo(map: GeoMap): this {
    if (this.map === map) return this;
    this.remove();
    this.map = map;
    map.attachOverlay(this, this.element);

    return this;
  }

  remove(): this {
    this.map?.detachOverlay(this, this.element);
    this.map = undefined;
    this.size = undefined;

    return this;
  }

  /** @internal */
  position(viewport: Point): void {
    const map = this.map;

    if (!map) return;
    this.size ??= [this.element.offsetWidth || 300, this.element.offsetHeight || 150];

    const [w, h] = this.size;
    const ground = map.elevationAt(this.lonLat) ?? 0;
    const local = map.engine.localMatrix(this.lonLat[0], this.lonLat[1], ground);
    // The element's pixels to the local frame (metres, x east, y north, z up): its bottom edge centered on the
    // position, `height` up, facing `heading`, leant back by `pitch`
    const rad = Math.PI / 180;
    const [hd, pt] = [this.heading * rad, this.pitch * rad];
    const s = this.width / w;
    // Across it (left to right as read from the side it faces), and up it
    const [ax, ay] = [-Math.cos(hd), Math.sin(hd)];
    const [ux, uy, uz] = [-Math.sin(hd) * Math.sin(pt), -Math.cos(hd) * Math.sin(pt), Math.cos(pt)];
    const top = h * s;
    const origin = [-ax * (this.width / 2) + ux * top, -ay * (this.width / 2) + uy * top, this.height + uz * top];
    const toLocal = [ax * s, ay * s, 0, 0, -ux * s, -uy * s, -uz * s, 0, 0, 0, 0, 0, ...origin, 1];
    // Clip space to the map's CSS pixels; the depth flattened
    const [vw, vh] = viewport;
    const toScreen = [vw / 2, 0, 0, 0, 0, -vh / 2, 0, 0, 0, 0, 0, 0, vw / 2, vh / 2, 0, 1];
    const clip = multiply(local, toLocal);
    // Behind the camera (its center): hidden
    const centerW = (clip[3] ?? 0) * (w / 2) + (clip[7] ?? 0) * (h / 2) + (clip[15] ?? 0);

    // Hidden without touching its display (the page's layout of its content)
    if (!(centerW > 0)) {
      this.element.style.visibility = 'hidden';

      return;
    }
    this.element.style.visibility = '';

    // Its depth axis the identity, out of the rest: its points have no depth, and a transform that cannot be
    // inverted is not drawn
    const m = multiply(toScreen, clip);

    m.splice(8, 4, 0, 0, 1, 0);
    [2, 6, 14].forEach((k) => {
      m[k] = 0;
    });
    this.element.style.transform = `matrix3d(${m.join(',')})`;
  }
}
