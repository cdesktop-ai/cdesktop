import { FolderIcon, PlusIcon, TrashIcon } from '@phosphor-icons/react';
import { useCallback, useEffect, useMemo } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { GitBranch, Repo } from 'shared/types';

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';
import { FolderPickerPopover } from '@vibe/ui/components/FolderPickerPopover';
import { Checkbox } from '@vibe/ui/components/Checkbox';
import { useCreateMode } from '@/features/create-mode/model/useCreateMode';
import { repoApi } from '@/shared/lib/api';
import { FolderPickerDialog } from '@/shared/dialogs/shared/FolderPickerDialog';
import { BranchChip, BranchMenuList, chipClassName } from './BranchChip';

function repoDisplayName(repo: Repo): string {
  return repo.display_name || repo.name;
}

export function ComposerChipRow({ disabled }: { disabled?: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const {
    repos,
    addRepo,
    removeRepo,
    clearRepos,
    targetBranches,
    setTargetBranch,
    useWorktree,
    setUseWorktree,
  } = useCreateMode();

  const primary = repos[0] ?? null;
  const secondaries = useMemo(() => repos.slice(1), [repos]);

  const { data: recents = [] } = useQuery<Repo[]>({
    queryKey: ['repos', 'recent'],
    queryFn: () => repoApi.listRecent(),
    staleTime: 30_000,
  });

  const attachedIds = useMemo(() => new Set(repos.map((r) => r.id)), [repos]);

  // Folder picks no longer pop a branch dialog. Seed target_branch with the
  // repo's currently-checked-out branch (HEAD), falling back to
  // `default_target_branch`. User adjusts via the branch chip if they care.
  // The fetch also warms the BranchMenuList cache so the dropdown is instant.
  const resolveInitialBranch = useCallback(
    async (repo: Repo): Promise<string | null> => {
      if (!repo.is_git) return null;
      try {
        const branches = await queryClient.fetchQuery<GitBranch[]>({
          queryKey: ['repos', repo.id, 'branches'],
          queryFn: () => repoApi.getBranches(repo.id),
          staleTime: 30_000,
        });
        const current = branches.find((b) => b.is_current);
        if (current) return current.name;
      } catch {
        // Fall through to default_target_branch
      }
      return repo.default_target_branch ?? null;
    },
    [queryClient]
  );

  const swapPrimary = useCallback(
    async (repo: Repo) => {
      clearRepos();
      addRepo(repo);
      const branch = await resolveInitialBranch(repo);
      if (branch) setTargetBranch(repo.id, branch);
    },
    [addRepo, clearRepos, resolveInitialBranch, setTargetBranch]
  );

  const addSecondary = useCallback(
    async (repo: Repo) => {
      if (attachedIds.has(repo.id)) return;
      addRepo(repo);
      const branch = await resolveInitialBranch(repo);
      if (branch) setTargetBranch(repo.id, branch);
    },
    [addRepo, attachedIds, resolveInitialBranch, setTargetBranch]
  );

  const removeSecondary = useCallback(
    (repoId: string) => {
      removeRepo(repoId);
    },
    [removeRepo]
  );

  const openFolderAndApply = useCallback(
    async (apply: (repo: Repo) => void | Promise<void>) => {
      const path = await FolderPickerDialog.show({
        title: t('dialogs.selectGitRepository', {
          defaultValue: 'Select folder',
        }),
      });
      if (!path) return;
      const repo = await repoApi.register({ path });
      queryClient.invalidateQueries({ queryKey: ['repos'] });
      await apply(repo);
    },
    [queryClient, t]
  );

  // Labels
  const pickPlaceholder = t('createMode.composerChips.pickFolderPlaceholder', {
    defaultValue: 'Pick a folder…',
  });
  const worktreeLabel = t('createMode.composerChips.worktreeChip', {
    defaultValue: 'Worktree',
  });
  const addFolderLabel = t('createMode.composerChips.addFolderMenuItem', {
    defaultValue: 'Add folder',
  });

  const primaryBranch = primary ? targetBranches[primary.id] : undefined;
  const showBranchChip = !!primary && primary.is_git;
  const showWorktreeChip = repos.length > 0 && repos.every((r) => r.is_git);

  return (
    <>
      {/* Primary folder chip */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" disabled={disabled} className={chipClassName}>
            <FolderIcon weight="bold" className="size-icon-xs" />
            <span className="max-w-[180px] truncate">
              {primary ? repoDisplayName(primary) : pickPlaceholder}
            </span>
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <FolderPickerPopover
            recents={recents}
            onSelect={swapPrimary}
            onOpenFolder={() => openFolderAndApply(swapPrimary)}
          />
        </DropdownMenuContent>
      </DropdownMenu>

      {/* Branch chip — only when primary is git. */}
      {showBranchChip && primary && (
        <BranchChip
          repo={primary}
          currentBranch={primaryBranch}
          onSelectBranch={(branch) => setTargetBranch(primary.id, branch)}
          disabled={disabled}
        />
      )}

      {/* Worktree chip — only when all attached repos are git */}
      {showWorktreeChip && (
        <label
          className={`${chipClassName} cursor-pointer`}
          title={worktreeLabel}
        >
          <Checkbox
            checked={useWorktree}
            onCheckedChange={setUseWorktree}
            className="h-3.5 w-3.5"
            disabled={disabled}
          />
          <span>{worktreeLabel}</span>
        </label>
      )}

      {/* Secondary folder chips — popover lists branches + Remove. Swap is
          not inline; users remove + re-add via the `+` menu. */}
      {secondaries.map((repo) => (
        <DropdownMenu key={repo.id}>
          <DropdownMenuTrigger asChild>
            <button type="button" disabled={disabled} className={chipClassName}>
              <FolderIcon weight="bold" className="size-icon-xs" />
              <span className="max-w-[160px] truncate">
                {repoDisplayName(repo)}
              </span>
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            <SecondaryChipPopoverContent
              repo={repo}
              currentBranch={targetBranches[repo.id]}
              onSelectBranch={(branch) => setTargetBranch(repo.id, branch)}
              onRemove={() => removeSecondary(repo.id)}
            />
          </DropdownMenuContent>
        </DropdownMenu>
      ))}

      {/* `+` menu — v1 only exposes Add folder */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            disabled={disabled}
            className={chipClassName}
            aria-label={addFolderLabel}
          >
            <PlusIcon weight="bold" className="size-icon-xs" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <FolderIcon weight="bold" />
              <span className="flex-1">{addFolderLabel}</span>
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              <FolderPickerPopover
                recents={recents}
                excludeRepoIds={attachedIds}
                onSelect={addSecondary}
                onOpenFolder={() => openFolderAndApply(addSecondary)}
              />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}

/**
 * Landing helper: when the create-mode state has no attached repos on mount,
 * auto-select the most-recently-used one so the composer lands with a primary
 * pre-selected. Consumers should call this in a `useEffect` inside the
 * container that owns the `CreateMode` state.
 */
export function useAutoAttachMostRecent(): void {
  const { repos, addRepo, setTargetBranch, hasResolvedInitialRepoDefaults } =
    useCreateMode();

  const shouldAuto = hasResolvedInitialRepoDefaults && repos.length === 0;

  const { data: recents } = useQuery<Repo[]>({
    queryKey: ['repos', 'recent'],
    queryFn: () => repoApi.listRecent(),
    enabled: shouldAuto,
    staleTime: 30_000,
  });

  const hasAttached = repos.length > 0;

  // NOTE: we deliberately skip `pickBranchForRepo` here and fall back to
  // `default_target_branch` so the landing doesn't pop a modal on mount.
  // If the repo has no default branch, it lands without one and the user
  // picks via the chip.
  useEffect(() => {
    if (!shouldAuto || hasAttached || !recents || recents.length === 0) return;
    const first = recents[0];
    addRepo(first);
    if (first.is_git && first.default_target_branch) {
      setTargetBranch(first.id, first.default_target_branch);
    }
  }, [shouldAuto, hasAttached, recents, addRepo, setTargetBranch]);
}

interface SecondaryChipPopoverContentProps {
  repo: Repo;
  currentBranch: string | null | undefined;
  onSelectBranch: (branch: string) => void;
  onRemove: () => void;
}

function SecondaryChipPopoverContent({
  repo,
  currentBranch,
  onSelectBranch,
  onRemove,
}: SecondaryChipPopoverContentProps) {
  const { t } = useTranslation();
  const removeLabel = t('createMode.composerChips.removeFolderAction', {
    defaultValue: 'Remove',
  });

  return (
    <>
      {repo.is_git && (
        <>
          <BranchMenuList
            repo={repo}
            currentBranch={currentBranch}
            onSelectBranch={onSelectBranch}
          />
          <DropdownMenuSeparator />
        </>
      )}
      <DropdownMenuItem
        icon={TrashIcon}
        variant="destructive"
        onSelect={onRemove}
      >
        {removeLabel}
      </DropdownMenuItem>
    </>
  );
}
