const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
  0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
  0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
  0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

const H = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];

const W = new Uint32Array(64);

function compress(block: Uint8Array, out: Uint32Array): void {
  for (let i = 0; i < 16; i += 1) {
    const j = i * 4;
    W[i] = (((block[j] ?? 0) << 24) | ((block[j + 1] ?? 0) << 16) | ((block[j + 2] ?? 0) << 8) | (block[j + 3] ?? 0)) >>> 0;
  }
  for (let i = 16; i < 64; i += 1) {
    const a = W[i - 15] ?? 0;
    const b = W[i - 2] ?? 0;
    const s0 = ((a >>> 7) | (a << 25)) ^ ((a >>> 18) | (a << 14)) ^ (a >>> 3);
    const s1 = ((b >>> 17) | (b << 15)) ^ ((b >>> 19) | (b << 13)) ^ (b >>> 10);
    W[i] = ((W[i - 16] ?? 0) + s0 + (W[i - 7] ?? 0) + s1) >>> 0;
  }
  let a = out[0] ?? 0;
  let b = out[1] ?? 0;
  let c = out[2] ?? 0;
  let d = out[3] ?? 0;
  let e = out[4] ?? 0;
  let f = out[5] ?? 0;
  let g = out[6] ?? 0;
  let h = out[7] ?? 0;
  for (let i = 0; i < 64; i += 1) {
    const S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
    const ch = (e & f) ^ (~e & g);
    const t1 = (h + S1 + ch + (K[i] ?? 0) + (W[i] ?? 0)) >>> 0;
    const S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
    const maj = (a & b) ^ (a & c) ^ (b & c);
    const t2 = (S0 + maj) >>> 0;
    h = g;
    g = f;
    f = e;
    e = (d + t1) >>> 0;
    d = c;
    c = b;
    b = a;
    a = (t1 + t2) >>> 0;
  }
  out[0] = ((out[0] ?? 0) + a) >>> 0;
  out[1] = ((out[1] ?? 0) + b) >>> 0;
  out[2] = ((out[2] ?? 0) + c) >>> 0;
  out[3] = ((out[3] ?? 0) + d) >>> 0;
  out[4] = ((out[4] ?? 0) + e) >>> 0;
  out[5] = ((out[5] ?? 0) + f) >>> 0;
  out[6] = ((out[6] ?? 0) + g) >>> 0;
  out[7] = ((out[7] ?? 0) + h) >>> 0;
}

export function sha256(bytes: Uint8Array): Uint32Array {
  const total = Math.ceil((bytes.length + 9) / 64) * 64;
  const padded = new Uint8Array(total);
  padded.set(bytes);
  padded[bytes.length] = 0x80;
  const bitsLength = bytes.length * 8;
  padded[total - 4] = (bitsLength >>> 24) & 0xff;
  padded[total - 3] = (bitsLength >>> 16) & 0xff;
  padded[total - 2] = (bitsLength >>> 8) & 0xff;
  padded[total - 1] = bitsLength & 0xff;
  const out = new Uint32Array(H);
  for (let offset = 0; offset < total; offset += 64) {
    compress(padded.subarray(offset, offset + 64), out);
  }
  return out;
}

export function zeroBits(word: number, bits: number): boolean {
  if (bits <= 0) {
    return true;
  }
  if (bits >= 32) {
    return word === 0;
  }
  return word >>> (32 - bits) === 0;
}

export function workDone(seed: string, proof: string, bits: number): boolean {
  if (!/^\d{1,20}$/.test(proof) || bits > 32) {
    return false;
  }
  return zeroBits(sha256(new TextEncoder().encode(`${seed}:${proof}`))[0] ?? 1, bits);
}

const BATCH = 20_000;

export async function solveWork(seed: string, bits: number, signal?: AbortSignal): Promise<string> {
  const head = new TextEncoder().encode(`${seed}:`);
  if (head.length + 20 + 9 > 64 || bits > 32) {
    throw new Error('work');
  }
  const block = new Uint8Array(64);
  const out = new Uint32Array(8);
  block.set(head);
  for (let nonce = 0; nonce < Number.MAX_SAFE_INTEGER; nonce += 1) {
    if (nonce % BATCH === 0 && nonce > 0) {
      await new Promise((resolve) => setTimeout(resolve, 0));
      if (signal?.aborted) {
        throw new Error('aborted');
      }
    }
    const digits = String(nonce);
    let at = head.length;
    for (let i = 0; i < digits.length; i += 1) {
      block[at] = digits.charCodeAt(i);
      at += 1;
    }
    block[at] = 0x80;
    block.fill(0, at + 1, 56);
    const length = at * 8;
    block[56] = 0;
    block[57] = 0;
    block[58] = 0;
    block[59] = 0;
    block[60] = (length >>> 24) & 0xff;
    block[61] = (length >>> 16) & 0xff;
    block[62] = (length >>> 8) & 0xff;
    block[63] = length & 0xff;
    out.set(H);
    compress(block, out);
    if (zeroBits(out[0] ?? 1, bits)) {
      return digits;
    }
  }
  throw new Error('work');
}
