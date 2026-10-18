# Custom layers

Your own WebGL drawing in the map's frame, at its place among the layers: raw WebGL, three.js or any library that can
draw into an existing context. As MapLibre's custom layers, with the matrices to put it exactly on the map.

```ts
const layer = map.addCustomLayer({
  renderingMode: '3d',
  onAdd(gl, map) {
    // create programs, buffers, textures (again after a context loss)
  },
  render(gl, frame) {
    const matrix = frame.matrixAt([2.4225, 48.8452], 0); // metres around that point: x east, y north, z up
    // draw with it
  },
  onRemove(gl, map) {
    // delete what onAdd created
  },
});
layer.moveBefore(labels);
layer.remove();
```

`render` is called at the layer's place in every frame the map draws: what is under it is drawn, what is above it
comes after. The map draws frames on demand: for an animation, ask for one every frame (`map.requestRender()` from
`requestAnimationFrame`, or from `render` itself).

## The matrices

`frame.matrixAt(lonLat, altitude)` is the model-view-projection matrix (16 numbers, column-major, for
`uniformMatrix4fv`) of a local frame at that position, in metres: x east, y north, z up, its origin at `altitude`
metres above the ellipsoid (the relief is drawn at its altitude times its exaggeration). It is made in double
precision relative to the camera, then handed over in single precision: what you draw stays exact to the millimetre at
any zoom, without the jitter of large coordinates in a `float`. The frame is flat (tangent to the globe there): draw
within a few kilometres of the position, and take one matrix per object for objects far apart. On the globe its axes
follow the sphere; past the horizon, hide what lies behind the globe yourself (its surface writes no depth).

`frame.camera` is the camera of the frame (`center`, `zoom`, `pitch`, `bearing`).

## WebGL state

`render` starts with the canvas bound and its viewport set, blending on (`SRC_ALPHA`, `ONE_MINUS_SRC_ALPHA`), no
culling, counter-clockwise front faces, and:

- `'2d'` (the default): no depth test, drawn over what is under it;
- `'3d'`: the depth test on (`LEQUAL`, depth written), in the depth buffer of the 3D layers: the relief, the
  buildings, the 3D Tiles and the models hide what you draw behind them, and what you draw hides them.

Whatever state `render` changes is set back afterwards (program, buffers, vertex array, textures, framebuffer,
viewport, blending, depth, stencil, culling, pixel storage): it cannot disturb the map. Libraries that cache the WebGL
state, as three.js does, must forget it before they draw: the map changed it since.

The map is busy drawing while `render` runs: read the frame, not the map (`map.getCamera()` is the one exception,
which gives the frame's camera); change layers or the camera in a `render` event listener or later. An error thrown by
`render` is reported in the console and the frame goes on.

## Lifecycle

`onAdd(gl, map)` runs when the layer is added, and again when the WebGL context comes back after a loss (its objects
were lost with it: create them again). When the map moves onto the engine with 3D Tiles (the first 3D Tiles layer or
model), the context stays the same and so do your objects. `onRemove(gl, map)` runs when the layer is removed or the
map destroyed.

## Raw WebGL

A triangle 200 metres wide standing on the ground:

```ts
const at: [number, number] = [2.4225, 48.8452];
let program: WebGLProgram;
let buffer: WebGLBuffer;
let matrix: WebGLUniformLocation | null;

map.addCustomLayer({
  renderingMode: '3d',
  onAdd(gl) {
    const shader = (type: number, source: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, source);
      gl.compileShader(s);
      return s;
    };
    program = gl.createProgram()!;
    gl.attachShader(program, shader(gl.VERTEX_SHADER, `#version 300 es
      uniform mat4 u_matrix;
      in vec3 a_position;
      void main() { gl_Position = u_matrix * vec4(a_position, 1.0); }`));
    gl.attachShader(program, shader(gl.FRAGMENT_SHADER, `#version 300 es
      precision highp float;
      out vec4 color;
      void main() { color = vec4(0.9, 0.3, 0.2, 0.9); }`));
    gl.linkProgram(program);
    matrix = gl.getUniformLocation(program, 'u_matrix');
    buffer = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    // Metres around `at`: x east, y north, z up
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-100, 0, 0, 100, 0, 0, 0, 0, 150]), gl.STATIC_DRAW);
  },
  render(gl, frame) {
    gl.useProgram(program);
    gl.uniformMatrix4fv(matrix, false, frame.matrixAt(at));
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  },
  onRemove(gl) {
    gl.deleteProgram(program);
    gl.deleteBuffer(buffer);
  },
});
```

(`a_position` is attribute 0, the first and only one of the program.)

## three.js

A three.js scene laid out in metres around an anchor, z up, drawn with the map's context: its renderer on the map's
canvas, the frame's matrix as its camera's projection, its WebGL state forgotten before it draws.

```ts
import * as THREE from 'three';

const anchor: [number, number] = [2.4225, 48.8452];
const scene = new THREE.Scene();
scene.add(new THREE.HemisphereLight(0xffffff, 0x8899aa, 1.6));
const knot = new THREE.Mesh(new THREE.TorusKnotGeometry(14, 4), new THREE.MeshLambertMaterial({ color: 0xe8553b }));
knot.position.set(0, 0, 60); // 60 m above the anchor
scene.add(knot);
const camera = new THREE.Camera();
let renderer: THREE.WebGLRenderer;

map.addCustomLayer({
  renderingMode: '3d',
  onAdd(gl) {
    renderer = new THREE.WebGLRenderer({ canvas: map.getCanvas(), context: gl, antialias: true });
    renderer.autoClear = false; // the map clears the frame
  },
  render(gl, frame) {
    camera.projectionMatrix.fromArray(frame.matrixAt(anchor));
    camera.projectionMatrixInverse.copy(camera.projectionMatrix).invert();
    renderer.resetState();
    renderer.setViewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
    renderer.render(scene, camera);
  },
  onRemove() {
    renderer.dispose();
  },
});
```

The camera stays at the origin with the view in its projection matrix: materials that depend on the eye's position
(specular highlights, `MeshStandardMaterial`'s reflections) need it set from the matrix's inverse; Lambert and basic
materials do not. The map's picking does not see a custom layer's objects.

<LiveExample slug="threejs" height="480px" />

For glTF models at positions, [`addModels`](./models) draws them in the engine, instanced and picked, without three.js.
