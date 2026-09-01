/**
 * Sidebar folder groups are keyed by repository path, because that is the
 * identity that actually differs between two local checkouts of the same
 * repository. Their labels are not: four checkouts of `catapult-games` render
 * four identical headers unless the colliding ones say where they live.
 */

const PATH_SEPARATOR = /[/\\]+/;

function segmentsOf(path: string): string[] {
  return path.split(PATH_SEPARATOR).filter(Boolean);
}

export interface RepoGroupIdentity {
  /** Repository path. Unique per group. */
  path: string;
  /** Label the group would render on its own. */
  label: string;
}

/**
 * Map each colliding group's path to the shortest trailing run of ancestor
 * folders that tells it apart from the others sharing its label. Groups whose
 * label is already unique are absent from the map.
 */
export function repoGroupQualifiers(
  groups: ReadonlyArray<RepoGroupIdentity>
): Map<string, string> {
  const byLabel = new Map<string, RepoGroupIdentity[]>();
  for (const group of groups) {
    const bucket = byLabel.get(group.label);
    if (bucket) {
      bucket.push(group);
    } else {
      byLabel.set(group.label, [group]);
    }
  }

  const qualifiers = new Map<string, string>();
  for (const bucket of byLabel.values()) {
    if (bucket.length < 2) continue;

    const segments = bucket.map((group) => segmentsOf(group.path));
    const deepest = Math.max(...segments.map((parts) => parts.length));

    // Widen the window one ancestor at a time and stop as soon as it separates
    // every path in the bucket. Paths are unique, so the full ancestor chain
    // always separates them.
    for (let depth = 1; depth <= deepest; depth += 1) {
      const suffixes = segments.map((parts) =>
        parts.slice(Math.max(0, parts.length - 1 - depth), -1).join('/')
      );
      if (new Set(suffixes).size !== suffixes.length) continue;

      bucket.forEach((group, index) => {
        if (suffixes[index]) qualifiers.set(group.path, suffixes[index]);
      });
      break;
    }
  }
  return qualifiers;
}
