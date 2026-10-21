/** What the pages of the examples share */

/** The element of an id; with a type (`HTMLInputElement`…), checked to be one */
export function element(id: string): HTMLElement;
export function element<T extends HTMLElement>(id: string, type: abstract new () => T): T;
export function element(id: string, type: abstract new () => HTMLElement = HTMLElement): HTMLElement {
  const found = document.getElementById(id);

  if (!found) throw new Error(`#${id} missing from the page`);
  if (!(found instanceof type)) throw new Error(`#${id} is not a ${type.name}`);

  return found;
}
