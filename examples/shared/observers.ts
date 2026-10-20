/**
 * The observers of the viewshed examples: a pin each, its settings in its popup (one popup open at a time), the
 * edges of its cone on the ground, the whole given to `map.setViewshed` at each change. The page has a `#count` and
 * a `#clear` button; a click on the map adds an observer, a pin drags, its popup removes it.
 */
import { type GeoMap, type LonLat, Marker, Popup, type ViewshedOptions } from 'isard';
import { element } from './page.ts';

/** What an observer's popup sets */
export interface ObserverSettings {
  /** Metres above what it stands on */
  height: number;
  range: number;
  oneWay: boolean;
  direction: number;
  tilt: number;
  fov: number;
  verticalFov: number;
}

interface Observer extends ObserverSettings {
  pin: Marker;
  popup: Popup;
}

const SLIDERS: [keyof ObserverSettings, string, number, number, number, string][] = [
  ['height', 'Height above it', 1, 60, 1, ' m'],
  ['direction', 'Direction', 0, 359, 1, '°'],
  ['tilt', 'Tilt', -60, 30, 1, '°'],
  ['fov', 'Field of view', 10, 170, 5, '°'],
  ['verticalFov', 'Vertical field of view', 10, 170, 5, '°'],
  ['range', 'Range', 200, 2000, 50, ' m'],
];

const line = (...children: (Node | string)[]) => {
  const p = document.createElement('p');
  p.append(...children);
  return p;
};

/** The edge of a cone: from `at`, `metres` along `heading` (degrees clockwise from north) */
const along = ([lon, lat]: LonLat, heading: number, metres: number): LonLat => {
  const b = (heading * Math.PI) / 180;
  const k = metres / 6378137;
  return [
    lon + ((k * Math.sin(b)) / Math.cos((lat * Math.PI) / 180)) * (180 / Math.PI),
    lat + k * Math.cos(b) * (180 / Math.PI),
  ];
};

/**
 * Observers on `map`. `standOn` gives the metres of what stands under a point (a roof), with its name for the popup;
 * `defaults` the settings of a new observer. Returns the function adding one, its popup open
 */
export function observers(
  map: GeoMap,
  options: {
    standOn?: (at: LonLat) => { metres: number; what: string } | undefined;
    defaults?: Partial<ObserverSettings>;
  } = {},
): (at: LonLat, settings?: Partial<ObserverSettings>) => void {
  const all: Observer[] = [];
  let named = 0;
  const edges = map.addLines({ data: [], color: '#1c7ed6', width: 2.5 });
  const base = (o: Observer) => options.standOn?.(o.pin.getLonLat())?.metres ?? 0;

  const show = () => {
    map.setViewshed(
      all.map((o): ViewshedOptions => {
        // The pin raised to the eye, on what it stands on
        const height = base(o) + o.height;
        o.pin.setHeight(height);
        return {
          position: o.pin.getLonLat(),
          height,
          range: o.range,
          ...(o.oneWay ? { direction: o.direction, tilt: o.tilt, fov: o.fov, verticalFov: o.verticalFov } : {}),
        };
      }),
    );
    edges.setData(
      all
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
    element('count').textContent = `${all.length} observer${all.length > 1 ? 's' : ''}`;
    element<HTMLButtonElement>('clear').disabled = all.length === 0;
  };

  const add = (at: LonLat, settings: Partial<ObserverSettings> = {}) => {
    const form = document.createElement('div');
    form.className = 'observer';
    const popup = new Popup({ offset: [0, -34], maxWidth: '220px', closeOnClick: false }).setDOMContent(form);
    const pin = new Marker({ draggable: true }).setLonLat(at).setPopup(popup).addTo(map);
    const observer: Observer = {
      pin,
      popup,
      height: 2,
      range: 900,
      oneWay: true,
      direction: 170,
      tilt: 0,
      fov: 60,
      verticalFov: 120,
      ...options.defaults,
      ...settings,
    };
    all.push(observer);
    named += 1;

    const name = document.createElement('strong');
    name.textContent = `Observer ${named}`;
    const where = document.createElement('span');
    form.append(line(name, where));
    const slider = ([key, label, min, max, step, unit]: (typeof SLIDERS)[number]) => {
      const input = Object.assign(document.createElement('input'), { type: 'range', min, max, step });
      input.value = String(observer[key]);
      const value = document.createElement('span');
      const read = () => {
        (observer[key] as number) = Number(input.value);
        value.textContent = `${input.value}${unit}`;
      };
      read();
      input.addEventListener('input', () => {
        read();
        show();
      });
      const labelled = document.createElement('label');
      labelled.append(label, ' ', value, input);
      return line(labelled);
    };
    const oneWay = Object.assign(document.createElement('input'), { type: 'checkbox', checked: observer.oneWay });
    oneWay.addEventListener('input', () => {
      observer.oneWay = oneWay.checked;
      show();
    });
    const oneWayLabel = document.createElement('label');
    oneWayLabel.append(oneWay, ' One way, as a camera');
    const remove = Object.assign(document.createElement('button'), { type: 'button', textContent: 'Remove' });
    remove.addEventListener('click', () => {
      pin.remove();
      all.splice(all.indexOf(observer), 1);
      show();
    });
    const [height, ...cone] = SLIDERS;
    form.append(slider(height as (typeof SLIDERS)[number]), line(oneWayLabel), ...cone.map(slider), line(remove));

    // One popup open at a time
    popup.on('open', () => {
      for (const other of all) if (other !== observer) other.popup.remove();
    });
    // What it stands on, as the pin moves
    const stand = () => {
      const under = options.standOn?.(pin.getLonLat());
      where.textContent = under ? ` · on ${under.what}, ${Math.round(under.metres)} m` : '';
    };
    stand();
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
    for (const o of all.splice(0)) o.pin.remove();
    show();
  });
  return add;
}
