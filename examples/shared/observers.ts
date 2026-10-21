/**
 * The observers of the viewshed examples: a pin each, its settings in its popup (one popup open at a time), the
 * whole given to `map.setViewshed` at each change. The page has a `#count`, a `#clear` button and
 * an `#opacity` slider; a click on the map adds an observer, a pin drags, its popup removes it. On surfaces, an observer
 * clicked or dragged onto a roof or a mesh stands on it.
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
  /** On surfaces: the metres above the relief of what it stands on */
  base: number;
}

type Slider = [keyof Omit<ObserverSettings, 'oneWay'>, string, number, number, number, string];

/** The height first, the cone after the one-way box */
const SLIDERS: [Slider, ...Slider[]] = [
  ['height', 'Height above it', 0, 60, 0.1, ' m'],
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

/**
 * Observers on `map`. `standOn` gives the metres of what stands under a point (a roof), with its name for the popup;
 * or with `surfaces` the observers stand on whatever is drawn under the click or the drag (`map.pickSurface`).
 * `defaults`: the settings of a new observer. Returns the function adding one, its popup open
 */
export const observers = (
  map: GeoMap,
  options: {
    standOn?: (at: LonLat) => { metres: number; what: string } | undefined;
    surfaces?: boolean;
    defaults?: Partial<ObserverSettings>;
  } = {},
): ((at: LonLat, settings?: Partial<ObserverSettings>, standing?: number) => void) => {
  const all: Observer[] = [];
  let named = 0;
  const opacity = element('opacity', HTMLInputElement);
  /** What an observer stands on, if anything above the relief */
  const under = (o: Observer) => {
    if (!options.surfaces) return options.standOn?.(o.pin.getLonLat());

    return o.base > 0.5 ? { metres: o.base, what: 'a surface' } : undefined;
  };
  const base = (o: Observer) => under(o)?.metres ?? 0;

  const show = () => {
    map.setViewshed(
      all.map((o): ViewshedOptions => {
        // The pin raised to the eye, on what it stands on
        const height = base(o) + o.height;

        o.pin.setHeight(height, base(o));

        return {
          position: o.pin.getLonLat(),
          height,
          range: o.range,
          ...(o.oneWay ? { direction: o.direction, tilt: o.tilt, fov: o.fov, verticalFov: o.verticalFov } : {}),
          opacity: Number(opacity.value) / 100,
        };
      }),
    );
    element('opacity-value').textContent = `${opacity.value} %`;
    element('count').textContent = `${all.length} observer${all.length > 1 ? 's' : ''}`;
    element('clear', HTMLButtonElement).disabled = all.length === 0;
  };

  const add = (at: LonLat, settings: Partial<ObserverSettings> = {}, standing = 0) => {
    const form = document.createElement('div');

    form.className = 'observer';

    const popup = new Popup({ offset: [0, -34], maxWidth: '220px', closeOnClick: false }).setDOMContent(form);
    const pin = new Marker({ draggable: true }).setLonLat(at).setPopup(popup).addTo(map);
    const observer: Observer = {
      pin,
      popup,
      base: standing,
      height: 1,
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

    const slider = ([key, label, min, max, step, unit]: Slider) => {
      const input = Object.assign(document.createElement('input'), { type: 'range', min, max, step });

      input.value = String(observer[key]);

      const value = document.createElement('span');
      const read = () => {
        observer[key] = Number(input.value);
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

    form.append(slider(height), line(oneWayLabel), ...cone.map(slider), line(remove));

    // One popup open at a time
    popup.on('open', () => {
      all.forEach((other) => {
        if (other !== observer) other.popup.remove();
      });
    });

    // What it stands on, as the pin moves
    const stand = () => {
      const on = under(observer);

      where.textContent = on ? ` · on ${on.what}, ${Math.round(on.metres)} m` : '';
    };

    stand();

    // The pin follows the pointer at once; what it stands on and the viewshed follow a few times a second, and at
    // the release (each costs a frame of the scene)
    let pending = 0;
    const follow = () => {
      pending = 0;
      if (options.surfaces) observer.base = map.surfaceAt(pin.getLonLat()) ?? 0;
      stand();
      show();
    };

    pin.on('drag', () => {
      pending ||= window.setTimeout(follow, 200);
    });
    pin.on('dragend', () => {
      window.clearTimeout(pending);
      follow();
    });
    pin.togglePopup();
    show();
  };

  map.on('click', ({ point, lonLat }) => {
    const hit = options.surfaces ? map.pickSurface(point) : undefined;

    if (hit) add(hit.lonLat, {}, hit.height);
    else if (lonLat) add(lonLat);
  });
  opacity.addEventListener('input', show);
  element('clear').addEventListener('click', () => {
    all.splice(0).forEach((o) => {
      o.pin.remove();
    });
    show();
  });

  return add;
};
