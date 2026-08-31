import { describe, it, expect } from 'vitest';
import { repoGroupQualifiers } from './repoGroupLabels';

describe('repoGroupQualifiers', () => {
  it('leaves a unique label unqualified', () => {
    const qualifiers = repoGroupQualifiers([
      { path: '/Users/dev/Code/sightmesh', label: 'sightmesh' },
      { path: '/Users/dev/Code/catapult-games', label: 'catapult-games' },
    ]);

    expect(qualifiers.size).toBe(0);
  });

  it('disambiguates the four live catapult-games checkouts by parent folder', () => {
    // Captured from the live database: four repo rows, one label, four paths.
    const qualifiers = repoGroupQualifiers([
      {
        path: '/Users/clarkpeng/Documents/Code/catapult-games',
        label: 'catapult-games',
      },
      {
        path: '/Users/clarkpeng/.local/share/sightmesh/.cdesktop-workspaces/329a-recorder-program/catapult-games',
        label: 'catapult-games',
      },
      {
        path: '/Users/clarkpeng/.local/share/sightmesh/.cdesktop-workspaces/aa43-recorder-server/catapult-games',
        label: 'catapult-games',
      },
      {
        path: '/Users/clarkpeng/.local/share/sightmesh/.cdesktop-workspaces/fdbf-recorder-program/catapult-games',
        label: 'catapult-games',
      },
    ]);

    expect([...qualifiers.values()]).toEqual([
      'Code',
      '329a-recorder-program',
      'aa43-recorder-server',
      'fdbf-recorder-program',
    ]);
  });

  it('qualifies only the labels that actually collide', () => {
    const qualifiers = repoGroupQualifiers([
      { path: '/a/one/repo', label: 'repo' },
      { path: '/a/two/repo', label: 'repo' },
      { path: '/a/one/solo', label: 'solo' },
    ]);

    expect(qualifiers.get('/a/one/repo')).toBe('one');
    expect(qualifiers.get('/a/two/repo')).toBe('two');
    expect(qualifiers.has('/a/one/solo')).toBe(false);
  });

  it('walks further up when the immediate parent also collides', () => {
    const qualifiers = repoGroupQualifiers([
      { path: '/checkouts/alpha/src/repo', label: 'repo' },
      { path: '/checkouts/beta/src/repo', label: 'repo' },
    ]);

    expect(qualifiers.get('/checkouts/alpha/src/repo')).toBe('alpha/src');
    expect(qualifiers.get('/checkouts/beta/src/repo')).toBe('beta/src');
  });

  it('handles a nested path that shares its parent name with a shallower one', () => {
    const qualifiers = repoGroupQualifiers([
      { path: '/work/repo', label: 'repo' },
      { path: '/mirror/work/repo', label: 'repo' },
    ]);

    expect(qualifiers.get('/work/repo')).toBe('work');
    expect(qualifiers.get('/mirror/work/repo')).toBe('mirror/work');
  });

  it('splits Windows-style separators', () => {
    const qualifiers = repoGroupQualifiers([
      { path: 'C:\\src\\one\\repo', label: 'repo' },
      { path: 'C:\\src\\two\\repo', label: 'repo' },
    ]);

    expect(qualifiers.get('C:\\src\\one\\repo')).toBe('one');
    expect(qualifiers.get('C:\\src\\two\\repo')).toBe('two');
  });
});
