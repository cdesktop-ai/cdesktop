export type ExecutionRoutingExecutor = 'CODEX' | 'CLAUDE_CODE';
export type ExecutionRoutingBillingClass = 'subscription' | 'metered';
export type MeteredFallbackPolicy = 'auto' | 'ask' | 'never';
export type SelectionStatus = 'resolved' | 'approval_needed' | 'blocked';
export type RouteHealthStatus =
  | 'eligible'
  | 'selected'
  | 'cooling'
  | 'approval_required'
  | 'blocked'
  | 'unavailable';

export interface ExecutionRoutingRouteFixture {
  id: string;
  label: string;
  executor: ExecutionRoutingExecutor;
  provider: string;
  model: string;
  billingClass: ExecutionRoutingBillingClass;
  accountPool?: string;
  account?: string;
  accountAlias: string | null;
  health: RouteHealthStatus;
  statusText: string;
  resetAt?: string;
  retryAfterSeconds?: number;
  lastUsedAt?: string;
}

export interface ExecutionRoutingSettingsFixture {
  enabled: boolean;
  meteredFallback: MeteredFallbackPolicy;
  sameRouteRetries: number;
  transientBackoffSeconds: number[];
  approvalTimeoutMinutes: number;
  allRoutesExhausted: 'block';
  notifyOnSwap: boolean;
  exposeAccountAlias: boolean;
  routes: ExecutionRoutingRouteFixture[];
}

export interface ExecutionRoutingSelectionFixture {
  status: SelectionStatus;
  reason: string | null;
  selectedRouteId: string | null;
  preferredModel: string | null;
  trace: string[];
}

export interface AgentRouteFixture {
  id: string;
  name: string;
  role: string;
  currentRouteId: string | null;
  requestedModel: string;
  status: 'running' | 'idle' | 'waiting_for_approval' | 'blocked';
  lastDecision: string;
}

export interface ExecutionRoutingFixture {
  settings: ExecutionRoutingSettingsFixture;
  selection: ExecutionRoutingSelectionFixture;
  agents: AgentRouteFixture[];
}

export const executionRoutingFixture: ExecutionRoutingFixture = {
  settings: {
    enabled: true,
    meteredFallback: 'ask',
    sameRouteRetries: 2,
    transientBackoffSeconds: [5, 20],
    approvalTimeoutMinutes: 0,
    allRoutesExhausted: 'block',
    notifyOnSwap: true,
    exposeAccountAlias: true,
    routes: [
      {
        id: 'codex-luna-subscriptions',
        label: 'Codex subscriptions',
        executor: 'CODEX',
        provider: 'OpenAI',
        model: 'gpt-5.6-luna',
        billingClass: 'subscription',
        accountPool: 'codex',
        accountAlias: 'codex-sub1',
        health: 'selected',
        statusText: 'Selected for new sessions',
        lastUsedAt: '2026-08-18T17:42:00Z',
      },
      {
        id: 'claude-opus-subscriptions',
        label: 'Claude subscriptions',
        executor: 'CLAUDE_CODE',
        provider: 'Anthropic',
        model: 'opus',
        billingClass: 'subscription',
        accountPool: 'claude',
        accountAlias: 'max-a',
        health: 'eligible',
        statusText: 'Eligible fallback',
      },
      {
        id: 'codex-metered-api',
        label: 'Codex metered API',
        executor: 'CODEX',
        provider: 'OpenAI',
        model: 'gpt-5.6-luna',
        billingClass: 'metered',
        account: 'codex-api',
        accountAlias: 'codex-api',
        health: 'approval_required',
        statusText: 'Requires approval when subscription routes exhaust',
      },
      {
        id: 'cooldown-recovery',
        label: 'Cooldown recovery',
        executor: 'CODEX',
        provider: 'OpenAI',
        model: 'gpt-5.6-luna',
        billingClass: 'subscription',
        accountPool: 'codex',
        accountAlias: 'cooling',
        health: 'cooling',
        statusText: 'Cooling down after quota failure',
        resetAt: '2026-08-18T19:05:00Z',
        retryAfterSeconds: 4980,
      },
    ],
  },
  selection: {
    status: 'resolved',
    reason: null,
    selectedRouteId: 'codex-luna-subscriptions',
    preferredModel: null,
    trace: [
      'route codex-luna-subscriptions: codex-sub1 eligible',
      'selected route codex-luna-subscriptions account codex-sub1',
    ],
  },
  agents: [
    {
      id: 'agent-dashboard-shell',
      name: 'Dashboard shell',
      role: 'Frontend lane E',
      currentRouteId: 'codex-luna-subscriptions',
      requestedModel: 'gpt-5.6-luna',
      status: 'running',
      lastDecision: 'subscription-first route selected',
    },
    {
      id: 'agent-contract',
      name: 'Contract bridge',
      role: 'Lane A/B dependency',
      currentRouteId: 'claude-opus-subscriptions',
      requestedModel: 'opus',
      status: 'idle',
      lastDecision: 'cross-provider fallback eligible',
    },
    {
      id: 'agent-metered',
      name: 'Metered review',
      role: 'Recovery fixture',
      currentRouteId: 'codex-metered-api',
      requestedModel: 'gpt-5.6-luna',
      status: 'waiting_for_approval',
      lastDecision: 'meteredFallback=ask requested approval',
    },
  ],
};

export function getExecutionRoutingRouteById(routeId: string | null) {
  if (!routeId) {
    return null;
  }

  return (
    executionRoutingFixture.settings.routes.find(
      (route) => route.id === routeId
    ) ?? null
  );
}
