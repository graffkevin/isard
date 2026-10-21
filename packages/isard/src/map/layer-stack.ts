/**
 * The drawing order of a map's layers, kept beside the engine's: the layers waiting for the engine with 3D Tiles are
 * in the order but not in the engine yet, and the order recreates the layers in a new engine after a context loss.
 */

import type { Layer } from '../layers/layers.ts';
import type { Engine } from '../wasm/isard.js';
import { report } from './checks.ts';

/** A map's layers in drawing order, bottom first, and those kept on top */
export class LayerStack {
  /** Layers in drawing order (bottom first), to recreate them after a context loss */
  readonly layers: Layer[] = [];
  /** Layers new layers go under */
  readonly onTop = new Set<Layer>();
  private readonly engine: () => Engine;

  /** `engine`: the map's engine of the moment */
  constructor(engine: () => Engine) {
    this.engine = engine;
  }

  has(layer: object): boolean {
    return this.layers.includes(layer as Layer);
  }

  /** Puts a new layer on top, under the layers kept on top (the basemap's labels) */
  add(layer: Layer): void {
    this.layers.push(layer);

    const top = this.layers.find((l) => this.onTop.has(l) && l !== layer);

    if (top) this.move(layer, top);
  }

  /** Forgets a removed layer */
  forget(layer: object): void {
    this.unlist(layer);
    this.onTop.delete(layer as Layer);
  }

  /** Moves a layer under `before`, or on top of all */
  move(layer: object, before: object | undefined): void {
    const handle = layer as Layer;

    this.unlist(handle);

    const index = before ? this.layers.indexOf(before as Layer) : -1;
    const at = index < 0 ? this.layers.length : index;

    this.layers.splice(at, 0, handle);
    if (handle.id >= 0) this.engine().moveLayer(handle.id, this.engineIndex(at));
  }

  /** A layer made again in the engine under a new id: `old` removed, the new one moved to the layer's place */
  replace(layer: object, old: number): void {
    const engine = this.engine();

    engine.removeLayer(old);

    const index = this.layers.indexOf(layer as Layer);

    if (index >= 0) engine.moveLayer((layer as Layer).id, this.engineIndex(index));
  }

  /** Moves a layer under all the others */
  toBottom(layer: Layer): void {
    this.engine().moveLayer(layer.id, 0);
    this.unlist(layer);
    this.layers.unshift(layer);
  }

  /** Creates the layers in a new engine, in order, but those still `waiting` (for the engine with 3D Tiles); a layer
   * that fails is reported and left out */
  reattach(engine: Engine, waiting: (layer: Layer) => boolean): void {
    // No layer refers to an id of the old engine while the others are created
    this.layers.forEach((layer) => {
      layer.forgetEngine();
    });
    this.layers.forEach((layer) => {
      if (waiting(layer)) return;
      try {
        layer.attach(engine);
      } catch (error) {
        report(error);
      }
    });
    // Links between layers, once every layer has its new id (buildings colored by a network above them)
    this.layers.forEach((layer) => {
      if (layer.id < 0) return;
      try {
        layer.relink(engine);
      } catch (error) {
        report(error);
      }
    });
  }

  /** Frees what the layers hold beside the engine (custom layers' WebGL objects, models' data) */
  dispose(): void {
    this.layers.forEach((layer) => {
      layer.dispose();
    });
  }

  /** Takes a layer out of the drawing order (to put it back elsewhere, or for good) */
  private unlist(layer: object): void {
    const index = this.layers.indexOf(layer as Layer);

    if (index >= 0) this.layers.splice(index, 1);
  }

  /** Index in the engine of the layer at `index`: the layers waiting for the engine with 3D Tiles are not in it yet */
  private engineIndex(index: number): number {
    return this.layers.slice(0, index).filter((layer) => layer.id >= 0).length;
  }
}
