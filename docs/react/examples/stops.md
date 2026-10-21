# Stops on a line

The map and what it shows as components (`isard/react`): each click on the map adds a stop, a marker with its popup
of JSX; a line joins the stops, its data sent again when they change, the layer kept; the panel's controls change the
line's style and the basemap through props. Removing a stop unmounts its marker.

<LiveExample slug="react/stops" height="520px" />

<<< @/../examples/react/stops/main.tsx
