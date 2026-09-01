import { useTranslation } from 'react-i18next';
import { useQuery } from '@tanstack/react-query';
import { CheckIcon, GitBranchIcon, SpinnerIcon } from '@phosphor-icons/react';
import type { GitBranch, Repo } from 'shared/types';
import { repoApi } from '@/shared/lib/api';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';

export const chipClassName =
  'inline-flex items-center gap-half rounded-md bg-secondary px-base py-half ' +
  'min-h-7 text-sm text-normal hover:bg-panel ' +
  'disabled:cursor-not-allowed disabled:opacity-50 ' +
  'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand';

interface BranchMenuListProps {
  repo: Repo;
  currentBranch: string | null | undefined;
  onSelectBranch: (branch: string) => void;
  allowCurrent?: boolean;
  currentLabel?: string;
  onSelectCurrent?: () => void;
}

/**
 * Branch list as DropdownMenu items. Renders nothing when the repo is
 * non-Git — callers gate on `repo.is_git`.
 */
export function BranchMenuList({
  repo,
  currentBranch,
  onSelectBranch,
  allowCurrent,
  currentLabel,
  onSelectCurrent,
}: BranchMenuListProps) {
  const { t } = useTranslation();
  const isGit = repo.is_git;

  const { data: branches = [], isLoading } = useQuery<GitBranch[]>({
    queryKey: ['repos', repo.id, 'branches'],
    queryFn: () => repoApi.getBranches(repo.id),
    enabled: isGit,
    staleTime: 30_000,
  });

  if (!isGit) return null;

  if (isLoading) {
    return (
      <DropdownMenuItem disabled icon={SpinnerIcon}>
        <span className="text-low">
          {t('states.loading', { defaultValue: 'Loading…' })}
        </span>
      </DropdownMenuItem>
    );
  }
  if (branches.length === 0) {
    return (
      <DropdownMenuItem disabled>
        <span className="text-low">
          {t('createMode.composerChips.noBranches', {
            defaultValue: 'No branches',
          })}
        </span>
      </DropdownMenuItem>
    );
  }
  return (
    <>
      {allowCurrent && (
        <DropdownMenuItem
          icon={!currentBranch ? CheckIcon : GitBranchIcon}
          onSelect={() => onSelectCurrent?.()}
        >
          <span className="truncate">{currentLabel ?? 'Current branch'}</span>
        </DropdownMenuItem>
      )}
      {branches.map((b) => (
        <DropdownMenuItem
          key={b.name}
          icon={b.name === currentBranch ? CheckIcon : GitBranchIcon}
          onSelect={() => onSelectBranch(b.name)}
        >
          <span className="truncate">{b.name}</span>
        </DropdownMenuItem>
      ))}
    </>
  );
}

interface BranchChipProps {
  repo: Repo;
  currentBranch: string | null | undefined;
  onSelectBranch: (branch: string) => void;
  disabled?: boolean;
  allowCurrent?: boolean;
  currentLabel?: string;
  onSelectCurrent?: () => void;
}

/**
 * Composer-style chip + dropdown for picking a branch on a single repo.
 * Used by the new-session composer (primary branch) and the routines form.
 */
export function BranchChip({
  repo,
  currentBranch,
  onSelectBranch,
  disabled,
  allowCurrent,
  currentLabel,
  onSelectCurrent,
}: BranchChipProps) {
  const { t } = useTranslation();
  if (!repo.is_git) return null;
  const placeholder = t('createMode.composerChips.pickBranch', {
    defaultValue: 'Select branch',
  });
  const label = currentBranch
    ? currentBranch
    : allowCurrent
      ? (currentLabel ?? 'Current branch')
      : placeholder;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" disabled={disabled} className={chipClassName}>
          <GitBranchIcon weight="bold" className="size-icon-xs" />
          <span className="max-w-[140px] truncate">{label}</span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <BranchMenuList
          repo={repo}
          currentBranch={currentBranch}
          onSelectBranch={onSelectBranch}
          allowCurrent={allowCurrent}
          currentLabel={currentLabel}
          onSelectCurrent={onSelectCurrent}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
