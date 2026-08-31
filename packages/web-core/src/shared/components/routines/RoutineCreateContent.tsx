import { useTranslation } from 'react-i18next';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { CaretRightIcon, LightningIcon } from '@phosphor-icons/react';
import type { CreateRoutine, Routine } from 'shared/types';
import { routinesApi } from '@/shared/lib/api';
import { useRoutineNavigation } from '@/shared/hooks/useAppNavigation';
import { RoutineForm, type RoutineFormValues } from './RoutineForm';

/**
 * Inner routines-create view, mounted inside SessionGrid's anchor cell.
 */
export function RoutineCreateContent() {
  const { t } = useTranslation('common');
  const routineNavigation = useRoutineNavigation();
  const queryClient = useQueryClient();

  const createMutation = useMutation<Routine, Error, CreateRoutine>({
    mutationFn: (payload) => routinesApi.create(payload),
    onSuccess: (routine) => {
      queryClient.invalidateQueries({ queryKey: ['routines'] });
      routineNavigation.goToRoutine(routine.id);
    },
  });

  const handleSubmit = (values: RoutineFormValues) => {
    const payload: CreateRoutine = {
      name: values.name,
      description: values.description,
      instructions: values.instructions,
      repo_id: values.repo_id,
      target_branch: values.target_branch,
      use_worktree: values.use_worktree,
      executor_config: values.executor_config,
      schedule_kind: values.schedule_kind,
      schedule_time: values.schedule_time,
      schedule_dow: values.schedule_dow,
      enabled: values.enabled,
    };
    createMutation.mutate(payload);
  };

  return (
    <div className="flex-1 min-w-0 h-full overflow-auto bg-primary flex flex-col">
      <header className="flex items-center gap-half px-double pt-double text-base text-low">
        <button
          type="button"
          onClick={routineNavigation.goToRoutines}
          className="flex items-center gap-half hover:text-normal transition-colors"
        >
          <LightningIcon className="size-icon-xs" />
          {t('routines.title')}
        </button>
        <CaretRightIcon className="size-icon-xs" weight="bold" />
        <span className="text-normal">{t('routines.newPageTitle')}</span>
      </header>

      <div className="flex justify-center">
        <div className="w-chat max-w-full px-[35px] pt-[6vh] pb-[6vh] flex flex-col gap-double">
          <RoutineForm
            submitLabel={t('routines.form.submitCreate')}
            submitting={createMutation.isPending}
            onSubmit={handleSubmit}
            onCancel={routineNavigation.goToRoutines}
          />

          {createMutation.isError && (
            <p className="text-sm text-error">
              {createMutation.error?.message ?? 'Failed to create routine'}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
