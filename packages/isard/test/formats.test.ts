import { describe, expect, test } from 'bun:test';
import { parseGpx } from '../src/data/gpx.ts';
import { parseKml } from '../src/data/kml.ts';
import { parseTopoJson, type Topology } from '../src/data/topojson.ts';
import { child, childText, descendants, parseXml } from '../src/data/xml.ts';

describe('parseXml', () => {
  test('elements, attributes, text; prefixes dropped; comments, CDATA, entities, a DOCTYPE', () => {
    const root = parseXml(`<?xml version="1.0" encoding="UTF-8"?>
      <!DOCTYPE doc [ <!ENTITY e "x"> ]>
      <!-- a comment with <tags> -->
      <a:doc xmlns:a="urn:a" x:id='1 &amp; 2' b="&#x3c;&#62;"><item>A &lt;b&gt; &quot;c&quot; &#233;</item>
        <empty/><item><![CDATA[<raw> & ]]>tail</item><b:item>third</b:item>
      </a:doc>`);
    expect(root.name).toBe('doc');
    expect(root.attributes).toEqual({ a: 'urn:a', id: '1 & 2', b: '<>' });
    expect(descendants(root, 'item').map((i) => i.text)).toEqual(['A <b> "c" é', '<raw> & tail', 'third']);
    expect(child(root, 'empty')?.children).toEqual([]);
    expect(childText(root, 'missing')).toBeUndefined();
  });

  test('lenient: a stray closing tag ignored, an unclosed element ended by its parent, unknown entities kept', () => {
    const root = parseXml('<r><a><b>1</b></c></a><d>&nbsp;<</d><e>2</r>');
    expect(root.children.map((c) => c.name)).toEqual(['a', 'd', 'e']);
    expect(childText(root, 'd')).toBe('&nbsp;<');
    expect(childText(root, 'e')).toBe('2');
    expect(() => parseXml('no element')).toThrow();
  });
});

const KML = `<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
<Document>
  <name>Paris</name>
  <Style id="red"><LineStyle><color>ff0000ff</color><width>3</width></LineStyle>
    <PolyStyle><color>7f00ff00</color></PolyStyle></Style>
  <StyleMap id="map"><Pair><key>normal</key><styleUrl>#red</styleUrl></Pair>
    <Pair><key>highlight</key><styleUrl>#other</styleUrl></Pair></StyleMap>
  <Schema name="s" id="s"><SimpleField name="height" type="int"/></Schema>
  <Folder><name>Monuments</name>
    <Placemark id="p1">
      <name>Panthéon</name>
      <description><![CDATA[<b>Dome</b> of 83 m]]></description>
      <styleUrl>#map</styleUrl>
      <ExtendedData><Data name="architect"><value>Soufflot</value></Data>
        <SchemaData schemaUrl="#s"><SimpleData name="height">83</SimpleData></SchemaData></ExtendedData>
      <Point><coordinates> 2.3461,48.8462,35 </coordinates></Point>
    </Placemark>
    <Placemark><name>Walk</name>
      <Style><LineStyle><color>80ff0000</color></LineStyle></Style>
      <LineString><tessellate>1</tessellate><coordinates>
        2.34,48.84 2.35,48.85
        2.36,48.86
      </coordinates></LineString>
    </Placemark>
  </Folder>
  <Placemark><name>Block</name><styleUrl>#red</styleUrl>
    <Polygon><outerBoundaryIs><LinearRing><coordinates>0,0 1,0 1,1 0,1 0,0</coordinates></LinearRing></outerBoundaryIs>
      <innerBoundaryIs><LinearRing><coordinates>0.2,0.2 0.4,0.2 0.4,0.4 0.2,0.2</coordinates></LinearRing></innerBoundaryIs>
    </Polygon></Placemark>
  <Placemark><name>Both</name><MultiGeometry>
    <Point><coordinates>1,2</coordinates></Point>
    <LineString><coordinates>1,2 3,4</coordinates></LineString>
  </MultiGeometry></Placemark>
  <Placemark><name>Points</name><MultiGeometry>
    <Point><coordinates>1,2</coordinates></Point><Point><coordinates>3,4</coordinates></Point>
  </MultiGeometry></Placemark>
  <Placemark><name>Track</name><gx:Track><when>2026-10-18T10:00:00Z</when>
    <gx:coord>2.3 48.8 30</gx:coord><gx:coord>2.4 48.9 32</gx:coord></gx:Track></Placemark>
  <Placemark><name>Nowhere</name></Placemark>
</Document>
</kml>`;

describe('parseKml', () => {
  const { features } = parseKml(KML);

  test('every placemark, in folders or not; points with their altitude, name, description, extended data', () => {
    expect(features.map((f) => f.properties.name)).toEqual([
      'Panthéon',
      'Walk',
      'Block',
      'Both',
      'Points',
      'Track',
      'Nowhere',
    ]);
    const [pantheon] = features;
    expect(pantheon?.id).toBe('p1');
    expect(pantheon?.geometry).toEqual({ type: 'Point', coordinates: [2.3461, 48.8462, 35] });
    expect(pantheon?.properties).toMatchObject({
      description: '<b>Dome</b> of 83 m',
      architect: 'Soufflot',
      height: '83',
    });
  });

  test('styles: shared through a StyleMap, inline, as stroke and fill colors and opacities', () => {
    expect(features[0]?.properties).toMatchObject({
      stroke: '#ff0000',
      'stroke-opacity': 1,
      'stroke-width': 3,
      fill: '#00ff00',
      'fill-opacity': 0.498,
    });
    expect(features[1]?.properties).toMatchObject({ stroke: '#0000ff', 'stroke-opacity': 0.502 });
  });

  test('lines across lines, polygons with holes, MultiGeometry, gx:Track, no geometry', () => {
    expect(features[1]?.geometry).toEqual({
      type: 'LineString',
      coordinates: [
        [2.34, 48.84],
        [2.35, 48.85],
        [2.36, 48.86],
      ],
    });
    const block = features[2]?.geometry as { type: string; coordinates: number[][][] };
    expect(block.type).toBe('Polygon');
    expect(block.coordinates.map((ring) => ring.length)).toEqual([5, 4]);
    expect(features[3]?.geometry?.type).toBe('GeometryCollection');
    expect(features[4]?.geometry).toEqual({
      type: 'MultiPoint',
      coordinates: [
        [1, 2],
        [3, 4],
      ],
    });
    expect(features[5]?.geometry).toEqual({
      type: 'LineString',
      coordinates: [
        [2.3, 48.8, 30],
        [2.4, 48.9, 32],
      ],
    });
    expect(features[6]?.geometry).toBeNull();
  });
});

const GPX = `<?xml version="1.0" encoding="UTF-8" standalone="no" ?>
<gpx xmlns="http://www.topografix.com/GPX/1/1" version="1.1" creator="test"
  xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">
  <metadata><name>Vincennes</name><time>2026-10-18T09:00:00Z</time></metadata>
  <wpt lat="48.8457" lon="2.4246"><ele>51.2</ele><time>2026-10-18T09:00:00Z</time><name>Start</name><sym>Flag</sym></wpt>
  <wpt lat="48.84" lon="2.43"><name>No altitude</name></wpt>
  <wpt lon="2.43"><name>Broken</name></wpt>
  <rte><name>Plan</name><rtept lat="48.84" lon="2.42"/><rtept lat="48.85" lon="2.43"/></rte>
  <trk><name>Run</name><type>running</type>
    <trkseg>
      <trkpt lat="48.8457" lon="2.4246"><ele>51</ele><time>2026-10-18T09:00:00Z</time>
        <extensions><gpxtpx:TrackPointExtension><gpxtpx:hr>120</gpxtpx:hr></gpxtpx:TrackPointExtension></extensions></trkpt>
      <trkpt lat="48.8460" lon="2.4250"><ele>52</ele><time>2026-10-18T09:00:05Z</time></trkpt>
    </trkseg>
  </trk>
  <trk><name>Two segments</name>
    <trkseg><trkpt lat="1" lon="2"/><trkpt lat="3" lon="4"/></trkseg>
    <trkseg><trkpt lat="5" lon="6"/><trkpt lat="7" lon="8"/></trkseg>
    <trkseg><trkpt lat="9" lon="9"/></trkseg>
  </trk>
  <trk><name>Empty</name><trkseg></trkseg></trk>
</gpx>`;

describe('parseGpx', () => {
  const { features } = parseGpx(GPX);

  test('waypoints as points with their altitude and time; a broken one left out', () => {
    expect(features.slice(0, 2).map((f) => f.geometry)).toEqual([
      { type: 'Point', coordinates: [2.4246, 48.8457, 51.2] },
      { type: 'Point', coordinates: [2.43, 48.84] },
    ]);
    expect(features[0]?.properties).toEqual({ name: 'Start', sym: 'Flag', time: '2026-10-18T09:00:00Z' });
  });

  test('a route and tracks as lines, several segments as a multi-line, times of the points kept', () => {
    expect(features.slice(2).map((f) => [f.properties.name, f.geometry?.type])).toEqual([
      ['Plan', 'LineString'],
      ['Run', 'LineString'],
      ['Two segments', 'MultiLineString'],
    ]);
    expect(features[3]?.geometry).toEqual({
      type: 'LineString',
      coordinates: [
        [2.4246, 48.8457, 51],
        [2.425, 48.846, 52],
      ],
    });
    expect(features[3]?.properties).toEqual({
      name: 'Run',
      type: 'running',
      times: ['2026-10-18T09:00:00Z', '2026-10-18T09:00:05Z'],
    });
    // The segment of one point is no line
    expect((features[4]?.geometry as { coordinates: unknown[] } | undefined)?.coordinates.length).toBe(2);
  });
});

describe('parseTopoJson', () => {
  /** Two squares sharing an edge (arc 0), quantized, and a point */
  const topology: Topology = {
    type: 'Topology',
    transform: { scale: [0.5, 0.25], translate: [10, 40] },
    arcs: [
      [
        [2, 0],
        [0, 4],
      ],
      [
        [2, 4],
        [-2, 0],
        [0, -4],
        [2, 0],
      ],
      [
        [2, 0],
        [2, 0],
        [0, 4],
        [-2, 0],
      ],
    ],
    objects: {
      squares: {
        type: 'GeometryCollection',
        geometries: [
          { type: 'Polygon', arcs: [[0, 1]], id: 'west', properties: { n: 1 } },
          { type: 'Polygon', arcs: [[2, -1]], properties: { n: 2 } },
          { type: 'Point', coordinates: [4, 4] },
          { type: null },
        ],
      },
      edge: { type: 'LineString', arcs: [0] },
    },
  };

  test('arcs decoded (deltas, scale, translation) and stitched, reversed with ~i; ids and properties kept', () => {
    const { features } = parseTopoJson(topology);
    expect(features.length).toBe(4);
    expect(features[0]).toMatchObject({ id: 'west', properties: { n: 1 } });
    expect(features[0]?.geometry).toEqual({
      type: 'Polygon',
      coordinates: [
        [
          [11, 40],
          [11, 41],
          [10, 41],
          [10, 40],
          [11, 40],
        ],
      ],
    });
    // The shared edge walked backwards, closing the eastern square
    expect((features[1]?.geometry as { coordinates: number[][][] } | undefined)?.coordinates[0]).toEqual([
      [11, 40],
      [12, 40],
      [12, 41],
      [11, 41],
      [11, 40],
    ]);
    expect(features[2]?.geometry).toEqual({ type: 'Point', coordinates: [12, 41] });
    expect(features[3]?.geometry).toBeNull();
  });

  test('an object by name; unquantized; a missing one throws', () => {
    expect(parseTopoJson(topology, 'edge').features[0]?.geometry).toEqual({
      type: 'LineString',
      coordinates: [
        [11, 40],
        [11, 41],
      ],
    });
    const plain: Topology = {
      type: 'Topology',
      arcs: [
        [
          [1, 2],
          [3, 4],
        ],
      ],
      objects: { l: { type: 'MultiLineString', arcs: [[~0]] } },
    };
    expect(parseTopoJson(plain).features[0]?.geometry).toEqual({
      type: 'MultiLineString',
      coordinates: [
        [
          [3, 4],
          [1, 2],
        ],
      ],
    });
    expect(() => parseTopoJson(topology, 'nope')).toThrow();
  });
});
