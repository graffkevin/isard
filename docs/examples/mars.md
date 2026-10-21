# Mars

A map of another body with an atmosphere: the globe takes the radius of Mars and its thin halo of dust, a butterscotch
glow at the limb. The Viking colour mosaic (MDIM 2.1) comes from NASA's Mars Trek in longitudes and latitudes,
reprojected in the page into Web Mercator tiles (`grid: 'equirectangular'`); Olympus Mons, Valles Marineris and the
landing sites are circles and labels, hidden past the horizon. MOLA's terrain model lifts the ground (`terrain: true`
on Mars): the buttons fly to Olympus Mons, 21 km high, and down the canyons of Valles Marineris (`#olympus-mons`,
`#valles-marineris` in the URL open them). A click flies to the point.

The same in [React](./react/mars).

<LiveExample slug="mars" height="520px" />

<<< @/../examples/mars/main.ts
