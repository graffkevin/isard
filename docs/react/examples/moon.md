# The Moon

A map of another body: the globe takes the Moon's radius, without atmosphere nor Earth basemap. The Lunar
Reconnaissance Orbiter's wide-angle mosaic comes from NASA's Moon Trek in longitudes and latitudes, reprojected in the
page into Web Mercator tiles (`grid: 'equirectangular'`), over the relief of LOLA's terrain model (`terrain: true` on
the Moon); the Apollo landing sites are circles and labels. The buttons fly to pitched views of the craters Tycho and
Copernicus, 4 to 5 km deep (`#tycho`, `#copernicus` in the URL open them). A click flies to the point.

In React (`isard/react`); the same in [TypeScript](../../examples/moon).

<LiveExample slug="react/moon" height="520px" />

<<< @/../examples/react/moon/main.tsx
