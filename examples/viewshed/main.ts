/**
 * Viewshed: what can be seen from points. From the forecourt of the Sacré-Cœur on the hill of Montmartre, over the
 * IGN's relief (RGE ALTI) and the BD TOPO buildings raised to their height: the scene is drawn from each observer
 * into a depth cube, and each pixel on screen tinted by whether something stands between it and every observer that
 * reaches it. A click on the map adds an observer; a click on its pin opens its settings (one popup open at a time),
 * and the pin drags. On a building, the observer stands on its roof.
 */
import { GeoMap, geoplateformeWfs, type LonLat, Marker, Popup, type ViewshedOptions } from 'isard';
import { element } from '../shared/page.ts';

type Building = { hauteur: number | null };

const map = await GeoMap.create(element('map'), {
  center: [2.3435, 48.8848],
  zoom: 15.6,
  pitch: 55,
  bearing: 160,
  basemap: 'ign-plan-grey-vector',
  terrain: { model: 'ign' },
  projection: 'mercator',
});

const buildings = await geoplateformeWfs<Building>('BDTOPO_V3:batiment', [2.332, 48.877, 2.356, 48.893]);
const roofHeight = (p: Building) => p.hauteur ?? 6;
map.addExtrusions({ data: buildings, height: roofHeight, color: '#e4e1d8' });

// The buildings' outlines and bounding boxes, to find the roof under a pin
type Ring = LonLat[];
const footprints = buildings.features.flatMap((f) => {
  const g = f.geometry;
  let polygons: Ring[][] = [];
  if (g?.type === 'Polygon') polygons = [g.coordinates as Ring[]];
  else if (g?.type === 'MultiPolygon') polygons = g.coordinates as Ring[][];
  return polygons.map((rings) => {
    const outer = rings[0] ?? [];
    const lons = outer.map((c) => c[0]);
    const lats = outer.map((c) => c[1]);
    const box = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
    return { rings, box, height: roofHeight(f.properties) };
  });
});
const inside = ([x, y]: LonLat, ring: Ring) => {
  let within = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i] as LonLat;
    const [xj, yj] = ring[j] as LonLat;
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) within = !within;
  }
  return within;
};
/** Metres of the roof under a point, 0 off the buildings */
const roofAt = (at: LonLat) => {
  const [x, y] = at;
  const under = footprints.find(
    ({ rings, box }) =>
      x >= (box[0] ?? 0) &&
      y >= (box[1] ?? 0) &&
      x <= (box[2] ?? 0) &&
      y <= (box[3] ?? 0) &&
      inside(at, rings[0] ?? []) &&
      !rings.slice(1).some((hole) => inside(at, hole)),
  );
  return under?.height ?? 0;
};

/** An observer's settings, as its popup's inputs give them */
interface Observer {
  pin: Marker;
  popup: Popup;
  height: number;
  range: number;
  oneWay: boolean;
  direction: number;
  fov: number;
}
const observers: Observer[] = [];
let named = 0;

// The edges of the cones on the ground, from each observer to its range
const edges = map.addLines({ data: [], color: '#1c7ed6', width: 2.5 });
const along = ([lon, lat]: LonLat, heading: number, metres: number): LonLat => {
  const b = (heading * Math.PI) / 180;
  const k = metres / 6378137;
  return [
    lon + ((k * Math.sin(b)) / Math.cos((lat * Math.PI) / 180)) * (180 / Math.PI),
    lat + k * Math.cos(b) * (180 / Math.PI),
  ];
};

const show = () => {
  map.setViewshed(
    observers.map(
      (o): ViewshedOptions => ({
        position: o.pin.getLonLat(),
        height: roofAt(o.pin.getLonLat()) + o.height,
        range: o.range,
        ...(o.oneWay ? { direction: o.direction, fov: o.fov } : {}),
      }),
    ),
  );
  edges.setData(
    observers
      .filter((o) => o.oneWay)
      .map((o) => {
        const at = o.pin.getLonLat();
        const [a, b] = [-1, 1].map((side) => along(at, o.direction + (side * o.fov) / 2, o.range));
        return {
          type: 'Feature' as const,
          properties: {},
          geometry: { type: 'LineString' as const, coordinates: [a ?? at, at, b ?? at] },
        };
      }),
  );
  element('count').textContent = `${observers.length} observer${observers.length > 1 ? 's' : ''}`;
  element<HTMLButtonElement>('clear').disabled = observers.length === 0;
};

const add = (lonLat: LonLat, settings: Partial<Observer> = {}) => {
  const form = element<HTMLTemplateElement>('observer').content.firstElementChild?.cloneNode(true) as HTMLElement;
  const popup = new Popup({ offset: [0, -34], maxWidth: '220px', closeOnClick: false }).setDOMContent(form);
  const pin = new Marker({ draggable: true }).setLonLat(lonLat).setPopup(popup).addTo(map);
  const observer: Observer = { pin, popup, height: 2, range: 900, oneWay: true, direction: 170, fov: 60, ...settings };
  observers.push(observer);
  named += 1;
  const part = <T extends HTMLElement>(name: string) => form.querySelector(`.${name}`) as T;
  part('name').textContent = `Observer ${named}`;
  const sliders = { height: 'm', direction: '°', fov: '°', range: 'm' } as const;
  for (const [name, unit] of Object.entries(sliders) as [keyof typeof sliders, string][]) {
    const input = part<HTMLInputElement>(name);
    const value = part(`${name}-value`);
    input.value = String(observer[name]);
    const read = () => {
      observer[name] = Number(input.value);
      value.textContent = `${input.value}${unit === 'm' ? ' m' : unit}`;
    };
    read();
    input.addEventListener('input', () => {
      read();
      show();
    });
  }
  const oneWay = part<HTMLInputElement>('one-way');
  oneWay.checked = observer.oneWay;
  oneWay.addEventListener('input', () => {
    observer.oneWay = oneWay.checked;
    show();
  });
  part('remove').addEventListener('click', () => {
    pin.remove();
    observers.splice(observers.indexOf(observer), 1);
    show();
  });
  // One popup open at a time
  popup.on('open', () => {
    for (const other of observers) if (other !== observer) other.popup.remove();
  });
  // On a roof or on the ground, as the pin moves
  const stand = () => {
    const roof = roofAt(pin.getLonLat());
    part('where').textContent = roof > 0 ? ` · on a roof, ${Math.round(roof)} m` : '';
  };
  stand();
  // The view follows the pin as it is dragged
  pin.on('drag', () => {
    stand();
    show();
  });
  pin.togglePopup();
  show();
};

map.on('click', ({ lonLat }) => {
  if (lonLat) add(lonLat);
});
element('clear').addEventListener('click', () => {
  for (const o of observers.splice(0)) o.pin.remove();
  show();
});
add([2.34305, 48.88615]);
