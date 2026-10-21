/**
 * A cache keeping its most recently used entries (the decoded tiles and images of the workers, the source tiles of a
 * reprojection, the labels of the vector tiles): a `Map` in order of use, the least recent dropped past `capacity`.
 */
export class Lru<K, V> {
  private readonly entries = new Map<K, V>();

  /** `evict`: told of each entry dropped for room (not of those deleted) */
  constructor(
    public capacity: number,
    private readonly evict?: (value: V, key: K) => void,
  ) {}

  get size(): number {
    return this.entries.size;
  }

  /** The value of a key, now the most recently used; undefined if absent */
  get(key: K): V | undefined {
    const value = this.entries.get(key);

    if (value !== undefined) {
      this.entries.delete(key);
      this.entries.set(key, value);
    }

    return value;
  }

  /** Whether a key is kept, without using it */
  has(key: K): boolean {
    return this.entries.has(key);
  }

  /** Adds or replaces a value, the most recently used; drops the least recent ones past the capacity */
  set(key: K, value: V): void {
    this.entries.delete(key);
    this.entries.set(key, value);
    for (const [oldest, old] of this.entries) {
      if (this.entries.size <= this.capacity) break;
      this.entries.delete(oldest);
      this.evict?.(old, oldest);
    }
  }

  delete(key: K): boolean {
    return this.entries.delete(key);
  }

  /** The values, least recently used first */
  values(): IterableIterator<V> {
    return this.entries.values();
  }

  clear(): void {
    this.entries.clear();
  }
}
