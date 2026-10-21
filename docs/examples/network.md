# Travel times on hover

The streets of the IGN's BD TOPO around Saint-Mandé (Géoplateforme WFS) as a network layer, over PLAN.IGN. At every move the walking time from the cursor to every node is worked out in the page (a Dijkstra over a few thousand nodes, well under a millisecond) and handed to the map in one call, drawn by the frame of the move.

The same in [React](./react/network).

<LiveExample slug="network" height="520px" />

<<< @/../examples/network/main.ts
