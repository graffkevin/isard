/**
 * Street-level panoramas: the oriented images of a vehicle's cameras around each of its positions (Stereopolis, the
 * iTowns immersive examples), projected onto the scene around the panorama shown. Their positions and attitudes come
 * as GeoJSON points (Lambert-93 by default, altitudes NGF-IGN69 raised by the RAF20 geoid onto the IGN relief), the
 * calibration of the cameras as JSON (rotation and position in the panorama frame, intrinsics, MicMac's radial
 * distortion), the images from a URL template with `{cameraId}` and `{panoId}`.
 *
 * ```ts
 * const panoramas = await map.addPanoramas({ url, orientationsUrl, calibrationUrl });
 * await panoramas.show(panoramas.nearest([2.3345, 48.8505]).index);
 * ```
 */
import { getCrs } from './crs.ts';
import { withHeaders } from './requests.ts';
import type { FirstPersonView, LonLat } from './types.ts';

export interface PanoramaOptions {
  /** URL of an image: `{cameraId}` and `{panoId}` replaced by a camera's id and a panorama's */
  url: string;
  /** GeoJSON points: each panorama's position, and its `id`, `heading`, `pitch`, `roll` (degrees) in properties */
  orientationsUrl: string;
  /** JSON array of the cameras: `id`, `rotation` (row-major), `position`, `projection`, `size`, `distortion` */
  calibrationUrl: string;
  /** CRS of the positions (default the GeoJSON's `crs` member, else `EPSG:2154`) */
  crs?: string;
  /** Altitudes above the geoid (NGF-IGN69), raised by the RAF20 geoid onto the IGN relief (default true in
   * Lambert-93) */
  geoid?: boolean;
  /** Radius of the sphere around the panorama its images are a backdrop on, metres (default 1000) */
  backgroundDistance?: number;
  /** 0-1 (default 1) */
  opacity?: number;
}

/** A panorama: where it is and which way the vehicle faced */
export interface Panorama {
  index: number;
  id: number | string;
  lonLat: LonLat;
  /** Metres above the ellipsoid */
  height: number;
  /** Degrees clockwise from north */
  heading: number;
}

/** What the map gives the panoramas */
export interface PanoramaHost {
  setPanorama(
    lon: number,
    lat: number,
    height: number,
    rotation: Float64Array,
    cameras: Float64Array,
    radius: number,
    opacity: number,
  ): void;
  setPanoramaImage(index: number, image: ImageBitmap): void;
  clearPanorama(): void;
  geoidHeight(lon: number, lat: number): number;
  /** The street-level eye, and setting it (undefined: back to the camera turning around a point) */
  firstPerson(): Readonly<Required<FirstPersonView>> | undefined;
  setFirstPerson(view: FirstPersonView | undefined): void;
  elevationAt(lonLat: LonLat): number | undefined;
  /** Follows the clicks on the map (with the ground clicked) and the keys typed on it; returns what stops */
  listen(click: (lonLat: LonLat | undefined) => void, key: (e: KeyboardEvent) => void): () => void;
}

interface Calibration {
  id: number | string;
  rotation: number[];
  position: number[];
  projection: number[];
  size: number[];
  distortion?: { pps: number[]; poly357: number[]; limit?: number };
}

interface Attitude {
  heading: number;
  pitch: number;
  roll: number;
}

const RAD = Math.PI / 180;

/** 3 × 3 product, row-major */
function times(a: number[], b: number[]): number[] {
  const out: number[] = [];
  for (let r = 0; r < 3; r++) {
    for (let c = 0; c < 3; c++) {
      let sum = 0;
      for (let k = 0; k < 3; k++) sum += (a[r * 3 + k] ?? 0) * (b[k * 3 + c] ?? 0);
      out.push(sum);
    }
  }
  return out;
}

const rotX = (t: number) => [1, 0, 0, 0, Math.cos(t), -Math.sin(t), 0, Math.sin(t), Math.cos(t)];
const rotY = (t: number) => [Math.cos(t), 0, Math.sin(t), 0, 1, 0, -Math.sin(t), 0, Math.cos(t)];
const rotZ = (t: number) => [Math.cos(t), -Math.sin(t), 0, Math.sin(t), Math.cos(t), 0, 0, 0, 1];

/**
 * The panorama frame to east, north, up (row-major): the vehicle's attitude in the CRS's grid (iTowns'
 * `quaternionFromRollPitchHeading`: Ry(-roll) · Rx(-pitch) · Rz(-heading)), turned by the meridian convergence of the
 * projection (Lambert-93: sin(46.5°) · (3° - longitude))
 */
export function panoramaRotation(attitude: Attitude, lon: number, lambert93: boolean): number[] {
  const vehicle = times(times(rotY(-attitude.roll * RAD), rotX(-attitude.pitch * RAD)), rotZ(-attitude.heading * RAD));
  const convergence = lambert93 ? Math.sin(46.5 * RAD) * (3 - lon) * RAD : 0;
  return times(rotZ(convergence), vehicle);
}

/** The 25 numbers of a camera for `setPanorama` */
function cameraNumbers(c: Calibration): number[] {
  const p = c.projection;
  const d = c.distortion;
  return [
    ...c.rotation.slice(0, 9),
    ...c.position.slice(0, 3),
    p[0] ?? 1,
    p[4] ?? 1,
    p[2] ?? 0,
    p[5] ?? 0,
    p[1] ?? 0,
    c.size[0] ?? 1,
    c.size[1] ?? 1,
    d?.pps[0] ?? 0,
    d?.pps[1] ?? 0,
    d?.poly357[0] ?? 0,
    d?.poly357[1] ?? 0,
    d?.poly357[2] ?? 0,
    d?.limit ?? 0,
  ];
}

/** The panoramas of a set, one shown at a time */
export class Panoramas {
  /** The panorama shown, if any */
  current: Panorama | undefined;
  private shown = 0;

  /** @internal */
  constructor(
    private readonly host: PanoramaHost,
    private readonly options: Required<Pick<PanoramaOptions, 'url' | 'backgroundDistance' | 'opacity'>>,
    readonly list: readonly Panorama[],
    private readonly attitudes: readonly Attitude[],
    private readonly cameras: readonly Calibration[],
    private readonly lambert93: boolean,
  ) {}

  /** The panorama nearest to a position */
  nearest(lonLat: LonLat): Panorama | undefined {
    const cos = Math.cos(lonLat[1] * RAD);
    let best: Panorama | undefined;
    let distance = Number.POSITIVE_INFINITY;
    for (const p of this.list) {
      const d = ((p.lonLat[0] - lonLat[0]) * cos) ** 2 + (p.lonLat[1] - lonLat[1]) ** 2;
      if (d < distance) [best, distance] = [p, d];
    }
    return best;
  }

  /** Shows a panorama (its index in `list`): projected as soon as its first image arrives, the others after */
  async show(index: number): Promise<void> {
    const pano = this.list[index];
    const attitude = this.attitudes[index];
    if (!pano || !attitude) return;
    const shown = ++this.shown;
    this.current = pano;
    const rotation = panoramaRotation(attitude, pano.lonLat[0], this.lambert93);
    const { url, backgroundDistance, opacity } = this.options;
    this.host.setPanorama(
      pano.lonLat[0],
      pano.lonLat[1],
      pano.height,
      Float64Array.from(rotation),
      Float64Array.from(this.cameras.flatMap(cameraNumbers)),
      backgroundDistance,
      opacity,
    );
    await Promise.all(
      this.cameras.map(async (camera, k) => {
        const src = url.replaceAll('{cameraId}', String(camera.id)).replaceAll('{panoId}', String(pano.id));
        const response = await fetch(src, withHeaders(src));
        if (!response.ok) throw new Error(`isard: panorama image ${src}: ${response.status}`);
        const image = await createImageBitmap(await response.blob(), { premultiplyAlpha: 'none' });
        // Another panorama shown since: this image is not its
        if (shown === this.shown) this.host.setPanoramaImage(k, image);
        image.close();
      }),
    );
  }

  /** Hides the panorama shown */
  hide(): void {
    this.shown++;
    this.current = undefined;
    this.host.clearPanorama();
  }

  /** Stops the walk (`enter`) */
  private stopWalking: (() => void) | undefined;

  /**
   * Street-level walk from panorama to panorama (the iTowns immersive view): the eye on a panorama, the drag
   * turning it; a click goes to the panorama nearest to the clicked point, Z and S (or Page Up and Page Down) to the
   * next and the previous ones. Starts on the panorama `index`, else the one nearest to `near`
   */
  async enter(start: { index?: number; near?: LonLat } = {}): Promise<void> {
    const first = start.index ?? (start.near && this.nearest(start.near)?.index) ?? 0;
    this.stopWalking?.();
    this.stopWalking = this.host.listen(
      (lonLat) => {
        const target = lonLat && this.nearest(lonLat);
        if (target && target !== this.current) void this.go(target.index);
      },
      (e) => {
        const step = { z: 1, Z: 1, PageUp: 1, s: -1, S: -1, PageDown: -1 }[e.key];
        if (!step || !this.current) return;
        e.preventDefault();
        void this.go((this.current.index + step + this.list.length) % this.list.length);
      },
    );
    await this.go(first, this.list[first]?.heading);
  }

  /** Goes to a panorama during the walk, the eye looking the same way (or `bearing`) */
  async go(index: number, bearing?: number): Promise<void> {
    const pano = this.list[index];
    if (!pano) return;
    const eye = this.host.firstPerson();
    // The eye at the panorama's cameras, above the ground there
    const ground = this.host.elevationAt(pano.lonLat);
    this.host.setFirstPerson({
      lonLat: pano.lonLat,
      height: ground === undefined ? 2.5 : Math.max(0.5, pano.height - ground),
      bearing: bearing ?? eye?.bearing ?? pano.heading,
      ...(eye ? { pitch: eye.pitch, fov: eye.fov } : {}),
    });
    await this.show(index);
  }

  /** Ends the walk: the panorama hidden, the camera turning around a point again, above where the eye stood */
  exit(): void {
    this.stopWalking?.();
    this.stopWalking = undefined;
    this.hide();
    this.host.setFirstPerson(undefined);
  }
}

/** Loads a set of panoramas: their positions and attitudes, the calibration of their cameras */
export async function loadPanoramas(host: PanoramaHost, options: PanoramaOptions): Promise<Panoramas> {
  const json = async <T>(src: string) => {
    const response = await fetch(src, withHeaders(src));
    if (!response.ok) throw new Error(`isard: ${src}: ${response.status}`);
    return (await response.json()) as T;
  };
  const [orientations, cameras] = await Promise.all([
    json<{
      crs?: { properties?: { name?: string } };
      features: { geometry: { coordinates: number[] }; properties: Record<string, unknown> }[];
    }>(options.orientationsUrl),
    json<Calibration[]>(options.calibrationUrl),
  ]);
  const crsName = options.crs ?? orientations.crs?.properties?.name ?? 'EPSG:2154';
  const lambert93 = /2154/.test(crsName);
  const crs = getCrs(crsName);
  const geoid = options.geoid ?? lambert93;
  const list: Panorama[] = [];
  const attitudes: Attitude[] = [];
  for (const feature of orientations.features) {
    const [x = 0, y = 0, z = 0] = feature.geometry.coordinates;
    const [lon = 0, lat = 0] = crs.inverse([x, y]);
    const p = feature.properties;
    const number = (key: string) => (typeof p[key] === 'number' ? (p[key] as number) : 0);
    attitudes.push({ heading: number('heading'), pitch: number('pitch'), roll: number('roll') });
    list.push({
      index: list.length,
      id: (p.id as number | string | undefined) ?? list.length,
      lonLat: [lon, lat],
      height: z + (geoid ? host.geoidHeight(lon, lat) : 0),
      heading: number('heading'),
    });
  }
  return new Panoramas(
    host,
    {
      url: options.url,
      backgroundDistance: options.backgroundDistance ?? 1000,
      opacity: options.opacity ?? 1,
    },
    list,
    attitudes,
    cameras,
    lambert93,
  );
}
