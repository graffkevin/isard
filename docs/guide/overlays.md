# Markers, popups, controls

HTML elements placed over the map, moved after every frame (they follow the relief), hidden behind the camera and past
the globe's horizon. The wheel zooms the map over them.

## Markers

```ts
import { Marker, Popup } from '@ign-junn/isard';

const marker = new Marker({ draggable: true, anchor: 'bottom' })
  .setLonLat([2.4246, 48.8457])
  .setPopup(new Popup().setHTML('<b>IGN</b>'))
  .addTo(map);
marker.on('drag', ({ lonLat }) => console.log(lonLat));
```

Without `element`, a round pin anchored at its tip. `anchor` (`'center'`, `'top'`, `'bottom-left'`…) and `offset`
place the element; a dragged marker stays on the relief.

## Popups

```ts
new Popup({ closeButton: true, closeOnClick: true, maxWidth: '280px', anchor: 'bottom', offset: [0, -10] })
  .setLonLat(lonLat)
  .setHTML('<p>…</p>') // or setText, setDOMContent
  .addTo(map);
```

## Controls

```ts
import { NavigationControl } from '@ign-junn/isard';

map.addControl(new NavigationControl({ visualizePitch: true }), 'bottom-right');
```

A control is any object with `onAdd(map)` returning an element and `onRemove(map)`. Controls stack from their corner
inward; the attribution is added by the map (`attributionControl: false` to remove it, or `{ compact,
customAttribution }`).

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
