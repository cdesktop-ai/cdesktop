import type { PromptKind } from 'shared/types';

/** Plain prompts only get a "show more" affordance once they are this tall. */
const LONG_PROMPT_LINE_THRESHOLD = 20;

export interface UserMessageCollapse {
  /** Whether the message renders a collapse affordance at all. */
  readonly collapsible: boolean;
  /** Whether the collapsed form hides the body behind a labelled header. */
  readonly collapsesBehindHeader: boolean;
}

/**
 * Collapse behaviour is decided by the marker the backend recorded on the
 * message, never by what the message says. Prompts without a marker keep the
 * length-based "show more" behaviour they have always had.
 */
export function resolveUserMessageCollapse(
  promptKind: PromptKind | undefined,
  content: string
): UserMessageCollapse {
  if (promptKind === 'spawn') {
    return { collapsible: true, collapsesBehindHeader: true };
  }

  return {
    collapsible: content.split('\n').length > LONG_PROMPT_LINE_THRESHOLD,
    collapsesBehindHeader: false,
  };
}
