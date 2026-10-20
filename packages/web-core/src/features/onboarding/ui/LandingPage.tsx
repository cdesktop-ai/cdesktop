import {
  forwardRef,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  BookOpenIcon,
  BirdIcon,
  CheckIcon,
  CowIcon,
  DeviceMobileIcon,
  GithubLogoIcon,
  MusicNoteIcon,
  MusicNotesIcon,
  SpeakerHighIcon,
  SpeakerXIcon,
  WarningIcon,
  WaveformIcon,
  type Icon,
} from '@phosphor-icons/react';
import { ProviderForm } from '@/shared/dialogs/settings/settings/ProviderForm';
import { useCreateProvider } from '@/shared/hooks/useProviders';
import { useQueries } from '@tanstack/react-query';
import { configApi } from '@/shared/lib/api';
import type { IconProps } from '@phosphor-icons/react';
import { usePostHog } from 'posthog-js/react';
import { useTranslation } from 'react-i18next';
import { siDiscord } from 'simple-icons';
import {
  BaseCodingAgent,
  EditorType,
  SoundFile,
  ThemeMode,
  type EditorConfig,
} from 'shared/types';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { filterAndSortAgents } from '@/shared/lib/agentOrder';
import { useTheme } from '@/shared/hooks/useTheme';
import { AgentIcon, getAgentName } from '@/shared/components/AgentIcon';
import { IdeIcon } from '@/shared/components/IdeIcon';
import { getIdeName } from '@/shared/lib/ideName';
import { cn, playSound } from '@/shared/lib/utils';
import { isTauriApp } from '@/shared/lib/platform';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';

type SoundOption = {
  value: SoundFile;
  label: string;
  icon: Icon;
};

const SOUND_OPTIONS: SoundOption[] = [
  {
    value: SoundFile.ABSTRACT_SOUND1,
    label: 'Abstract Sound 1',
    icon: WaveformIcon,
  },
  {
    value: SoundFile.ABSTRACT_SOUND2,
    label: 'Abstract Sound 2',
    icon: MusicNoteIcon,
  },
  {
    value: SoundFile.ABSTRACT_SOUND3,
    label: 'Abstract Sound 3',
    icon: MusicNotesIcon,
  },
  {
    value: SoundFile.ABSTRACT_SOUND4,
    label: 'Abstract Sound 4',
    icon: SpeakerHighIcon,
  },
  {
    value: SoundFile.COW_MOOING,
    label: 'Cow Mooing',
    icon: CowIcon,
  },
  {
    value: SoundFile.PHONE_VIBRATION,
    label: 'Phone Vibration',
    icon: DeviceMobileIcon,
  },
  {
    value: SoundFile.ROOSTER,
    label: 'Rooster',
    icon: BirdIcon,
  },
];

const DiscordIcon: Icon = forwardRef<SVGSVGElement, IconProps>(
  ({ className, color = 'currentColor' }, ref) => (
    <svg
      ref={ref}
      className={className}
      viewBox="0 0 24 24"
      fill={color}
      aria-hidden="true"
    >
      <path d={siDiscord.path} />
    </svg>
  )
);
DiscordIcon.displayName = 'DiscordIcon';

const SOCIAL_LINKS = [
  {
    label: 'Discord',
    href: 'https://discord.gg/AC4nwVtJM3',
    icon: DiscordIcon,
  },
  {
    label: 'GitHub',
    href: 'https://github.com/cdesktop-ai/cdesktop',
    icon: GithubLogoIcon,
  },
  {
    label: 'Docs',
    href: 'https://cdesktop.ai/docs',
    icon: BookOpenIcon,
  },
];

const REMOTE_ONBOARDING_EVENTS = {
  STAGE_VIEWED: 'remote_onboarding_ui_stage_viewed',
  STAGE_SUBMITTED: 'remote_onboarding_ui_stage_submitted',
  STAGE_COMPLETED: 'remote_onboarding_ui_stage_completed',
  STAGE_FAILED: 'remote_onboarding_ui_stage_failed',
} as const;

function randomDefaultSoundFile(): SoundFile {
  const randomIndex = Math.floor(Math.random() * SOUND_OPTIONS.length);
  return SOUND_OPTIONS[randomIndex]?.value ?? SoundFile.COW_MOOING;
}

type ProviderGuideId = 'openrouter' | 'gemini' | 'nvidia';

function OnboardingProviderSection({
  selectedAgent,
  onBack,
  onContinue,
}: {
  selectedAgent: BaseCodingAgent;
  onBack: () => void;
  onContinue: () => void;
}) {
  const { t } = useTranslation('common');
  const [guide, setGuide] = useState<ProviderGuideId | null>(null);
  const [showProviderForm, setShowProviderForm] = useState(false);
  const [added, setAdded] = useState(false);
  const createProvider = useCreateProvider();
  const [responseMode, setResponseMode] = useState<'call' | 'chat'>(() =>
    localStorage.getItem('freeclaude-response-mode') === 'call' ? 'call' : 'chat'
  );

  const selectResponseMode = (mode: 'call' | 'chat') => {
    setResponseMode(mode);
    localStorage.setItem('freeclaude-response-mode', mode);
  };

  return (
    <div className="h-screen overflow-y-auto bg-primary p-4 sm:p-8">
      <div className="mx-auto flex min-h-full max-w-4xl flex-col justify-center">
        <div className="rounded-xl border border-border bg-secondary shadow-xl">
          <header className="border-b border-border p-6">
            <p className="text-xs font-medium uppercase tracking-wider text-brand">
              {t('landing.apiSetupProgress')}
            </p>
            <h1 className="mt-2 text-2xl font-semibold text-high">
              {t('landing.apiSetupTitle')}
            </h1>
            <p className="mt-2 max-w-3xl text-sm text-low">
              {t('landing.apiSetupDescription', { agent: getAgentName(selectedAgent) })}
            </p>
          </header>

          <div className="space-y-6 p-6">
            <section>
              <h2 className="text-sm font-semibold text-high">
                {t('landing.apiRecommendationsTitle')}
              </h2>
              <div className="mt-3 grid gap-3 md:grid-cols-3">
                {(['openrouter', 'gemini', 'nvidia'] as const).map((id) => (
                  <article key={id} className="flex flex-col rounded-lg border border-border bg-panel p-4">
                    <p className="font-medium text-high">{t(`landing.api.${id}.name`)}</p>
                    <p className="mt-2 flex-1 text-xs leading-relaxed text-low">
                      {t(`landing.api.${id}.summary`)}
                    </p>
                    <button
                      type="button"
                      onClick={() => setGuide(id)}
                      className="mt-4 self-start text-xs font-medium text-brand hover:underline"
                    >
                      {t('landing.readApiGuide')}
                    </button>
                  </article>
                ))}
              </div>
            </section>

            {guide && (
              <section className="rounded-lg border border-brand/40 bg-brand/5 p-4" aria-live="polite">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <h2 className="font-semibold text-high">
                      {t(`landing.api.${guide}.name`)}
                    </h2>
                    <ol className="mt-3 list-decimal space-y-2 pl-5 text-sm leading-relaxed text-normal">
                      {t(`landing.api.${guide}.guide`)
                        .split(/(?=\d+\.\s)/)
                        .filter(Boolean)
                        .map((step, index) => (
                          <li key={`${guide}-step-${index}`}>
                            {step.replace(/^\d+\.\s/, '')}
                          </li>
                        ))}
                    </ol>
                    {guide === 'gemini' && (
                      <p className="mt-2 text-xs text-low">
                        {t('landing.api.gemini.privacy')}
                      </p>
                    )}
                  </div>
                  <button
                    type="button"
                    onClick={() => setGuide(null)}
                    className="rounded px-2 py-1 text-xs text-low hover:bg-secondary"
                  >
                    {t('landing.closeApiGuide')}
                  </button>
                </div>
                <a
                  href={
                    guide === 'openrouter'
                      ? 'https://openrouter.ai/settings/keys'
                      : guide === 'gemini'
                        ? 'https://aistudio.google.com/app/apikey'
                        : 'https://build.nvidia.com/'
                  }
                  target="_blank"
                  rel="noopener noreferrer"
                  className="mt-3 inline-block text-sm font-medium text-brand hover:underline"
                >
                  {t('landing.createApiKey')}
                </a>
              </section>
            )}

            <section className="rounded-lg border border-border p-4">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <h2 className="text-sm font-semibold text-high">
                    {t('landing.responseStyleTitle')}
                  </h2>
                  <p className="mt-1 text-xs text-low">
                    {t('landing.responseStyleDescription')}
                  </p>
                </div>
                <div role="group" aria-label={t('composer.responseMode.label')} className="flex rounded-md border border-border p-0.5">
                  {(['call', 'chat'] as const).map((mode) => (
                    <button
                      key={mode}
                      type="button"
                      aria-pressed={responseMode === mode}
                      title={t(`composer.responseMode.${mode}Description`)}
                      onClick={() => selectResponseMode(mode)}
                      className={`rounded px-3 py-2 text-sm ${responseMode === mode ? 'bg-accent font-medium text-foreground' : 'text-low hover:text-high'}`}
                    >
                      {t(`composer.responseMode.${mode}`)}
                    </button>
                  ))}
                </div>
              </div>
            </section>

            <section>
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <h2 className="text-sm font-semibold text-high">
                    {t('landing.addApiKeyTitle')}
                  </h2>
                  <p className="mt-1 text-xs text-low">
                    {t('landing.addApiKeyDescription')}
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => setShowProviderForm((open) => !open)}
                  className="rounded-md border border-border px-3 py-2 text-sm text-high hover:bg-primary"
                >
                  {t(showProviderForm ? 'landing.closeProviderForm' : 'landing.openProviderForm')}
                </button>
              </div>
              {added && (
                <p role="status" className="mt-3 flex items-center gap-2 text-sm text-success">
                  <CheckIcon weight="bold" /> {t('landing.providerAdded')}
                </p>
              )}
              {showProviderForm && (
                <div className="mt-4 rounded-lg border border-border bg-panel p-4">
          <ProviderForm
            onSave={async (data) => {
              await createProvider.mutateAsync(
                data as import('shared/types').CreateProvider
              );
              setAdded(true);
              setShowProviderForm(false);
            }}
            onCancel={() => setShowProviderForm(false)}
            saveLabel={t('landing.saveProvider')}
          />
                </div>
              )}
            </section>
          </div>

          <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-border p-6">
            <button type="button" onClick={onBack} className="rounded-md px-3 py-2 text-sm text-low hover:bg-panel hover:text-high">
              {t('landing.backToPreferences')}
            </button>
            <p className="order-last w-full text-center text-xs text-low sm:order-none sm:w-auto">
              {t('landing.apiKeyOptional')}
            </p>
            <div className="flex gap-2">
              <button type="button" onClick={onContinue} className="rounded-md px-3 py-2 text-sm text-low hover:text-high">
                {t('landing.skipApiSetup')}
              </button>
              <PrimaryButton value={t('landing.continueToApp')} onClick={onContinue} />
            </div>
          </footer>
        </div>
      </div>
    </div>
  );
}

function resolveTheme(theme: ThemeMode): 'light' | 'dark' {
  if (theme === ThemeMode.SYSTEM) {
    return window.matchMedia('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light';
  }
  return theme === ThemeMode.DARK ? 'dark' : 'light';
}

export function LandingPage() {
  const appNavigation = useAppNavigation();
  const { t } = useTranslation('common');
  const { theme } = useTheme();
  const { config, profiles, updateAndSaveConfig, loading } = useUserSystem();
  const posthog = usePostHog();

  const [initialized, setInitialized] = useState(false);
  const [showProviderSetup, setShowProviderSetup] = useState(false);
  const [saving, setSaving] = useState(false);
  const [selectedAgent, setSelectedAgent] = useState<BaseCodingAgent>(
    BaseCodingAgent.CLAUDE_CODE
  );
  const [agentSelectionTouched, setAgentSelectionTouched] = useState(false);
  const [editorType, setEditorType] = useState<EditorType>(EditorType.VS_CODE);
  const [editorSelectionTouched, setEditorSelectionTouched] = useState(false);
  const [customCommand, setCustomCommand] = useState('');
  const [soundEnabled, setSoundEnabled] = useState(true);
  const [soundFile, setSoundFile] = useState<SoundFile>(randomDefaultSoundFile);
  const hasTrackedStageViewRef = useRef(false);
  const hasRedirectedToRootRef = useRef(false);

  const trackRemoteOnboardingEvent = useCallback(
    (eventName: string, properties: Record<string, unknown> = {}) => {
      posthog?.capture(eventName, {
        ...properties,
        flow: 'remote_onboarding_ui',
        source: 'frontend',
      });
    },
    [posthog]
  );

  const logoSrc =
    resolveTheme(theme) === 'dark' ? '/favicon.svg' : '/favicon.svg';

  useEffect(() => {
    if (!config || initialized) return;

    setSelectedAgent(config.executor_profile.executor);
    setEditorType(config.editor.editor_type);
    setCustomCommand(config.editor.custom_command || '');
    setInitialized(true);
  }, [config, initialized]);

  useEffect(() => {
    if (!config || !initialized || hasTrackedStageViewRef.current) return;

    trackRemoteOnboardingEvent(REMOTE_ONBOARDING_EVENTS.STAGE_VIEWED, {
      stage: 'landing',
    });
    hasTrackedStageViewRef.current = true;
  }, [config, initialized, trackRemoteOnboardingEvent]);

  useEffect(() => {
    if (
      !config?.remote_onboarding_acknowledged ||
      hasRedirectedToRootRef.current
    ) {
      return;
    }

    hasRedirectedToRootRef.current = true;
    appNavigation.goToRoot({ replace: true });
  }, [appNavigation, config?.remote_onboarding_acknowledged]);

  const executorOptions = useMemo(() => {
    const source = profiles
      ? (Object.keys(profiles) as BaseCodingAgent[])
      : [...Object.values(BaseCodingAgent)];
    return filterAndSortAgents(source);
  }, [profiles]);

  const agentAvailability = useQueries({
    queries: executorOptions.map((agent) => ({
      queryKey: ['onboarding-agent-availability', agent],
      queryFn: () => configApi.checkAgentAvailability(agent),
      staleTime: 5 * 60 * 1000,
      retry: false,
    })),
  });
  const availableEditorOptions = useMemo(
    () => Object.values(EditorType).filter((editor) => editor !== EditorType.CUSTOM),
    []
  );
  const editorAvailability = useQueries({
    queries: availableEditorOptions.map((editor) => ({
      queryKey: ['onboarding-editor-availability', editor],
      queryFn: () => configApi.checkEditorAvailability(editor),
      staleTime: 5 * 60 * 1000,
      retry: false,
    })),
  });

  useEffect(() => {
    if (
      !initialized ||
      agentSelectionTouched ||
      agentAvailability.length === 0 ||
      agentAvailability.some((query) => !query.isFetched)
    ) {
      return;
    }
    const selectedIndex = executorOptions.indexOf(selectedAgent);
    const selectedAvailability = agentAvailability[selectedIndex]?.data?.type;
    const currentAgentAvailable =
      selectedAvailability === 'LOGIN_DETECTED' ||
      selectedAvailability === 'INSTALLATION_FOUND';
    if (currentAgentAvailable) return;

    const recommendedIndex = agentAvailability.findIndex(
      (query) => query.data?.type === 'LOGIN_DETECTED'
    );
    const installedIndex = agentAvailability.findIndex(
      (query) => query.data?.type === 'INSTALLATION_FOUND'
    );
    const availableIndex = recommendedIndex >= 0 ? recommendedIndex : installedIndex;
    if (availableIndex >= 0) {
      setSelectedAgent(executorOptions[availableIndex]);
    }
  }, [
    agentAvailability,
    agentSelectionTouched,
    executorOptions,
    initialized,
    selectedAgent,
  ]);

  useEffect(() => {
    if (
      !initialized ||
      editorSelectionTouched ||
      editorAvailability.length === 0 ||
      editorAvailability.some((query) => !query.isFetched)
    ) {
      return;
    }
    const selectedIndex = availableEditorOptions.findIndex(
      (editor) => editor === editorType
    );
    if (selectedIndex >= 0 && editorAvailability[selectedIndex]?.data?.available) {
      return;
    }
    const recommendedIndex = editorAvailability.findIndex(
      (query) => query.data?.available
    );
    if (recommendedIndex >= 0) {
      setEditorType(availableEditorOptions[recommendedIndex]);
    }
  }, [
    availableEditorOptions,
    editorAvailability,
    editorSelectionTouched,
    editorType,
    initialized,
  ]);

  const editorOptions = useMemo(() => Object.values(EditorType), []);

  const previewSound = async (value: SoundFile) => {
    try {
      await playSound(`/api/sounds/${value}`);
    } catch (err) {
      console.error('Failed to play sound:', err);
    }
  };

  const openExternalLink = (url: string) => {
    window.open(url, '_blank', 'noopener,noreferrer');
  };

  const handleSoundSelect = (value: SoundFile) => {
    setSoundEnabled(true);
    setSoundFile(value);
    void previewSound(value);
  };

  const isCustomEditorValid =
    editorType !== EditorType.CUSTOM || customCommand.trim() !== '';
  const canContinue = !saving && isCustomEditorValid;

  const handleContinue = async () => {
    if (!config || !canContinue) return;

    const editorConfig: EditorConfig = {
      editor_type: editorType,
      custom_command:
        editorType === EditorType.CUSTOM ? customCommand.trim() : null,
      remote_ssh_host: null,
      remote_ssh_user: null,
      auto_install_extension: true,
    };

    trackRemoteOnboardingEvent(REMOTE_ONBOARDING_EVENTS.STAGE_SUBMITTED, {
      stage: 'landing',
      method: 'continue',
      selected_agent: selectedAgent,
      editor_type: editorType,
      custom_editor_command_set:
        editorType === EditorType.CUSTOM && customCommand.trim() !== '',
      sound_enabled: soundEnabled,
      sound_file: soundEnabled ? soundFile : null,
    });

    setSaving(true);
    const success = await updateAndSaveConfig({
      onboarding_acknowledged: true,
      disclaimer_acknowledged: true,
      executor_profile: {
        executor: selectedAgent,
        variant: null,
      },
      editor: editorConfig,
      notifications: {
        ...config.notifications,
        sound_enabled: soundEnabled,
        sound_file: soundFile,
      },
    });
    setSaving(false);

    if (success) {
      trackRemoteOnboardingEvent(REMOTE_ONBOARDING_EVENTS.STAGE_COMPLETED, {
        stage: 'landing',
        destination: '/onboarding/providers',
      });
      setShowProviderSetup(true);
      return;
    }

    trackRemoteOnboardingEvent(REMOTE_ONBOARDING_EVENTS.STAGE_FAILED, {
      stage: 'landing',
      reason: 'config_save_failed',
    });
  };

  if (loading || !config || !initialized) {
    return (
      <div className="h-screen bg-primary flex items-center justify-center">
        <p className="text-low">{t('landing.loading')}</p>
      </div>
    );
  }

  if (config.remote_onboarding_acknowledged) {
    return null;
  }

  if (showProviderSetup) {
    return (
      <OnboardingProviderSection
        selectedAgent={selectedAgent}
        onBack={() => setShowProviderSetup(false)}
        onContinue={() => appNavigation.goToOnboardingSignIn({ replace: true })}
      />
    );
  }

  return (
    <div className="h-screen bg-primary flex items-center justify-center p-double">
      {isTauriApp() && (
        <div
          data-tauri-drag-region
          className="fixed inset-x-0 top-0 h-10 z-10"
        />
      )}
      <div className="flex max-h-full w-full max-w-5xl flex-col rounded-sm border border-border bg-secondary">
        {/* Header */}
        <header className="shrink-0 space-y-base p-double pb-base">
          <div className="flex items-center justify-between">
            <img
              src={logoSrc}
              alt="FreeClaude Voice"
              className="h-8 w-auto logo"
            />
            <div className="flex flex-wrap items-center gap-2">
              {SOCIAL_LINKS.map((link) => (
                <PrimaryButton
                  key={link.label}
                  value={link.label}
                  variant="tertiary"
                  actionIcon={link.icon}
                  onClick={() => openExternalLink(link.href)}
                />
              ))}
            </div>
          </div>
            <div>
              <h1 className="text-xl font-semibold text-high">
                {t('landing.preferencesTitle')}
              </h1>
              <p className="mt-1 text-sm text-low">
                {t('landing.preferencesDescription')}
              </p>
            </div>
            <div className="rounded-sm border border-brand bg-brand/20 p-base">
            <div className="flex items-start gap-base">
              <WarningIcon
                className="size-icon-sm text-brand shrink-0 mt-[2px]"
                weight="fill"
              />
              <p className="text-sm text-normal">
                {t('landing.safetyNotice')}{' '}
                <a
                  href="https://cdesktop.ai/docs/safety-notice"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-brand hover:underline"
                >
                  {t('landing.learnMore')}
                </a>
                .
              </p>
            </div>
          </div>
        </header>

        {/* 3-column grid */}
        <div className="min-h-0 flex-1 overflow-y-auto px-double pb-double">
          <div className="grid grid-cols-1 gap-double md:grid-cols-3">
            {/* Column 1: Coding Agent */}
            <section className="space-y-half">
              <h2 className="text-sm font-medium text-high">{t('landing.codingAgentHeading')}</h2>
              <div className="grid gap-1.5">
                {executorOptions.map((agent) => {
                  const selected = selectedAgent === agent;

                  return (
                    <button
                      key={agent}
                      type="button"
                      onClick={() => {
                        setAgentSelectionTouched(true);
                        setSelectedAgent(agent);
                      }}
                      className={cn(
                        'flex items-center gap-base rounded-sm border px-base py-half text-left',
                        selected
                          ? 'border-brand bg-brand/10'
                          : 'border-border bg-panel hover:bg-primary'
                      )}
                    >
                      <AgentIcon
                        agent={agent}
                        className="size-icon-xl shrink-0"
                      />
                      <span className="text-sm text-normal flex-1 truncate">
                        {getAgentName(agent)}
                      </span>
                      {selected && (
                        <CheckIcon
                          className="size-icon-xs text-brand shrink-0"
                          weight="bold"
                        />
                      )}
                    </button>
                  );
                })}
              </div>
            </section>

            {/* Column 2: Code Editor */}
            <section className="space-y-half">
              <h2 className="text-sm font-medium text-high">{t('landing.codeEditorHeading')}</h2>
              <div className="grid gap-1.5">
                {editorOptions.map((editor) => {
                  const selected = editorType === editor;

                  return (
                    <button
                      key={editor}
                      type="button"
                      onClick={() => {
                        setEditorSelectionTouched(true);
                        setEditorType(editor);
                      }}
                      className={cn(
                        'flex items-center gap-base rounded-sm border px-base py-half text-left',
                        selected
                          ? 'border-brand bg-brand/10'
                          : 'border-border bg-panel hover:bg-primary'
                      )}
                    >
                      <IdeIcon
                        editorType={editor}
                        className="size-icon-sm shrink-0"
                      />
                      <span className="text-sm text-normal flex-1 truncate">
                        {getIdeName(editor)}
                      </span>
                      {selected && (
                        <CheckIcon
                          className="size-icon-xs text-brand shrink-0"
                          weight="bold"
                        />
                      )}
                    </button>
                  );
                })}
              </div>

              {editorType === EditorType.CUSTOM && (
                <div className="space-y-half">
                  <label className="text-sm font-medium text-normal">
                    {t('landing.customCommandLabel')}
                  </label>
                  <input
                    type="text"
                    value={customCommand}
                    onChange={(e) => setCustomCommand(e.target.value)}
                    placeholder={t('landing.customCommandPlaceholder')}
                    className={cn(
                      'w-full bg-panel border rounded-sm px-base py-half text-sm text-high',
                      'placeholder:text-low placeholder:opacity-80 focus:outline-none',
                      'focus:ring-1 focus:ring-brand',
                      customCommand.trim() === ''
                        ? 'border-warning/60'
                        : 'border-border'
                    )}
                  />
                </div>
              )}
            </section>

            {/* Column 3: Notification Sound */}
            <section className="space-y-half">
              <h2 className="text-sm font-medium text-high">
                {t('landing.notificationSoundHeading')}
              </h2>
              <div className="grid gap-1.5">
                {SOUND_OPTIONS.map((option) => {
                  const Icon = option.icon;
                  const selected = soundEnabled && soundFile === option.value;

                  return (
                    <button
                      key={option.value}
                      type="button"
                      onClick={() => handleSoundSelect(option.value)}
                      className={cn(
                        'flex items-center gap-base rounded-sm border px-base py-half text-left',
                        selected
                          ? 'border-brand bg-brand/10'
                          : 'border-border bg-panel hover:bg-primary'
                      )}
                    >
                      <Icon
                        className={cn(
                          'size-icon-sm shrink-0',
                          selected ? 'text-brand' : 'text-normal'
                        )}
                        weight={selected ? 'fill' : 'bold'}
                      />
                      <span className="text-sm text-normal flex-1 truncate">
                        {option.label}
                      </span>
                      {selected && (
                        <CheckIcon
                          className="size-icon-xs text-brand shrink-0"
                          weight="bold"
                        />
                      )}
                    </button>
                  );
                })}
                <button
                  type="button"
                  onClick={() => setSoundEnabled(false)}
                  className={cn(
                    'flex items-center gap-base rounded-sm border px-base py-half text-left',
                    !soundEnabled
                      ? 'border-brand bg-brand/10'
                      : 'border-border bg-panel hover:bg-primary'
                  )}
                >
                  <SpeakerXIcon
                    className={cn(
                      'size-icon-sm shrink-0',
                      !soundEnabled ? 'text-brand' : 'text-normal'
                    )}
                    weight={!soundEnabled ? 'fill' : 'bold'}
                  />
                  <span className="text-sm text-normal flex-1">{t('landing.noSound')}</span>
                  {!soundEnabled && (
                    <CheckIcon
                      className="size-icon-xs text-brand shrink-0"
                      weight="bold"
                    />
                  )}
                </button>
              </div>
            </section>
          </div>
        </div>

        {/* Footer */}
        <div className="shrink-0 border-t border-border p-double pt-base flex items-center justify-between gap-base">
          <p className="text-xs text-low">
            By continuing you agree to the{' '}
            <a
              href="https://cdesktop.ai/terms"
              target="_blank"
              rel="noopener noreferrer"
              className="text-brand hover:underline"
            >
              {t('landing.terms')}
            </a>{' '}
            and{' '}
            <a
              href="https://cdesktop.ai/privacy"
              target="_blank"
              rel="noopener noreferrer"
              className="text-brand hover:underline"
            >
              {t('landing.privacyPolicy')}
            </a>
            .
          </p>
          <PrimaryButton
            value={saving ? t('landing.saving') : t('landing.continue')}
            onClick={handleContinue}
            disabled={!canContinue}
          />
        </div>
      </div>
    </div>
  );
}
