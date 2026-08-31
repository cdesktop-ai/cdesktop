/**
 * Spawn-teammate modal.
 *
 * Visually mirrors the existing-session composer: header + name/prompt
 * fields, then a control rail with executor chip + preset/permission
 * (left slot) + provider/model picker (right slot). Initial picker state
 * is shared with the caller workspace via `useWorkspacePickerSelection`.
 *
 * POSTs to `/api/workspaces/{id}/teammates` and surfaces structured
 * `TeammateError.code`s inline. See `plans/agent-teams-mvp.md` UI section.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useMutation } from '@tanstack/react-query';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/KeyboardDialog';
import { Input } from '@vibe/ui/components/Input';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';
import { CheckIcon } from '@phosphor-icons/react';
import { ModelSelectorContainer } from '@/shared/components/ModelSelectorContainer';
import { ProviderModelPicker } from '@/shared/components/ProviderModelPicker';
import { AgentIcon, getAgentName } from '@/shared/components/AgentIcon';
import { SettingsDialog } from '@/shared/dialogs/settings/SettingsDialog';
import { sessionsApi } from '@/shared/lib/api';
import { ApiError } from '@/shared/lib/api';
import { useExecutorConfig } from '@/shared/hooks/useExecutorConfig';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { useWorkspacePickerSelection } from '@/shared/hooks/useWorkspacePickerSelection';
import { isAgentDefaultModelId } from '@/shared/lib/agentDefaultModel';
import type {
  BaseCodingAgent,
  ExecutorConfig,
  ExecutorProfileId,
  SpawnTeammateRequest,
} from 'shared/types';

const MAX_NAME_LEN = 24;

export interface SpawnTeammateModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workspaceId: string;
  /** Caller's most recently used executor config; seeds the picker. */
  lastUsedConfig: ExecutorConfig | null;
  /** Stable executor identity to compute presets against (falls back to lastUsedConfig.executor). */
  configExecutorProfile?: ExecutorProfileId | null;
  /** Called after successful spawn with the new session id. */
  onSpawned?: (sessionId: string) => void;
}

const agentChipClassName =
  'inline-flex items-center gap-half rounded-md bg-secondary px-base py-half ' +
  'min-h-7 text-sm text-normal hover:bg-panel ' +
  'disabled:cursor-not-allowed disabled:opacity-50 ' +
  'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand';

function AgentChip({
  selected,
  options,
  onChange,
  disabled,
}: {
  selected: BaseCodingAgent | null;
  options: BaseCodingAgent[];
  onChange: (agent: BaseCodingAgent) => void;
  disabled?: boolean;
}) {
  if (options.length === 0) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className={agentChipClassName}
        >
          <AgentIcon agent={selected} className="h-[0.9rem] w-[0.9rem]" />
          <span className="max-w-[140px] truncate">
            {selected ? getAgentName(selected) : 'Agent'}
          </span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        {options.map((agent) => (
          <DropdownMenuItem
            key={agent}
            badge={selected === agent ? <CheckIcon weight="bold" /> : undefined}
            onSelect={() => onChange(agent)}
          >
            <span className="flex items-center gap-2">
              <AgentIcon agent={agent} className="h-[0.9rem] w-[0.9rem]" />
              <span>{getAgentName(agent)}</span>
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Map server-side `TeammateError.code` to a localized inline message. */
function translateTeammateError(
  t: (key: string) => string,
  err: ApiError<unknown>
): string {
  const code = parseCode(err.message);
  switch (code) {
    case 'PROVIDER_NOT_CONFIGURED':
      return t('conversation.team.errorProviderNotConfigured');
    case 'EXECUTOR_REQUIRES_PROVIDER':
      return t('conversation.team.errorExecutorRequiresProvider');
    case 'NAME_INVALID':
      return t('conversation.team.errorNameInvalid');
    case 'WORKSPACE_ARCHIVED':
      return t('conversation.team.errorWorkspaceArchived');
    default:
      return err.message || t('conversation.team.errorGeneric');
  }
}

function parseCode(message: string | undefined): string | null {
  if (!message) return null;
  const match = message.match(/(?:TeammateError:\s*)?([A-Z_]+):/);
  return match ? match[1] : null;
}

export function SpawnTeammateModal({
  open,
  onOpenChange,
  workspaceId,
  lastUsedConfig,
  configExecutorProfile,
  onSpawned,
}: SpawnTeammateModalProps) {
  const { t } = useTranslation(['tasks']);
  const { profiles } = useUserSystem();

  const [name, setName] = useState('');
  const [prompt, setPrompt] = useState('');
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const nameInputRef = useRef<HTMLInputElement>(null);

  // `autoFocus` on the input can lose to portal mount timing; re-focus on
  // every open so the name field is always the landing focus target.
  useEffect(() => {
    if (open) {
      requestAnimationFrame(() => nameInputRef.current?.focus());
    }
  }, [open]);

  const {
    executorConfig,
    effectiveExecutor,
    selectedVariant,
    executorOptions,
    variantOptions,
    presetOptions,
    setExecutor,
    setVariant,
    setOverrides,
  } = useExecutorConfig({
    profiles,
    lastUsedConfig,
    configExecutorProfile,
  });

  const {
    selectedProviderId,
    selectedModelId,
    selectedReasoningId,
    preferredEffortId,
    setSelection,
    setPreferredEffort,
  } = useWorkspacePickerSelection(workspaceId);

  const nameTooLong = name.length > MAX_NAME_LEN;
  const canSubmit = name.trim().length > 0 && !nameTooLong;

  const mutation = useMutation({
    mutationFn: async (body: SpawnTeammateRequest) =>
      sessionsApi.spawnTeammate(workspaceId, body),
    onSuccess: (data) => {
      setName('');
      setPrompt('');
      setErrorMessage(null);
      onOpenChange(false);
      onSpawned?.(data.session_id);
    },
    onError: (err: unknown) => {
      if (err instanceof ApiError) {
        setErrorMessage(translateTeammateError(t, err));
      } else if (err instanceof Error) {
        setErrorMessage(err.message);
      } else {
        setErrorMessage(t('conversation.team.errorGeneric'));
      }
    },
  });

  const handleSubmit = useCallback(() => {
    if (!canSubmit || mutation.isPending) return;
    setErrorMessage(null);
    // Resolve the "agent default" sentinel (empty-string model id) to
    // `null` so the spawn applier skips `--model` instead of sending
    // `--model ""`, which Claude rejects with `400: model: String should
    // have at least 1 character`.
    const resolvedModelId = isAgentDefaultModelId(selectedModelId)
      ? null
      : (selectedModelId ?? executorConfig?.model_id ?? null);
    const resolvedReasoningId =
      selectedReasoningId ?? executorConfig?.reasoning_id ?? null;
    const cfg: ExecutorConfig | null = executorConfig
      ? {
          ...executorConfig,
          model_id: resolvedModelId,
          reasoning_id: resolvedReasoningId,
        }
      : null;
    const body: SpawnTeammateRequest = {
      name: name.trim(),
    };
    if (prompt.trim().length > 0) body.prompt = prompt;
    if (cfg) body.executor_config = cfg;
    if (selectedProviderId) body.selected_provider_id = selectedProviderId;
    mutation.mutate(body);
  }, [
    canSubmit,
    executorConfig,
    mutation,
    name,
    prompt,
    selectedModelId,
    selectedProviderId,
    selectedReasoningId,
  ]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('conversation.team.spawnTitle')}</DialogTitle>
          <DialogDescription>
            {t('conversation.team.spawnDescription')}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-base">
          <label className="flex flex-col gap-half">
            <span className="text-sm font-medium">
              {t('conversation.team.nameLabel')}
            </span>
            <Input
              ref={nameInputRef}
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value.replace(/[\r\n]+/g, ' '))}
              placeholder={t('conversation.team.namePlaceholder')}
              maxLength={MAX_NAME_LEN}
              aria-invalid={nameTooLong || undefined}
            />
            <span className="text-xs text-low">
              {t('conversation.team.namePlaceholderHelp')}
            </span>
          </label>

          <label className="flex flex-col gap-half">
            <span className="text-sm font-medium">
              {t('conversation.team.promptLabel')}
            </span>
            <textarea
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              placeholder={t('conversation.team.promptPlaceholder')}
              rows={5}
              className="rounded-sm bg-white px-base py-half text-sm dark:bg-secondary focus:outline-none"
            />
          </label>

          {effectiveExecutor && (
            <div className="flex flex-wrap items-center justify-between gap-base">
              <div className="flex flex-wrap items-center gap-base">
                <AgentChip
                  selected={effectiveExecutor}
                  options={executorOptions}
                  onChange={setExecutor}
                  disabled={mutation.isPending}
                />
                <ModelSelectorContainer
                  slot="left"
                  agent={effectiveExecutor}
                  workspaceId={workspaceId}
                  onAdvancedSettings={() =>
                    SettingsDialog.show({ initialSection: 'agents' })
                  }
                  presets={variantOptions}
                  selectedPreset={selectedVariant}
                  onPresetSelect={setVariant}
                  onOverrideChange={setOverrides}
                  executorConfig={executorConfig}
                  presetOptions={presetOptions}
                />
              </div>
              <ProviderModelPicker
                selectedProviderId={selectedProviderId}
                selectedModelId={selectedModelId}
                selectedReasoningId={selectedReasoningId}
                preferredEffortId={preferredEffortId}
                activeAgent={effectiveExecutor}
                onManageProviders={() =>
                  SettingsDialog.show({ initialSection: 'providers' })
                }
                onSelectionChange={(providerId, modelId, reasoningId) =>
                  setSelection(providerId, modelId, reasoningId)
                }
                onPreferredEffortChange={setPreferredEffort}
              />
            </div>
          )}

          {errorMessage && (
            <p className="rounded-sm bg-error/10 px-base py-half text-sm text-error">
              {errorMessage}
            </p>
          )}
        </div>

        <DialogFooter>
          <PrimaryButton
            variant="tertiary"
            onClick={() => onOpenChange(false)}
            disabled={mutation.isPending}
            value={t('conversation.team.cancel')}
          />
          <PrimaryButton
            onClick={handleSubmit}
            disabled={!canSubmit || mutation.isPending}
            actionIcon={mutation.isPending ? 'spinner' : undefined}
            value={
              mutation.isPending
                ? t('conversation.team.submitting')
                : t('conversation.team.submit')
            }
          />
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
