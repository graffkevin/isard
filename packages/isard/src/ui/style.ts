/**
 * Styles of the DOM overlays (markers, popups, controls), injected once per document so the package needs no CSS
 * file. Colors are custom properties, defaults on the root with no specificity: an application restyles them on any
 * element around the map (its container's class may be replaced by a framework: nothing depends on it).
 */

const CSS = `
:where(:root) {
  --isard-ctrl-bg: #fff;
  --isard-ctrl-fg: #333;
  --isard-ctrl-hover: rgb(0 0 0 / 0.05);
  --isard-ctrl-active: #1d2433;
  --isard-ctrl-active-fg: #fff;
  --isard-ctrl-border: #ddd;
  --isard-ctrl-shadow: 0 0 0 2px rgb(0 0 0 / 0.1);
  --isard-popup-bg: #fff;
  --isard-popup-fg: #1d2433;
  --isard-marker-color: #3a6fd8;
  --isard-attrib-bg: rgb(255 255 255 / 0.5);
  --isard-attrib-fg: rgb(0 0 0 / 0.75);
  --isard-focus: #3a6fd8;
}
.isard-canvas:focus { outline: none; }
.isard-canvas:focus-visible { outline: 2px solid var(--isard-focus); outline-offset: -2px; }
.isard-overlay { position: absolute; inset: 0; overflow: hidden; pointer-events: none; z-index: 1; }
.isard-ctrl-top-left, .isard-ctrl-top-right, .isard-ctrl-bottom-left, .isard-ctrl-bottom-right {
  position: absolute; pointer-events: none; z-index: 2;
}
.isard-ctrl-top-left { top: 0; left: 0; }
.isard-ctrl-top-right { top: 0; right: 0; }
.isard-ctrl-bottom-left { bottom: 0; left: 0; }
.isard-ctrl-bottom-right { bottom: 0; right: 0; }
.isard-ctrl { clear: both; pointer-events: auto; transform: translate(0, 0); }
.isard-ctrl-top-left .isard-ctrl { float: left; margin: 10px 0 0 10px; }
.isard-ctrl-top-right .isard-ctrl { float: right; margin: 10px 10px 0 0; }
.isard-ctrl-bottom-left .isard-ctrl { float: left; margin: 0 0 10px 10px; }
.isard-ctrl-bottom-right .isard-ctrl { float: right; margin: 0 10px 10px 0; }
.isard-ctrl-group { background: var(--isard-ctrl-bg); border-radius: 4px; box-shadow: var(--isard-ctrl-shadow); }
.isard-ctrl-group button {
  display: block; width: 29px; height: 29px; padding: 0; margin: 0; border: 0; box-sizing: border-box;
  background: transparent; color: var(--isard-ctrl-fg); cursor: pointer; font: inherit;
}
.isard-ctrl-group button + button { border-top: 1px solid var(--isard-ctrl-border); }
.isard-ctrl-group button:first-child { border-radius: 4px 4px 0 0; }
.isard-ctrl-group button:last-child { border-radius: 0 0 4px 4px; }
.isard-ctrl-group button:only-child { border-radius: inherit; }
.isard-ctrl-group button:hover { background-color: var(--isard-ctrl-hover); }
.isard-ctrl-group button:focus-visible { outline: 2px solid var(--isard-ctrl-active); outline-offset: -2px; }
.isard-ctrl-group button.active { background: var(--isard-ctrl-active); color: var(--isard-ctrl-active-fg); }
.isard-ctrl-group button svg { display: block; margin: auto; pointer-events: none; }
.isard-ctrl-compass svg { transition: transform 0.05s; }
.isard-marker { position: absolute; top: 0; left: 0; pointer-events: auto; will-change: transform; }
.isard-marker-draggable { cursor: grab; touch-action: none; }
.isard-marker-dragging { cursor: grabbing; }
.isard-marker-stem { position: absolute; z-index: -1; width: 2px; margin-left: -1px; transform-origin: top center; background: var(--isard-marker-color); opacity: 0.8; pointer-events: none; }
.isard-marker-stem::after { content: ""; position: absolute; bottom: -3px; left: -2px; width: 6px; height: 6px; border-radius: 50%; background: inherit; }
.isard-marker-pin { display: block; color: var(--isard-marker-color); filter: drop-shadow(0 1px 2px rgb(0 0 0 / 0.35)); }
.isard-panel { position: absolute; top: 0; left: 0; transform-origin: 0 0; pointer-events: auto; will-change: transform; }
.isard-popup {
  position: absolute; top: 0; left: 0; display: flex; pointer-events: none; will-change: transform; z-index: 1;
  color: var(--isard-popup-fg);
}
.isard-popup-anchor-top, .isard-popup-anchor-top-left, .isard-popup-anchor-top-right { flex-direction: column; }
.isard-popup-anchor-bottom, .isard-popup-anchor-bottom-left, .isard-popup-anchor-bottom-right {
  flex-direction: column-reverse;
}
.isard-popup-anchor-left { flex-direction: row; }
.isard-popup-anchor-right { flex-direction: row-reverse; }
.isard-popup-tip { width: 0; height: 0; border: 10px solid transparent; z-index: 1; }
.isard-popup-anchor-top .isard-popup-tip { align-self: center; border-top: none; border-bottom-color: var(--isard-popup-bg); }
.isard-popup-anchor-top-left .isard-popup-tip {
  align-self: flex-start; border-top: none; border-left: none; border-bottom-color: var(--isard-popup-bg);
}
.isard-popup-anchor-top-right .isard-popup-tip {
  align-self: flex-end; border-top: none; border-right: none; border-bottom-color: var(--isard-popup-bg);
}
.isard-popup-anchor-bottom .isard-popup-tip { align-self: center; border-bottom: none; border-top-color: var(--isard-popup-bg); }
.isard-popup-anchor-bottom-left .isard-popup-tip {
  align-self: flex-start; border-bottom: none; border-left: none; border-top-color: var(--isard-popup-bg);
}
.isard-popup-anchor-bottom-right .isard-popup-tip {
  align-self: flex-end; border-bottom: none; border-right: none; border-top-color: var(--isard-popup-bg);
}
.isard-popup-anchor-left .isard-popup-tip { align-self: center; border-left: none; border-right-color: var(--isard-popup-bg); }
.isard-popup-anchor-right .isard-popup-tip { align-self: center; border-right: none; border-left-color: var(--isard-popup-bg); }
.isard-popup-content {
  position: relative; padding: 12px 14px; border-radius: 4px; background: var(--isard-popup-bg);
  box-shadow: 0 1px 4px rgb(0 0 0 / 0.2); pointer-events: auto; overflow-wrap: break-word;
}
.isard-popup-has-close .isard-popup-content { padding-right: 26px; }
.isard-popup-anchor-top-left .isard-popup-content { border-top-left-radius: 0; }
.isard-popup-anchor-top-right .isard-popup-content { border-top-right-radius: 0; }
.isard-popup-anchor-bottom-left .isard-popup-content { border-bottom-left-radius: 0; }
.isard-popup-anchor-bottom-right .isard-popup-content { border-bottom-right-radius: 0; }
.isard-popup-close {
  position: absolute; top: 0; right: 0; width: 22px; height: 22px; padding: 0; border: 0; border-radius: 0 4px 0 4px;
  background: transparent; color: inherit; font: 18px/22px system-ui, sans-serif; cursor: pointer;
}
.isard-popup-close:hover { background: rgb(0 0 0 / 0.05); }
.isard-popup-close:focus-visible, .isard-attrib-button:focus-visible, .isard-attrib a:focus-visible {
  outline: 2px solid var(--isard-focus); outline-offset: -2px;
}
.isard-attrib {
  display: flex; align-items: center; gap: 4px; padding: 0 5px; border-radius: 4px; background: var(--isard-attrib-bg);
  color: var(--isard-attrib-fg); font: 12px/20px system-ui, sans-serif;
}
.isard-ctrl-bottom-right .isard-ctrl.isard-attrib, .isard-ctrl-bottom-left .isard-ctrl.isard-attrib { margin-bottom: 0; }
.isard-ctrl-bottom-right .isard-ctrl.isard-attrib { margin-right: 0; border-radius: 4px 0 0 0; }
.isard-ctrl-bottom-left .isard-ctrl.isard-attrib { margin-left: 0; border-radius: 0 4px 0 0; }
.isard-attrib a { color: inherit; text-decoration: none; }
.isard-attrib a:hover { text-decoration: underline; }
.isard-attrib-button {
  display: none; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 12px; flex: none;
  background: var(--isard-ctrl-bg); color: var(--isard-ctrl-fg); cursor: pointer;
}
.isard-attrib-button svg { display: block; margin: auto; }
.isard-ctrl.isard-attrib.isard-attrib-compact { min-height: 24px; padding: 2px; border-radius: 12px; margin: 0 10px 10px; }
.isard-attrib-compact .isard-attrib-button { display: block; }
.isard-attrib-compact .isard-attrib-inner { display: none; padding: 0 6px 0 2px; }
.isard-attrib-compact.isard-attrib-open { background: var(--isard-ctrl-bg); }
.isard-attrib-compact.isard-attrib-open .isard-attrib-inner { display: block; }
`;

const styled = new WeakSet<Document>();

/** Adds the overlay styles to a document, once; first in the head so application rules win at equal specificity */
export const injectStyle = (doc: Document): void => {
  if (styled.has(doc)) return;
  styled.add(doc);

  const style = doc.createElement('style');

  style.dataset.isard = '';
  style.textContent = CSS;
  doc.head.prepend(style);
};
