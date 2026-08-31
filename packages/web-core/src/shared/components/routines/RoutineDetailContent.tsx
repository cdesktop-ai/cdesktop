import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ArrowClockwiseIcon,
  CaretRightIcon,
  LightningIcon,
  PencilSimpleIcon,
  PlayIcon,
  TrashIcon,
} from '@phosphor-icons/react';
import type {
  Repo,
  Routine,
  RoutineRun,
  RoutineRunStatus,
  UpdateRoutine,
} from 'shared/types';
import { repoApi, routinesApi } from '@/shared/lib/api';
import { Button } from '@vibe/ui/components/Button';
import { IconButton } from '@vibe/ui/components/IconButton';
import { Switch } from '@vibe/ui/components/Switch';
import { ConfirmDialog } from '@/shared/dialogs/shared/ConfirmDialog';
import { useSessionGridStore } from '@/shared/stores/useSessionGridStore';
import {
  useAppNavigation,
  useRoutineNavigation,
} from '@/shared/hooks/useAppNavigation';
import { formatScheduleSummary } from './scheduleFormat';
import { RoutineFormModal } from './RoutineFormModal';
import type { RoutineFormValues } from './RoutineForm';

function formatTimestamp(value: string | null | undefined): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

function statusLabel(
  status: RoutineRunStatus,
  t: (key: string) => string
): string {
  return t(`routines.status.${status}`);
}

function statusBadgeClass(status: RoutineRunStatus): string {
  switch (status) {
    case 'running':
      return 'bg-blue-500/15 text-blue-500';
    case 'done':
      return 'bg-success/15 text-success';
    case 'failed':
      return 'bg-error/15 text-error';
    case 'skipped':
      return 'bg-low/15 text-low';
    case 'pending':
    default:
      return 'bg-secondary text-low';
  }
}

interface RoutineDetailContentProps {
  routineId: string;
}

/**
 * Inner routine-detail view, mounted inside SessionGrid's anchor cell.
 *
 * History rows open the matching workspace in cell 1 of the grid (next to
 * this routines anchor) rather than navigating the whole page to
 * /workspaces/$workspaceId — so the user stays on the routines page and can
 * jump between runs without losing the detail context.
 */
export function RoutineDetailContent({ routineId }: RoutineDetailContentProps) {
  const { t } = useTranslation('common');
  const appNavigation = useAppNavigation();
  const routineNavigation = useRoutineNavigation();
  const queryClient = useQueryClient();
  const [isEditOpen, setIsEditOpen] = useState(false);

  const { data: routine, isLoading } = useQuery<Routine>({
    queryKey: ['routines', routineId],
    queryFn: () => routinesApi.get(routineId),
    enabled: !!routineId,
  });

  const { data: runs = [] } = useQuery<RoutineRun[]>({
    queryKey: ['routines', routineId, 'runs'],
    queryFn: () => routinesApi.listRuns(routineId),
    enabled: !!routineId,
    refetchInterval: 5000,
    refetchOnWindowFocus: true,
  });

  const { data: repo } = useQuery<Repo>({
    queryKey: ['repos', routine?.repo_id],
    queryFn: () => repoApi.getById(routine!.repo_id),
    enabled: !!routine?.repo_id,
  });

  const openWorkspaceInSecondCell = (workspaceId: string) => {
    useSessionGridStore.setState((state) => {
      const grid = state.grid;
      const firstGroup = grid.groups[0];
      if (!firstGroup) return state;

      // Already mounted somewhere → focus that cell without rearranging.
      for (const group of grid.groups) {
        for (const cell of group.cells) {
          if (cell.sessionId === workspaceId) {
            return { grid: { ...grid, focusedCellId: cell.id } };
          }
        }
      }

      const newCell = {
        id:
          typeof crypto !== 'undefined' && 'randomUUID' in crypto
            ? crypto.randomUUID()
            : `cell-${Math.random().toString(36).slice(2)}-${Date.now()}`,
        sessionId: workspaceId,
      };

      // If there's already a second group / second cell, replace the cell
      // immediately to the right of the anchor so the click always lands in
      // the same visual slot.
      if (grid.groups.length === 2) {
        const otherGroup = grid.groups[1];
        const replaced = {
          ...otherGroup,
          cells: [newCell, ...otherGroup.cells.slice(1)],
        };
        return {
          grid: {
            ...grid,
            groups: [firstGroup, replaced],
            focusedCellId: newCell.id,
          },
        };
      }
      if (firstGroup.cells.length === 2) {
        const replaced = {
          ...firstGroup,
          cells: [firstGroup.cells[0], newCell],
        };
        return {
          grid: {
            ...grid,
            groups: [replaced, ...grid.groups.slice(1)],
            focusedCellId: newCell.id,
          },
        };
      }

      // Single anchor cell only → grow into a side-by-side layout. Match
      // splitFromPill's default for `right`: vertical primary orientation.
      return {
        grid: {
          ...grid,
          primaryOrientation: 'vertical',
          groups: [firstGroup, { cells: [newCell], splitRatio: 0.5 }],
          focusedCellId: newCell.id,
        },
      };
    });
  };

  const runNowMutation = useMutation({
    mutationFn: () => routinesApi.runNow(routineId),
    onSuccess: (response) => {
      queryClient.invalidateQueries({
        queryKey: ['routines', routineId, 'runs'],
      });
      queryClient.invalidateQueries({ queryKey: ['routines', routineId] });
      if (response.workspace_id) {
        openWorkspaceInSecondCell(response.workspace_id);
      }
    },
  });

  const updateMutation = useMutation<Routine, Error, UpdateRoutine>({
    mutationFn: (payload) => routinesApi.update(routineId, payload),
    onSuccess: (next) => {
      queryClient.setQueryData(['routines', routineId], next);
      queryClient.invalidateQueries({ queryKey: ['routines'] });
    },
  });

  const deleteMutation = useMutation({
    mutationFn: () => routinesApi.delete(routineId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['routines'] });
      routineNavigation.goToRoutines();
    },
  });

  const folderLabel = useMemo(() => {
    if (!repo) return routine?.repo_id ?? '';
    return repo.display_name || repo.name;
  }, [repo, routine?.repo_id]);

  const handleEditSubmit = (values: RoutineFormValues) => {
    const payload: UpdateRoutine = {
      name: values.name,
      description: values.description,
      instructions: values.instructions,
      target_branch: values.target_branch,
      use_worktree: values.use_worktree,
      executor_config: values.executor_config,
      schedule_kind: values.schedule_kind,
      schedule_time: values.schedule_time,
      schedule_dow: values.schedule_dow,
      enabled: values.enabled,
    };
    updateMutation.mutate(payload, {
      onSuccess: () => setIsEditOpen(false),
    });
  };

  const handleToggleEnabled = (next: boolean) => {
    updateMutation.mutate({ enabled: next });
  };

  const handleDelete = async () => {
    if (!routine) return;
    const result = await ConfirmDialog.show({
      title: t('routines.deleteConfirmTitle'),
      message: t('routines.deleteConfirm', {
        name: routine.name,
        count: runs.length,
      }),
      confirmText: t('routines.delete'),
      cancelText: t('routines.form.cancel'),
      variant: 'destructive',
    });
    if (result === 'confirmed') {
      deleteMutation.mutate();
    }
  };

  if (isLoading) {
    return (
      <div className="h-full bg-primary flex items-center justify-center">
        <p className="text-base text-low">…</p>
      </div>
    );
  }

  if (!routine) {
    return (
      <div className="h-full bg-primary flex items-center justify-center">
        <p className="text-base text-low">Routine not found.</p>
      </div>
    );
  }

  const nextRunStr = routine.next_run_at
    ? formatTimestamp(routine.next_run_at)
    : t('routines.nextRunNever');

  return (
    <div className="flex-1 min-w-0 h-full overflow-auto bg-primary flex flex-col">
      {/* Full-width header pinned to the corners */}
      <header className="flex items-center justify-between gap-base px-double pt-double">
        <div className="flex items-center gap-half text-base text-low min-w-0">
          <button
            type="button"
            onClick={routineNavigation.goToRoutines}
            className="flex items-center gap-half hover:text-normal transition-colors"
          >
            <LightningIcon className="size-icon-xs" />
            {t('routines.title')}
          </button>
          <CaretRightIcon className="size-icon-xs" weight="bold" />
          <span className="text-normal truncate">{routine.name}</span>
        </div>
        <div className="flex items-center gap-double">
          <IconButton
            icon={PencilSimpleIcon}
            onClick={() => setIsEditOpen(true)}
            aria-label={t('routines.edit')}
            title={t('routines.edit')}
          />
          <IconButton
            icon={TrashIcon}
            onClick={handleDelete}
            disabled={deleteMutation.isPending}
            aria-label={t('routines.delete')}
            title={t('routines.delete')}
          />
          <Button
            type="button"
            size="sm"
            onClick={() => runNowMutation.mutate()}
            disabled={runNowMutation.isPending}
          >
            <PlayIcon className="size-icon-xs mr-half" weight="fill" />
            {t('routines.runNow')}
          </Button>
        </div>
      </header>

      {/* Centered body */}
      <div className="flex justify-center">
        <div className="w-chat max-w-full px-[35px] pt-[6vh] pb-[6vh] flex flex-col gap-double">
          {/* Two-column body */}
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-double">
            {/* Left column */}
            <div className="flex flex-col gap-double">
              <section>
                <h2 className="text-sm uppercase tracking-wide text-low mb-half">
                  {t('routines.sections.description')}
                </h2>
                <p className="text-base text-normal whitespace-pre-wrap">
                  {routine.description}
                </p>
              </section>

              <section>
                <h2 className="text-sm uppercase tracking-wide text-low mb-half">
                  {t('routines.sections.status')}
                </h2>
                <div className="flex items-center gap-base">
                  <Switch
                    checked={routine.enabled}
                    onCheckedChange={handleToggleEnabled}
                    disabled={updateMutation.isPending}
                  />
                  <span className="text-base text-normal">
                    {routine.enabled
                      ? t('routines.enabled')
                      : t('routines.disabled')}
                  </span>
                </div>
                {routine.enabled && (
                  <p className="mt-half text-base text-low">
                    {t('routines.nextRun', { when: nextRunStr })}
                  </p>
                )}
              </section>

              <section>
                <h2 className="text-sm uppercase tracking-wide text-low mb-half">
                  {t('routines.sections.folder')}
                </h2>
                <p className="text-base text-normal">{folderLabel}</p>
              </section>

              <section>
                <h2 className="text-sm uppercase tracking-wide text-low mb-half">
                  {t('routines.sections.repeats')}
                </h2>
                <p className="text-base text-normal">
                  {formatScheduleSummary(routine, t)}
                </p>
              </section>
            </div>

            {/* Right column */}
            <div className="flex flex-col gap-double">
              <section>
                <h2 className="text-sm uppercase tracking-wide text-low mb-half">
                  {t('routines.sections.instructions')}
                </h2>
                <pre className="rounded-sm bg-secondary p-base text-base text-normal whitespace-pre-wrap font-sans">
                  {routine.instructions}
                </pre>
              </section>

              <section>
                <div className="flex items-center justify-between mb-half">
                  <h2 className="text-sm uppercase tracking-wide text-low">
                    {t('routines.history.title')}
                  </h2>
                  {runNowMutation.isPending && (
                    <ArrowClockwiseIcon
                      className="size-icon-xs text-low animate-spin"
                      weight="bold"
                    />
                  )}
                </div>
                {runs.length === 0 ? (
                  <p className="text-base text-low">
                    {t('routines.history.empty')}
                  </p>
                ) : (
                  <ul className="flex flex-col gap-half">
                    {runs.map((run) => (
                      <li key={run.id}>
                        <button
                          type="button"
                          disabled={!run.workspace_id}
                          onClick={(event) => {
                            if (!run.workspace_id) return;
                            // cmd/ctrl-click → navigate away (full-page);
                            // plain click → open in cell 1 next to routines.
                            if (event.metaKey || event.ctrlKey) {
                              appNavigation.goToWorkspace(run.workspace_id);
                              return;
                            }
                            openWorkspaceInSecondCell(run.workspace_id);
                          }}
                          className="w-full flex items-center justify-between gap-base rounded-sm bg-secondary px-base py-half text-left hover:bg-panel transition-colors disabled:cursor-default disabled:hover:bg-secondary focus:outline-none focus-visible:ring-1 focus-visible:ring-brand"
                        >
                          <div className="flex flex-col min-w-0">
                            <span className="text-base text-normal truncate">
                              {formatTimestamp(run.scheduled_at)}
                            </span>
                            {run.skip_reason && (
                              <span className="text-xs text-low">
                                {t(
                                  `routines.skipReason.${
                                    run.skip_reason === 'app_offline'
                                      ? 'appOffline'
                                      : run.skip_reason === 'prev_still_running'
                                        ? 'prevStillRunning'
                                        : 'appOffline'
                                  }`
                                )}
                              </span>
                            )}
                          </div>
                          <span
                            className={`text-xs px-base py-[2px] rounded-sm ${statusBadgeClass(
                              run.status
                            )}`}
                          >
                            {statusLabel(run.status, t)}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            </div>
          </div>

          <RoutineFormModal
            open={isEditOpen}
            onOpenChange={setIsEditOpen}
            initial={routine}
            submitting={updateMutation.isPending}
            onSubmit={handleEditSubmit}
          />
        </div>
      </div>
    </div>
  );
}
