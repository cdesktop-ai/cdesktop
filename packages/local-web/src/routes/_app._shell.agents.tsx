import { createFileRoute } from '@tanstack/react-router';
import { ExecutionRoutingOverview } from '@/shared/components/execution-routing/ExecutionRoutingSummary';

export const Route = createFileRoute('/_app/_shell/agents')({
  component: ExecutionRoutingOverview,
});
