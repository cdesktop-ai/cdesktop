import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { useModelSelectorConfig } from '@/shared/hooks/useExecutorDiscovery';
import { getFirstProjectDestination } from '@/shared/lib/firstProjectDestination';
import { useOrganizationStore } from '@/shared/stores/useOrganizationStore';
import { useUiPreferencesStore } from '@/shared/stores/useUiPreferencesStore';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { StartupFeedbackCard } from './StartupFeedbackCard';

export function RootRedirectPage() {
  const {
    config,
    loading,
    loginStatus,
    systemError,
    reloadSystem,
  } = useUserSystem();
  const { t } = useTranslation('common');
  const setSelectedOrgId = useOrganizationStore((s) => s.setSelectedOrgId);
  const appNavigation = useAppNavigation();
  const discoveryAgent =
    loginStatus?.status === 'loggedin'
      ? (config?.executor_profile.executor ?? null)
      : null;
  const {
    loadingModels,
    loadingAgents,
    error: discoveryError,
    isInitialized: discoveryInitialized,
  } = useModelSelectorConfig(discoveryAgent);
  const [discoveryTimedOut, setDiscoveryTimedOut] = useState(false);
  const [startupDelayed, setStartupDelayed] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [manualGuideStep, setManualGuideStep] = useState<number | null>(null);
  const [setupCommandCopied, setSetupCommandCopied] = useState(false);
  const [copyCommandFailed, setCopyCommandFailed] = useState(false);
  const discoveryReady =
    !discoveryAgent ||
    !!discoveryError ||
    (discoveryInitialized && !loadingModels && !loadingAgents) ||
    discoveryTimedOut;

  useEffect(() => {
    if (systemError && !retrying) {
      setStartupDelayed(true);
      return;
    }
    const timeout = window.setTimeout(() => setStartupDelayed(true), 12000);
    return () => window.clearTimeout(timeout);
  }, [retrying, systemError]);

  const retryConnection = async () => {
    setStartupDelayed(false);
    setRetrying(true);
    try {
      await reloadSystem();
    } finally {
      setRetrying(false);
    }
  };

  const copySetupCommand = async () => {
    try {
      await navigator.clipboard.writeText(
        'powershell -NoProfile -ExecutionPolicy RemoteSigned -File .\\scripts\\setup-windows.ps1'
      );
      setSetupCommandCopied(true);
      setCopyCommandFailed(false);
    } catch {
      setSetupCommandCopied(false);
      setCopyCommandFailed(true);
    }
  };

  useEffect(() => {
    if (!discoveryAgent) return;
    setDiscoveryTimedOut(false);
    const timeout = window.setTimeout(() => setDiscoveryTimedOut(true), 8000);
    return () => window.clearTimeout(timeout);
  }, [discoveryAgent]);

  useEffect(() => {
    if (loading || !config || !discoveryReady) {
      return;
    }

    let isActive = true;
    void (async () => {
      if (!config.remote_onboarding_acknowledged) {
        appNavigation.goToOnboarding({ replace: true });
        return;
      }

      if (loginStatus?.status !== 'loggedin') {
        appNavigation.goToWorkspacesCreate({ replace: true });
        return;
      }

      // Read saved selections imperatively to avoid re-triggering this effect
      // when the scratch store initializes from the server
      const { selectedOrgId, selectedProjectId } =
        useUiPreferencesStore.getState();

      const destination = await getFirstProjectDestination(
        setSelectedOrgId,
        selectedOrgId,
        selectedProjectId
      );
      if (!isActive) {
        return;
      }

      if (destination?.kind === 'project') {
        appNavigation.goToProject(destination.projectId, { replace: true });
        return;
      }

      appNavigation.goToWorkspacesCreate({ replace: true });
    })();

    return () => {
      isActive = false;
    };
  }, [
    appNavigation,
    config,
    discoveryReady,
    loading,
    loginStatus?.status,
    setSelectedOrgId,
  ]);

  return (
    <div className="relative flex h-screen items-center justify-center overflow-y-auto bg-primary px-4 py-8">
      <div className="flex w-full max-w-xl flex-col items-center gap-4 text-center">
        {!startupDelayed && (
          <div className="size-6 animate-spin rounded-full border-2 border-border border-t-brand" />
        )}
        <h1 className="text-base font-medium text-high">
          {t(
            startupDelayed
              ? 'landing.startupDelayed'
              : 'landing.synchronizing'
          )}
        </h1>
        <p className="max-w-sm text-sm text-low">
          {t(
            startupDelayed
              ? 'landing.startupDelayedDescription'
              : 'landing.synchronizingDescription'
          )}
        </p>
        {startupDelayed && (
          <div className="w-full rounded-xl border border-border bg-secondary/60 p-5 text-left text-sm text-low shadow-lg">
            <h2 className="font-medium text-high">
              {t('landing.prerequisitesTitle')}
            </h2>
            <p className="mt-1">{t('landing.prerequisitesDescription')}</p>
            <div className="mt-4 flex flex-wrap gap-2">
              <button
                type="button"
                onClick={() => void copySetupCommand()}
                className="rounded-md bg-brand px-3 py-2 text-sm font-medium text-white hover:opacity-90"
              >
                {t('landing.copySetupCommand')}
              </button>
              <button
                type="button"
                onClick={() => setManualGuideStep(1)}
                className="rounded-md border border-border px-3 py-2 text-sm text-high hover:bg-secondary"
              >
                {t('landing.showManualGuide')}
              </button>
            </div>
            <p className="mt-2 text-xs" aria-live="polite">
              {setupCommandCopied
                ? t('landing.setupCommandCopied')
                : copyCommandFailed
                  ? t('landing.setupCommandCopyFailed')
                  : t('landing.setupCommandHint')}
            </p>
            {systemError && (
              <p
                className="mt-2 break-words rounded bg-primary p-2 font-mono text-xs"
                role="status"
              >
                {systemError}
              </p>
            )}
            <p className="mt-2">{t('landing.startupExample')}</p>
          </div>
        )}
        {startupDelayed && (
          <button
            type="button"
            onClick={() => void retryConnection()}
            disabled={retrying}
            className="rounded-md border border-border px-3 py-2 text-sm text-high hover:bg-secondary"
          >
            {t(retrying ? 'landing.retryingStartup' : 'landing.retryStartup')}
          </button>
        )}
        <StartupFeedbackCard />
      </div>
      {manualGuideStep !== null && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <section
            id="manual-setup-guide"
            role="dialog"
            aria-modal="true"
            aria-labelledby="manual-setup-title"
            className="w-full max-w-lg rounded-xl border border-border bg-primary p-6 text-left shadow-2xl"
          >
            <div className="flex items-start justify-between gap-4">
              <div>
                <h2 id="manual-setup-title" className="text-lg font-semibold text-high">
                  {t('landing.manualGuideTitle')}
                </h2>
                <p className="mt-1 text-sm text-low">
                  {t('landing.manualStepCounter', { current: manualGuideStep, total: 8 })}
                </p>
              </div>
              <button
                type="button"
                onClick={() => setManualGuideStep(null)}
                className="rounded-md px-2 py-1 text-sm text-low hover:bg-secondary hover:text-high"
                aria-label={t('landing.closeManualGuide')}
              >
                ×
              </button>
            </div>
            <div
              className="mt-4 h-1.5 overflow-hidden rounded-full bg-secondary"
              role="progressbar"
              aria-valuemin={1}
              aria-valuemax={8}
              aria-valuenow={manualGuideStep}
              aria-label={t('landing.manualGuideProgress')}
            >
              <div
                className="h-full rounded-full bg-brand transition-[width]"
                style={{ width: `${(manualGuideStep / 8) * 100}%` }}
              />
            </div>
            <p className="mt-6 min-h-24 text-base leading-relaxed text-high">
              {t(`landing.manualStep${manualGuideStep}`)}
            </p>
            <div className="mt-8 flex items-center justify-between gap-3">
              <button
                type="button"
                onClick={() => setManualGuideStep((step) => Math.max(1, (step ?? 1) - 1))}
                disabled={manualGuideStep === 1}
                className="rounded-md border border-border px-3 py-2 text-sm text-high hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-40"
              >
                {t('landing.manualPrevious')}
              </button>
              {manualGuideStep < 8 ? (
                <button
                  type="button"
                  onClick={() => setManualGuideStep((step) => Math.min(8, (step ?? 1) + 1))}
                  className="rounded-md bg-brand px-4 py-2 text-sm font-medium text-white hover:opacity-90"
                >
                  {t('landing.manualNext')}
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => setManualGuideStep(null)}
                  className="rounded-md bg-brand px-4 py-2 text-sm font-medium text-white hover:opacity-90"
                >
                  {t('landing.manualFinish')}
                </button>
              )}
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
