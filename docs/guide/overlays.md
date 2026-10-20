# Markers, popups, controls

HTML elements placed over the map, moved after every frame (they follow the relief), hidden behind the camera and past
the globe's horizon. The wheel zooms the map over them.

## Markers

```ts
import { Marker, Popup } from 'isard';

const marker = new Marker({ draggable: true, anchor: 'bottom' })
  .setLonLat([2.4246, 48.8457])
  .setPopup(new Popup().setHTML('<b>Here</b>'))
  .addTo(map);
marker.on('drag', ({ lonLat }) => console.log(lonLat));
```

Without `element`, a round pin anchored at its tip. `anchor` (`'center'`, `'top'`, `'bottom-left'`…) and `offset`
place the element; a dragged marker stays on the relief.

`height` (or `setHeight(height, base)`) raises a marker `height` metres above the ground, with a stem down to `base`
metres, what it stands on (a roof, a mesh: `map.surfaceAt`; the ground by default); its popup is raised with it.

## Popups

```ts
new Popup({ closeButton: true, closeOnClick: true, maxWidth: '280px', anchor: 'bottom', offset: [0, -10] })
  .setLonLat(lonLat)
  .setHTML('<p>…</p>') // or setText, setDOMContent
  .addTo(map);
```

## Panels

An element of the page standing in 3D at a position, `width` metres wide: a video, a web page, a sign, as the CSS3D
objects of three.js. It faces `heading` (degrees clockwise from north), leans back by `pitch`, its bottom edge
`height` metres above the ground:

```ts
import { Panel } from 'isard';

const video = document.createElement('video');
video.src = 'camera.webm';
new Panel({ element: video, width: 40, heading: 180, height: 4 }).setLonLat([2.3431, 48.8861]).addTo(map);
```

It is placed after each frame by a CSS 3D transform from the camera: the element is not drawn by WebGL, so the scene
does not hide it; it is hidden when its position is behind the camera, and seen mirrored from behind. Its CSS size
is read once it is on the page (`resize()` reads it again). See [3D panels](../examples/panels).

## Controls

```ts
import { NavigationControl } from 'isard';

map.addControl(new NavigationControl({ visualizePitch: true }), 'bottom-right');
```

A control is any object with `onAdd(map)` returning an element and `onRemove(map)`. Controls stack from their corner
inward; the attribution is added by the map, with the credits of the basemap, hillshade and relief in use
(`attributionControl: false` to remove it, or `{ compact, customAttribution }`).

```ts
class TiltButton {
  onAdd(map) {
    const box = document.createElement('div');
    box.className = 'isard-ctrl-group';
    const button = box.appendChild(document.createElement('button'));
    button.textContent = '45°';
    button.onclick = () => map.easeTo({ pitch: map.getCamera().pitch ? 0 : 45 });
    return box;
  }
  onRemove() {}
}
```

## Styling

Classes are prefixed `isard-` (`.isard-ctrl-group`, `.isard-marker`, `.isard-popup`…). Colors are custom properties
with defaults on the root, restyled on any element around the map: `--isard-ctrl-bg`, `--isard-ctrl-fg`,
`--isard-ctrl-active`, `--isard-popup-bg`, `--isard-marker-color`…
