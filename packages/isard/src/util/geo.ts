/**
 * Web Mercator in [0, 1] across the world (y towards the south), for the page and the workers alike: the positions
 * sent to the engine, the tiles of a view, the flights, the label candidates of the vector tiles.
 */

/** The latitude where Web Mercator is square: the edges of the map */
const MAX_LAT = 85.051128779806589;

/** Mercator x of a longitude (degrees) */
export const mercatorX = (lon: number): number => {
  return (lon + 180) / 360;
};

/** Mercator y of a latitude (degrees), clamped to the edges of the map */
export const mercatorY = (lat: number): number => {
  const phi = (Math.max(-MAX_LAT, Math.min(MAX_LAT, lat)) * Math.PI) / 180;
  return 0.5 - Math.log(Math.tan(Math.PI / 4 + phi / 2)) / (2 * Math.PI);
};

/** Longitude (degrees) of a Mercator x */
export const longitude = (x: number): number => {
  return x * 360 - 180;
};

/** Latitude (degrees) of a Mercator y */
export const latitude = (y: number): number => {
  return (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;
};

/** Mercator x, y of a longitude and latitude */
export const toMercator = ([lon, lat]: readonly [number, number, ...unknown[]]): [x: number, y: number] => {
  return [mercatorX(lon), mercatorY(lat)];
};

/** Longitude and latitude of a Mercator x, y */
export const fromMercator = ([x, y]: readonly [number, number]): [lon: number, lat: number] => {
  return [longitude(x), latitude(y)];
};
