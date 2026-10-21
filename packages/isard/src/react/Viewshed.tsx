/** `<Viewshed>`: an observer's viewshed, several drawn together */

import { type ReactNode, useContext, useEffect, useMemo } from 'react';
import type { ViewshedOptions } from '../types.ts';
import { MapContext } from './context.ts';

/** An observer's viewshed; several `<Viewshed>` are drawn together */
export const Viewshed = (props: ViewshedOptions): ReactNode => {
  const binding = useContext(MapContext);
  const key = useMemo(() => ({}), []);
  // Numbers, strings and the position: compared by value
  const options = JSON.stringify(props);

  useEffect(() => {
    if (!binding) return;

    return () => {
      binding.viewsheds.delete(key);
      binding.applyViewsheds();
    };
  }, [binding, key]);

  useEffect(() => {
    if (!binding) return;
    binding.viewsheds.set(key, JSON.parse(options) as ViewshedOptions);
    binding.applyViewsheds();
  }, [binding, key, options]);

  return null;
};
