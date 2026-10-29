// Minimal semver parsing/comparison for the update-check fallback path
// (`src/lib/stores/update.ts`), used when the app was built without the
// `updater` cargo feature and so falls back to comparing the running
// version against the tag of the latest GitHub release "by hand" instead of
// relying on `@tauri-apps/plugin-updater`'s own (Rust-side) comparison.
//
// Deliberately small: just enough of https://semver.org to order
// `major.minor.patch[-prerelease]` correctly, including comparing
// prerelease identifiers numerically where possible (semver's own rule).
// Build metadata (`+...`) is parsed but ignored for comparisons, per spec.

export interface Semver {
  major: number;
  minor: number;
  patch: number;
  /** Dot-separated prerelease identifiers, e.g. ["rc", "1"] for "-rc.1". Empty for a final release. */
  prerelease: string[];
}

const SEMVER_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-.]+))?(?:\+[0-9A-Za-z-.]+)?$/;

/** Parses a semver string (optionally with a leading "v", e.g. a git tag). Returns null if invalid. */
export function parseSemver(input: string): Semver | null {
  const stripped = input.startsWith("v") ? input.slice(1) : input;
  const m = SEMVER_RE.exec(stripped);
  if (!m) return null;
  return {
    major: Number(m[1]),
    minor: Number(m[2]),
    patch: Number(m[3]),
    prerelease: m[4] ? m[4].split(".") : [],
  };
}

function compareIdentifier(a: string, b: string): number {
  const na = /^\d+$/.test(a) ? Number(a) : null;
  const nb = /^\d+$/.test(b) ? Number(b) : null;
  // Numeric identifiers are compared numerically; numeric always sorts
  // lower than alphanumeric; otherwise lexically (semver.org rule 11).
  if (na !== null && nb !== null) return na - nb;
  if (na !== null) return -1;
  if (nb !== null) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** -1 if a < b, 0 if equal, 1 if a > b (ignoring build metadata), per semver precedence rules. */
export function compareSemver(a: Semver, b: Semver): number {
  if (a.major !== b.major) return a.major - b.major;
  if (a.minor !== b.minor) return a.minor - b.minor;
  if (a.patch !== b.patch) return a.patch - b.patch;

  const aHasPre = a.prerelease.length > 0;
  const bHasPre = b.prerelease.length > 0;
  // A version with a prerelease has lower precedence than the same
  // major.minor.patch without one (e.g. 1.0.0-rc.1 < 1.0.0).
  if (aHasPre && !bHasPre) return -1;
  if (!aHasPre && bHasPre) return 1;
  if (!aHasPre && !bHasPre) return 0;

  const len = Math.max(a.prerelease.length, b.prerelease.length);
  for (let i = 0; i < len; i++) {
    if (a.prerelease[i] === undefined) return -1;
    if (b.prerelease[i] === undefined) return 1;
    const c = compareIdentifier(a.prerelease[i], b.prerelease[i]);
    if (c !== 0) return c;
  }
  return 0;
}

/** True if `latest` is a newer version than `current`. Invalid input is treated as "not newer". */
export function isNewerVersion(current: string, latest: string): boolean {
  const a = parseSemver(current);
  const b = parseSemver(latest);
  if (!a || !b) return false;
  return compareSemver(b, a) > 0;
}
