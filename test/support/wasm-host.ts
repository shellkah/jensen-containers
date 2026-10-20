import { readFileSync } from "node:fs";
import { isAbsolute, relative, resolve } from "node:path";

/** What a backend method answers: the envelope's `ok` value or its `error` message. */
export interface BackendReply {
  ok?: unknown;
  error?: string;
}

export interface LoadedBackend {
  call(method: string, input: unknown): BackendReply;
  /** Every path the backend asked `jensen.fs_read` for, in order, answered or not. */
  reads: string[];
  /** Every line the backend logged. */
  logs: string[];
}

interface Exports {
  memory: WebAssembly.Memory;
  jensen_alloc(len: number): number;
  jensen_free(ptr: number, len: number): void;
  jensen_call(methodPtr: number, methodLen: number, inputPtr: number, inputLen: number): bigint;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/**
 * Plays Jensen's side of the backend ABI: it instantiates `wasmPath`, answers `jensen.fs_read` from
 * `root` the way the plugin's `*.yaml`/`*.yml` scope would, and records reads and logs. Writing or
 * deleting a file throws.
 */
export async function loadBackend(wasmPath: string, root: string): Promise<LoadedBackend> {
  const reads: string[] = [];
  const logs: string[] = [];
  const rootPath = resolve(root);
  let exports: Exports | undefined;

  const wasm = (): Exports => {
    if (!exports) throw new Error("the backend is not instantiated yet");
    return exports;
  };
  // Memory grows during a call, so every access takes a fresh view.
  const bytes = (ptr: number, len: number) => new Uint8Array(wasm().memory.buffer, ptr, len);
  const text = (ptr: number, len: number) => decoder.decode(bytes(ptr, len).slice());
  const give = (data: Uint8Array, outPtr: number, outLen: number) => {
    const ptr = wasm().jensen_alloc(data.length);
    bytes(ptr, data.length).set(data);
    const view = new DataView(wasm().memory.buffer);
    view.setInt32(outPtr, ptr, true);
    view.setInt32(outLen, data.length, true);
  };

  const imports = {
    jensen: {
      fs_read(pathPtr: number, pathLen: number, outPtr: number, outLen: number): number {
        const path = text(pathPtr, pathLen);
        reads.push(path);
        const full = resolve(rootPath, path);
        const inside = relative(rootPath, full);
        const refusal =
          !/\.ya?ml$/.test(path) || isAbsolute(path) || inside.startsWith("..")
            ? `${path} is outside the plugin's file scopes`
            : undefined;
        if (refusal) {
          give(encoder.encode(refusal), outPtr, outLen);
          return 1;
        }
        try {
          give(readFileSync(full), outPtr, outLen);
          return 0;
        } catch (error) {
          give(
            encoder.encode(error instanceof Error ? error.message : String(error)),
            outPtr,
            outLen,
          );
          return 1;
        }
      },
      fs_write(): number {
        throw new Error("the backend must never write a file");
      },
      fs_delete(): number {
        throw new Error("the backend must never delete a file");
      },
      log(levelPtr: number, levelLen: number, textPtr: number, textLen: number): void {
        logs.push(`${text(levelPtr, levelLen)}: ${text(textPtr, textLen)}`);
      },
    },
  };

  const module = await WebAssembly.compile(readFileSync(wasmPath));
  const instance = await WebAssembly.instantiate(module, imports as WebAssembly.Imports);
  exports = instance.exports as unknown as Exports;

  return {
    reads,
    logs,
    call(method, input) {
      const put = (data: Uint8Array) => {
        const ptr = wasm().jensen_alloc(data.length);
        bytes(ptr, data.length).set(data);
        return { ptr, len: data.length };
      };
      const name = put(encoder.encode(method));
      const body = put(encoder.encode(JSON.stringify(input)));
      const packed = wasm().jensen_call(name.ptr, name.len, body.ptr, body.len);
      const outPtr = Number(packed >> 32n) >>> 0;
      const outLen = Number(packed & 0xffffffffn);
      const reply = JSON.parse(text(outPtr, outLen)) as BackendReply;
      wasm().jensen_free(outPtr, outLen);
      wasm().jensen_free(name.ptr, name.len);
      wasm().jensen_free(body.ptr, body.len);
      return reply;
    },
  };
}
