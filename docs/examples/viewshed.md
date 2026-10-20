# Viewshed

What can be seen from a point (`map.setViewshed`, a prototype): from the forecourt of the Sacré-Cœur, over the IGN relief (RGE ALTI) and the BD TOPO buildings of Montmartre. Seen in green, hidden in red, within the range. The scene is drawn from the observer into a depth cube, as for the shadows of a point light, and each pixel compares its distance to the observer with it. All around, or one way only as a camera: a cone given by a direction and a field of view, its edges drawn on the ground. Several observers: what any of them sees is seen. A click on the map adds one; a click on a pin opens its settings (its height above the ground, the cone, the range, a button to remove it), and the pin drags. On a BD TOPO building, the observer stands on its roof.

<LiveExample slug="viewshed" height="560px" />

<<< @/../examples/viewshed/main.ts
