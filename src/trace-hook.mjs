// Preloaded into Node processes spawned by xray. Resolution failures can be
// caught by the application, so these are observations, not proven breakages.
import * as nodeModule from 'node:module';
import { appendFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

let customResolutions = 0;
const recorded = new WeakMap();
function record(specifier, parent, error, customResolution) {
  if (!['MODULE_NOT_FOUND', 'ERR_MODULE_NOT_FOUND', 'ERR_PACKAGE_PATH_NOT_EXPORTED',
        'ERR_PACKAGE_IMPORT_NOT_DEFINED'].includes(error?.code) || typeof parent !== 'string') return;
  try {
    const identity = JSON.stringify([specifier, parent, customResolution]);
    if (recorded.get(error)?.has(identity) ||
        (!customResolution && recorded.get(error)?.has(JSON.stringify([specifier, parent, true])))) return;
    appendFileSync(process.env.XRAY_TRACE_FILE, JSON.stringify({ specifier, parent, code: error.code,
      message: String(error.message), customResolution }) + '\n');
    if (!recorded.has(error)) recorded.set(error, new Set());
    recorded.get(error).add(identity);
  } catch {
    // Observability must not change the failure the program receives.
  }
}
if (typeof nodeModule.registerHooks !== 'function') {
  console.error('xray: runtime tracing requires Node.js 22.15 or newer');
} else {
// The public resolution hook omits require.resolve's options. Mark explicit
// options before Node enters the hook so a foreign search path is never
// mistaken for a dependency that linking inside the caller would repair.
// Node 24 passes normalized options to the legacy resolver; Node 22 can bypass
// the public hook for custom paths altogether. Inspect property descriptors
// without invoking getters and forward the original arguments unchanged.
const Module = nodeModule.default;
const resolveFilename = Module._resolveFilename;
Module._resolveFilename = function (...args) {
  const descriptor = args[3] && Object.getOwnPropertyDescriptor(args[3], 'paths');
  const custom = descriptor && (!('value' in descriptor) || descriptor.value !== undefined);
  if (custom) customResolutions++;
  try { return Reflect.apply(resolveFilename, this, args); }
  catch (error) {
    if (custom) record(args[0], args[1] && Object.getOwnPropertyDescriptor(args[1], 'filename')?.value, error, true);
    throw error;
  }
  finally { if (custom) customResolutions--; }
};
nodeModule.registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      if (typeof context.parentURL === 'string' && context.parentURL.startsWith('file:')) {
        try {
          const parent = fileURLToPath(context.parentURL);
          record(specifier, parent, error, customResolutions > 0);
        } catch {
          // Observability must not change the failure the program receives.
        }
      }
      throw error;
    }
  },
});
}
