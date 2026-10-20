import { useState, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { MagnifyingGlassIcon, GearIcon } from '@phosphor-icons/react';
import { useQuery } from '@tanstack/react-query';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';
import { useProviders } from '@/shared/hooks/useProviders';
import { useModelSelectorConfig } from '@/shared/hooks/useExecutorDiscovery';
import {
  inferReasoningOptions,
  clampEffortToModel,
} from '@/shared/lib/reasoningCapability';
import { AGENT_DEFAULT_MODEL_ID } from '@/shared/lib/agentDefaultModel';
import { BaseCodingAgent } from 'shared/types';
import type { EnabledModel, Provider } from 'shared/types';
import { cn } from '@/shared/lib/utils';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';

interface RecentPair {
  model_id: string;
  provider_id: string;
}

interface ProviderModelPickerProps {
  selectedProviderId: string | null;
  selectedModelId: string | null;
  selectedReasoningId: string | null;
  preferredEffortId: string | null;
  /**
   * Currently active agent (composer's executor dropdown). Filters non-Default
   * providers by `record.perAgentEnabled[activeAgent] === true`. Default is
   * always shown. Pass null to disable the filter (legacy callers).
   */
  activeAgent?: BaseCodingAgent | null;
  onManageProviders: () => void;
  onSelectionChange: (
    providerId: string,
    modelId: string,
    reasoningId: string | null
  ) => void;
  onPreferredEffortChange: (effortId: string | null) => void;
}

export function ProviderModelPicker({
  selectedProviderId,
  selectedModelId,
  selectedReasoningId,
  preferredEffortId,
  activeAgent,
  onManageProviders,
  onSelectionChange,
  onPreferredEffortChange,
}: ProviderModelPickerProps) {
  const { t } = useTranslation('settings');
  const { data: providers = [] } = useProviders();
  const { config: agentModelConfig } = useModelSelectorConfig(
    activeAgent ?? null
  );

  // Default's DB-synthesized enabledModels is Claude-only. Substitute the
  // active agent's canonical list (sourced from executor discovery) so the
  // picker reflects what each agent will actually accept.
  const agentDefaultModels = useMemo<EnabledModel[]>(() => {
    if (!agentModelConfig) return [];
    return agentModelConfig.models.map((m) => ({
      id: m.id,
      displayName: m.name,
      ownedBy: null,
    }));
  }, [agentModelConfig]);

  const modelsForProvider = (p: Provider): EnabledModel[] => {
    if (p.kind === 'Default' && activeAgent && agentModelConfig) {
      // Prepend the "agent default" virtual entry so users who configure
      // their own agent can opt out of cdesktop forcing a `--model` flag.
      // Agents that report no models (e.g. Hermes, which has no
      // scriptable listing surface) collapse to just this sentinel.
      const virtualDefault: EnabledModel = {
        id: AGENT_DEFAULT_MODEL_ID,
        displayName: t('settings.providers.picker.defaultModel'),
        ownedBy: null,
      };
      return [virtualDefault, ...agentDefaultModels];
    }
    return p.enabledModels ?? [];
  };

  const { data: recents = [] } = useQuery({
    queryKey: ['providers', 'recents'],
    queryFn: async () => {
      const res = await makeLocalApiRequest('/api/providers/recents');
      const body = await res.json();
      return body.data as RecentPair[];
    },
  });

  const [search, setSearch] = useState('');
  const [open, setOpen] = useState(false);

  const allItems = useMemo(() => {
    const items: {
      provider: Provider;
      modelId: string;
      displayName: string;
    }[] = [];
    for (const p of providers) {
      if (!p.enabled) continue;
      // Default is always visible regardless of activeAgent — it's the
      // "use whatever native config" passthrough. For Preset/Custom, the
      // perAgentEnabled toggle gates picker visibility per plan §3.3.
      if (
        activeAgent &&
        p.kind !== 'Default' &&
        p.perAgentEnabled?.[activeAgent] !== true
      ) {
        continue;
      }
      for (const m of modelsForProvider(p)) {
        if (
          !search ||
          m.id.toLowerCase().includes(search.toLowerCase()) ||
          p.name.toLowerCase().includes(search.toLowerCase())
        ) {
          items.push({
            provider: p,
            modelId: m.id,
            displayName: m.displayName,
          });
        }
      }
    }
    return items;
  }, [providers, search, activeAgent, agentDefaultModels]);

  const grouped = useMemo(() => {
    const map = new Map<
      string,
      { provider: Provider; models: typeof allItems }
    >();
    for (const item of allItems) {
      const existing = map.get(item.provider.id);
      if (existing) {
        existing.models.push(item);
      } else {
        map.set(item.provider.id, { provider: item.provider, models: [item] });
      }
    }
    return [...map.values()];
  }, [allItems]);

  const recentItems = useMemo(() => {
    return recents
      .map((r) => {
        const provider = providers.find((p) => p.id === r.provider_id);
        if (!provider) return null;
        const model = modelsForProvider(provider).find(
          (m) => m.id === r.model_id
        );
        if (!model) return null;
        return {
          provider,
          modelId: r.model_id,
          displayName: model.displayName,
        };
      })
      .filter((x): x is NonNullable<typeof x> => x !== null);
  }, [recents, providers, activeAgent, agentDefaultModels]);

  const selectedProvider = providers.find((p) => p.id === selectedProviderId);
  // Hermes's ACP adapter has no surface for reasoning effort: env vars
  // are ignored, `set_session_model` only takes `provider:model`, and
  // `set_config_option` is a write-only stub. Effort comes from
  // ~/.hermes/config.yaml `model.reasoning_effort` only. Hide the
  // picker and surface a hint instead.
  const agentSuppressesEffort = activeAgent === BaseCodingAgent.HERMES;
  // selectedModelId === '' is the AGENT_DEFAULT_MODEL_ID sentinel. We
  // don't know what model the agent's ambient config will resolve to, so
  // fall back to inferReasoningOptions('') — the universal 3-tier set
  // (low/medium/high). Codex honors it via `-c model_reasoning_effort`;
  // Claude/Gemini/etc. ignore it without a model flag. Saving the
  // preference still helps when the user later picks a concrete model.
  const currentReasoningOptions = useMemo<string[]>(
    () =>
      agentSuppressesEffort ? [] : inferReasoningOptions(selectedModelId ?? ''),
    [selectedModelId, agentSuppressesEffort]
  );

  const selectedModel = selectedProvider
    ? modelsForProvider(selectedProvider).find((m) => m.id === selectedModelId)
    : undefined;
  const displayName = selectedModel?.displayName ?? selectedModelId ?? '';
  const contextMatch = displayName.match(/^(.*) \((\d+M) context\)$/);
  const rawName = contextMatch ? contextMatch[1] : displayName;
  const namePart = rawName.includes('/')
    ? rawName.split('/').slice(1).join('/')
    : rawName;
  const contextSuffix = contextMatch ? contextMatch[2] : null;
  const effortLabel =
    selectedReasoningId && !agentSuppressesEffort
      ? t(`settings.providers.effort.${selectedReasoningId}`)
      : null;
  // selectedModelId === '' is the AGENT_DEFAULT_MODEL_ID sentinel — a valid
  // pick. Use !== null so the trigger renders "Agent default" rather than
  // the "Model ▾" placeholder when the user opts into ambient agent config.
  const triggerLabel =
    selectedModelId !== null && selectedProvider ? (
      <>
        {namePart}
        {contextSuffix && <span className="text-low"> {contextSuffix}</span>}
        {effortLabel && <span className="text-low"> · {effortLabel}</span>}
      </>
    ) : (
      <>{t('settings.providers.picker.triggerPlaceholder')}</>
    );

  const selectModel = (providerId: string, modelId: string) => {
    const opts = inferReasoningOptions(modelId);
    const reasoning = clampEffortToModel(preferredEffortId, opts);
    onSelectionChange(providerId, modelId, reasoning);
  };

  const selectEffort = (effortId: string) => {
    onPreferredEffortChange(effortId);
    if (!selectedProviderId || !selectedModelId) return;
    const reasoning = clampEffortToModel(effortId, currentReasoningOptions);
    onSelectionChange(selectedProviderId, selectedModelId, reasoning);
  };

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <button className="flex items-center gap-half bg-secondary rounded-sm px-base py-half text-sm h-cta hover:bg-muted max-w-64 min-w-0 focus:outline-none focus-visible:ring-1 focus-visible:ring-brand">
          <span className="truncate">{triggerLabel}</span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="w-72 p-0 overflow-hidden flex flex-col"
      >
        <div className="flex items-center gap-2 px-2 py-1.5 border-b border-border">
          <MagnifyingGlassIcon className="w-3.5 h-3.5 text-low flex-shrink-0" />
          <input
            autoFocus
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder={t('settings.providers.picker.searchPlaceholder')}
            className="flex-1 text-[13px] bg-transparent outline-none"
          />
        </div>

        <div className="overflow-y-auto max-h-72">
          {!search && recentItems.length > 0 && (
            <div>
              <div className="px-2 py-1 text-[13px] font-medium text-low">
                {t('settings.providers.picker.recentlyUsed')}
              </div>
              {recentItems.map((item) => (
                <ModelRow
                  key={`${item.provider.id}::${item.modelId}`}
                  modelId={item.modelId}
                  displayName={item.displayName}
                  providerName={
                    item.provider.kind === 'Default'
                      ? t('settings.providers.defaultProviderName')
                      : item.provider.name
                  }
                  freeTier={isFreeTierModel(
                    item.provider.presetId ?? item.provider.id,
                    item.modelId
                  )}
                  isSelected={
                    item.provider.id === selectedProviderId &&
                    item.modelId === selectedModelId
                  }
                  onClick={() => selectModel(item.provider.id, item.modelId)}
                />
              ))}
              <div className="border-t border-border my-1" />
            </div>
          )}

          {!search &&
            grouped
              .filter((g) => g.provider.kind === 'Default')
              .map((g) => (
                <ProviderGroup
                  key={g.provider.id}
                  group={g}
                  selectedProviderId={selectedProviderId}
                  selectedModelId={selectedModelId}
                  onSelect={selectModel}
                />
              ))}

          {grouped
            .filter((g) => search || g.provider.kind !== 'Default')
            .map((g) => (
              <ProviderGroup
                key={g.provider.id}
                group={g}
                selectedProviderId={selectedProviderId}
                selectedModelId={selectedModelId}
                onSelect={selectModel}
              />
            ))}

          {allItems.length === 0 && (
            <div className="px-2 py-3 text-[13px] text-low text-center">
              {providers.filter((p) => p.enabled).length === 0
                ? t('settings.providers.picker.noProvidersEnabled')
                : t('settings.providers.picker.noModelsMatch')}
            </div>
          )}
        </div>

        {agentSuppressesEffort && (
          <div className="border-t border-border px-2 py-1.5 text-[11px] text-low leading-snug">
            {t('settings.providers.picker.effortUnsupportedHermes')}
          </div>
        )}

        {currentReasoningOptions.length > 0 && (
          <div className="border-t border-border px-2 py-1.5 flex items-center gap-2">
            <span className="text-[11px] text-low uppercase tracking-wide flex-shrink-0">
              {t('settings.providers.picker.effort')}
            </span>
            <div className="flex items-center gap-0.5 flex-1 justify-end">
              {currentReasoningOptions.map((id) => (
                <button
                  key={id}
                  onClick={() => selectEffort(id)}
                  title={t('settings.providers.picker.effortTooltip', {
                    label: t(`settings.providers.effort.${id}`),
                  })}
                  className={cn(
                    'px-1.5 py-0.5 text-[11px] rounded border border-border/60',
                    selectedReasoningId === id
                      ? 'bg-brand text-white border-brand'
                      : 'text-low hover:bg-secondary hover:text-high'
                  )}
                >
                  {t(`settings.providers.effort.${id}Short`)}
                </button>
              ))}
            </div>
          </div>
        )}

        <div className="border-t border-border">
          <button
            onClick={() => {
              setOpen(false);
              onManageProviders();
            }}
            className="w-full flex items-center gap-2 px-2 py-1.5 text-[13px] text-low hover:text-high hover:bg-muted"
          >
            <GearIcon className="w-3.5 h-3.5" />
            {t('settings.providers.picker.manageProviders')}
          </button>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ProviderGroup({
  group,
  selectedProviderId,
  selectedModelId,
  onSelect,
}: {
  group: {
    provider: Provider;
    models: { modelId: string; displayName: string }[];
  };
  selectedProviderId: string | null;
  selectedModelId: string | null;
  onSelect: (providerId: string, modelId: string) => void;
}) {
  const { t } = useTranslation('settings');
  const providerLabel =
    group.provider.kind === 'Default'
      ? t('settings.providers.defaultProviderName')
      : group.provider.name;
  return (
    <div>
      <div className="px-2 py-1 text-[13px] font-medium text-low">
        {providerLabel}
      </div>
      {group.models.map((m) => {
        const isSelected =
          group.provider.id === selectedProviderId &&
          m.modelId === selectedModelId;
        return (
          <ModelRow
            key={m.modelId}
            modelId={m.modelId}
            displayName={m.displayName}
            freeTier={isFreeTierModel(
              group.provider.presetId ?? group.provider.id,
              m.modelId
            )}
            isSelected={isSelected}
            onClick={() => onSelect(group.provider.id, m.modelId)}
          />
        );
      })}
    </div>
  );
}

function isFreeTierModel(providerId: string, modelId: string) {
  return (
    providerId === 'openrouter' &&
    (modelId.endsWith(':free') || modelId === 'openrouter/free')
  );
}

function ModelRow({
  modelId,
  displayName,
  providerName,
  freeTier = false,
  isSelected,
  onClick,
}: {
  modelId: string;
  displayName: string;
  providerName?: string;
  freeTier?: boolean;
  isSelected: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation('settings');
  return (
    <button
      onClick={onClick}
      className={cn(
        'w-full flex items-start justify-between gap-2 px-3 py-1.5 text-[13px] hover:bg-muted text-left',
        isSelected && 'bg-muted font-medium'
      )}
    >
      <span className="flex min-w-0 items-center gap-1.5">
        <span className="truncate">{displayName || modelId}</span>
        {freeTier && (
          <span className="shrink-0 rounded bg-success/15 px-1 py-0.5 text-[10px] text-success">
            {t('settings.providers.picker.freeTag')}
          </span>
        )}
      </span>
      {providerName && (
        <span className="text-low flex-shrink-0">{providerName}</span>
      )}
    </button>
  );
}
