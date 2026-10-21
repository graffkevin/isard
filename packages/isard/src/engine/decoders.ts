/**
 * Optional decoders of the worker, loaded on first use from configurable URLs (the official builds three.js and
 * Cesium use): Draco geometry (`KHR_draco_mesh_compression`) and KTX2 / Basis Universal textures (`KHR_texture_basisu`),
 * transcoded to the best compressed format the GPU supports.
 */
import type { CompressedTexture, DracoGeometry, DracoRequest, TextureFormats } from '../wasm/isard.js';

/**
 * Base URLs of the decoders (each ending with `/`).
 *
 * The decoders are Emscripten builds published as classic scripts: the worker fetches the script and runs it with
 * `new Function`, so a Content Security Policy must allow `'unsafe-eval'` in `script-src` (or `worker-src`) for 3D
 * Tiles that use Draco or KTX2. They are fetched from the default CDNs; to self-host them (offline, locked-down
 * networks, no third-party requests), copy the four files (`draco_wasm_wrapper_gltf.js`, `draco_decoder_gltf.wasm`
 * from the Draco release, `basis_transcoder.js`, `basis_transcoder.wasm` from three.js's `examples/jsm/libs/basis/`)
 * to your server and give their folders in `GeoMapOptions.decoders`.
 */
export interface DecoderUrls {
  /** Folder of `draco_wasm_wrapper_gltf.js` and `draco_decoder_gltf.wasm` (and, for Draco point clouds, of the full
   * decoder `draco_wasm_wrapper.js` and `draco_decoder.wasm`) */
  draco: string;
  /** Folder of `basis_transcoder.js` and `basis_transcoder.wasm` */
  basis: string;
}

export const DEFAULT_DECODERS: DecoderUrls = {
  draco: 'https://www.gstatic.com/draco/versioned/decoders/1.5.7/',
  basis: 'https://cdn.jsdelivr.net/npm/three@0.170.0/examples/jsm/libs/basis/',
};

// ── Draco (the subset of its Emscripten API used here) ──

interface DracoStatus {
  ok(): boolean;
  error_msg(): string;
}
interface DracoMesh {
  num_points(): number;
  num_faces(): number;
}
interface DracoAttribute {
  num_components(): number;
}
interface DracoDecoder {
  DecodeArrayToMesh(data: Int8Array, size: number, mesh: DracoMesh): DracoStatus;
  DecodeArrayToPointCloud(data: Int8Array, size: number, cloud: DracoMesh): DracoStatus;
  GetAttributeByUniqueId(mesh: DracoMesh, id: number): DracoAttribute;
  GetAttributeDataArrayForAllPoints(
    mesh: DracoMesh,
    attribute: DracoAttribute,
    type: number,
    bytes: number,
    pointer: number,
  ): boolean;
  GetTrianglesUInt32Array(mesh: DracoMesh, bytes: number, pointer: number): boolean;
}
interface DracoModule {
  Decoder: new () => DracoDecoder;
  Mesh: new () => DracoMesh;
  PointCloud: new () => DracoMesh;
  DT_FLOAT32: number;
  HEAPF32: Float32Array;
  HEAPU32: Uint32Array;
  _malloc(bytes: number): number;
  _free(pointer: number): void;
  destroy(object: object): void;
}

// ── Basis Universal transcoder ──

interface KTX2File {
  isValid(): boolean;
  getLevels(): number;
  getHasAlpha(): boolean;
  startTranscoding(): boolean;
  getImageLevelInfo(level: number, layer: number, face: number): { origWidth: number; origHeight: number };
  getImageTranscodedSizeInBytes(level: number, layer: number, face: number, format: number): number;
  transcodeImage(
    target: Uint8Array,
    level: number,
    layer: number,
    face: number,
    format: number,
    alphaForOpaque: number,
    channel0: number,
    channel1: number,
  ): boolean;
  close(): void;
  delete(): void;
}
interface BasisModule {
  initializeBasis(): void;
  KTX2File: new (data: Uint8Array) => KTX2File;
}

type Factory<T> = (options: { wasmBinary: ArrayBuffer }) => Promise<T>;

/** Loads an Emscripten factory published as a classic script (module workers have no importScripts); needs
 * `'unsafe-eval'` under a Content Security Policy (see `DecoderUrls`) */
async function factory<T>(url: string, name: string): Promise<Factory<T>> {
  const source = await (await fetch(url)).text();
  return new Function(`${source}\nreturn ${name};`)() as Factory<T>;
}

async function instantiate<T>(base: string, script: string, wasm: string, name: string): Promise<T> {
  const [create, binary] = await Promise.all([
    factory<T>(`${base}${script}`, name),
    fetch(`${base}${wasm}`).then((r) => r.arrayBuffer()),
  ]);
  return create({ wasmBinary: binary });
}

let draco: Promise<DracoModule> | undefined;
/** The full Draco decoder: the glTF build decodes meshes only, not the point clouds */
let dracoPoints: Promise<DracoModule> | undefined;
let basis: Promise<BasisModule> | undefined;

/** Decodes the Draco primitives of a tile (their points alone for point clouds) */
export async function decodeDraco(urls: DecoderUrls, requests: DracoRequest[]): Promise<DracoGeometry[]> {
  const meshes = requests.some((r) => !r.points);
  if (meshes) {
    draco ??= instantiate<DracoModule>(
      urls.draco,
      'draco_wasm_wrapper_gltf.js',
      'draco_decoder_gltf.wasm',
      'DracoDecoderModule',
    );
  }
  if (requests.some((r) => r.points)) {
    dracoPoints ??= instantiate<DracoModule>(
      urls.draco,
      'draco_wasm_wrapper.js',
      'draco_decoder.wasm',
      'DracoDecoderModule',
    );
  }
  const [forMeshes, forPoints] = await Promise.all([meshes ? draco : undefined, dracoPoints]);
  return requests.map((request) => {
    const module = (request.points ? forPoints : forMeshes) as DracoModule;
    const decoder = new module.Decoder();
    const mesh = request.points ? new module.PointCloud() : new module.Mesh();
    try {
      const data = new Int8Array(request.data.buffer, request.data.byteOffset, request.data.byteLength);
      const status = request.points
        ? decoder.DecodeArrayToPointCloud(data, data.byteLength, mesh)
        : decoder.DecodeArrayToMesh(data, data.byteLength, mesh);
      if (!status.ok()) throw new Error(`Draco: ${status.error_msg()}`);
      const floats = (id: number): Float32Array => {
        const attribute = decoder.GetAttributeByUniqueId(mesh, id);
        const count = mesh.num_points() * attribute.num_components();
        const pointer = module._malloc(count * 4);
        try {
          decoder.GetAttributeDataArrayForAllPoints(mesh, attribute, module.DT_FLOAT32, count * 4, pointer);
          return module.HEAPF32.slice(pointer / 4, pointer / 4 + count);
        } finally {
          module._free(pointer);
        }
      };
      if (request.points) {
        const geometry: DracoGeometry = {
          mesh: request.mesh,
          primitive: request.primitive,
          positions: floats(request.position),
        };
        if (request.color !== undefined) geometry.colors = floats(request.color);
        if (request.normal !== undefined) geometry.normals = floats(request.normal);
        return geometry;
      }
      const count = mesh.num_faces() * 3;
      const pointer = module._malloc(count * 4);
      let indices: Uint32Array;
      try {
        decoder.GetTrianglesUInt32Array(mesh, count * 4, pointer);
        indices = module.HEAPU32.slice(pointer / 4, pointer / 4 + count);
      } finally {
        module._free(pointer);
      }
      const geometry: DracoGeometry = {
        mesh: request.mesh,
        primitive: request.primitive,
        positions: floats(request.position),
        indices,
      };
      if (request.uv !== undefined) geometry.uvs = floats(request.uv);
      return geometry;
    } finally {
      module.destroy(mesh);
      module.destroy(decoder);
    }
  });
}

/** Basis target format and WebGL internal format, best first */
function target(formats: TextureFormats, alpha: boolean): { basis: number; gl: number } {
  if (formats.astc) return { basis: 10, gl: 0x93b0 }; // ASTC 4×4
  if (formats.bptc) return { basis: 6, gl: 0x8e8c }; // BC7
  if (formats.etc) return alpha ? { basis: 1, gl: 0x9278 } : { basis: 0, gl: 0x9274 }; // ETC2 RGBA / ETC1 as ETC2 RGB
  if (formats.s3tc) return alpha ? { basis: 3, gl: 0x83f3 } : { basis: 2, gl: 0x83f0 }; // BC3 / BC1
  return { basis: 13, gl: 0x8058 }; // RGBA8, uncompressed
}

/** Transcodes a KTX2 image for the GPU */
export async function transcodeKtx2(
  urls: DecoderUrls,
  formats: TextureFormats,
  bytes: Uint8Array,
): Promise<CompressedTexture> {
  basis ??= instantiate<BasisModule>(urls.basis, 'basis_transcoder.js', 'basis_transcoder.wasm', 'BASIS').then((m) => {
    m.initializeBasis();
    return m;
  });
  const module = await basis;
  const file = new module.KTX2File(bytes);
  try {
    if (!file.isValid() || !file.startTranscoding()) throw new Error('KTX2: invalid file');
    const { basis: format, gl } = target(formats, file.getHasAlpha());
    const levels: CompressedTexture['levels'] = [];
    for (let level = 0; level < file.getLevels(); level++) {
      const { origWidth: width, origHeight: height } = file.getImageLevelInfo(level, 0, 0);
      const data = new Uint8Array(file.getImageTranscodedSizeInBytes(level, 0, 0, format));
      if (!file.transcodeImage(data, level, 0, 0, format, 0, -1, -1)) throw new Error('KTX2: transcoding failed');
      levels.push({ width, height, data });
    }
    return { format: gl, levels };
  } finally {
    file.close();
    file.delete();
  }
}
