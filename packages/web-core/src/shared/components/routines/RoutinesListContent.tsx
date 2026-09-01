import { useTranslation } from 'react-i18next';
import { useQuery } from '@tanstack/react-query';
import {
  LightningIcon,
  PauseCircleIcon,
  PlusIcon,
} from '@phosphor-icons/react';
import type { Routine } from 'shared/types';
import { routinesApi } from '@/shared/lib/api';
import { useRoutineNavigation } from '@/shared/hooks/useAppNavigation';
import { Button } from '@vibe/ui/components/Button';
import { formatScheduleSummary } from './scheduleFormat';

function formatNextRun(nextRunAt: string | null): string | null {
  if (!nextRunAt) return null;
  const date = new Date(nextRunAt);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleString();
}

/**
 * Inner routines-list view, intended to be mounted inside SessionGrid's
 * anchor cell. The outer cell shell (sidebar, focus/opacity) is provided by
 * WorkspacesLayout + SessionGrid.
 */
export function RoutinesListContent() {
  const { t } = useTranslation('common');
  const routineNavigation = useRoutineNavigation();

  const { data: routines = [], isLoading } = useQuery<Routine[]>({
    queryKey: ['routines'],
    queryFn: () => routinesApi.list(),
    refetchOnWindowFocus: true,
  });

  return (
    <div className="flex-1 min-w-0 h-full overflow-auto bg-primary flex justify-center">
      <div className="w-chat max-w-full px-[35px] pt-[9vh] pb-[6vh] flex flex-col gap-double">
        <header className="flex items-start justify-between gap-base">
          <div className="flex flex-col gap-half">
            <h1 className="flex items-center gap-half text-lg font-semibold text-high">
              <LightningIcon className="size-icon-sm" />
              {t('routines.title')}
            </h1>
            <p className="text-base text-low">{t('routines.subtitle')}</p>
          </div>
          <Button type="button" onClick={routineNavigation.goToNewRoutine}>
            <PlusIcon className="size-icon-xs mr-half" weight="bold" />
            {t('routines.newButton')}
          </Button>
        </header>

        <div className="rounded-sm bg-secondary px-base py-half text-base text-low">
          {t('routines.banner')}
        </div>

        {isLoading ? (
          <p className="text-base text-low">…</p>
        ) : routines.length === 0 ? (
          <div className="rounded-sm bg-secondary p-double text-base text-low text-center">
            {t('routines.empty')}
          </div>
        ) : (
          <ul className="flex flex-col gap-base">
            {routines.map((routine) => {
              const nextRun = formatNextRun(routine.next_run_at);
              return (
                <li key={routine.id}>
                  <button
                    type="button"
                    onClick={() => routineNavigation.goToRoutine(routine.id)}
                    className="w-full text-left rounded-sm bg-secondary hover:bg-panel px-double py-base transition-colors focus:outline-none focus-visible:ring-1 focus-visible:ring-brand"
                  >
                    <div className="flex items-start justify-between gap-base">
                      <div className="min-w-0 flex flex-col gap-half">
                        <span className="font-medium text-high truncate">
                          {routine.name}
                        </span>
                        <span className="text-base text-low">
                          {formatScheduleSummary(routine, t)}
                          {routine.enabled && nextRun && (
                            <>
                              {' · '}
                              {t('routines.nextRun', { when: nextRun })}
                            </>
                          )}
                        </span>
                      </div>
                      {!routine.enabled && (
                        <div className="shrink-0 self-center flex items-center gap-half text-sm text-low">
                          <PauseCircleIcon
                            className="size-icon-xs"
                            weight="bold"
                          />
                          {t('routines.disabled')}
                        </div>
                      )}
                    </div>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
