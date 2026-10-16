import { defineConfig } from 'vitepress';

const examples = [
  ['The globe', 'globe'],
  ['A map', 'basic'],
  ['Travel times on hover', 'network'],
  ['GeoJSON layers', 'geojson'],
  ['Buildings', 'buildings'],
  ['Relief and a ground shader', 'relief'],
  ['3D Tiles', 'tiles3d'],
  ['Vector basemaps', 'vector'],
];

export default defineConfig({
  title: 'Isard',
  description:
    'WebGL2 map engine for large, interactive data layers on Géoplateforme basemaps: Rust core in WebAssembly, small TypeScript API',
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
      { text: 'API', link: '/api' },
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
      { icon: 'npm', link: 'https://www.npmjs.com/package/@ign-junn/isard' },
    ],
    search: { provider: 'local' },
    outline: { level: [2, 3] },
    footer: {
      message: 'MIT licence',
      copyright: "© Institut national de l'information géographique et forestière (IGN)",
    },
  },
});
