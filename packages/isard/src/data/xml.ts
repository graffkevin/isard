/**
 * A small XML reader for the formats read here (KML, GPX, WMTS capabilities): elements, their attributes and text,
 * CDATA, comments, entities. The same in the page, in workers and in tests (which have no `DOMParser`), and lenient:
 * a closing tag without its opening one is ignored, an unclosed element ends with its parent. Names are kept without
 * their namespace prefix (`ows:Identifier` is `Identifier`): the formats read here never mix two meanings of a name.
 * No DTD is read (no external entity, no entity expansion).
 */

export interface XmlElement {
  /** Local name, without its prefix */
  name: string;
  /** Attributes by local name */
  attributes: Record<string, string>;
  children: XmlElement[];
  /** Text directly inside, CDATA included, entities decoded */
  text: string;
}

const ENTITIES: Record<string, string> = { lt: '<', gt: '>', amp: '&', quot: '"', apos: "'" };

/** Text with its entities decoded (unknown ones kept as they are) */
const decode = (text: string): string => {
  if (!text.includes('&')) return text;
  return text.replace(/&(#x[\da-f]+|#\d+|\w+);/gi, (whole, entity: string) => {
    if (entity[0] === '#') {
      const code =
        entity[1] === 'x' || entity[1] === 'X' ? Number.parseInt(entity.slice(2), 16) : Number(entity.slice(1));
      return Number.isFinite(code) && code >= 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
    }
    return ENTITIES[entity] ?? whole;
  });
};

const local = (name: string) => name.slice(name.indexOf(':') + 1);

/** A start tag at the reader's position: its name, its attributes (quoted values may hold '>'), whether it closes */
const START = /<([^\s/>]+)((?:\s+[^\s=/>]+\s*=\s*(?:"[^"]*"|'[^']*'))*)\s*(\/?)>/y;

/** The document's root element; throws if there is none */
export const parseXml = (source: string): XmlElement => {
  const root: XmlElement = { name: '#document', attributes: {}, children: [], text: '' };
  const stack = [root];
  const top = () => stack[stack.length - 1] as XmlElement;
  let i = 0;
  while (i < source.length) {
    const lt = source.indexOf('<', i);
    const end = lt < 0 ? source.length : lt;
    if (end > i) top().text += decode(source.slice(i, end));
    if (lt < 0) break;
    if (source.startsWith('<!--', lt)) {
      const close = source.indexOf('-->', lt + 4);
      i = close < 0 ? source.length : close + 3;
    } else if (source.startsWith('<![CDATA[', lt)) {
      const close = source.indexOf(']]>', lt + 9);
      top().text += source.slice(lt + 9, close < 0 ? source.length : close);
      i = close < 0 ? source.length : close + 3;
    } else if (source[lt + 1] === '?' || source[lt + 1] === '!') {
      // A declaration, a processing instruction, a DOCTYPE (its internal subset skipped whole)
      const bracket = source.indexOf('[', lt);
      const gt = source.indexOf('>', lt);
      const subset = source[lt + 1] === '!' && bracket >= 0 && bracket < gt ? source.indexOf(']', bracket) : lt;
      const close = source.indexOf('>', subset);
      i = close < 0 ? source.length : close + 1;
    } else if (source[lt + 1] === '/') {
      const gt = source.indexOf('>', lt);
      const name = local(source.slice(lt + 2, gt < 0 ? source.length : gt).trim());
      let at = stack.length - 1;
      while (at > 0 && stack[at]?.name !== name) at -= 1;
      if (at > 0) stack.length = at;
      i = gt < 0 ? source.length : gt + 1;
    } else {
      START.lastIndex = lt;
      const tag = START.exec(source);
      if (!tag) {
        top().text += '<';
        i = lt + 1;
        continue;
      }
      const element: XmlElement = { name: local(tag[1] as string), attributes: {}, children: [], text: '' };
      for (const [, key, double, single] of (tag[2] as string).matchAll(/([^\s=]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g)) {
        element.attributes[local(key as string)] = decode(double ?? single ?? '');
      }
      top().children.push(element);
      if (!tag[3]) stack.push(element);
      i = START.lastIndex;
    }
  }
  const first = root.children[0];
  if (!first) throw new Error('isard: no XML element');
  return first;
};

/** The first child element of a name */
export const child = (element: XmlElement | undefined, name: string): XmlElement | undefined => {
  return element?.children.find((c) => c.name === name);
};

/** The child elements of a name */
export const children = (element: XmlElement | undefined, name: string): XmlElement[] => {
  return element?.children.filter((c) => c.name === name) ?? [];
};

/** The trimmed text of the first child of a name; undefined without one */
export const childText = (element: XmlElement | undefined, name: string): string | undefined => {
  return child(element, name)?.text.trim();
};

/** Every element of a name under an element, in document order (not inside a match) */
export const descendants = (element: XmlElement, name: string, out: XmlElement[] = []): XmlElement[] => {
  element.children.forEach((c) => {
    if (c.name === name) out.push(c);
    else descendants(c, name, out);
  });
  return out;
};
