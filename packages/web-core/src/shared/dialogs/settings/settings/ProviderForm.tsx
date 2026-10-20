import { useState, useEffect, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import {
  PlusIcon,
  TrashIcon,
  ArrowClockwiseIcon,
  CheckIcon,
  CaretRightIcon,
} from '@phosphor-icons/react';
import { useQuery } from '@tanstack/react-query';
import { makeLocalApiRequest } from '@/shared/lib/localApiTransport';
import type {
  Provider,
  CreateProvider,
  UpdateProvider,
  EnabledModel,
  ClaudePayload,
  CodexPayload,
  OpencodePayload,
  DeepseekTuiPayload, // DeepSeek TUI hidden for now; keep type for stub passthrough.
  GeminiPayload,
  HermesPayload,
} from 'shared/types';
import { cn } from '@/shared/lib/utils';
import {
  SettingsCard,
  SettingsField,
  SettingsTextarea,
} from './SettingsComponents';

// Catalog preset shape returned by /api/providers/catalog (matches
// crates/db/src/provider_catalog.rs::CatalogPreset).
interface CatalogPreset {
  id: string;
  name: string;
  agents: string[];
  claude: ClaudePayload;
  codex: CodexPayload;
  opencode: OpencodePayload;
  deepseekTui: DeepseekTuiPayload; // hidden from UI; preserved for stub passthrough
  gemini: GeminiPayload;
  hermes: HermesPayload;
  enabledModels: string[];
}

interface FetchedModel {
  id: string;
  owned_by: string | null;
}

// Order matters — controls the agent-section render order. Hermes last because
// its executor is deferred (note shown alongside).
const AGENT_KEYS = [
  'CLAUDE_CODE',
  'CODEX',
  'OPENCODE',
  'GEMINI',
  'HERMES',
] as const;
type AgentKey = (typeof AGENT_KEYS)[number];

const AGENT_LABEL: Record<AgentKey, string> = {
  CLAUDE_CODE: 'Claude',
  CODEX: 'Codex',
  OPENCODE: 'OpenCode',
  GEMINI: 'Gemini',
  HERMES: 'Hermes',
};

const HERMES_API_MODES = [
  'chat_completions',
  'anthropic_messages',
  'codex_responses',
] as const;

// ts-rs emits HashMap<String, T> as `{ [key in string]?: T }` (mapped-type
// shape, values implicitly optional). For form state we use the structurally
// equivalent `Record<string, T | undefined>`, which assigns cleanly to the
// generated payload types at save time.
type EnvMap = Record<string, string | undefined>;
// Match OpencodePayload['options'] = `{ [key in string]?: JsonValue }` from ts-rs.
// Use a structurally-compatible Record-style alias.
type JsonValuish = string | number | boolean | null | object;
type OptionsMap = Record<string, JsonValuish | undefined>;
type PerAgentEnabled = Record<string, boolean | undefined>;

const API_KEY_URL_BY_PRESET: Record<string, string> = {
  openrouter: 'https://openrouter.ai/settings/keys',
  nvidia: 'https://build.nvidia.com/',
};

function emptyPerAgentEnabled(): PerAgentEnabled {
  return Object.fromEntries(AGENT_KEYS.map((k) => [k, false]));
}

function envMapToText(env: EnvMap): string {
  return Object.entries(env)
    .filter((entry): entry is [string, string] => entry[1] !== undefined)
    .map(([k, v]) => `${k}=${v}`)
    .join('\n');
}

function textToEnvMap(text: string): EnvMap {
  const out: EnvMap = {};
  for (const line of text.split('\n')) {
    const eq = line.indexOf('=');
    if (eq > 0) out[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
  }
  return out;
}

function objectToText(obj: OptionsMap): string {
  return Object.entries(obj)
    .filter(([, v]) => v !== undefined)
    .map(([k, v]) => `${k}=${typeof v === 'string' ? v : JSON.stringify(v)}`)
    .join('\n');
}

function textToObject(text: string): OptionsMap {
  const out: OptionsMap = {};
  for (const line of text.split('\n')) {
    const eq = line.indexOf('=');
    if (eq > 0) {
      const k = line.slice(0, eq).trim();
      const v = line.slice(eq + 1).trim();
      if (v === 'true') out[k] = true;
      else if (v === 'false') out[k] = false;
      else if (/^-?\d+$/.test(v)) out[k] = Number(v);
      else out[k] = v;
    }
  }
  return out;
}

function ApiKeyOverrideField({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (v: string | null) => void;
}) {
  const { t } = useTranslation('settings');
  return (
    <SettingsField label={t('settings.providers.form.apiKeyOverride')}>
      <InputBase
        type="password"
        value={value ?? ''}
        onChange={(v) => onChange(v ? v : null)}
        placeholder={t('settings.providers.form.apiKeyOverrideHint')}
      />
    </SettingsField>
  );
}

function InputBase({
  value,
  onChange,
  placeholder,
  type = 'text',
  className,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: string;
  className?: string;
}) {
  return (
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className={cn(
        'w-full bg-secondary border border-border rounded-sm px-2 py-1.5 text-sm text-high',
        'placeholder:text-low focus:outline-none focus:ring-1 focus:ring-brand',
        className
      )}
    />
  );
}

// --- Per-agent section components ---

function AgentSection({
  agent,
  enabled,
  onToggleEnabled,
  expanded,
  onToggleExpanded,
  children,
}: {
  agent: AgentKey;
  enabled: boolean;
  onToggleEnabled: (v: boolean) => void;
  expanded: boolean;
  onToggleExpanded: () => void;
  children: React.ReactNode;
}) {
  return (
    <div className="border border-border rounded-sm">
      <button
        type="button"
        onClick={onToggleExpanded}
        className="flex w-full items-center gap-2 px-2 py-1.5 hover:bg-muted text-left"
      >
        <CaretRightIcon
          className={cn(
            'w-3 h-3 transition-transform shrink-0',
            expanded && 'rotate-90'
          )}
        />
        <span className="text-sm font-medium">{AGENT_LABEL[agent]}</span>
        <span className="ml-auto flex items-center gap-2">
          <span
            className="flex items-center"
            onClick={(e) => e.stopPropagation()}
          >
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => onToggleEnabled(e.target.checked)}
            />
          </span>
        </span>
      </button>
      {expanded && <div className="p-2 border-t border-border">{children}</div>}
    </div>
  );
}

interface ProviderFormProps {
  provider?: Provider;
  onSave: (data: CreateProvider | UpdateProvider) => Promise<void>;
  onCancel?: () => void;
  saveLabel?: string;
}

export function ProviderForm({
  provider,
  onSave,
  onCancel,
  saveLabel,
}: ProviderFormProps) {
  const { t } = useTranslation('settings');
  const resolvedSaveLabel =
    saveLabel ?? t('settings.providers.section.saveLabel');
  const { data: catalog } = useQuery({
    queryKey: ['providers', 'catalog'],
    queryFn: async () => {
      const res = await makeLocalApiRequest('/api/providers/catalog');
      const body = await res.json();
      return body.data as CatalogPreset[];
    },
    staleTime: Infinity,
  });

  const isCreate = !provider;
  const isDefault = provider?.kind === 'Default';

  const [selectedPresetId, setSelectedPresetId] = useState<string | null>(
    provider?.presetId ?? null
  );
  const [name, setName] = useState(provider?.name ?? '');
  const [apiKey, setApiKey] = useState('');
  const [perAgentEnabled, setPerAgentEnabled] = useState<PerAgentEnabled>(
    provider?.perAgentEnabled ?? emptyPerAgentEnabled()
  );

  // Per-agent payload state.
  const [claudePayload, setClaudePayload] = useState<ClaudePayload>(
    provider?.claude ?? {
      apiKeyField: 'ANTHROPIC_AUTH_TOKEN',
      baseUrl: null,
      haikuModel: null,
      apiKey: null,
      env: {},
    }
  );
  const [codexPayload, setCodexPayload] = useState<CodexPayload>(
    provider?.codex ?? { baseUrl: null, apiKey: null, env: {} }
  );
  const [opencodePayload, setOpencodePayload] = useState<OpencodePayload>(
    provider?.opencode ?? {
      npm: null,
      baseUrl: null,
      options: {},
      apiKey: null,
      env: {},
    }
  );
  // DeepSeek TUI hidden from form (not supported yet). Keep payload as a
  // pass-through stub so the required CreateProvider/UpdateProvider field is
  // satisfied without exposing UI. Re-enable state + section when supported.
  // const [deepseekTuiPayload, setDeepseekTuiPayload] =
  //   useState<DeepseekTuiPayload>(
  //     provider?.deepseekTui ?? { baseUrl: null, apiKey: null, env: {} }
  //   );
  const deepseekTuiPayload: DeepseekTuiPayload = provider?.deepseekTui ?? {
    baseUrl: null,
    apiKey: null,
    env: {},
  };
  const [geminiPayload, setGeminiPayload] = useState<GeminiPayload>(
    provider?.gemini ?? { baseUrl: null, apiKey: null, env: {} }
  );
  const [hermesPayload, setHermesPayload] = useState<HermesPayload>(
    provider?.hermes ?? {
      baseUrl: null,
      apiMode: null,
      apiKey: null,
      env: {},
    }
  );

  // Agent the user was active on — auto-expand its chevron.
  const [expandedAgents, setExpandedAgents] = useState<Set<AgentKey>>(
    () => new Set(['CLAUDE_CODE'])
  );

  const [haikuFollowMain, setHaikuFollowMain] = useState(
    provider?.claude?.haikuModel == null
  );

  const [enabledModels, setEnabledModels] = useState<EnabledModel[]>(
    provider?.enabledModels ?? []
  );
  const [manualModelId, setManualModelId] = useState('');
  const [saving, setSaving] = useState(false);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [apiKeyVerified, setApiKeyVerified] = useState(false);
  const [fetchedModels, setFetchedModels] = useState<FetchedModel[]>([]);
  const [fetchSearch, setFetchSearch] = useState('');

  const handlePresetSelect = useCallback(
    (presetId: string | null) => {
      setSelectedPresetId(presetId);
      if (!presetId || !catalog) return;
      const preset = catalog.find((p) => p.id === presetId);
      if (!preset) return;
      setName(
        t(`settings.providers.presets.${preset.id}`, {
          defaultValue: preset.name,
        })
      );
      setClaudePayload(preset.claude);
      setCodexPayload(preset.codex);
      setOpencodePayload(preset.opencode);
      // setDeepseekTuiPayload(preset.deepseekTui); // DeepSeek TUI hidden
      setGeminiPayload(preset.gemini);
      setHermesPayload(preset.hermes);
      setHaikuFollowMain(preset.claude?.haikuModel == null);
      // Seed perAgentEnabled from agents[]: true for in-list, false otherwise.
      const seeded: PerAgentEnabled = {};
      for (const a of AGENT_KEYS) seeded[a] = preset.agents.includes(a);
      setPerAgentEnabled(seeded);
      // Seed enabledModels from preset's recommended list.
      setEnabledModels(
        preset.enabledModels.map((id) => ({
          id,
          displayName: id,
          ownedBy: null,
        }))
      );
    },
    [catalog, t]
  );

  // First-load sync when editing an existing provider.
  useEffect(() => {
    if (!provider) return;
    setApiKey(provider.apiKey ?? '');
  }, [provider]);

  const toggleExpanded = useCallback((agent: AgentKey) => {
    setExpandedAgents((prev) => {
      const next = new Set(prev);
      if (next.has(agent)) next.delete(agent);
      else next.add(agent);
      return next;
    });
  }, []);

  const handleFetchModels = async () => {
    setFetchingModels(true);
    setFetchError(null);
    try {
      // Probe whichever per-agent baseUrl is set (Claude first, then OpenAI-compat).
      const baseUrl =
        claudePayload.baseUrl ||
        opencodePayload.baseUrl ||
        codexPayload.baseUrl ||
        // deepseekTuiPayload.baseUrl || // DeepSeek TUI hidden
        '';
      if (!baseUrl) {
        setFetchError(t('settings.providers.form.apiKeyNoEndpoint'));
        return;
      }
      const res = await makeLocalApiRequest('/api/providers/fetch-models', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          base_url: baseUrl,
          api_key: apiKey,
          models_url: null,
        }),
      });
      if (!res.ok) throw new Error(await res.text());
      const body = await res.json();
      const fetched: FetchedModel[] = body.data?.models ?? [];
      setFetchedModels(fetched);
      setApiKeyVerified(true);
      setFetchError(null);
      setFetchSearch('');
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : String(e);
      setApiKeyVerified(false);
      if (/401|unauthori[sz]ed|invalid.*key/i.test(message)) {
        setFetchError(t('settings.providers.form.apiKeyInvalid'));
      } else if (/403|forbidden|permission/i.test(message)) {
        setFetchError(t('settings.providers.form.apiKeyForbidden'));
      } else if (/429|rate.?limit|quota/i.test(message)) {
        setFetchError(t('settings.providers.form.apiKeyQuota'));
      } else if (/request failed|timed? ?out|network|connect/i.test(message)) {
        setFetchError(t('settings.providers.form.apiKeyNetwork'));
      } else {
        setFetchError(t('settings.providers.form.apiKeyGenericError'));
      }
    } finally {
      setFetchingModels(false);
    }
  };

  const addManualModel = () => {
    const id = manualModelId.trim();
    if (!id || enabledModels.some((m) => m.id === id)) return;
    setEnabledModels((prev) => [
      ...prev,
      { id, displayName: id, ownedBy: null },
    ]);
    setManualModelId('');
  };

  const removeModel = (id: string) =>
    setEnabledModels((prev) => prev.filter((m) => m.id !== id));

  const handleSave = async () => {
    setSaving(true);
    try {
      const claude: ClaudePayload = {
        ...claudePayload,
        haikuModel: haikuFollowMain ? null : claudePayload.haikuModel,
      };
      const data: CreateProvider | UpdateProvider = isCreate
        ? {
            name,
            kind: selectedPresetId ? 'Preset' : 'Custom',
            presetId: selectedPresetId,
            apiKey: apiKey || null,
            perAgentEnabled,
            claude,
            codex: codexPayload,
            opencode: opencodePayload,
            deepseekTui: deepseekTuiPayload,
            gemini: geminiPayload,
            hermes: hermesPayload,
            enabledModels,
          }
        : {
            name,
            presetId: provider?.presetId ?? null,
            enabled: provider?.enabled ?? true,
            apiKey: apiKey || null,
            perAgentEnabled,
            claude,
            codex: codexPayload,
            opencode: opencodePayload,
            deepseekTui: deepseekTuiPayload,
            gemini: geminiPayload,
            hermes: hermesPayload,
            enabledModels,
          };
      await onSave(data);
    } finally {
      setSaving(false);
    }
  };

  if (isDefault) {
    return (
      <div className="p-4 text-sm text-muted-foreground">
        {t('settings.providers.form.defaultDescription')}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      {isCreate && catalog && (
        <SettingsCard title={t('settings.providers.form.selectPreset')}>
          <div className="grid grid-cols-3 sm:grid-cols-4 gap-2 max-h-72 overflow-y-auto">
            {catalog.map((preset) => {
              const label = t(`settings.providers.presets.${preset.id}`, {
                defaultValue: preset.name,
              });
              return (
                <button
                  key={preset.id}
                  title={label}
                  onClick={() =>
                    handlePresetSelect(
                      selectedPresetId === preset.id ? null : preset.id
                    )
                  }
                  className={cn(
                    'flex min-h-[56px] flex-col items-center justify-center gap-1 rounded-lg border px-3 py-2.5 text-[13px] text-center transition-colors',
                    selectedPresetId === preset.id
                      ? 'border-brand bg-brand/10 ring-1 ring-brand'
                      : 'border-border hover:bg-muted hover:border-muted-foreground/30'
                  )}
                >
                  <span className="font-medium leading-tight line-clamp-2">
                    {label}
                  </span>
                </button>
              );
            })}
            <button
              onClick={() => handlePresetSelect(null)}
              className={cn(
                'flex min-h-[56px] flex-col items-center justify-center gap-1 rounded-lg border px-3 py-2.5 text-[13px] text-center transition-colors',
                selectedPresetId === null
                  ? 'border-brand bg-brand/10 ring-1 ring-brand'
                  : 'border-border hover:bg-muted hover:border-muted-foreground/30'
              )}
            >
              <PlusIcon className="w-4 h-4" />
              <span>{t('settings.providers.form.custom')}</span>
            </button>
          </div>
        </SettingsCard>
      )}

      <SettingsCard title={t('settings.providers.form.providerDetails')}>
        {selectedPresetId && API_KEY_URL_BY_PRESET[selectedPresetId] && (
          <div className="mb-3 rounded-sm border border-brand/40 bg-brand/5 p-3 text-sm text-normal">
            <p>{t('settings.providers.form.freeTierInfo')}</p>
            <a
              href={API_KEY_URL_BY_PRESET[selectedPresetId]}
              target="_blank"
              rel="noopener noreferrer"
              className="mt-1 inline-block text-brand hover:underline"
            >
              {t('settings.providers.form.createApiKey')}
            </a>
          </div>
        )}
        <SettingsField label={t('settings.providers.form.name')}>
          <InputBase
            value={name}
            onChange={setName}
            placeholder={t('settings.providers.form.namePlaceholder')}
          />
        </SettingsField>
        <SettingsField label={t('settings.providers.form.apiKey')}>
          <InputBase
            type="password"
            value={apiKey}
            onChange={(value) => {
              setApiKey(value);
              setApiKeyVerified(false);
              setFetchError(null);
            }}
            placeholder={t('settings.providers.form.apiKeyPlaceholder')}
          />
          <p className="mt-1 text-xs text-low">
            {t('settings.providers.form.apiKeyTestHint')}
          </p>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void handleFetchModels()}
              disabled={!apiKey.trim() || fetchingModels}
              className="flex items-center gap-1 rounded border border-border px-2 py-1 text-xs hover:bg-muted disabled:opacity-50"
            >
              <ArrowClockwiseIcon
                className={cn('h-3 w-3', fetchingModels && 'animate-spin')}
              />
              {t('settings.providers.form.verifyApiKey')}
            </button>
            {apiKeyVerified && !fetchError && (
              <span role="status" className="text-xs text-success">
                {t('settings.providers.form.apiKeyVerified', {
                  count: fetchedModels.length,
                })}
              </span>
            )}
            {fetchError && (
              <span role="alert" className="text-xs text-error">
                {fetchError}
              </span>
            )}
          </div>
        </SettingsField>
      </SettingsCard>

      <SettingsCard title={t('settings.providers.form.perAgentSettings')}>
        <div className="flex flex-col gap-2">
          <AgentSection
            agent="CLAUDE_CODE"
            enabled={perAgentEnabled.CLAUDE_CODE === true}
            onToggleEnabled={(v) =>
              setPerAgentEnabled((p) => ({ ...p, CLAUDE_CODE: v }))
            }
            expanded={expandedAgents.has('CLAUDE_CODE')}
            onToggleExpanded={() => toggleExpanded('CLAUDE_CODE')}
          >
            <SettingsField label={t('settings.providers.form.baseUrl')}>
              <InputBase
                value={claudePayload.baseUrl ?? ''}
                onChange={(v) =>
                  setClaudePayload((p) => ({ ...p, baseUrl: v || null }))
                }
                placeholder="https://api.example.com/v1"
              />
            </SettingsField>
            <ApiKeyOverrideField
              value={claudePayload.apiKey ?? null}
              onChange={(v) => setClaudePayload((p) => ({ ...p, apiKey: v }))}
            />
            <SettingsField label={t('settings.providers.form.authField')}>
              <div className="flex gap-2">
                {(['ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_API_KEY'] as const).map(
                  (field) => (
                    <button
                      key={field}
                      type="button"
                      onClick={() =>
                        setClaudePayload((p) => ({ ...p, apiKeyField: field }))
                      }
                      className={cn(
                        'text-xs px-2 py-0.5 rounded border',
                        claudePayload.apiKeyField === field
                          ? 'border-brand bg-brand/10'
                          : 'border-border'
                      )}
                    >
                      {field}
                    </button>
                  )
                )}
              </div>
            </SettingsField>
            <SettingsField label={t('settings.providers.form.haikuModel')}>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={haikuFollowMain}
                  onChange={(e) => setHaikuFollowMain(e.target.checked)}
                />
                {t('settings.providers.form.followMainModel')}
              </label>
              {!haikuFollowMain && (
                <InputBase
                  value={claudePayload.haikuModel ?? ''}
                  onChange={(v) =>
                    setClaudePayload((p) => ({ ...p, haikuModel: v || null }))
                  }
                  placeholder="claude-haiku-4.5"
                  className="mt-1"
                />
              )}
            </SettingsField>
            <SettingsField label={t('settings.providers.form.extraEnvVars')}>
              <SettingsTextarea
                value={envMapToText(claudePayload.env)}
                onChange={(v) =>
                  setClaudePayload((p) => ({ ...p, env: textToEnvMap(v) }))
                }
                placeholder="KEY=value"
                monospace
                rows={3}
              />
            </SettingsField>
          </AgentSection>

          <AgentSection
            agent="CODEX"
            enabled={perAgentEnabled.CODEX === true}
            onToggleEnabled={(v) =>
              setPerAgentEnabled((p) => ({ ...p, CODEX: v }))
            }
            expanded={expandedAgents.has('CODEX')}
            onToggleExpanded={() => toggleExpanded('CODEX')}
          >
            <SettingsField label={t('settings.providers.form.baseUrl')}>
              <InputBase
                value={codexPayload.baseUrl ?? ''}
                onChange={(v) =>
                  setCodexPayload((p) => ({ ...p, baseUrl: v || null }))
                }
                placeholder="https://api.example.com/v1"
              />
            </SettingsField>
            <ApiKeyOverrideField
              value={codexPayload.apiKey ?? null}
              onChange={(v) => setCodexPayload((p) => ({ ...p, apiKey: v }))}
            />
            <SettingsField label={t('settings.providers.form.extraEnvVars')}>
              <SettingsTextarea
                value={envMapToText(codexPayload.env)}
                onChange={(v) =>
                  setCodexPayload((p) => ({ ...p, env: textToEnvMap(v) }))
                }
                placeholder="KEY=value"
                monospace
                rows={3}
              />
            </SettingsField>
          </AgentSection>

          <AgentSection
            agent="OPENCODE"
            enabled={perAgentEnabled.OPENCODE === true}
            onToggleEnabled={(v) =>
              setPerAgentEnabled((p) => ({ ...p, OPENCODE: v }))
            }
            expanded={expandedAgents.has('OPENCODE')}
            onToggleExpanded={() => toggleExpanded('OPENCODE')}
          >
            <SettingsField label={t('settings.providers.form.baseUrl')}>
              <InputBase
                value={opencodePayload.baseUrl ?? ''}
                onChange={(v) =>
                  setOpencodePayload((p) => ({ ...p, baseUrl: v || null }))
                }
                placeholder="https://api.example.com/v1"
              />
            </SettingsField>
            <ApiKeyOverrideField
              value={opencodePayload.apiKey ?? null}
              onChange={(v) => setOpencodePayload((p) => ({ ...p, apiKey: v }))}
            />
            <SettingsField label={t('settings.providers.form.extraOptions')}>
              <SettingsTextarea
                value={objectToText(opencodePayload.options)}
                onChange={(v) =>
                  setOpencodePayload((p) => ({
                    ...p,
                    options: textToObject(v),
                  }))
                }
                placeholder={t('settings.providers.form.optionsPlaceholder')}
                monospace
                rows={3}
              />
            </SettingsField>
            <SettingsField label={t('settings.providers.form.extraEnvVars')}>
              <SettingsTextarea
                value={envMapToText(opencodePayload.env)}
                onChange={(v) =>
                  setOpencodePayload((p) => ({ ...p, env: textToEnvMap(v) }))
                }
                placeholder="KEY=value"
                monospace
                rows={3}
              />
            </SettingsField>
          </AgentSection>

          <AgentSection
            agent="GEMINI"
            enabled={perAgentEnabled.GEMINI === true}
            onToggleEnabled={(v) =>
              setPerAgentEnabled((p) => ({ ...p, GEMINI: v }))
            }
            expanded={expandedAgents.has('GEMINI')}
            onToggleExpanded={() => toggleExpanded('GEMINI')}
          >
            <SettingsField label={t('settings.providers.form.baseUrl')}>
              <InputBase
                value={geminiPayload.baseUrl ?? ''}
                onChange={(v) =>
                  setGeminiPayload((p) => ({ ...p, baseUrl: v || null }))
                }
                placeholder="https://generativelanguage.googleapis.com"
              />
            </SettingsField>
            <ApiKeyOverrideField
              value={geminiPayload.apiKey ?? null}
              onChange={(v) => setGeminiPayload((p) => ({ ...p, apiKey: v }))}
            />
            <SettingsField label={t('settings.providers.form.extraEnvVars')}>
              <SettingsTextarea
                value={envMapToText(geminiPayload.env)}
                onChange={(v) =>
                  setGeminiPayload((p) => ({ ...p, env: textToEnvMap(v) }))
                }
                placeholder="KEY=value"
                monospace
                rows={3}
              />
            </SettingsField>
          </AgentSection>

          <AgentSection
            agent="HERMES"
            enabled={perAgentEnabled.HERMES === true}
            onToggleEnabled={(v) =>
              setPerAgentEnabled((p) => ({ ...p, HERMES: v }))
            }
            expanded={expandedAgents.has('HERMES')}
            onToggleExpanded={() => toggleExpanded('HERMES')}
          >
            <SettingsField label={t('settings.providers.form.baseUrl')}>
              <InputBase
                value={hermesPayload.baseUrl ?? ''}
                onChange={(v) =>
                  setHermesPayload((p) => ({ ...p, baseUrl: v || null }))
                }
                placeholder="https://api.example.com/v1"
              />
            </SettingsField>
            <ApiKeyOverrideField
              value={hermesPayload.apiKey ?? null}
              onChange={(v) => setHermesPayload((p) => ({ ...p, apiKey: v }))}
            />
            <SettingsField label={t('settings.providers.form.apiMode')}>
              <select
                value={hermesPayload.apiMode ?? ''}
                onChange={(e) =>
                  setHermesPayload((p) => ({
                    ...p,
                    apiMode: e.target.value || null,
                  }))
                }
                className="w-full bg-secondary border border-border rounded-sm px-2 py-1.5 text-sm text-high"
              >
                <option value="">
                  {t('settings.providers.form.apiModeNone')}
                </option>
                {HERMES_API_MODES.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </SettingsField>
            <SettingsField label={t('settings.providers.form.extraEnvVars')}>
              <SettingsTextarea
                value={envMapToText(hermesPayload.env)}
                onChange={(v) =>
                  setHermesPayload((p) => ({ ...p, env: textToEnvMap(v) }))
                }
                placeholder="KEY=value"
                monospace
                rows={3}
              />
            </SettingsField>
          </AgentSection>
        </div>
      </SettingsCard>

      <SettingsCard title={t('settings.providers.form.models')}>
        {enabledModels.length > 0 && (
          <ul className="flex flex-col gap-1 mb-2 max-h-40 overflow-y-auto">
            {enabledModels.map((m) => (
              <li
                key={m.id}
                className="flex items-center justify-between text-xs px-2 py-1 rounded bg-secondary"
              >
                <span>{m.id}</span>
                <button
                  onClick={() => removeModel(m.id)}
                  className="text-low hover:text-error"
                >
                  <TrashIcon className="w-3 h-3" />
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className="flex gap-2 mb-2">
          <InputBase
            value={manualModelId}
            onChange={setManualModelId}
            placeholder={t('settings.providers.form.manualPlaceholder')}
            className="flex-1"
          />
          <button
            onClick={addManualModel}
            disabled={!manualModelId.trim()}
            className="px-2 py-1 rounded border border-border text-xs hover:bg-muted disabled:opacity-50"
          >
            {t('settings.providers.form.add')}
          </button>
        </div>
        {fetchedModels.length > 0 && (
          <div className="border border-border rounded p-2 flex flex-col gap-2">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-low">
                {t('settings.providers.form.fetchedCount', {
                  count: fetchedModels.length,
                })}
              </span>
              <button
                onClick={() => setFetchedModels([])}
                className="text-low hover:text-high text-xs"
              >
                {t('settings.providers.form.clear')}
              </button>
            </div>
            <InputBase
              value={fetchSearch}
              onChange={setFetchSearch}
              placeholder={t('settings.providers.form.search')}
            />
            <ul className="flex flex-col gap-0.5 max-h-56 overflow-y-auto">
              {fetchedModels
                .filter((m) =>
                  fetchSearch
                    ? m.id.toLowerCase().includes(fetchSearch.toLowerCase())
                    : true
                )
                .slice(0, 200)
                .map((m) => {
                  const enabled = enabledModels.some((e) => e.id === m.id);
                  return (
                    <li
                      key={m.id}
                      className="flex items-center gap-2 text-xs px-2 py-1 rounded hover:bg-muted"
                    >
                      <input
                        type="checkbox"
                        checked={enabled}
                        onChange={(e) => {
                          if (e.target.checked) {
                            setEnabledModels((prev) => [
                              ...prev,
                              {
                                id: m.id,
                                displayName: m.id,
                                ownedBy: m.owned_by,
                              },
                            ]);
                          } else {
                            setEnabledModels((prev) =>
                              prev.filter((x) => x.id !== m.id)
                            );
                          }
                        }}
                      />
                      <span>{m.id}</span>
                      {m.owned_by && (
                        <span className="text-low ml-auto">{m.owned_by}</span>
                      )}
                    </li>
                  );
                })}
            </ul>
          </div>
        )}
      </SettingsCard>

      <div className="flex justify-end gap-2">
        {onCancel && (
          <button
            onClick={onCancel}
            className="px-3 py-1.5 text-sm rounded border border-border hover:bg-muted"
          >
            {t('settings.providers.form.cancel')}
          </button>
        )}
        <button
          onClick={handleSave}
          disabled={saving || !name || enabledModels.length === 0}
          className="flex items-center gap-1 px-3 py-1.5 text-sm rounded bg-brand text-white hover:bg-brand/90 disabled:opacity-50"
          title={
            enabledModels.length === 0
              ? t('settings.providers.form.atLeastOneModel')
              : undefined
          }
        >
          {saving ? (
            <ArrowClockwiseIcon className="w-3 h-3 animate-spin" />
          ) : (
            <CheckIcon className="w-3 h-3" />
          )}
          {resolvedSaveLabel}
        </button>
      </div>
    </div>
  );
}
