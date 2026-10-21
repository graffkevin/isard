/**
 * Headers added to the requests of the page by URL prefix: an access token for private sources (a Géoplateforme
 * private stream, a server behind single sign-on), on every request isard makes, from the page, its workers (vector
 * tiles, 3D Tiles and point clouds) and its engine (raster tiles, tilesets): `setRequestHeaders(prefix, headers)`,
 * again with a new token when it is renewed. The rules of a URL are merged, the longest prefix last (it wins).
 */

/** Rules: a URL prefix and its headers, the shortest prefix first */
export type HeaderRules = [prefix: string, headers: Record<string, string>][];

let rules: HeaderRules = [];
const listeners = new Set<(rules: HeaderRules) => void>();

/**
 * Adds `headers` to every request isard makes to a URL starting with `prefix` (in the page and its workers), or with
 * no headers, stops: `setRequestHeaders('https://data.geopf.fr/private/', { Authorization: 'Bearer …' })`. The
 * server must allow these headers (CORS). For the whole page: every map, the ones to come too
 */
export const setRequestHeaders = (prefix: string, headers?: Record<string, string>): void => {
  const kept = rules.filter(([p]) => p !== prefix);
  if (headers && Object.keys(headers).length > 0) kept.push([prefix, { ...headers }]);
  kept.sort(([p], [q]) => p.length - q.length);
  setHeaderRules(kept);
  for (const listener of listeners) listener(rules);
};

/** @internal The rules now */
export const headerRules = (): HeaderRules => {
  return rules;
};

/** @internal The rules, from the page (a worker's copy) */
export const setHeaderRules = (next: HeaderRules): void => {
  rules = next;
};

/** @internal Follows the rules (the workers and engines of a map); returns the function that stops */
export const onHeaderRules = (listener: (rules: HeaderRules) => void): (() => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};

/** @internal `init` with the headers of the rules matching `url` (unchanged when none does) */
export const withHeaders = <T extends RequestInit>(url: string | URL, init?: T): T => {
  const href = String(url);
  const matching = rules.filter(([prefix]) => href.startsWith(prefix));
  if (matching.length === 0) return init ?? ({} as T);
  const headers = new Headers(init?.headers);
  for (const [, set] of matching) for (const [name, value] of Object.entries(set)) headers.set(name, value);
  return { ...(init ?? ({} as T)), headers };
};
