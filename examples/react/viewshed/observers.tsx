/**
 * The observers of the viewshed examples in React: a state of observers, each a pin with its settings in its popup
 * (one popup open at a time) and its `<Viewshed>`; the panel's count, "Remove all" button and opacity slider. A click
 * on the map adds an observer, a pin drags, its popup removes it. On surfaces, an observer clicked or dragged onto a
 * roof or a mesh stands on it.
 */
import type { LonLat } from 'isard';
import { Marker, Popup, useMap, useMapEvent, Viewshed } from 'isard/react';
import { useRef, useState } from 'react';

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
  id: number;
  lonLat: LonLat;
  /** On surfaces: the metres above the relief of what it stands on */
  base: number;
}

type Slider = [keyof Omit<ObserverSettings, 'oneWay'>, string, number, number, number, string];

const SLIDERS: Slider[] = [
  ['height', 'Height above it', 0, 60, 0.1, ' m'],
  ['direction', 'Direction', 0, 359, 1, '°'],
  ['tilt', 'Tilt', -60, 30, 1, '°'],
  ['fov', 'Field of view', 10, 170, 5, '°'],
  ['verticalFov', 'Vertical field of view', 10, 170, 5, '°'],
  ['range', 'Range', 200, 2000, 50, ' m'],
];

/**
 * The observers' state, the first one at `start` with its popup open. `defaults`: the settings of a new observer
 */
export const useObservers = (start: LonLat, defaults: Partial<ObserverSettings> = {}) => {
  const create = (id: number, lonLat: LonLat, base = 0): Observer => ({
    id,
    lonLat,
    base,
    height: 1,
    range: 900,
    oneWay: true,
    direction: 170,
    tilt: 0,
    fov: 60,
    verticalFov: 120,
    ...defaults,
  });
  const [list, setList] = useState(() => [create(1, start)]);
  const [open, setOpen] = useState<number | undefined>(1);
  const [opacity, setOpacity] = useState(70);
  // The observers are named in order, the removed ones' names not given again
  const named = useRef(1);

  return {
    list,
    open,
    setOpen,
    opacity,
    setOpacity,
    add: (lonLat: LonLat, base = 0) => {
      named.current += 1;

      const observer = create(named.current, lonLat, base);

      setList((all) => [...all, observer]);
      setOpen(observer.id);
    },
    update: (id: number, change: Partial<Observer>) =>
      setList((all) => all.map((o) => (o.id === id ? { ...o, ...change } : o))),
    remove: (id: number) => setList((all) => all.filter((o) => o.id !== id)),
    clear: () => setList([]),
  };
};

export type Observers = ReturnType<typeof useObservers>;

interface PinsProps {
  observers: Observers;
  /** The metres of what stands under a point (a roof), with its name for the popup */
  standOn?: (at: LonLat) => { metres: number; what: string } | undefined;
  /** The observers stand on whatever is drawn under the click or the drag (`map.pickSurface`) */
  surfaces?: boolean;
}

/** What an observer stands on, if anything above the relief */
const under = (o: Observer, { standOn, surfaces }: Omit<PinsProps, 'observers'>) => {
  if (!surfaces) return standOn?.(o.lonLat);

  return o.base > 0.5 ? { metres: o.base, what: 'a surface' } : undefined;
};

/** An observer's pin, its popup and its viewshed */
const Pin = ({ observer: o, observers, ...stand }: PinsProps & { observer: Observer }) => {
  const map = useMap();
  const { update, remove, open, setOpen, opacity } = observers;
  const on = under(o, stand);
  const base = on?.metres ?? 0;
  // The pin raised to the eye, on what it stands on
  const height = base + o.height;
  // The pin follows the pointer at once; what it stands on and the viewshed follow a few times a second, and at the
  // release (each costs a frame of the scene)
  const pending = useRef(0);
  const dragged = useRef(o.lonLat);
  const follow = () => {
    pending.current = 0;

    const lonLat = dragged.current;

    update(o.id, stand.surfaces ? { lonLat, base: map?.surfaceAt(lonLat) ?? 0 } : { lonLat });
  };
  const [first, ...cone] = SLIDERS;
  const slider = ([key, label, min, max, step, unit]: Slider) => (
    <p key={key}>
      <label>
        {label} <span>{`${o[key]}${unit}`}</span>
        <input
          type="range"
          min={min}
          max={max}
          step={step}
          value={o[key]}
          onChange={(e) => update(o.id, { [key]: Number(e.target.value) })}
        />
      </label>
    </p>
  );

  return (
    <>
      <Marker
        lonLat={o.lonLat}
        height={height}
        base={base}
        draggable
        onDrag={(lonLat) => {
          dragged.current = lonLat;
          pending.current ||= window.setTimeout(follow, 200);
        }}
        onDragEnd={(lonLat) => {
          window.clearTimeout(pending.current);
          dragged.current = lonLat;
          follow();
        }}
      >
        {/* One popup open at a time: the one the state names */}
        <Popup
          lonLat={o.lonLat}
          offset={[0, -34]}
          maxWidth="220px"
          closeOnClick={false}
          open={open === o.id}
          onOpen={() => setOpen(o.id)}
          onClose={() => setOpen((current) => (current === o.id ? undefined : current))}
        >
          <div className="observer">
            <p>
              <strong>Observer {o.id}</strong>
              <span>{on ? ` · on ${on.what}, ${Math.round(on.metres)} m` : ''}</span>
            </p>
            {first && slider(first)}
            <p>
              <label>
                <input
                  type="checkbox"
                  checked={o.oneWay}
                  onChange={(e) => update(o.id, { oneWay: e.target.checked })}
                />{' '}
                One way, as a camera
              </label>
            </p>
            {cone.map(slider)}
            <p>
              <button type="button" onClick={() => remove(o.id)}>
                Remove
              </button>
            </p>
          </div>
        </Popup>
      </Marker>
      <Viewshed
        position={o.lonLat}
        height={height}
        range={o.range}
        {...(o.oneWay ? { direction: o.direction, tilt: o.tilt, fov: o.fov, verticalFov: o.verticalFov } : {})}
        opacity={opacity / 100}
      />
    </>
  );
};

/** The observers on the map around; a click on it adds one (on the surface clicked with `surfaces`) */
export const ObserverPins = ({ observers, standOn, surfaces }: PinsProps) => {
  const map = useMap();

  useMapEvent('click', ({ point, lonLat }) => {
    const hit = surfaces ? map?.pickSurface(point) : undefined;

    if (hit) observers.add(hit.lonLat, hit.height);
    else if (lonLat) observers.add(lonLat);
  });

  const stand = { ...(standOn ? { standOn } : {}), ...(surfaces ? { surfaces } : {}) };

  return observers.list.map((o) => <Pin key={o.id} observer={o} observers={observers} {...stand} />);
};

/** The panel of a viewshed example */
export const ObserversPanel = ({ title, observers }: { title: string; observers: Observers }) => {
  const { list, clear, opacity, setOpacity } = observers;

  return (
    <div className="panel">
      <h1>{title}</h1>
      <p>Seen in green, hidden in red. Click the map to add an observer, a pin to set its view; drag it to move it.</p>
      <p>
        <span>
          {list.length} observer{list.length > 1 ? 's' : ''}
        </span>{' '}
        <button type="button" disabled={list.length === 0} onClick={clear}>
          Remove all
        </button>
      </p>
      <p>
        <label>
          Opacity <span>{opacity} %</span>
          <input
            type="range"
            min={10}
            max={100}
            step={5}
            value={opacity}
            onChange={(e) => setOpacity(Number(e.target.value))}
          />
        </label>
      </p>
    </div>
  );
};
