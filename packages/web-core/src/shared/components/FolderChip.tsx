import { useCallback } from 'react';
import { FolderIcon } from '@phosphor-icons/react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { Repo } from 'shared/types';

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from '@vibe/ui/components/Dropdown';
import { FolderPickerPopover } from '@vibe/ui/components/FolderPickerPopover';
import { repoApi } from '@/shared/lib/api';
import { FolderPickerDialog } from '@/shared/dialogs/shared/FolderPickerDialog';

const chipClassName =
  'inline-flex items-center gap-half rounded-md bg-secondary px-base py-half ' +
  'min-h-7 text-sm text-normal hover:bg-panel ' +
  'disabled:cursor-not-allowed disabled:opacity-50 ' +
  'focus:outline-none focus-visible:ring-1 focus-visible:ring-brand';

interface FolderChipProps {
  selectedRepoId: string | null;
  onSelect: (repoId: string) => void;
  disabled?: boolean;
}

export function FolderChip({
  selectedRepoId,
  onSelect,
  disabled,
}: FolderChipProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const { data: repos = [] } = useQuery<Repo[]>({
    queryKey: ['repos'],
    queryFn: () => repoApi.list(),
    staleTime: 30_000,
  });
  const { data: recents = [] } = useQuery<Repo[]>({
    queryKey: ['repos', 'recent'],
    queryFn: () => repoApi.listRecent(),
    staleTime: 30_000,
  });

  const selected = selectedRepoId
    ? (repos.find((r) => r.id === selectedRepoId) ?? null)
    : null;

  const pickPlaceholder = t('createMode.composerChips.pickFolderPlaceholder', {
    defaultValue: 'Pick a folder…',
  });

  const openFolderAndSelect = useCallback(async () => {
    const path = await FolderPickerDialog.show({
      title: t('dialogs.selectGitRepository', {
        defaultValue: 'Select folder',
      }),
    });
    if (!path) return;
    const repo = await repoApi.register({ path });
    queryClient.invalidateQueries({ queryKey: ['repos'] });
    queryClient.invalidateQueries({ queryKey: ['repos', 'recent'] });
    onSelect(repo.id);
  }, [onSelect, queryClient, t]);

  // Show recents first; append any non-recent repos so user always has access
  // to the full list (recents alone can be empty on a fresh install).
  const recentIds = new Set(recents.map((r) => r.id));
  const orderedRepos = [
    ...recents,
    ...repos.filter((r) => !recentIds.has(r.id)),
  ];

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" disabled={disabled} className={chipClassName}>
          <FolderIcon weight="bold" className="size-icon-xs" />
          <span className="max-w-[180px] truncate">
            {selected
              ? selected.display_name || selected.name
              : pickPlaceholder}
          </span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <FolderPickerPopover
          recents={orderedRepos}
          onSelect={(repo) => onSelect(repo.id)}
          onOpenFolder={openFolderAndSelect}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
