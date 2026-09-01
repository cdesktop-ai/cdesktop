import {
  CheckCircleIcon,
  ClockCountdownIcon,
  LockKeyIcon,
  ProhibitIcon,
  WarningCircleIcon,
} from '@phosphor-icons/react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { cn } from '@/shared/lib/utils';
import { executionProcessesApi, meteredApprovalsApi } from '@/shared/lib/api';
import { useFirstCellSessionId } from '@/shared/stores/useSessionGridStore';
import type { ExecutionProcessOutcome } from 'shared/types';
import type {
  MeteredApproval,
  MeteredApprovalState,
} from '@/shared/lib/execution-routing/apiTypes';
import {
  executionRoutingFixture,
  getExecutionRoutingRouteById,
  type AgentRouteFixture,
  type ExecutionRoutingRouteFixture,
  type RouteHealthStatus,
} from '@/shared/lib/execution-routing/fixtures';

const healthTone: Record<RouteHealthStatus, string> = {
  selected: 'border-success/40 bg-success/10 text-success',
  eligible: 'border-border bg-secondary text-normal',
  cooling: 'border-brand/40 bg-brand/10 text-brand',
  approval_required: 'border-brand/40 bg-brand/10 text-brand',
  blocked: 'border-error/40 bg-error/10 text-error',
  unavailable: 'border-border bg-secondary text-low',
};

const healthIcon = {
  selected: CheckCircleIcon,
  eligible: CheckCircleIcon,
  cooling: ClockCountdownIcon,
  approval_required: LockKeyIcon,
  blocked: ProhibitIcon,
  unavailable: WarningCircleIcon,
} satisfies Record<RouteHealthStatus, typeof CheckCircleIcon>;

export function RoutingBadge({
  children,
  tone = 'default',
}: {
  children: React.ReactNode;
  tone?: 'default' | 'success' | 'brand' | 'error';
}) {
  return (
    <span
      className={cn(
        'inline-flex min-w-0 items-center rounded-sm border px-half py-[2px] text-xs font-medium',
        tone === 'success' && 'border-success/40 bg-success/10 text-success',
        tone === 'brand' && 'border-brand/40 bg-brand/10 text-brand',
        tone === 'error' && 'border-error/40 bg-error/10 text-error',
        tone === 'default' && 'border-border bg-secondary text-low'
      )}
    >
      <span className="truncate">{children}</span>
    </span>
  );
}

export function RouteHealthBadge({
  route,
}: {
  route: ExecutionRoutingRouteFixture;
}) {
  const Icon = healthIcon[route.health];
  return (
    <span
      className={cn(
        'inline-flex min-w-0 items-center gap-half rounded-sm border px-half py-[2px] text-xs font-medium',
        healthTone[route.health]
      )}
    >
      <Icon className="size-3 shrink-0" weight="bold" />
      <span className="truncate">{route.statusText}</span>
    </span>
  );
}

export function RouteCard({
  route,
  compact = false,
}: {
  route: ExecutionRoutingRouteFixture;
  compact?: boolean;
}) {
  return (
    <div
      className={cn(
        'rounded-sm border border-border bg-panel',
        compact ? 'p-base' : 'p-double'
      )}
    >
      <div className="flex min-w-0 items-start justify-between gap-base">
        <div className="min-w-0">
          <div className="flex min-w-0 flex-wrap items-center gap-half">
            <h3 className="truncate text-base font-medium text-high">
              {route.label}
            </h3>
            <RoutingBadge
              tone={route.billingClass === 'metered' ? 'brand' : 'success'}
            >
              {route.billingClass}
            </RoutingBadge>
          </div>
          <p className="mt-half truncate text-sm text-low">
            {route.executor} / {route.provider} / {route.model}
          </p>
        </div>
        <RouteHealthBadge route={route} />
      </div>

      <div className="mt-base grid gap-half text-sm text-normal sm:grid-cols-2">
        <div>
          <span className="text-low">
            {route.billingClass === 'subscription' ? 'Pool' : 'Account'}
          </span>
          <span className="ml-half font-medium">
            {route.accountPool ?? route.account}
          </span>
        </div>
        <div>
          <span className="text-low">Alias</span>
          <span className="ml-half font-medium">
            {route.accountAlias ?? 'Hidden'}
          </span>
        </div>
        {route.retryAfterSeconds && (
          <div>
            <span className="text-low">Retry</span>
            <span className="ml-half font-medium">
              {Math.ceil(route.retryAfterSeconds / 60)}m
            </span>
          </div>
        )}
        {route.resetAt && (
          <div>
            <span className="text-low">Reset</span>
            <span className="ml-half font-medium">
              {new Date(route.resetAt).toLocaleTimeString([], {
                hour: 'numeric',
                minute: '2-digit',
              })}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

export function AgentRouteRow({ agent }: { agent: AgentRouteFixture }) {
  const route = getExecutionRoutingRouteById(agent.currentRouteId);
  return (
    <div className="grid min-w-0 grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_auto] items-center gap-base border-b border-border px-base py-base last:border-b-0">
      <div className="min-w-0">
        <p className="truncate text-sm font-medium text-high">{agent.name}</p>
        <p className="truncate text-xs text-low">{agent.role}</p>
      </div>
      <div className="min-w-0">
        <p className="truncate text-sm text-normal">
          {route?.label ?? 'No route'}
        </p>
        <p className="truncate text-xs text-low">{agent.lastDecision}</p>
      </div>
      <RoutingBadge
        tone={
          agent.status === 'blocked'
            ? 'error'
            : agent.status === 'waiting_for_approval'
              ? 'brand'
              : 'default'
        }
      >
        {agent.status.replaceAll('_', ' ')}
      </RoutingBadge>
    </div>
  );
}

function approvalTone(state: MeteredApprovalState) {
  return state === 'blocked' || state === 'denied'
    ? 'error'
    : state === 'pending'
      ? 'brand'
      : 'success';
}

function approvalLabel(state: MeteredApprovalState) {
  return state.replaceAll('_', ' ');
}

function OutcomeRow({ outcome }: { outcome: ExecutionProcessOutcome }) {
  return (
    <div className="flex min-w-0 items-center justify-between gap-base border-b border-border px-base py-base last:border-b-0">
      <div className="min-w-0">
        <p className="truncate text-sm font-medium text-high">
          {outcome.outcome.safe_message}
        </p>
        <p className="mt-half truncate text-xs text-low">
          {new Date(outcome.created_at).toLocaleString()}
          {outcome.outcome.provider_code
            ? ` · ${outcome.outcome.provider_code}`
            : ''}
        </p>
      </div>
      <RoutingBadge
        tone={outcome.outcome.class === 'user_stopped' ? 'default' : 'error'}
      >
        {outcome.outcome.class.replaceAll('_', ' ')}
      </RoutingBadge>
    </div>
  );
}

export function MeteredApprovalCard({
  approval,
  onRespond,
  isResponding,
}: {
  approval: MeteredApproval;
  onRespond: (approved: boolean, reason?: string) => void;
  isResponding: boolean;
}) {
  const [reason, setReason] = useState('');
  const pending = approval.state === 'pending';

  return (
    <div className="rounded-sm border border-brand/40 bg-brand/5 p-base">
      <div className="flex min-w-0 items-start justify-between gap-base">
        <div className="min-w-0">
          <p className="text-sm font-medium text-high">Metered execution</p>
          <p className="mt-half truncate text-xs text-low">
            Command {approval.session_command_id}
            {approval.account_alias ? ` · ${approval.account_alias}` : ''}
          </p>
        </div>
        <RoutingBadge tone={approvalTone(approval.state)}>
          {approvalLabel(approval.state)}
        </RoutingBadge>
      </div>
      {approval.reason && (
        <p className="mt-base text-sm text-normal">{approval.reason}</p>
      )}
      {pending && (
        <>
          <textarea
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            placeholder="Optional reason"
            aria-label="Approval reason"
            className="mt-base min-h-16 w-full resize-y rounded-sm border border-border bg-secondary px-half py-half text-sm text-normal placeholder:text-low focus:outline-none focus:ring-1 focus:ring-brand"
          />
          <div className="mt-base flex flex-wrap justify-end gap-half">
            <button
              type="button"
              disabled={isResponding}
              onClick={() => onRespond(false, reason || undefined)}
              className="rounded-sm border border-error/40 px-base py-half text-sm text-error hover:bg-error/10 disabled:opacity-50"
            >
              Deny
            </button>
            <button
              type="button"
              disabled={isResponding}
              onClick={() => onRespond(true, reason || undefined)}
              className="rounded-sm bg-brand px-base py-half text-sm text-white hover:opacity-90 disabled:opacity-50"
            >
              Approve
            </button>
          </div>
        </>
      )}
    </div>
  );
}

export function ExecutionRoutingOverview() {
  const { settings, selection } = executionRoutingFixture;
  const selectedRoute = getExecutionRoutingRouteById(selection.selectedRouteId);
  const sessionId = useFirstCellSessionId();
  const queryClient = useQueryClient();
  const [resolvedApprovals, setResolvedApprovals] = useState<MeteredApproval[]>(
    []
  );
  const approvalsQuery = useQuery({
    queryKey: ['metered-approvals'],
    queryFn: meteredApprovalsApi.listPending,
    refetchInterval: 5_000,
  });
  const respondMutation = useMutation({
    mutationFn: ({
      id,
      approved,
      reason,
    }: {
      id: string;
      approved: boolean;
      reason?: string;
    }) => meteredApprovalsApi.respond(id, { approved, reason }),
    onSuccess: (approval) => {
      setResolvedApprovals((current) => [approval, ...current].slice(0, 5));
      queryClient.invalidateQueries({ queryKey: ['metered-approvals'] });
    },
  });
  const pendingApprovals = approvalsQuery.data ?? [];
  const outcomesQuery = useQuery({
    queryKey: ['execution-outcomes', sessionId],
    queryFn: () => executionProcessesApi.listOutcomes(sessionId),
    enabled: Boolean(sessionId),
    refetchInterval: 5_000,
  });
  const outcomes = outcomesQuery.data ?? [];

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-primary">
      <div className="border-b border-border bg-panel px-double py-base">
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-base">
          <div className="min-w-0">
            <h1 className="text-xl font-semibold text-high">Agents</h1>
            <p className="mt-half text-sm text-low">
              Fixture-backed execution routing shell
            </p>
          </div>
          <div className="flex min-w-0 flex-wrap gap-half">
            <RoutingBadge tone={settings.enabled ? 'success' : 'error'}>
              routing {settings.enabled ? 'enabled' : 'disabled'}
            </RoutingBadge>
            <RoutingBadge tone="brand">
              metered fallback: {settings.meteredFallback}
            </RoutingBadge>
          </div>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-double">
        <div className="grid gap-double xl:grid-cols-[minmax(0,1fr)_360px]">
          <div className="min-w-0 space-y-base">
            <div className="rounded-sm border border-border bg-panel">
              <div className="border-b border-border px-base py-base">
                <h2 className="text-base font-medium text-high">
                  Execution outcomes
                </h2>
                <p className="mt-half text-sm text-low">
                  Normalized outcomes for the active session.
                </p>
              </div>
              {!sessionId ? (
                <p className="px-base py-base text-sm text-low">
                  Select a session to view its outcomes.
                </p>
              ) : outcomesQuery.isError ? (
                <p className="px-base py-base text-sm text-error">
                  Unable to load execution outcomes.
                </p>
              ) : outcomes.length === 0 ? (
                <p className="px-base py-base text-sm text-low">
                  No normalized outcomes recorded for this session.
                </p>
              ) : (
                <div>
                  {outcomes.map((outcome) => (
                    <OutcomeRow
                      key={outcome.execution_process_id}
                      outcome={outcome}
                    />
                  ))}
                </div>
              )}
            </div>
            {(pendingApprovals.length > 0 ||
              resolvedApprovals.length > 0 ||
              approvalsQuery.isError) && (
              <div className="rounded-sm border border-border bg-panel p-double">
                <div className="flex items-center justify-between gap-base">
                  <div>
                    <h2 className="text-base font-medium text-high">
                      Metered approvals
                    </h2>
                    <p className="mt-half text-sm text-low">
                      Durable decisions from the execution dispatcher.
                    </p>
                  </div>
                  {pendingApprovals.length > 0 && (
                    <RoutingBadge tone="brand">
                      {pendingApprovals.length} pending
                    </RoutingBadge>
                  )}
                </div>
                <div className="mt-base space-y-base">
                  {pendingApprovals.map((approval) => (
                    <MeteredApprovalCard
                      key={approval.id}
                      approval={approval}
                      isResponding={respondMutation.isPending}
                      onRespond={(approved, reason) =>
                        respondMutation.mutate({
                          id: approval.id,
                          approved,
                          reason,
                        })
                      }
                    />
                  ))}
                  {resolvedApprovals.map((approval) => (
                    <MeteredApprovalCard
                      key={approval.id}
                      approval={approval}
                      isResponding={false}
                      onRespond={() => undefined}
                    />
                  ))}
                </div>
                {approvalsQuery.isError && (
                  <p className="mt-base text-sm text-error">
                    Unable to load metered approvals.
                  </p>
                )}
              </div>
            )}
            <div className="rounded-sm border border-border bg-panel p-double">
              <p className="text-sm font-medium text-high">Current selection</p>
              <div className="mt-base">
                {selectedRoute ? (
                  <RouteCard route={selectedRoute} compact />
                ) : (
                  <p className="text-sm text-low">
                    No route selected in this fixture.
                  </p>
                )}
              </div>
              <div className="mt-base rounded-sm bg-secondary p-base">
                {selection.trace.map((line) => (
                  <p
                    key={line}
                    className="truncate font-ibm-plex-mono text-xs text-low"
                  >
                    {line}
                  </p>
                ))}
              </div>
            </div>

            <div className="grid gap-base lg:grid-cols-2">
              {settings.routes.map((route) => (
                <RouteCard key={route.id} route={route} />
              ))}
            </div>
          </div>

          <div className="min-w-0 rounded-sm border border-border bg-panel">
            <div className="border-b border-border px-base py-base">
              <h2 className="text-base font-medium text-high">
                Agent bindings
              </h2>
              <p className="mt-half text-sm text-low">
                Local test data for the future route-status API.
              </p>
            </div>
            <div>
              {executionRoutingFixture.agents.map((agent) => (
                <AgentRouteRow key={agent.id} agent={agent} />
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
