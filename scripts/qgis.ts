/**
 * Packages the QGIS plugin: the viewer's build (apps/viewer/dist, built by `bun run build` first when missing or with
 * `--build`) copied into integrations/qgis/isard_viewer/viewer, the plugin's version set to the package's, then
 * integrations/qgis/dist/isard_viewer-<version>.zip, installable with QGIS's "Install from ZIP".
 */
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { deflateRawSync } from 'node:zlib';
import { $ } from 'bun';

const root = join(import.meta.dir, '..');
$.cwd(root);
const plugin = join(root, 'integrations/qgis/isard_viewer');
const viewerDist = join(root, 'apps/viewer/dist');
const out = join(root, 'integrations/qgis/dist');

if (process.argv.includes('--build') || !existsSync(join(viewerDist, 'index.html'))) {
  await $`bun run build`;
}
if (!existsSync(join(viewerDist, 'index.html'))) {
  console.error('apps/viewer/dist/index.html is missing after `bun run build`: the viewer was not built');
  process.exit(1);
}

// The viewer, served by the plugin's local server
const viewer = join(plugin, 'viewer');
rmSync(viewer, { recursive: true, force: true });
// Without the source maps: a third of the archive, of no use inside QGIS
cpSync(viewerDist, viewer, { recursive: true, filter: (src) => !src.endsWith('.map') });

// The plugin's version is the package's
const manifest: unknown = JSON.parse(readFileSync(join(root, 'packages/isard/package.json'), 'utf8'));
const version =
  typeof manifest === 'object' && manifest !== null && 'version' in manifest && typeof manifest.version === 'string'
    ? manifest.version
    : undefined;

if (!version) {
  console.error('packages/isard/package.json has no version');
  process.exit(1);
}
const metadataPath = join(plugin, 'metadata.txt');
const metadata = readFileSync(metadataPath, 'utf8');
const updated = metadata.replace(/^version=.*$/m, `version=${version}`);
if (updated !== metadata) writeFileSync(metadataPath, updated);

/** The files of the plugin, without Python caches and system files */
const files = (dir: string): string[] => {
  return readdirSync(dir)
    .filter((name) => name !== '__pycache__' && name !== '.DS_Store' && !name.endsWith('.pyc'))
    .flatMap((name) => {
      const path = join(dir, name);

      return statSync(path).isDirectory() ? files(path) : [path];
    })
    .sort();
};

/** A ZIP archive (deflated entries, UTF-8 names), enough for QGIS's installer and any unzip */
const zip = (entries: { name: string; data: Uint8Array }[]): Uint8Array => {
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  // 1980-01-01 00:00: archives identical from one run to the next
  const time = 0;
  const date = (1 << 5) | 1;
  const offset = entries.reduce((offset, { name, data }) => {
    const nameBytes = new TextEncoder().encode(name);
    const deflated = deflateRawSync(data, { level: 9 });
    const stored = deflated.length >= data.length;
    const body = stored ? data : deflated;
    const crc = Bun.hash.crc32(data);
    const local = new Uint8Array(30 + nameBytes.length);
    const lv = new DataView(local.buffer);

    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(6, 0x0800, true);
    lv.setUint16(8, stored ? 0 : 8, true);
    lv.setUint16(10, time, true);
    lv.setUint16(12, date, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, body.length, true);
    lv.setUint32(22, data.length, true);
    lv.setUint16(26, nameBytes.length, true);
    local.set(nameBytes, 30);

    const central = new Uint8Array(46 + nameBytes.length);
    const cv = new DataView(central.buffer);

    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, (3 << 8) | 20, true);
    cv.setUint16(6, 20, true);
    cv.setUint16(8, 0x0800, true);
    cv.setUint16(10, stored ? 0 : 8, true);
    cv.setUint16(12, time, true);
    cv.setUint16(14, date, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, body.length, true);
    cv.setUint32(24, data.length, true);
    cv.setUint16(28, nameBytes.length, true);
    // Unix permissions rw-r--r--
    cv.setUint32(38, 0o100644 << 16, true);
    cv.setUint32(42, offset, true);
    central.set(nameBytes, 46);
    locals.push(local, body);
    centrals.push(central);

    return offset + local.length + body.length;
  }, 0);
  const centralSize = centrals.reduce((n, c) => n + c.length, 0);
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);

  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, entries.length, true);
  ev.setUint16(10, entries.length, true);
  ev.setUint32(12, centralSize, true);
  ev.setUint32(16, offset, true);

  const parts = [...locals, ...centrals, end];
  const result = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));

  parts.reduce((at, part) => {
    result.set(part, at);

    return at + part.length;
  }, 0);

  return result;
};

const entries = files(plugin).map((path) => ({
  name: `isard_viewer/${relative(plugin, path).split(sep).join('/')}`,
  data: readFileSync(path),
}));
mkdirSync(out, { recursive: true });
const archive = join(out, `isard_viewer-${version}.zip`);
writeFileSync(archive, zip(entries));
console.log(
  `${relative(root, archive)}: ${entries.length} files, ${(statSync(archive).size / 1024).toFixed(0)} KB (QGIS: Plugins › Manage and Install Plugins › Install from ZIP)`,
);
