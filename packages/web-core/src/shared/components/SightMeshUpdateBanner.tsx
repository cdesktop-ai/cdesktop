import type { SightMeshUpdateStatus } from '@/shared/lib/api';
import { cn } from '@/shared/lib/utils';

interface SightMeshUpdateBannerProps {
  update: SightMeshUpdateStatus;
}

export function SightMeshUpdateBanner({ update }: SightMeshUpdateBannerProps) {
  const version = update.pending_version;
  let message: string | null = null;
  let failed = false;

  if (update.status === 'staged' || update.status === 'waiting-for-idle') {
    message = `${version ? `cdesktop ${version}` : 'A cdesktop update'} is ready and will activate when agents, approvals, and queued messages are idle.`;
  } else if (update.status === 'activating') {
    message = `Updating cdesktop${version ? ` to ${version}` : ''}. New prompts are paused briefly.`;
  } else if (update.status === 'failed') {
    message =
      'The cdesktop update failed and the previous version was restored. Run sightmesh update status for details.';
    failed = true;
  }

  if (!message) return null;

  return (
    <div
      role="status"
      className={cn(
        'w-full border-b border-border bg-secondary px-base py-half text-center text-sm text-low',
        failed && 'text-error'
      )}
    >
      {message}
    </div>
  );
}
