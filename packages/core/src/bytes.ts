/** Byte/hex primitives shared by every layer. */

export function toHex(bytes: Uint8Array | ArrayLike<number>): string {
  const arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  return "0x" + Array.from(arr, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function hexToBytes(hex: string): Uint8Array {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(h.substring(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/**
 * Normalize bytes or a hex string (with or without 0x) to a 0x-prefixed hex
 * string — the wire format PAPI v2 uses for fixed-size binary (`SizedHex`).
 */
export function asHex(input: string | Uint8Array | ArrayLike<number>): `0x${string}` {
  if (typeof input === "string") {
    return (input.startsWith("0x") ? input : `0x${input}`) as `0x${string}`;
  }
  return toHex(input) as `0x${string}`;
}

export function bytesEq(
  a: ArrayLike<number> | null | undefined,
  b: ArrayLike<number> | null | undefined,
): boolean {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  const CHUNK = 0x8000; // String.fromCharCode arg-count limit headroom
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
