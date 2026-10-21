/**
 * The map's canvas: created, named for screen readers, its WebGL context's loss and restoration followed, the device
 * pixel ratio watched (a canvas's pixels follow it).
 */

/** A canvas filling its container, the pointer's gestures left to the map */
export const createCanvas = (): HTMLCanvasElement => {
  const canvas = document.createElement('canvas');

  Object.assign(canvas.style, { display: 'block', width: '100%', height: '100%', touchAction: 'none' });

  return canvas;
};

/** Reached with Tab when the keys move the map (`keyboard`); named for screen readers either way */
export const labelCanvas = (canvas: HTMLCanvasElement, keyboard: boolean, label: string): void => {
  canvas.classList.add('isard-canvas');
  canvas.setAttribute('role', keyboard ? 'application' : 'region');
  canvas.setAttribute('aria-roledescription', 'map');
  canvas.setAttribute('aria-label', label);
  if (keyboard) canvas.tabIndex = 0;
};

/**
 * Follows the loss and restoration of the canvas's WebGL context (the browser may take the GPU away: driver reset,
 * background tab on mobile): `lost` once it is gone, the context kept to be restored, `restored` once it is back.
 * Returns what stops following them
 */
export const watchContext = (canvas: HTMLCanvasElement, lost: () => void, restored: () => void): (() => void) => {
  const onLost = (event: Event) => {
    event.preventDefault();
    lost();
  };

  canvas.addEventListener('webglcontextlost', onLost);
  canvas.addEventListener('webglcontextrestored', restored);

  return () => {
    canvas.removeEventListener('webglcontextlost', onLost);
    canvas.removeEventListener('webglcontextrestored', restored);
  };
};

/** Calls `changed` when the device pixel ratio changes (browser zoom, a window moved to another screen); returns what
 * stops watching it */
export const watchPixelRatio = (changed: () => void): (() => void) => {
  if (typeof matchMedia !== 'function') return () => undefined;

  let stop = (): void => undefined;
  const watch = () => {
    const query = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
    const listener = () => {
      // A query for the new ratio
      watch();
      changed();
    };

    query.addEventListener('change', listener, { once: true });
    stop = () => query.removeEventListener('change', listener);
  };

  watch();

  return () => stop();
};
