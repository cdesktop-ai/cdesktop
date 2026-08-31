import { useState } from 'react';
import {
  CheckCircleIcon,
  ClockCounterClockwiseIcon,
  LockKeyIcon,
} from '@phosphor-icons/react';
import { useQuery } from '@tanstack/react-query';
import { Switch } from '@vibe/ui/components/Switch';
import { cn } from '@/shared/lib/utils';
import { executionRoutingApi } from '@/shared/lib/api';
import type { ExecutionRoutingSettings } from 'shared/types';
import {
  RouteCard,
  RoutingBadge,
} from '@/shared/components/execution-routing/ExecutionRoutingSummary';
import {
  executionRoutingFixture,
  type ExecutionRoutingRouteFixture,
  type MeteredFallbackPolicy,
} from '@/shared/lib/execution-routing/fixtures';
import { SettingsCard, SettingsField } from './SettingsComponents';

const fallbackOptions: {
  value: MeteredFallbackPolicy;
  label: string;
  description: string;
}[] = [
  {
    value: 'auto',
    label: 'Auto',
    description: 'Use the first eligible metered route after subscriptions.',
  },
  {
    value: 'ask',
    label: 'Ask',
    description: 'Pause for approval before a metered route is used.',
  },
  {
    value: 'never',
    label: 'Never',
    description: 'Block instead of resolving a metered route.',
  },
];

/// A live route carries only what sightmesh persists. Its health, resolved
/// provider and account alias are runtime state that the settings file does
/// not hold, so they are shown as unavailable rather than invented.
function LiveRouteRow({
  route,
  position,
}: {
  route: ExecutionRoutingSettings['routes'][number];
  position: number;
}) {
  return (
    <div className="grid gap-base md:grid-cols-[32px_minmax(0,1fr)]">
      <div className="flex h-8 w-8 items-center justify-center rounded-sm border border-border bg-secondary text-sm font-medium text-low">
        {position}
      </div>
      <div className="rounded-sm border border-border bg-panel p-base">
        <div className="flex min-w-0 flex-wrap items-center gap-half">
          <h3 className="truncate text-base font-medium text-high">
            {route.id}
          </h3>
          <RoutingBadge
            tone={route.billingClass === 'metered' ? 'brand' : 'success'}
          >
            {route.billingClass}
          </RoutingBadge>
        </div>
        <p className="mt-half truncate text-sm text-low">
          {route.executor} / {route.model}
        </p>
        {(route.accountPool ?? route.account) && (
          <div className="mt-base text-sm text-normal">
            <span className="text-low">
              {route.accountPool ? 'Pool' : 'Account'}
            </span>
            <span className="ml-half font-medium">
              {route.accountPool ?? route.account}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

export function ExecutionRoutingSettingsSection() {
  const settingsQuery = useQuery({
    queryKey: ['execution-routing-settings'],
    queryFn: executionRoutingApi.getSettings,
  });
  // `null` means sightmesh has not configured routing on this host.
  const live = settingsQuery.data ?? null;
  // cdesktop only reads these settings, so the controls stay interactive
  // exclusively in fixture mode - a toggle that cannot persist would lie.
  const [preview, setPreview] = useState(executionRoutingFixture.settings);
  const source = live ?? preview;
  const readOnly = live !== null;

  return (
    <div className="space-y-6">
      <SettingsCard
        title="Execution routing"
        description={
          readOnly
            ? 'Live routing settings owned by sightmesh. cdesktop reads them; changes are made through sightmesh.'
            : 'Fixture-only shell for subscription-first agent routing. No routing settings are configured on this host.'
        }
      >
        {settingsQuery.isError && (
          <p className="rounded-sm border border-error/40 bg-panel px-base py-base text-sm text-error">
            Unable to read routing settings; showing fixtures instead.
          </p>
        )}
        <div className="grid gap-base md:grid-cols-3">
          <div className="rounded-sm border border-border bg-panel p-base">
            <div className="flex items-center gap-half text-sm font-medium text-high">
              <CheckCircleIcon className="size-icon-sm text-success" />
              Ordered routes
            </div>
            <p className="mt-half text-sm text-low">
              {source.routes.length} configured{' '}
              {readOnly ? 'routes' : 'fixtures'}
            </p>
          </div>
          <div className="rounded-sm border border-border bg-panel p-base">
            <div className="flex items-center gap-half text-sm font-medium text-high">
              <LockKeyIcon className="size-icon-sm text-brand" />
              Metered fallback
            </div>
            <p className="mt-half text-sm text-low">{source.meteredFallback}</p>
          </div>
          <div className="rounded-sm border border-border bg-panel p-base">
            <div className="flex items-center gap-half text-sm font-medium text-high">
              <ClockCounterClockwiseIcon className="size-icon-sm text-low" />
              Retry policy
            </div>
            <p className="mt-half text-sm text-low">
              {source.sameRouteRetries} retries /{' '}
              {source.transientBackoffSeconds.join(', ')}s
            </p>
          </div>
        </div>

        <SettingsField
          label="Routing enabled"
          description={
            readOnly
              ? 'Persisted by sightmesh. This view is read-only.'
              : 'Local preview state only. The backend API seam must own the persisted value.'
          }
        >
          <div className="flex items-center justify-between rounded-sm border border-border bg-panel px-base py-base">
            <span className="text-sm text-normal">
              Resolve new sessions through ordered routes
            </span>
            <Switch
              checked={source.enabled}
              disabled={readOnly}
              onCheckedChange={(enabled) =>
                setPreview((current) => ({ ...current, enabled }))
              }
            />
          </div>
        </SettingsField>

        <SettingsField
          label="Metered fallback"
          description={
            readOnly
              ? 'Persisted by sightmesh. This view is read-only.'
              : 'Mirrors the frozen contract values: auto, ask, never.'
          }
        >
          <div className="grid gap-base md:grid-cols-3">
            {fallbackOptions.map((option) => {
              const selected = source.meteredFallback === option.value;
              return (
                <button
                  key={option.value}
                  type="button"
                  disabled={readOnly}
                  onClick={() =>
                    setPreview((current) => ({
                      ...current,
                      meteredFallback: option.value,
                    }))
                  }
                  className={cn(
                    'rounded-sm border p-base text-left transition-colors',
                    selected
                      ? 'border-brand bg-brand/10'
                      : 'border-border bg-panel',
                    readOnly ? 'cursor-default' : 'hover:bg-secondary',
                    readOnly && !selected && 'opacity-60'
                  )}
                >
                  <span className="text-sm font-medium text-high">
                    {option.label}
                  </span>
                  <span className="mt-half block text-sm text-low">
                    {option.description}
                  </span>
                </button>
              );
            })}
          </div>
        </SettingsField>
      </SettingsCard>

      <SettingsCard
        title="Route order"
        description={
          readOnly
            ? 'Routes are evaluated top to bottom. Live route health is not part of the settings contract and is not shown.'
            : 'Routes are evaluated top to bottom; subscription routes can cross provider and model before metered fallback policy is applied.'
        }
      >
        <div className="space-y-base">
          {live
            ? live.routes.map((route, index) => (
                <LiveRouteRow
                  key={route.id}
                  route={route}
                  position={index + 1}
                />
              ))
            : preview.routes.map(
                (route: ExecutionRoutingRouteFixture, index: number) => (
                  <div
                    key={route.id}
                    className="grid gap-base md:grid-cols-[32px_minmax(0,1fr)]"
                  >
                    <div className="flex h-8 w-8 items-center justify-center rounded-sm border border-border bg-secondary text-sm font-medium text-low">
                      {index + 1}
                    </div>
                    <RouteCard route={route} compact />
                  </div>
                )
              )}
        </div>
      </SettingsCard>

      <SettingsCard
        title="Decision states"
        description="Fixture examples for the status vocabulary that A1/B still need to expose through typed APIs."
      >
        <div className="flex flex-wrap gap-half">
          <RoutingBadge tone="success">resolved</RoutingBadge>
          <RoutingBadge tone="brand">approval needed</RoutingBadge>
          <RoutingBadge tone="error">blocked</RoutingBadge>
          <RoutingBadge>cooldown</RoutingBadge>
          <RoutingBadge>reset pending</RoutingBadge>
          <RoutingBadge>retry backoff</RoutingBadge>
        </div>
      </SettingsCard>
    </div>
  );
}
