import { type ReactNode, useRef } from 'react';
import {
  ArrowBendDownLeftIcon,
  CheckIcon,
  PaperclipIcon,
  SpinnerIcon,
  XIcon,
} from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { Checkbox } from './Checkbox';
import { ChatBoxBase, VisualVariant, type DropzoneProps } from './ChatBoxBase';
import { DropdownMenuItem, DropdownMenuLabel } from './Dropdown';
import type { LocalAttachmentMetadata } from './WorkspaceContext';
import { ToolbarDropdown, ToolbarIconButton } from './Toolbar';

export interface EditorProps {
  value: string;
  onChange: (value: string) => void;
}

export interface ModelSelectorProps<TExecutorConfig = unknown> {
  onAdvancedSettings: () => void;
  presets: string[];
  selectedPreset: string | null;
  onPresetSelect: (presetId: string | null) => void;
  onOverrideChange: (partial: Partial<TExecutorConfig>) => void;
  executorConfig: TExecutorConfig | null;
  presetOptions: TExecutorConfig | null | undefined;
}

export interface ExecutorProps<TExecutor extends string = string> {
  selected: TExecutor | null;
  options: TExecutor[];
  onChange: (executor: TExecutor) => void;
}

export interface SaveAsDefaultProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  visible: boolean;
}

export interface LinkedIssueBadgeProps {
  simpleId: string;
  title: string;
  onRemove: () => void;
}

export interface CreateChatBoxEditorRenderProps<
  TExecutor extends string = string,
> {
  value: string;
  onChange: (value: string) => void;
  onCmdEnter: () => void;
  disabled: boolean;
  repoIds?: string[];
  repoId?: string;
  executor: TExecutor | null;
  onPasteFiles?: (files: File[]) => void;
  localAttachments?: LocalAttachmentMetadata[];
}

interface CreateChatBoxProps<TExecutor extends string = string> {
  editor: EditorProps;
  renderEditor: (props: CreateChatBoxEditorRenderProps<TExecutor>) => ReactNode;
  agentIcon?: ReactNode;
  onSend: () => void;
  isSending: boolean;
  disabled?: boolean;
  executor: ExecutorProps<TExecutor>;
  formatExecutorLabel?: (executor: TExecutor) => string;
  emptyExecutorLabel?: string;
  saveAsDefault?: SaveAsDefaultProps;
  error?: string | null;
  repoIds?: string[];
  repoId?: string;
  modelSelector?: ReactNode;
  /**
   * Model-selector controls that belong on the bottom-left (preset,
   * permission policy, agent / "profile"). Rendered ahead of the
   * paperclip in the footer-left toolbar.
   */
  modelSelectorLeft?: ReactNode;
  onPasteFiles?: (files: File[]) => void;
  localAttachments?: LocalAttachmentMetadata[];
  dropzone?: DropzoneProps;
  onEditRepos?: () => void;
  repoSummaryLabel?: string;
  repoSummaryTitle?: string;
  linkedIssue?: LinkedIssueBadgeProps | null;
  worktreeToggle?: ReactNode;
  /**
   * Chip row rendered above the editor (folder, branch, worktree, `+`).
   * When provided, the legacy repo-summary button + worktree checkbox in
   * the footer are suppressed — the chip row is the canonical UI for
   * editing those values.
   */
  chipRow?: ReactNode;
}

/**
 * Lightweight chat box for create mode.
 * Supports sending and attachments - no queue, stop, or feedback functionality.
 */
function defaultExecutorLabel(executor: string) {
  return executor
    .replace(/[_-]+/g, ' ')
    .toLowerCase()
    .replace(/\b\w/g, (char) => char.toUpperCase());
}

export function CreateChatBox<TExecutor extends string = string>({
  editor,
  renderEditor,
  agentIcon,
  onSend,
  isSending,
  disabled = false,
  executor,
  formatExecutorLabel = defaultExecutorLabel,
  emptyExecutorLabel = 'Select Executor',
  saveAsDefault,
  error,
  repoIds,
  repoId,
  modelSelector,
  modelSelectorLeft,
  onPasteFiles,
  localAttachments,
  dropzone,
  onEditRepos,
  repoSummaryLabel,
  repoSummaryTitle,
  linkedIssue,
  worktreeToggle,
  chipRow,
}: CreateChatBoxProps<TExecutor>) {
  const { t } = useTranslation(['common', 'tasks']);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const isDisabled = disabled || isSending;
  const canSend = editor.value.trim().length > 0 && !isDisabled;

  // Ref-dispatch so Lexical's listener always sees the freshest canSend/onSend (avoids stale-closure no-op on fast Enter).
  const latestCmdEnterRef = useRef<() => void>(() => {});
  latestCmdEnterRef.current = () => {
    if (canSend) {
      onSend();
    }
  };
  const handleCmdEnter = () => latestCmdEnterRef.current();

  const handleAttachClick = () => {
    fileInputRef.current?.click();
  };

  const handleFileInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = Array.from(e.target.files || []);
    if (files.length > 0 && onPasteFiles) {
      onPasteFiles(files);
    }
    e.target.value = '';
  };

  const executorLabel = executor.selected
    ? formatExecutorLabel(executor.selected)
    : emptyExecutorLabel;

  // Skip rendering the header row entirely when none of its slots have content.
  // Otherwise an empty `<></>` here counts as truthy and ChatBoxBase paints a
  // phantom row of vertical padding.
  const hasHeaderLeftContent =
    !!agentIcon || executor.options.length > 1 || saveAsDefault?.visible;

  return (
    <ChatBoxBase
      editor={renderEditor({
        value: editor.value,
        onChange: editor.onChange,
        onCmdEnter: handleCmdEnter,
        disabled: isDisabled,
        repoIds,
        repoId,
        executor: executor.selected ?? null,
        onPasteFiles,
        localAttachments,
      })}
      error={error}
      visualVariant={VisualVariant.NORMAL}
      dropzone={dropzone}
      modelSelector={modelSelector}
      chipRow={chipRow}
      headerLeft={
        hasHeaderLeftContent ? (
          <>
            {agentIcon}
            {executor.options.length > 1 && (
              <ToolbarDropdown label={executorLabel} disabled={isDisabled}>
                <DropdownMenuLabel>
                  {t('tasks:conversation.executors')}
                </DropdownMenuLabel>
                {executor.options.map((exec) => (
                  <DropdownMenuItem
                    key={exec}
                    icon={executor.selected === exec ? CheckIcon : undefined}
                    onClick={() => executor.onChange(exec)}
                  >
                    {formatExecutorLabel(exec)}
                  </DropdownMenuItem>
                ))}
              </ToolbarDropdown>
            )}
            {saveAsDefault?.visible && (
              <label className="flex items-center gap-1.5 text-sm text-low cursor-pointer ml-2">
                <Checkbox
                  checked={saveAsDefault.checked}
                  onCheckedChange={saveAsDefault.onChange}
                  className="h-3.5 w-3.5"
                  disabled={isDisabled}
                />
                <span>{t('tasks:conversation.saveAsDefault')}</span>
              </label>
            )}
          </>
        ) : undefined
      }
      footerLeft={
        <>
          {modelSelectorLeft}
          <ToolbarIconButton
            icon={PaperclipIcon}
            aria-label={t('tasks:taskFormDialog.attachFile')}
            title={t('tasks:taskFormDialog.attachFile')}
            onClick={handleAttachClick}
            disabled={isDisabled}
          />
          <input
            ref={fileInputRef}
            type="file"
            multiple
            className="hidden"
            onChange={handleFileInputChange}
          />
          {!chipRow && onEditRepos && repoSummaryLabel && (
            <button
              type="button"
              onClick={onEditRepos}
              title={repoSummaryTitle}
              disabled={isDisabled}
              className="max-w-[320px] truncate text-sm text-normal hover:text-high disabled:cursor-not-allowed disabled:opacity-50"
            >
              {repoSummaryLabel}
            </button>
          )}
          {!chipRow && worktreeToggle}
          {linkedIssue && (
            <>
              <div
                className="inline-flex items-center gap-half whitespace-nowrap text-sm text-low"
                title={linkedIssue.title}
              >
                <span className="font-mono text-xs text-normal">
                  {linkedIssue.simpleId}
                </span>
                <button
                  type="button"
                  onClick={linkedIssue.onRemove}
                  disabled={isDisabled}
                  className="inline-flex items-center text-low hover:text-error transition-colors disabled:cursor-not-allowed disabled:opacity-50"
                  aria-label={`Remove link to ${linkedIssue.simpleId}`}
                >
                  <XIcon className="size-icon-xs" weight="bold" />
                </button>
              </div>
            </>
          )}
        </>
      }
      editorOverlay={
        isSending ? (
          <SpinnerIcon
            weight="bold"
            className="size-icon-xs text-low animate-spin"
            aria-label={t('tasks:conversation.workspace.creating')}
          />
        ) : (
          <button
            type="button"
            onClick={onSend}
            disabled={!canSend}
            aria-label={t('tasks:conversation.workspace.create')}
            title={t('tasks:conversation.workspace.create')}
            className="block leading-none text-[hsl(0_0%_30%)] hover:text-normal disabled:cursor-not-allowed disabled:hover:text-[hsl(0_0%_30%)]"
          >
            <ArrowBendDownLeftIcon
              weight="bold"
              className="size-icon-xs"
              aria-hidden="true"
            />
          </button>
        )
      }
    />
  );
}
