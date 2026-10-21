# three.js in the map

A custom layer renders a three.js scene into the map's WebGL context, in the `'3d'` mode: the BD TOPO buildings hide
the cubes circling between them, and the objects hide the buildings. The scene is laid out in metres around an
anchor; `frame.matrixAt(anchor)` is its camera's matrix, exact at any zoom. See [custom layers](../../layers/custom).

In React (`isard/react`); the same in [TypeScript](../../examples/threejs).

<LiveExample slug="react/threejs" height="520px" />

<<< @/../examples/react/threejs/main.tsx
