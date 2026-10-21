# 3D Tiles features

A city of blocks from the 3D Tiles samples (Cesium's, b3dm with batch tables): click one, its properties come from
its tile's batch table (`pickFeature` on a 3D Tiles layer; in 3D Tiles 1.1, the property tables of
`EXT_structural_metadata`, as the IGN's Bâtiments 3D give their BD TOPO attributes). The color tints the layer
(`setStyle({ color })`).

In React (`isard/react`); the same in [TypeScript](../../examples/tiles3d-features).

<LiveExample slug="react/tiles3d-features" height="520px" />

<<< @/../examples/react/tiles3d-features/main.tsx
