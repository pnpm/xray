#!/usr/bin/env node
// Explicit maintenance command: emit a reviewable candidate, never write it.
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { REGISTRY_PINS, cutoffForPins, validateRegistryPins } from './registry-cutoff.mjs'

export function splitExactPin(input) {
  const separator = input.lastIndexOf('@')
  if (separator <= 0) throw new Error(`expected an exact package@version: ${input}`)
  const pin = { name: input.slice(0, separator), version: input.slice(separator + 1), publishedAt: '2000-01-01T00:00:00.000Z', integrity: `sha512-${Buffer.alloc(64).toString('base64')}` }
  validateRegistryPins({ schemaVersion: 1, pins: [pin] })
  return { name: pin.name, version: pin.version }
}

export async function fetchPublishedPin(identity, fetcher = fetch) {
  const response = await fetcher(`https://registry.npmjs.org/${encodeURIComponent(identity.name)}`, {
    redirect: 'error', signal: AbortSignal.timeout(60_000), headers: { accept: 'application/json' },
  })
  if (!response.ok) throw new Error(`registry metadata unavailable for ${identity.name}: HTTP ${response.status}`)
  const metadata = await response.json()
  const version = metadata.versions?.[identity.version]
  if (metadata.name !== identity.name || version?.name !== identity.name || version?.version !== identity.version) throw new Error(`registry identity mismatch for ${identity.name}@${identity.version}`)
  const pin = { ...identity, publishedAt: metadata.time?.[identity.version], integrity: version.dist?.integrity }
  validateRegistryPins({ schemaVersion: 1, pins: [pin] })
  return pin
}

export async function candidatePins(arguments_, fetcher = fetch) {
  if (arguments_.length === 0) throw new Error('provide one or more exact package@version pins')
  const pins = REGISTRY_PINS.map((pin) => ({ ...pin }))
  const seen = new Set(pins.map((pin) => `${pin.name}@${pin.version}`))
  for (const argument of arguments_) {
    const identity = splitExactPin(argument)
    const id = `${identity.name}@${identity.version}`
    if (seen.has(id)) throw new Error(`registry pin already exists: ${id}`)
    seen.add(id)
    pins.push(await fetchPublishedPin(identity, fetcher))
  }
  pins.sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0)
  validateRegistryPins({ schemaVersion: 1, pins })
  return { schemaVersion: 1, pins }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const candidate = await candidatePins(process.argv.slice(2))
    process.stdout.write(`${JSON.stringify(candidate, null, 2)}\n`)
    process.stderr.write(`Candidate cutoff ${cutoffForPins(candidate.pins)}; review metadata and commit the candidate explicitly.\n`)
  } catch (error) {
    process.stderr.write(`Could not prepare registry pins: ${error.message}\n`)
    process.exitCode = 1
  }
}
