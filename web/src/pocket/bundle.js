// A CityIR bundle: named typed arrays and a JSON record in one file.
//
//   u32 'CIR1' | u32 length of the JSON | JSON, padded with spaces to 8 bytes | array data
//
// The JSON is { meta, arrays: { name: { type, offset, length } } }: `type` is the typed array's name, `offset` is in
// bytes from the start of the array data and `length` in elements. Every array starts on an 8-byte boundary.
export class Bundle {
  constructor(meta = {}) { this.meta = meta; this.arrays = {}; this.parts = []; this.size = 0; }

  add(name, array) {
    if (!array || !array.length) return this;
    const pad = (8 - (this.size % 8)) % 8;
    if (pad) { this.parts.push(new Uint8Array(pad)); this.size += pad; }
    this.arrays[name] = { type: array.constructor.name, offset: this.size, length: array.length };
    this.parts.push(new Uint8Array(array.buffer, array.byteOffset, array.byteLength));
    this.size += array.byteLength;
    return this;
  }

  blob() {
    let json = new TextEncoder().encode(JSON.stringify({ meta: this.meta, arrays: this.arrays }));
    const pad = (8 - ((8 + json.length) % 8)) % 8;
    if (pad) { const j = new Uint8Array(json.length + pad).fill(32); j.set(json); json = j; }
    const head = new Uint32Array([0x31524943, json.length]);
    return new Blob([head, json, ...this.parts]);
  }
}
