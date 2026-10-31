// Preloaded into Node processes spawned by xray. Resolution failures can be
// caught by the application, so these are observations, not proven breakages.
import * as nodeModule from 'node:module';
import { appendFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

if (typeof nodeModule.registerHooks !== 'function') {
  console.error('xray: runtime tracing requires Node.js 22.15 or newer');
} else nodeModule.registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      if ((error?.code === 'MODULE_NOT_FOUND' || error?.code === 'ERR_MODULE_NOT_FOUND') &&
          typeof context.parentURL === 'string' && context.parentURL.startsWith('file:')) {
        try {
          const parent = fileURLToPath(context.parentURL);
          appendFileSync(process.env.XRAY_TRACE_FILE, JSON.stringify({ specifier, parent }) + '\n');
        } catch {
          // Observability must not change the failure the program receives.
        }
      }
      throw error;
    }
  },
});
