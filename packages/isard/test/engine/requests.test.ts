import { afterEach, describe, expect, test } from 'bun:test';
import { headerRules, onHeaderRules, setRequestHeaders, withHeaders } from '../../src/engine/requests.ts';

afterEach(() => {
  headerRules().forEach(([prefix]) => {
    setRequestHeaders(prefix);
  });
});

describe('setRequestHeaders', () => {
  test('the rules of a URL merged, the longest prefix last; others untouched', () => {
    setRequestHeaders('https://data.geopf.fr/private/', { Authorization: 'Bearer b' });
    setRequestHeaders('https://data.geopf.fr/', { 'X-Client': 'junn', Authorization: 'Bearer a' });
    const headers = new Headers(
      withHeaders<RequestInit>('https://data.geopf.fr/private/wmts?x=1', { cache: 'reload' }).headers,
    );
    expect(headers.get('Authorization')).toBe('Bearer b');
    expect(headers.get('X-Client')).toBe('junn');
    const init = { signal: new AbortController().signal };
    expect(withHeaders('https://tile.example.org/1/2/3.png', init)).toBe(init);
  });

  test('a range kept with the headers; a token renewed; no headers stop the rule', () => {
    setRequestHeaders('https://s3.example.org/', { Authorization: 'Bearer 1' });
    const ranged = new Headers(
      withHeaders('https://s3.example.org/a.copc.laz', { headers: { Range: 'bytes=0-9' } }).headers,
    );
    expect([ranged.get('Range'), ranged.get('Authorization')]).toEqual(['bytes=0-9', 'Bearer 1']);
    const seen: number[] = [];
    const stop = onHeaderRules((rules) => seen.push(rules.length));
    setRequestHeaders('https://s3.example.org/', { Authorization: 'Bearer 2' });
    expect(new Headers(withHeaders('https://s3.example.org/x').headers).get('Authorization')).toBe('Bearer 2');
    setRequestHeaders('https://s3.example.org/');
    stop();
    setRequestHeaders('https://s3.example.org/', { Authorization: 'Bearer 3' });
    expect(seen).toEqual([1, 0]);
  });
});
