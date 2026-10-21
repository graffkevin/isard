/** Options of the Vite plugin of isard */
export interface IsardViteOptions {
  /** Folder of the build output its files are copied to (default `isard`) */
  folder?: string;
}

/** A Vite plugin (the fields isard uses; typed as Vite's `Plugin` where Vite is installed) */
export interface IsardVitePlugin {
  name: string;
  [hook: string]: unknown;
}

/**
 * The Vite plugin of isard: its engines and workers found in development, copied into the build and their folder given
 * to the maps; nothing else to configure
 */
export default function isard(options?: IsardViteOptions): IsardVitePlugin;
