// SPDX-License-Identifier: Apache-2.0

/**
 * Minimal SCALE codec for the file-system format types (`DirectoryNode`,
 * `FileManifest`) of `crates/primitives/file-system`. Covers only what those
 * types use: compact lengths, `u8` enum indices, `u32`/`u64` little-endian,
 * `H256`, and bounded byte vectors. Bounds are checked on encode and decode.
 */

/** Thrown when bytes are not a valid encoding or a value breaks a bound. */
export class ScaleError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ScaleError";
  }
}

const U32_MAX = 0xffff_ffff;
const U64_MAX = (1n << 64n) - 1n;

/** Appends SCALE-encoded values to a growing buffer. */
export class ScaleWriter {
  private buf = new Uint8Array(256);
  private len = 0;

  private reserve(n: number): void {
    if (this.len + n <= this.buf.length) return;
    let size = this.buf.length * 2;
    while (size < this.len + n) size *= 2;
    const next = new Uint8Array(size);
    next.set(this.buf.subarray(0, this.len));
    this.buf = next;
  }

  /** Raw bytes, no length prefix. */
  raw(bytes: Uint8Array): this {
    this.reserve(bytes.length);
    this.buf.set(bytes, this.len);
    this.len += bytes.length;
    return this;
  }

  u8(value: number): this {
    if (!Number.isInteger(value) || value < 0 || value > 0xff) throw new ScaleError(`u8 out of range: ${value}`);
    return this.raw(Uint8Array.of(value));
  }

  u32(value: number): this {
    if (!Number.isInteger(value) || value < 0 || value > U32_MAX) throw new ScaleError(`u32 out of range: ${value}`);
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, value, true);
    return this.raw(out);
  }

  u64(value: bigint): this {
    if (value < 0n || value > U64_MAX) throw new ScaleError(`u64 out of range: ${value}`);
    const out = new Uint8Array(8);
    new DataView(out.buffer).setBigUint64(0, value, true);
    return this.raw(out);
  }

  /** Compact-encoded integer, limited to the `u32` range (all lengths here). */
  compact(value: number): this {
    if (!Number.isInteger(value) || value < 0 || value > U32_MAX) {
      throw new ScaleError(`compact out of range: ${value}`);
    }
    if (value < 1 << 6) return this.u8(value << 2);
    if (value < 1 << 14) {
      const v = (value << 2) | 0b01;
      return this.raw(Uint8Array.of(v & 0xff, v >> 8));
    }
    if (value < 1 << 30) {
      const out = new Uint8Array(4);
      new DataView(out.buffer).setUint32(0, ((value << 2) | 0b10) >>> 0, true);
      return this.raw(out);
    }
    // Big-integer mode: 4 value bytes, so the prefix is `(4 - 4) << 2 | 0b11`.
    this.u8(0b11);
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, value, true);
    return this.raw(out);
  }

  h256(value: Uint8Array): this {
    if (value.length !== 32) throw new ScaleError(`H256 must be 32 bytes, got ${value.length}`);
    return this.raw(value);
  }

  /** `BoundedVec<u8, max>`: compact length, then the bytes. */
  bytes(value: Uint8Array, max: number, field: string): this {
    if (value.length > max) throw new ScaleError(`${field} is ${value.length} bytes, max ${max}`);
    return this.compact(value.length).raw(value);
  }

  /** `BoundedVec<T, max>`: compact length, then each item. */
  vec<T>(items: readonly T[], max: number, field: string, item: (w: this, v: T) => void): this {
    if (items.length > max) throw new ScaleError(`${field} has ${items.length} items, max ${max}`);
    this.compact(items.length);
    for (const v of items) item(this, v);
    return this;
  }

  finish(): Uint8Array {
    return this.buf.slice(0, this.len);
  }
}

/** Reads SCALE-encoded values from a byte array. */
export class ScaleReader {
  private pos = 0;

  constructor(private readonly input: Uint8Array) {}

  private take(n: number): Uint8Array {
    if (this.pos + n > this.input.length) {
      throw new ScaleError(`unexpected end of input: need ${n} bytes at offset ${this.pos}`);
    }
    const out = this.input.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  private view(n: number): DataView {
    const b = this.take(n);
    return new DataView(b.buffer, b.byteOffset, n);
  }

  u8(): number {
    return this.take(1)[0];
  }

  u32(): number {
    return this.view(4).getUint32(0, true);
  }

  u64(): bigint {
    return this.view(8).getBigUint64(0, true);
  }

  /** Compact integer in the `u32` range. Rejects non-canonical encodings. */
  compact(): number {
    const first = this.input[this.pos];
    if (first === undefined) throw new ScaleError(`unexpected end of input at offset ${this.pos}`);
    switch (first & 0b11) {
      case 0b00:
        return this.u8() >> 2;
      case 0b01: {
        const v = this.view(2).getUint16(0, true) >> 2;
        if (v < 1 << 6) throw new ScaleError("non-canonical compact encoding");
        return v;
      }
      case 0b10: {
        const v = this.view(4).getUint32(0, true) >>> 2;
        if (v < 1 << 14) throw new ScaleError("non-canonical compact encoding");
        return v;
      }
      default: {
        if (first !== 0b11) throw new ScaleError("compact value exceeds u32");
        this.u8();
        const v = this.u32();
        if (v < 1 << 30) throw new ScaleError("non-canonical compact encoding");
        return v;
      }
    }
  }

  h256(): Uint8Array {
    return this.take(32).slice();
  }

  bytes(max: number, field: string): Uint8Array {
    const len = this.compact();
    if (len > max) throw new ScaleError(`${field} is ${len} bytes, max ${max}`);
    return this.take(len).slice();
  }

  vec<T>(max: number, field: string, item: (r: this) => T): T[] {
    const len = this.compact();
    if (len > max) throw new ScaleError(`${field} has ${len} items, max ${max}`);
    const out: T[] = [];
    for (let i = 0; i < len; i++) out.push(item(this));
    return out;
  }

  /** Throw unless every input byte was consumed (`decode_all`). */
  end(): void {
    if (this.pos !== this.input.length) {
      throw new ScaleError(`${this.input.length - this.pos} trailing bytes after the value`);
    }
  }
}
