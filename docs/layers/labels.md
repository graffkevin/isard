# Labels

```ts
map.addSymbols({
  data,                                  // points; lines are labelled at their middle, polygons at their centroid
  text: (p) => `${p.name}\n${p.level} m`,
  font: 'Source Sans Pro Bold',          // the Géoplateforme's fonts: Regular, Bold, Italic, Semibold
  size: [[12, 11], [16, 14]],
  color: '#16324f',
  haloColor: '#fff',
  haloWidth: 1.6,
  anchor: 'top',
  offset: [0, 1.3],                      // ems
  maxWidth: 10,                          // ems, then the text wraps
  placement: 'point',                    // or 'line': along the line, kept upright
  overlap: false,                        // true: always shown, hiding nothing
});
```

Text is drawn from signed distance fields (MapLibre's glyph format), loaded from the Géoplateforme on first use
(`glyphs` option for others). Lines break at `\n` and wrap at `maxWidth`; runs can have their own scale and font, as
MapLibre's `format`:

```ts
text: (p) => [{ text: p.name }, { text: '\nno data', scale: 0.85, font: 'Source Sans Pro Italic' }],
```

Labels are placed at every frame, every symbol layer sharing one collision grid from the top layer down, in data
order within a layer: a label shows only where no label placed before it does. Far labels of a tilted view are
smaller (MapLibre's perspective ratio).
