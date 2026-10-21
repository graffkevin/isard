/**
 * Swipe: two states of a place on either side of a line. The IGN orthophotos of 1950-1965 over today's (Saint-Denis
 * and the Stade de France, built since), the old ones shown left of the line only (`map.setSwipe`); the line drags.
 */
import { GeoMap, geoplateformeWmts } from 'isard';
import { element } from '../shared/page.ts';

const map = await GeoMap.create(element('map'), {
  center: [2.3601, 48.9245],
  zoom: 15.5,
  basemap: 'ign-ortho',
  projection: 'mercator',
});
const old = map.addRaster(geoplateformeWmts('ORTHOIMAGERY.ORTHOPHOTOS.1950-1965', { maxZoom: 18 }));

const line = element('line');
const place = (x: number) => {
  const width = map.getContainer().clientWidth;
  const position = Math.max(0, Math.min(width, x));

  line.style.left = `${position}px`;
  map.setSwipe({ position, layers: [old] });
};
line.addEventListener('pointerdown', (e) => {
  line.setPointerCapture(e.pointerId);

  const move = (m: PointerEvent) => place(m.clientX - map.getContainer().getBoundingClientRect().left);
  const up = () => {
    line.removeEventListener('pointermove', move);
    line.removeEventListener('pointerup', up);
  };

  line.addEventListener('pointermove', move);
  line.addEventListener('pointerup', up);
  e.preventDefault();
});
place(map.getContainer().clientWidth / 2);
map.on('resize', ({ width }) => place(Math.min(width, Number.parseFloat(line.style.left) || width / 2)));
