import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
  PencilSimpleIcon,
  ArrowUUpLeftIcon,
  CaretDownIcon,
} from '@phosphor-icons/react';
import { Clipboard, Check } from 'lucide-react';
import { cn } from '../lib/cn';
import { Tooltip } from './Tooltip';

export interface ChatUserMessageRenderProps {
  content: string;
  workspaceId?: string;
}

interface ChatUserMessageProps {
  content: string;
  expanded?: boolean;
  onToggle?: () => void;
  className?: string;
  workspaceId?: string;
  onEdit?: () => void;
  onReset?: () => void;
  isGreyed?: boolean;
  /** When set, the message collapses behind this header instead of showing a
   *  clipped preview with a "show more" link. */
  collapsedLabel?: string;
  renderMarkdown: (props: ChatUserMessageRenderProps) => ReactNode;
}

export function ChatUserMessage({
  content,
  expanded = true,
  onToggle,
  className,
  workspaceId,
  onEdit,
  onReset,
  isGreyed,
  collapsedLabel,
  renderMarkdown,
}: ChatUserMessageProps) {
  const { t } = useTranslation('tasks');
  const [justCopied, setJustCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(content);
      setJustCopied(true);
      window.setTimeout(() => setJustCopied(false), 1500);
    } catch {
      // clipboard may be unavailable (e.g. permission denied); silently ignore
    }
  };

  const collapsible = Boolean(onToggle);
  const showActions = !isGreyed;

  return (
    <div
      className={cn(
        'group/user-msg flex flex-col items-start gap-half',
        isGreyed && 'opacity-50 pointer-events-none',
        className
      )}
    >
      <div
        className={cn(
          'max-w-[75%] rounded-xl rounded-bl-sm px-double py-base text-left',
          'bg-accent-blue text-accent-blue-foreground'
        )}
      >
        {collapsedLabel ? (
          <>
            <button
              type="button"
              onClick={onToggle}
              aria-expanded={expanded}
              className="flex items-center gap-half text-xs opacity-80 hover:opacity-100"
            >
              <CaretDownIcon
                className={cn(
                  'size-icon-xs transition-transform',
                  !expanded && '-rotate-90'
                )}
              />
              <span>{collapsedLabel}</span>
            </button>
            {expanded && (
              <div className="mt-half">
                {renderMarkdown({ content, workspaceId })}
              </div>
            )}
          </>
        ) : (
          <>
            <div className={cn(!expanded && 'max-h-[140px] overflow-hidden')}>
              {renderMarkdown({ content, workspaceId })}
            </div>
            {collapsible && (
              <button
                type="button"
                onClick={onToggle}
                className="mt-half text-xs opacity-80 hover:underline hover:opacity-100"
              >
                {expanded
                  ? t('conversation.showLess')
                  : t('conversation.showMore')}
              </button>
            )}
          </>
        )}
      </div>

      {showActions && (
        <div
          className={cn(
            'flex items-center gap-half opacity-0 transition-opacity',
            'group-hover/user-msg:opacity-100 group-focus-within/user-msg:opacity-100'
          )}
        >
          {onReset && (
            <Tooltip content={t('conversation.actions.resetTooltip')}>
              <button
                type="button"
                onClick={onReset}
                className="rounded p-1 text-low transition-colors hover:bg-muted hover:text-normal"
                aria-label={t('conversation.actions.reset')}
              >
                <ArrowUUpLeftIcon className="size-icon-xs" />
              </button>
            </Tooltip>
          )}
          {onEdit && (
            <Tooltip content={t('conversation.actions.edit')}>
              <button
                type="button"
                onClick={onEdit}
                className="rounded p-1 text-low transition-colors hover:bg-muted hover:text-normal"
                aria-label={t('conversation.actions.edit')}
              >
                <PencilSimpleIcon className="size-icon-xs" />
              </button>
            </Tooltip>
          )}
          <Tooltip
            content={
              justCopied
                ? t('conversation.actions.copied')
                : t('conversation.actions.copy')
            }
          >
            <button
              type="button"
              onClick={handleCopy}
              className="rounded p-1 text-low transition-colors hover:bg-muted hover:text-normal"
              aria-label={t('conversation.actions.copy')}
            >
              {justCopied ? (
                <Check className="size-icon-xs" />
              ) : (
                <Clipboard className="size-icon-xs" />
              )}
            </button>
          </Tooltip>
        </div>
      )}
    </div>
  );
}
