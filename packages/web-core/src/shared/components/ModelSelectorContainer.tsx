import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  CheckIcon,
  FastForwardIcon,
  GearIcon,
  HandIcon,
  ListBulletsIcon,
  PencilSimpleIcon,
  SlidersHorizontalIcon,
  WarningIcon,
  type Icon,
} from '@phosphor-icons/react';
import type { ExecutorConfig, ModelInfo, ReasoningOption } from 'shared/types';
import { BaseCodingAgent, PermissionPolicy } from 'shared/types';
import { toPrettyCase } from '@/shared/lib/string';
import {
  getModelKey,
  getRecentModelEntries,
  getRecentReasoningByModel,
  touchRecentModel,
  updateRecentModelEntries,
  setRecentReasoning,
} from '@/shared/lib/recentModels';
import {
  getReasoningLabel,
  getSelectedModel,
  escapeAttributeValue,
  parseModelId,
  appendPresetModel,
  resolveDefaultModelId,
  isModelAvailable,
  resolveDefaultReasoningId,
} from '@/shared/lib/modelSelector';
import { profilesApi } from '@/shared/lib/api';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { getResolvedTheme, useTheme } from '@/shared/hooks/useTheme';
import { useModelSelectorConfig } from '@/shared/hooks/useExecutorDiscovery';
import { ModelSelectorPopover } from '@vibe/ui/components/ModelSelectorPopover';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTriggerButton,
} from '@vibe/ui/components/Dropdown';

// Permissions are a hardcoded vec![…] in each executor's discover_options
// (e.g. crates/executors/src/executors/claude.rs); they don't depend on
// workdir or runtime state. Mirror Claude's list here so the picker
// renders synchronously instead of waiting on the discovery WebSocket.
// If the Rust list changes, update this too.
const CLAUDE_PERMISSIONS: PermissionPolicy[] = [
  PermissionPolicy.SUPERVISED,
  PermissionPolicy.ACCEPT_EDITS,
  PermissionPolicy.PLAN,
  PermissionPolicy.AUTO_MODE,
  PermissionPolicy.BYPASS_PERMISSIONS,
];

const reasoningOptions = (includeMax: boolean): ReasoningOption[] =>
  [
    ['low', 'Low'],
    ['medium', 'Medium'],
    ['high', 'High'],
    ['xhigh', 'Extra High'],
    ...(includeMax ? ([['max', 'Max']] as const) : []),
  ].map(([id, label]) => ({ id, label, is_default: id === 'high' }));

interface ModelSelectorContainerProps {
  agent: BaseCodingAgent | null;
  workspaceId: string | undefined;
  sessionId?: string;
  onAdvancedSettings: () => void;
  presets: string[];
  selectedPreset: string | null;
  onPresetSelect: (presetId: string | null) => void;
  onOverrideChange: (partial: Partial<ExecutorConfig>) => void;
  executorConfig: ExecutorConfig | null;
  presetOptions: ExecutorConfig | null | undefined;
  /**
   * Which subset of dropdowns to render.
   * - `right` (default): just the model picker (rendered in the
   *   composer's bottom-right alongside the context gauge).
   * - `left`: preset + permission policy + agent ("profile") — these
   *   live in the composer's bottom-left toolbar.
   * - `all`: every dropdown in one node (legacy callers).
   */
  slot?: 'right' | 'left' | 'all';
}

export function ModelSelectorContainer({
  agent,
  workspaceId,
  sessionId,
  onAdvancedSettings,
  presets,
  selectedPreset,
  onPresetSelect,
  onOverrideChange,
  executorConfig,
  presetOptions,
  slot = 'all',
}: ModelSelectorContainerProps) {
  const { t } = useTranslation('common');
  const { theme } = useTheme();
  const resolvedTheme = getResolvedTheme(theme);
  const [isOpen, setIsOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [expandedProviderId, setExpandedProviderId] = useState('');
  const { profiles, setProfiles, reloadSystem } = useUserSystem();
  const defaultLabel = t('modelSelector.default');
  const loadingLabel = t('states.loading');

  const permissionMetaByPolicy: Record<
    PermissionPolicy,
    { label: string; icon: Icon; triggerColorClass?: string }
  > = {
    [PermissionPolicy.SUPERVISED]: {
      label: t('modelSelector.permissionAsk'),
      icon: HandIcon,
    },
    [PermissionPolicy.ACCEPT_EDITS]: {
      label: t('modelSelector.permissionAcceptEdits'),
      icon: PencilSimpleIcon,
    },
    [PermissionPolicy.PLAN]: {
      label: t('modelSelector.permissionPlan'),
      icon: ListBulletsIcon,
    },
    [PermissionPolicy.AUTO_MODE]: {
      label: t('modelSelector.permissionAuto'),
      icon: FastForwardIcon,
      triggerColorClass: '!text-error',
    },
    [PermissionPolicy.BYPASS_PERMISSIONS]: {
      label: t('modelSelector.permissionBypass'),
      icon: WarningIcon,
      triggerColorClass: '!text-yellow-500',
    },
  };

  const resolvedPreset =
    selectedPreset ??
    (presets.includes('DEFAULT') ? 'DEFAULT' : (presets[0] ?? null));

  const {
    config: streamConfig,
    loadingModels,
    error: streamError,
  } = useModelSelectorConfig(agent, {
    workspaceId: sessionId ? workspaceId : undefined,
    sessionId,
  });

  useEffect(() => {
    if (streamError) {
      console.error('Failed to fetch model config', streamError);
    }
  }, [streamError]);

  const baseConfig = streamConfig;
  const presetConfig = appendPresetModel(baseConfig, presetOptions?.model_id);
  const config = appendPresetModel(
    presetConfig,
    executorConfig?.model_id,
    reasoningOptions(
      agent === BaseCodingAgent.CLAUDE_CODE || agent === BaseCodingAgent.CODEX
    )
  );

  const availableProviderIds = useMemo(
    () => config?.providers.map((item) => item.id) ?? [],
    [config?.providers]
  );
  const hasProviders = availableProviderIds.length > 0;
  const providerIdMap = useMemo(
    () => new Map(availableProviderIds.map((id) => [id.toLowerCase(), id])),
    [availableProviderIds]
  );
  const resolveProviderId = (value?: string | null) =>
    value ? (providerIdMap.get(value.toLowerCase()) ?? null) : null;

  const { providerId: configProviderId, modelId: configModelId } = useMemo(
    () => parseModelId(executorConfig?.model_id, hasProviders),
    [executorConfig?.model_id, hasProviders]
  );

  const fallbackProviderId = availableProviderIds[0] ?? null;
  const resolvedConfigProviderId = resolveProviderId(configProviderId);

  const { providerId: presetProviderId } = useMemo(
    () => parseModelId(presetOptions?.model_id, hasProviders),
    [presetOptions?.model_id, hasProviders]
  );
  const resolvedPresetProviderId = resolveProviderId(presetProviderId);

  const hasDefaultModel = Boolean(config?.default_model);
  const selectedProviderId =
    resolvedConfigProviderId ??
    resolvedPresetProviderId ??
    (hasDefaultModel ? fallbackProviderId : null);

  const defaultModelId = config
    ? resolveDefaultModelId(
        config.models,
        selectedProviderId,
        config.default_model,
        hasProviders
      )
    : null;

  const { modelId: presetModelId } = useMemo(
    () => parseModelId(presetOptions?.model_id, hasProviders),
    [presetOptions?.model_id, hasProviders]
  );

  const presetModelMatchesProvider =
    !selectedProviderId ||
    !resolvedPresetProviderId ||
    resolvedPresetProviderId === selectedProviderId;
  const resolvedPresetModelId = presetModelMatchesProvider
    ? presetModelId
    : null;

  const selectedModelId = (() => {
    const candidate = configModelId ?? resolvedPresetModelId ?? defaultModelId;
    if (!candidate || !config || !selectedProviderId) return candidate;
    const hasMatch = isModelAvailable(config, selectedProviderId, candidate);
    return hasMatch
      ? candidate
      : resolveDefaultModelId(
          config.models,
          selectedProviderId,
          config.default_model,
          hasProviders
        );
  })();

  const selectedModel = config
    ? getSelectedModel(config.models, selectedProviderId, selectedModelId)
    : null;

  const recentReasoningByModel = getRecentReasoningByModel(profiles, agent);

  const presetReasoningId =
    resolvedPresetModelId && selectedModelId === resolvedPresetModelId
      ? (presetOptions?.reasoning_id ?? null)
      : null;

  const recentReasoningId = useMemo(() => {
    if (!selectedModel || !recentReasoningByModel) return null;
    const key = selectedModel.provider_id
      ? `${selectedModel.provider_id}/${selectedModel.id}`
      : selectedModel.id;
    const keyLower = key.toLowerCase();
    for (const [k, v] of Object.entries(recentReasoningByModel)) {
      if (k.toLowerCase() === keyLower) {
        if (selectedModel.reasoning_options.some((o) => o.id === v)) return v;
      }
    }
    return null;
  }, [selectedModel, recentReasoningByModel]);

  const selectedReasoningId =
    executorConfig?.reasoning_id ??
    presetReasoningId ??
    recentReasoningId ??
    resolveDefaultReasoningId(selectedModel?.reasoning_options ?? []);

  const defaultAgentId =
    config?.agents.find((entry) => entry.is_default)?.id ?? null;

  const selectedAgentId =
    executorConfig?.agent_id !== undefined
      ? executorConfig.agent_id
      : (presetOptions?.agent_id ?? defaultAgentId);

  const availablePermissions =
    agent === BaseCodingAgent.CLAUDE_CODE
      ? CLAUDE_PERMISSIONS
      : (config?.permissions ?? []);
  const supportsPermissions = availablePermissions.length > 0;

  const basePermissionPolicy = supportsPermissions
    ? (presetOptions?.permission_policy ?? availablePermissions[0] ?? null)
    : null;
  const permissionPolicy = supportsPermissions
    ? (executorConfig?.permission_policy ?? basePermissionPolicy)
    : null;

  // LRU persistence (on popover close)

  const recentModelEntries = getRecentModelEntries(profiles, agent);
  const pendingModelRef = useRef<ModelInfo | null>(null);
  const pendingReasoningRef = useRef<string | null>(null);

  const persistPendingSelections = useCallback(() => {
    if (!profiles || !agent) return;
    if (!pendingModelRef.current && !pendingReasoningRef.current) return;

    let nextProfiles = profiles;

    const model = pendingModelRef.current;
    if (model) {
      pendingModelRef.current = null;
      const current = getRecentModelEntries(nextProfiles, agent);
      const nextEntries = touchRecentModel(current, model);
      nextProfiles = updateRecentModelEntries(nextProfiles, agent, nextEntries);
    }

    const reasoningModel =
      model ??
      (selectedModelId && config
        ? getSelectedModel(config.models, selectedProviderId, selectedModelId)
        : null);
    if (pendingReasoningRef.current && reasoningModel) {
      nextProfiles = setRecentReasoning(
        nextProfiles,
        agent,
        reasoningModel,
        pendingReasoningRef.current
      );
      pendingReasoningRef.current = null;
    }

    if (nextProfiles !== profiles) {
      setProfiles(nextProfiles);
      void profilesApi
        .save(JSON.stringify({ executors: nextProfiles }, null, 2))
        .catch((error) => {
          console.error('Failed to save recent models', error);
          void reloadSystem();
        });
    }
  }, [
    agent,
    config,
    profiles,
    reloadSystem,
    selectedModelId,
    selectedProviderId,
    setProfiles,
  ]);

  const handleModelSelect = (modelId: string | null, providerId?: string) => {
    const modelOverride = (() => {
      if (!modelId) return null;
      if (providerId) return `${providerId}/${modelId}`;
      return modelId;
    })();
    onOverrideChange({ model_id: modelOverride });

    pendingModelRef.current =
      modelId && config
        ? (() => {
            const selectedId = modelId.toLowerCase();
            if (!providerId) {
              return (
                config.models.find((m) => m.id.toLowerCase() === selectedId) ??
                null
              );
            }
            const provider = providerId.toLowerCase();
            return (
              config.models.find(
                (m) =>
                  m.id.toLowerCase() === selectedId &&
                  m.provider_id?.toLowerCase() === provider
              ) ?? null
            );
          })()
        : null;
    pendingReasoningRef.current = null;
  };

  const handleReasoningSelect = (reasoningId: string | null) => {
    onOverrideChange({ reasoning_id: reasoningId });
    pendingReasoningRef.current = reasoningId;
  };

  const handleAgentSelect = (id: string | null) => {
    onOverrideChange({ agent_id: id });
  };

  const handlePermissionPolicyChange = (policy: PermissionPolicy) => {
    if (!supportsPermissions) return;
    onOverrideChange({ permission_policy: policy });
  };

  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    setSearchQuery('');
  }, [selectedProviderId]);

  useEffect(() => {
    if (!isOpen) {
      setSearchQuery('');
      return;
    }
    requestAnimationFrame(() => {
      const node = scrollRef.current;
      if (!node) return;
      if (selectedModelId && config) {
        const selected = getSelectedModel(
          config.models,
          selectedProviderId,
          selectedModelId
        );
        if (selected) {
          const key = getModelKey(selected);
          const selector = `[data-model-key="${escapeAttributeValue(key)}"]`;
          const target = node.querySelector(selector);
          if (target instanceof HTMLElement) {
            target.scrollIntoView({ block: 'nearest' });
            return;
          }
        }
      }
      if (!selectedModelId) {
        node.scrollTop = node.scrollHeight;
      }
    });
  }, [config, isOpen, selectedModelId, selectedProviderId]);

  const handleOpenChange = (open: boolean) => {
    setIsOpen(open);
    if (open) {
      const selected =
        selectedModelId && config
          ? getSelectedModel(config.models, selectedProviderId, selectedModelId)
          : null;
      setExpandedProviderId(selected?.provider_id ?? selectedProviderId ?? '');
    } else {
      persistPendingSelections();
    }
  };

  useEffect(() => {
    if (isOpen) return;
    persistPendingSelections();
  }, [isOpen, persistPendingSelections]);

  const formatPresetLabel = (preset: string) =>
    preset.toUpperCase() === 'DEFAULT'
      ? t('modelSelector.default')
      : toPrettyCase(preset);

  const presetLabel = resolvedPreset
    ? formatPresetLabel(resolvedPreset)
    : defaultLabel;

  const showModelSelector =
    !!config && (loadingModels || config.models.length > 0);
  const showDefaultOption =
    !!config && !config.default_model && config.models.length > 0;
  const displaySelectedModel =
    showModelSelector && config
      ? getSelectedModel(config.models, selectedProviderId, selectedModelId)
      : null;
  const reasoningLabel = displaySelectedModel
    ? getReasoningLabel(
        displaySelectedModel.reasoning_options,
        selectedReasoningId
      )
    : null;
  const modelLabelBase = loadingModels
    ? loadingLabel
    : (displaySelectedModel?.name ?? selectedModelId ?? defaultLabel);
  const contextMatch = modelLabelBase.match(/^(.*) \((\d+M) context\)$/);
  const modelNamePart = contextMatch ? contextMatch[1] : modelLabelBase;
  const contextSuffix = contextMatch ? contextMatch[2] : null;
  const modelLabel = (
    <>
      {modelNamePart}
      {contextSuffix && <span className="text-low"> {contextSuffix}</span>}
      {reasoningLabel && <span className="text-low"> · {reasoningLabel}</span>}
    </>
  );

  const agentLabel = selectedAgentId
    ? (config?.agents.find((entry) => entry.id === selectedAgentId)?.label ??
      toPrettyCase(selectedAgentId))
    : defaultLabel;

  const permissionMeta = permissionPolicy
    ? (permissionMetaByPolicy[permissionPolicy] ?? null)
    : null;

  const showLeft = slot === 'left' || slot === 'all';
  const showRight = slot === 'right' || slot === 'all';

  const presetNode = (
    <DropdownMenu>
      <DropdownMenuTriggerButton
        size="sm"
        icon={SlidersHorizontalIcon}
        label={
          resolvedPreset?.toLowerCase() !== 'default' ? presetLabel : undefined
        }
        showCaret={false}
      />
      <DropdownMenuContent align="start">
        <DropdownMenuLabel>{t('modelSelector.preset')}</DropdownMenuLabel>
        {presets.length > 0 ? (
          presets.map((preset) => (
            <DropdownMenuItem
              key={preset}
              icon={preset === resolvedPreset ? CheckIcon : undefined}
              onClick={() => onPresetSelect?.(preset)}
            >
              {formatPresetLabel(preset)}
            </DropdownMenuItem>
          ))
        ) : (
          <DropdownMenuItem disabled>{presetLabel}</DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem icon={GearIcon} onClick={onAdvancedSettings}>
          {t('modelSelector.custom')}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );

  const modelNode =
    showModelSelector && config ? (
      <ModelSelectorPopover
        isOpen={isOpen}
        onOpenChange={handleOpenChange}
        trigger={
          <DropdownMenuTriggerButton
            size="sm"
            label={modelLabel}
            disabled={loadingModels}
          />
        }
        config={config}
        error={streamError}
        selectedProviderId={selectedProviderId}
        selectedModelId={selectedModelId}
        selectedReasoningId={selectedReasoningId}
        searchQuery={searchQuery}
        onSearchChange={setSearchQuery}
        onModelSelect={handleModelSelect}
        onReasoningSelect={handleReasoningSelect}
        recentModelEntries={recentModelEntries}
        showDefaultOption={showDefaultOption}
        onSelectDefault={() => handleModelSelect(null)}
        scrollRef={scrollRef}
        expandedProviderId={expandedProviderId}
        onExpandedProviderIdChange={setExpandedProviderId}
        resolvedTheme={resolvedTheme}
        allowCustomModelId
      />
    ) : null;

  const permissionNode =
    permissionPolicy && availablePermissions.length > 0 ? (
      <DropdownMenu>
        <DropdownMenuTriggerButton
          size="sm"
          label={
            permissionMeta ? (
              <span className={permissionMeta.triggerColorClass}>
                {permissionMeta.label}
              </span>
            ) : undefined
          }
          showCaret={false}
        />
        <DropdownMenuContent align="start">
          <DropdownMenuLabel>
            {t('modelSelector.permissions')}
          </DropdownMenuLabel>
          {availablePermissions.map((policy) => {
            const meta = permissionMetaByPolicy[policy];
            return (
              <DropdownMenuItem
                key={policy}
                icon={meta?.icon ?? HandIcon}
                onClick={() => handlePermissionPolicyChange(policy)}
              >
                {meta?.label ?? toPrettyCase(policy)}
              </DropdownMenuItem>
            );
          })}
        </DropdownMenuContent>
      </DropdownMenu>
    ) : null;

  const agentNode =
    config && config.agents.length > 0 ? (
      <DropdownMenu>
        <DropdownMenuTriggerButton size="sm" label={agentLabel} />
        <DropdownMenuContent align="start">
          <DropdownMenuLabel>{t('modelSelector.agent')}</DropdownMenuLabel>
          <DropdownMenuItem
            icon={selectedAgentId === null ? CheckIcon : undefined}
            onClick={() => handleAgentSelect(null)}
          >
            {t('modelSelector.default')}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          {config.agents.map((agentOption) => (
            <DropdownMenuItem
              key={agentOption.id}
              icon={agentOption.id === selectedAgentId ? CheckIcon : undefined}
              onClick={() => handleAgentSelect(agentOption.id)}
            >
              {agentOption.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    ) : null;

  return (
    <>
      {showLeft && (
        <>
          {permissionNode}
          {presetNode}
          {agentNode}
        </>
      )}
      {showRight && modelNode}
    </>
  );
}
