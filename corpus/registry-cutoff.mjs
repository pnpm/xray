// The install corpora resolve without a committed lockfile, so every transitive
// dependency would otherwise float to whatever the registry serves today, and
// the tests then run that code (tsc, vite, esbuild, node scripts). Resolving
// against a fixed point in time keeps the graph reproducible and keeps versions
// published after the cutoff, including compromised ones, out of CI.
//
// pnpm has no absolute "published before" setting, so the cutoff is expressed
// as a `minimumReleaseAge` equal to the minutes elapsed since it. Every pinned
// direct dependency must have been published before REGISTRY_CUTOFF; move the
// cutoff forward when adding or bumping a pin.
export const REGISTRY_CUTOFF = '2026-10-31T00:00:00Z'

export function minimumReleaseAge(now = Date.now()) {
  const minutes = Math.ceil((now - Date.parse(REGISTRY_CUTOFF)) / 60_000)
  if (!(minutes > 0)) throw new Error(`registry cutoff ${REGISTRY_CUTOFF} is in the future`)
  return minutes
}
