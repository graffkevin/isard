import { existsSync, readFileSync } from 'node:fs';
import { defineConfig } from 'vitepress';

/** The API reference, generated from the package's declarations by TypeDoc (`typedoc`, before `vitepress`) */
const referenceSidebar = new URL('../reference/typedoc-sidebar.json', import.meta.url);
const reference = existsSync(referenceSidebar) ? JSON.parse(readFileSync(referenceSidebar, 'utf8')) : [];

const examples = [
  ['The globe', 'globe'],
  ['A map', 'basic'],
  ['Travel times on hover', 'network'],
  ['GeoJSON layers', 'geojson'],
  ['Buildings', 'buildings'],
  ['Relief and a ground shader', 'relief'],
  ['3D Tiles', 'tiles3d'],
  ['Vector basemaps', 'vector'],
  ['The Moon', 'moon'],
  ['Mars', 'mars'],
];

export default defineConfig({
  title: 'Isard',
  description: 'A WebGL2 map engine for large, interactive data layers: Rust core in WebAssembly, small TypeScript API',
  // Served by GitHub Pages under the repository's name
  base: '/isard/',
  cleanUrls: true,
  lastUpdated: true,
  head: [
    ['link', { rel: 'icon', href: '/isard/logo.svg', type: 'image/svg+xml' }],
    ['meta', { name: 'theme-color', content: '#2f6fd1' }],
  ],
  themeConfig: {
    logo: '/logo.svg',
    nav: [
      { text: 'Guide', link: '/guide/getting-started', activeMatch: '/guide/' },
      { text: 'Layers', link: '/layers/', activeMatch: '/layers/' },
      { text: 'Examples', link: '/examples/', activeMatch: '/examples/' },
      {
        text: 'API',
        activeMatch: '/(api|reference/)',
        items: [
          { text: 'Overview', link: '/api' },
          { text: 'Reference', link: '/reference/' },
        ],
      },
      { text: 'Changelog', link: '/changelog' },
    ],
    sidebar: {
      '/guide/': [
        {
          text: 'Guide',
          items: [
            { text: 'Getting started', link: '/guide/getting-started' },
            { text: 'Concepts', link: '/guide/concepts' },
            { text: 'Basemaps and relief', link: '/guide/basemaps' },
            { text: 'Géoplateforme (IGN) data', link: '/guide/geoplateforme' },
            { text: 'Events and picking', link: '/guide/events' },
            { text: 'Markers, popups, controls', link: '/guide/overlays' },
            { text: 'Performance', link: '/guide/performance' },
            { text: 'Develop and release', link: '/guide/develop' },
          ],
        },
      ],
      '/layers/': [
        {
          text: 'Layers',
          items: [
            { text: 'Overview', link: '/layers/' },
            { text: 'Styling features', link: '/layers/styling' },
            { text: 'Lines, circles, fills', link: '/layers/features' },
            { text: 'Labels', link: '/layers/labels' },
            { text: 'Networks', link: '/layers/networks' },
            { text: 'Buildings', link: '/layers/buildings' },
            { text: 'Ground shaders', link: '/layers/ground' },
            { text: 'Rasters', link: '/layers/rasters' },
            { text: '3D Tiles', link: '/layers/tiles3d' },
          ],
        },
      ],
      '/reference/': [{ text: 'Reference', link: '/reference/', items: reference }],
      '/examples/': [
        {
          text: 'Examples',
          items: [
            { text: 'All examples', link: '/examples/' },
            ...examples.map(([text, slug]) => ({ text, link: `/examples/${slug}` })),
          ],
        },
      ],
    },
    socialLinks: [
      { icon: 'github', link: 'https://github.com/graffkevin/isard' },
      { icon: 'npm', link: 'https://www.npmjs.com/package/isard' },
    ],
    search: { provider: 'local' },
    outline: { level: [2, 3] },
    footer: {
      message: 'MIT licence',
      copyright: '© 2026 Kevin Graff',
    },
  },
});
