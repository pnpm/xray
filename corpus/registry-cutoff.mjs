// Corpus installs intentionally resolve without committed lockfiles. Keep their
// version eligibility bounded by a reviewed, committed point in time. New pins
// must be added to registry-pins.json by an explicit reviewable update; running
// tests never fetches registry metadata or moves the boundary forward.
import { readFileSync } from 'node:fs'

const originalCutoff = '2026-10-31T00:00:00.000Z'
export const RUN_WINDOW_MS = 45 * 60_000
const packageName = /^(?:@[a-z0-9][a-z0-9._~-]*\/)?[a-z0-9][a-z0-9._~-]*$/
const exactVersion = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/

export function validateRegistryPins(document) {
  if (document?.schemaVersion !== 1 || !Array.isArray(document.pins) || document.pins.length === 0) throw new Error('invalid registry pin manifest')
  const seen = new Set()
  for (const pin of document.pins) {
    if (typeof pin?.name !== 'string' || !packageName.test(pin.name) || typeof pin.version !== 'string' || !exactVersion.test(pin.version)) throw new Error('invalid registry pin name or exact version')
    const id = `${pin.name}@${pin.version}`
    if (seen.has(id)) throw new Error(`duplicate registry pin ${id}`)
    seen.add(id)
    if (typeof pin.publishedAt !== 'string' || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(pin.publishedAt) || !Number.isFinite(Date.parse(pin.publishedAt)) || new Date(pin.publishedAt).toISOString() !== pin.publishedAt) throw new Error(`invalid publishedAt for ${id}`)
    if (typeof pin.integrity !== 'string' || !/^sha512-[A-Za-z0-9+/]{86}==$/.test(pin.integrity) || Buffer.from(pin.integrity.slice(7), 'base64').length !== 64 || Buffer.from(pin.integrity.slice(7), 'base64').toString('base64') !== pin.integrity.slice(7)) throw new Error(`invalid integrity for ${id}`)
  }
  return document.pins
}

// Preserve the existing boundary when all pins predate it; do not silently
// narrow existing transitive resolution. A run-window margin plus one minute for pnpm rounding keeps pins eligible at
// the beginning of a bounded run while its cutoff never advances during it.
export function cutoffForPins(pins) {
  const latest = Math.max(Date.parse(originalCutoff), ...pins.map((pin) => Date.parse(pin.publishedAt) + RUN_WINDOW_MS + 60_000))
  if (!Number.isFinite(latest)) throw new Error('invalid registry publication timestamp')
  return new Date(Math.ceil(latest / 60_000) * 60_000).toISOString()
}

const pinDocument = JSON.parse(readFileSync(new URL('./registry-pins.json', import.meta.url), 'utf8'))
export const REGISTRY_PINS = validateRegistryPins(pinDocument)
export const REGISTRY_CUTOFF = cutoffForPins(REGISTRY_PINS)
const allowedPins = new Set(REGISTRY_PINS.map(({ name, version }) => `${name}@${version}`))

// Install fixtures must use exact, reviewed direct pins. Return the original
// object so this can wrap a package.json dependency map at its point of use.
export function auditedDependencies(dependencies) {
  for (const [name, version] of Object.entries(dependencies)) {
    if (typeof version !== 'string' || !allowedPins.has(`${name}@${version}`)) throw new Error(`unaudited corpus dependency ${name}@${version}; update registry-pins.json explicitly`)
  }
  return dependencies
}

export function minimumReleaseAge(now = Date.now()) {
  const minutes = Math.ceil((now - Date.parse(REGISTRY_CUTOFF)) / 60_000)
  if (!(minutes > 0)) throw new Error(`registry cutoff ${REGISTRY_CUTOFF} is in the future`)
  return minutes
}

// A single fixture policy must cover all pnpm invocations, including those
// started by xray. Compute age at the deadline, then stop child processes by
// that deadline so their effective publication cutoff cannot move forward.
export function registryWindow(now = Date.now()) {
  if (now <= Date.parse(REGISTRY_CUTOFF)) throw new Error(`registry cutoff ${REGISTRY_CUTOFF} is in the future`)
  const deadline = now + RUN_WINDOW_MS
  return { deadline, releaseAge: minimumReleaseAge(deadline) }
}

export function remainingTime(deadline, maximum, now = Date.now()) {
  const remaining = Math.floor(deadline - now)
  if (remaining <= 0) throw new Error('corpus registry window expired')
  return Math.min(remaining, maximum)
}
