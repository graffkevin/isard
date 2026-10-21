/** The credits of a map's raster layers, and who is told when they change (the attribution controls). */

/** The credits of the raster layers that have some (basemaps, hillshade, the Géoplateforme's rasters…) */
export class Credits {
  private readonly byLayer = new Map<object, string>();
  /** Called when the credits of the layers or relief change */
  private readonly listeners = new Set<() => void>();

  /** A layer's credit */
  set(layer: object, credit: string): void {
    this.byLayer.set(layer, credit);
    this.changed();
  }

  /** Forgets a removed layer's credit, if it had one */
  forget(layer: object): void {
    if (this.byLayer.delete(layer)) this.changed();
  }

  /** The credits of `layers` (in their order) and of the relief, each once */
  of(layers: readonly object[], relief: string | undefined): string[] {
    const credits = [...layers.map((layer) => this.byLayer.get(layer)), relief];

    return [...new Set(credits.filter((c): c is string => c !== undefined))];
  }

  /** Calls `listener` when the credits change; returns what stops it */
  onChange(listener: () => void): () => void {
    this.listeners.add(listener);

    return () => this.listeners.delete(listener);
  }

  /** Tells the listeners the credits changed */
  changed(): void {
    this.listeners.forEach((listener) => {
      listener();
    });
  }
}
