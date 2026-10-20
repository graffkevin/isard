# Panoramas and the street-level view

## Panoramas

Oriented images of a vehicle's cameras around each of its positions (Stereopolis, the iTowns immersive examples),
projected onto the scene around the panorama shown:

```ts
const panoramas = await map.addPanoramas({
  url: 'https://…/{cameraId}-{panoId}.jpg', // an image of a camera at a panorama
  orientationsUrl: 'https://…/panoramas.geojson', // positions and attitudes
  calibrationUrl: 'https://…/cameras.json', // the cameras
  backgroundDistance: 1200, // metres of the backdrop sphere
});
await panoramas.show(panoramas.nearest([2.3348, 48.8506]).index);
```

- **Positions** (`orientationsUrl`): GeoJSON points, Lambert-93 by default (or the CRS of the file's `crs` member,
  or `crs`), altitudes above the geoid (NGF-IGN69) raised by the RAF20 geoid onto the IGN relief (`geoid: false` for
  heights above the ellipsoid); each with its `id` and the vehicle's `heading`, `pitch`, `roll` in degrees.
- **Cameras** (`calibrationUrl`): a JSON array, iTowns' format: each camera's `id`, `rotation` (row-major, from the
  panorama's frame to the camera's: x right, y down, z forward) and `position` in the panorama's frame (metres),
  `projection` (its intrinsics), `size`, and MicMac's radial `distortion` (`pps`, `poly357`, `limit`).
- **Images** (`url`): `{cameraId}` and `{panoId}` replaced. Images smaller than their calibration (a reduced set) are
  read as well.

Each pixel of the scene drawn (relief, buildings, 3D Tiles, models) is taken into each camera of the panorama shown
and colored by its image there, the cameras blended toward their borders; beyond `backgroundDistance` and where
nothing is drawn (the sky), the pixel is taken on a sphere of that radius around the panorama. The geometry the
images fall on is the map's own: buildings raised from the BD TOPO, a photomesh, the relief.

`panoramas.list` gives each panorama (`index`, `id`, `lonLat`, `height`, `heading`), `current` the one shown,
`hide()` hides it. The requests go through `setRequestHeaders` like the others (a private source).

## The street-level view

```ts
await panoramas.enter({ near: [2.3348, 48.8506] }); // the eye on a panorama
panoramas.exit(); // back to the camera turning around a point
```

`enter` puts the eye at the panorama's cameras: the drag turns it, the wheel narrows or widens its field of view, a
click on the ground goes to the panorama nearest to the point clicked, Z and S (or Page Up and Page Down) to the next
and the previous ones, `go(index)` from the page. `exit` gives the camera back above where the eye stood, facing the
same way.

Without panoramas, the camera itself goes to street level:

```ts
map.setFirstPerson({ lonLat: [2.3431, 48.8862], height: 1.7, bearing: 180, pitch: 90, fov: 75 });
map.firstPerson; // the eye, while there is one
map.setFirstPerson(undefined);
```

`pitch` runs from straight down (0) through level (90) to straight up (180). The drag turns the eye, the arrows too;
`setCamera` turns the camera around a point again.

<LiveExample slug="panoramas" height="520px" />
