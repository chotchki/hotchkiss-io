"use strict";
// Greylist toll worker (Phase CX). Runs the proof-of-work OFF the main thread: the image chain
// digest + the HMAC answer. The SHA-256 / HMAC / chain here are validated against the Rust
// server kernel's known-answer vector (see docs/greylist-challenge-design.md) — they MUST stay
// byte-for-byte identical, so do not "optimize" the algorithm without re-checking the KAT.
//
// A pure-JS synchronous SHA-256 is used deliberately (NOT crypto.subtle): ~188k sequential
// tiny hashes through WebCrypto's async per-call path would crawl.

const K = new Uint32Array([
  0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
  0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
  0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
  0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
  0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
  0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
  0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
  0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2
]);
function rotr(x, n) { return (x >>> n) | (x << (32 - n)); }

function sha256(msg) {
  const l = msg.length;
  const bitLen = l * 8;
  const withPad = (((l + 8) >> 6) + 1) << 6;
  const m = new Uint8Array(withPad);
  m.set(msg);
  m[l] = 0x80;
  const dv = new DataView(m.buffer);
  dv.setUint32(withPad - 8, Math.floor(bitLen / 0x100000000));
  dv.setUint32(withPad - 4, bitLen >>> 0);
  let h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a,
      h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19;
  const w = new Uint32Array(64);
  for (let off = 0; off < withPad; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4);
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i-15], 7) ^ rotr(w[i-15], 18) ^ (w[i-15] >>> 3);
      const s1 = rotr(w[i-2], 17) ^ rotr(w[i-2], 19) ^ (w[i-2] >>> 10);
      w[i] = (w[i-16] + s0 + w[i-7] + s1) | 0;
    }
    let a=h0,b=h1,c=h2,d=h3,e=h4,f=h5,g=h6,h=h7;
    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e,6) ^ rotr(e,11) ^ rotr(e,25);
      const ch = (e & f) ^ ((~e) & g);
      const t1 = (h + S1 + ch + K[i] + w[i]) | 0;
      const S0 = rotr(a,2) ^ rotr(a,13) ^ rotr(a,22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) | 0;
      h=g; g=f; f=e; e=(d+t1)|0; d=c; c=b; b=a; a=(t1+t2)|0;
    }
    h0=(h0+a)|0; h1=(h1+b)|0; h2=(h2+c)|0; h3=(h3+d)|0;
    h4=(h4+e)|0; h5=(h5+f)|0; h6=(h6+g)|0; h7=(h7+h)|0;
  }
  const out = new Uint8Array(32);
  const odv = new DataView(out.buffer);
  [h0,h1,h2,h3,h4,h5,h6,h7].forEach((hh, i) => odv.setUint32(i * 4, hh >>> 0));
  return out;
}

function concat(a, b) { const c = new Uint8Array(a.length + b.length); c.set(a); c.set(b, a.length); return c; }

function hmacSha256(key, msg) {
  const B = 64;
  let k = key;
  if (k.length > B) k = sha256(k);
  const kp = new Uint8Array(B); kp.set(k);
  const ipad = new Uint8Array(B), opad = new Uint8Array(B);
  for (let i = 0; i < B; i++) { ipad[i] = kp[i] ^ 0x36; opad[i] = kp[i] ^ 0x5c; }
  return sha256(concat(opad, sha256(concat(ipad, msg))));
}

// The slow image chain: h[i] = SHA256(pixel[i] ‖ h[i-1]), retain all, then SHA256 over the
// retained hashes in REVERSE order. Matches greylist::challenge::image_digest exactly.
function imageDigest(rgba, onProgress) {
  let h = new Uint8Array(32); // IV = 32 zero bytes
  const n = rgba.length >> 2;
  const store = new Uint8Array(n * 32);
  const buf = new Uint8Array(36);
  for (let i = 0; i < n; i++) {
    buf[0] = rgba[i*4]; buf[1] = rgba[i*4+1]; buf[2] = rgba[i*4+2]; buf[3] = rgba[i*4+3];
    buf.set(h, 4);
    h = sha256(buf);
    store.set(h, i * 32);
    if ((i & 4095) === 0) onProgress(i / n);
  }
  const rev = new Uint8Array(n * 32);
  for (let i = 0; i < n; i++) rev.set(store.subarray((n-1-i)*32, (n-i)*32), i * 32);
  return sha256(rev);
}

function b64url(bytes) {
  let s = "";
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

self.onmessage = (e) => {
  const { rgba, seed } = e.data; // both Uint8Array
  const clock = () => (typeof performance !== "undefined" && performance.now) ? performance.now() : Date.now();
  const t0 = clock();
  const digest = imageDigest(rgba, (frac) => self.postMessage({ type: "progress", pct: Math.floor(frac * 100) }));
  const answer = hmacSha256(seed, digest);
  self.postMessage({ type: "done", answer: b64url(answer), ms: Math.round(clock() - t0) });
};
