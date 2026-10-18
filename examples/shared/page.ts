/** What the pages of the examples share */

/** The element of an id, typed */
export function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`#${id} missing from the page`);
  return found as T;
}
