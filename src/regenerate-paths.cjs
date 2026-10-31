// Rust canonical paths on Windows may use the verbatim prefix, while
// TypeScript and Node commonly return normal drive or UNC spellings.
const path = require('node:path');
const fs = require('node:fs');
function normalize(value, windows = process.platform === 'win32') {
  if (!windows) return path.normalize(value);
  const normalized = value.replaceAll('/', '\\');
  if (/^\\\\\?\\UNC\\/i.test(normalized)) return path.win32.normalize('\\\\' + normalized.slice(8));
  if (normalized.startsWith('\\\\?\\')) return path.win32.normalize(normalized.slice(4));
  return path.win32.normalize(normalized);
}
function key(value, windows = process.platform === 'win32') {
  if (value === undefined) return undefined;
  const result = normalize(value, windows);
  return windows ? result.toLowerCase() : result;
}
// The JavaScript realpath implementation can preserve Windows 8.3 path
// components. The native API resolves them to the canonical long spelling.
function canonical(value) {
  const input = normalize(value);
  const realpath = process.platform === 'win32' ? fs.realpathSync.native : fs.realpathSync;
  return normalize(realpath(input));
}
module.exports = { normalize, key, canonical };
