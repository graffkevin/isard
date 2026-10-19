# Viewshed

What can be seen from a point (`map.setViewshed`, a prototype): from the forecourt of the Sacré-Cœur, over the IGN relief (RGE ALTI) and the BD TOPO buildings of Montmartre. Seen in green, hidden in red, within the range. The scene is drawn from the observer into a depth cube, as for the shadows of a point light, and each pixel compares its distance to the observer with it. All around, or one way only as a camera: a cone given by a direction and an angle of view, its edges drawn on the ground. The pin is the observer: drag it, or click elsewhere; the sliders set its height above the ground, the cone and the range.

<LiveExample slug="viewshed" height="560px" />

<<< @/../examples/viewshed/main.ts
