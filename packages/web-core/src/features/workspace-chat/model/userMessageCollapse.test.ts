import { describe, it, expect } from 'vitest';
import { resolveUserMessageCollapse } from './userMessageCollapse';

const SHORT = 'audit the diff';
const LONG = Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n');

describe('resolveUserMessageCollapse', () => {
  it('collapses a spawn-marked message behind a header regardless of length', () => {
    const result = resolveUserMessageCollapse('spawn', SHORT);

    expect(result.collapsible).toBe(true);
    expect(result.collapsesBehindHeader).toBe(true);
  });

  it('collapses spawn-marked messages whose text looks like an ordinary prompt', () => {
    // Same text, different marker: only the marker decides.
    expect(
      resolveUserMessageCollapse('spawn', SHORT).collapsesBehindHeader
    ).toBe(true);
    expect(
      resolveUserMessageCollapse('user', SHORT).collapsesBehindHeader
    ).toBe(false);
  });

  it('never collapses an unmarked message behind a header, however long', () => {
    const result = resolveUserMessageCollapse('user', LONG);

    expect(result.collapsible).toBe(true);
    expect(result.collapsesBehindHeader).toBe(false);
  });

  it('leaves short unmarked messages fully expanded', () => {
    const result = resolveUserMessageCollapse('user', SHORT);

    expect(result.collapsible).toBe(false);
    expect(result.collapsesBehindHeader).toBe(false);
  });

  it('treats a missing marker as a plain user prompt', () => {
    // Historical rows carry no marker and must render exactly as before.
    expect(resolveUserMessageCollapse(undefined, SHORT)).toEqual(
      resolveUserMessageCollapse('user', SHORT)
    );
  });
});
