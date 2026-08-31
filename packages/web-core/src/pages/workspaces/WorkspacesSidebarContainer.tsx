import React, { useState, useMemo, useCallback, useEffect } from 'react';
import {
  usePillDragStore,
  useDraggingWorkspaceId,
} from '@/shared/stores/usePillDragStore';
import { useSessionGridStore } from '@/shared/stores/useSessionGridStore';
import { useLocation, useParams } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import { workspacesApi } from '@/shared/lib/api';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import { workspaceRecordKeys } from '@/shared/hooks/useWorkspaceRecord';
import { useTranslation } from 'react-i18next';
import { ThemeMode } from 'shared/types';
import { useWorkspaceContext } from '@/shared/hooks/useWorkspaceContext';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { useUserContext } from '@/shared/hooks/useUserContext';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import { useScratch } from '@/shared/hooks/useScratch';
import { useFolderSeedStore } from '@/shared/stores/useFolderSeedStore';
import { useTheme } from '@/shared/hooks/useTheme';
import { ScratchType, type DraftWorkspaceData } from 'shared/types';
import { splitMessageToTitleDescription } from '@/shared/lib/string';
import { repoGroupQualifiers } from '@/shared/lib/repoGroupLabels';
import { useIsMobile } from '@/shared/hooks/useIsMobile';
import {
  PERSIST_KEYS,
  usePersistedExpanded,
  useUiPreferencesStore,
  type WorkspacePrFilter,
  type WorkspaceSortBy,
  type WorkspaceSortOrder,
} from '@/shared/stores/useUiPreferencesStore';
import type { Workspace } from '@/shared/hooks/useWorkspaces';
import { CommandBarDialog } from '@/shared/dialogs/command-bar/CommandBarDialog';
import { SettingsDialog } from '@/shared/dialogs/settings/SettingsDialog';
import { NavbarSidebarSearchSlot } from '@/shared/components/ui-new/containers/NavbarSidebarSearchSlot';
import { ControlPanelMenu } from './ControlPanelMenu';
import { UnpinDragIndicator } from './UnpinDragIndicator';
import { useActions } from '@/shared/hooks/useActions';
import { useActionVisibilityContext } from '@/shared/hooks/useActionVisibilityContext';
import { NavbarActionGroups } from '@/shared/actions';
import {
  type ActionDefinition,
  type NavbarItem as ActionNavbarItem,
  isSpecialIcon,
  getActionIcon,
  getActionTooltip,
  isActionActive,
  isActionEnabled,
  isActionVisible,
} from '@/shared/types/actions';
import { IconButton } from '@vibe/ui/components/IconButton';
import {
  WorkspacesSidebar,
  type WorkspacesSidebarFolderGroup,
  type WorkspacesSidebarPersistKeys,
  type WorkspaceLayoutMode,
} from '@vibe/ui/components/WorkspacesSidebar';
import {
  MultiSelectDropdown,
  type MultiSelectDropdownOption,
} from '@vibe/ui/components/MultiSelectDropdown';
import { PropertyDropdown } from '@vibe/ui/components/PropertyDropdown';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';
import {
  ButtonGroup,
  ButtonGroupItem,
} from '@vibe/ui/components/IconButtonGroup';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/Dialog';
import {
  // ArrowClockwiseIcon, // re-enable when uncommenting the dev refresh button below
  FolderIcon,
  GitPullRequestIcon,
  XIcon,
} from '@phosphor-icons/react';
import { useRemoteCloudHostsAppBarModel } from '@/shared/hooks/useRemoteCloudHosts';

// Fixed UUID for the universal workspace draft (same as in useCreateModeState.ts)
const DRAFT_WORKSPACE_ID = '00000000-0000-0000-0000-000000000001';

const PAGE_SIZE = 50;
const NO_PROJECT_ID = '__no_project__';

const PR_FILTER_OPTIONS: WorkspacePrFilter[] = ['all', 'has_pr', 'no_pr'];

const SORT_BY_OPTIONS: WorkspaceSortBy[] = ['updated_at', 'created_at'];

interface WorkspacesSidebarContainerProps {
  onScrollToBottom?: (behavior?: 'auto' | 'smooth') => void;
}

interface WorkspacesSortDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sortBy: WorkspaceSortBy;
  sortOrder: WorkspaceSortOrder;
  onSortByChange: (sortBy: WorkspaceSortBy) => void;
  onSortOrderChange: (sortOrder: WorkspaceSortOrder) => void;
}

// Sort/filter dialogs kept defined (hide, don't strip). Not rendered while
// enableFlatGrouping / enableAccordionGrouping are both false.
export function WorkspacesSortDialog({
  open,
  onOpenChange,
  sortBy,
  sortOrder,
  onSortByChange,
  onSortOrderChange,
}: WorkspacesSortDialogProps) {
  const { t } = useTranslation('common');

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md p-0">
        <div className="border-b border-border px-double pb-base pt-double">
          <DialogHeader className="space-y-half">
            <DialogTitle>
              {t('kanban.workspaceSidebar.sortDialogTitle')}
            </DialogTitle>
            <DialogDescription>
              {t('kanban.workspaceSidebar.sortDialogDescription')}
            </DialogDescription>
          </DialogHeader>
        </div>

        <div className="px-double py-double">
          <div className="flex flex-col gap-base">
            <div className="flex items-center justify-between gap-base">
              <span className="text-sm text-low">
                {t('kanban.workspaceSidebar.sortByLabel')}
              </span>
              <PropertyDropdown
                value={sortBy}
                options={SORT_BY_OPTIONS.map((option) => ({
                  value: option,
                  label:
                    option === 'updated_at'
                      ? t('kanban.workspaceSidebar.sortUpdatedAt')
                      : t('kanban.workspaceSidebar.sortCreatedAt'),
                }))}
                onChange={onSortByChange}
              />
            </div>
            <div className="flex items-center justify-between gap-base">
              <span className="text-sm text-low">
                {t('kanban.workspaceSidebar.sortOrderLabel')}
              </span>
              <ButtonGroup>
                <ButtonGroupItem
                  active={sortOrder === 'desc'}
                  onClick={() => onSortOrderChange('desc')}
                >
                  {t('kanban.sortDescending')}
                </ButtonGroupItem>
                <ButtonGroupItem
                  active={sortOrder === 'asc'}
                  onClick={() => onSortOrderChange('asc')}
                >
                  {t('kanban.sortAscending')}
                </ButtonGroupItem>
              </ButtonGroup>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

interface WorkspacesFilterDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectOptions: MultiSelectDropdownOption<string>[];
  projectIds: string[];
  prFilter: WorkspacePrFilter;
  hasActiveFilters: boolean;
  onProjectFilterChange: (projectIds: string[]) => void;
  onPrFilterChange: (prFilter: WorkspacePrFilter) => void;
  onClearFilters: () => void;
}

export function WorkspacesFilterDialog({
  open,
  onOpenChange,
  projectOptions,
  projectIds,
  prFilter,
  hasActiveFilters,
  onProjectFilterChange,
  onPrFilterChange,
  onClearFilters,
}: WorkspacesFilterDialogProps) {
  const { t } = useTranslation('common');

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md p-0">
        <div className="border-b border-border px-double pb-base pt-double">
          <DialogHeader className="space-y-half">
            <DialogTitle>
              {t('kanban.workspaceSidebar.filterDialogTitle')}
            </DialogTitle>
            <DialogDescription>
              {t('kanban.workspaceSidebar.filterDialogDescription')}
            </DialogDescription>
          </DialogHeader>
        </div>

        <div className="px-double py-double">
          <div className="flex flex-col items-start gap-base">
            <MultiSelectDropdown
              values={projectIds}
              options={projectOptions}
              onChange={onProjectFilterChange}
              icon={FolderIcon}
              label={t('kanban.workspaceSidebar.projectFilterLabel')}
            />
            <PropertyDropdown
              value={prFilter}
              options={PR_FILTER_OPTIONS.map((option) => ({
                value: option,
                label:
                  option === 'all'
                    ? t('kanban.workspaceSidebar.prFilterAll')
                    : option === 'has_pr'
                      ? t('kanban.workspaceSidebar.prFilterHasPr')
                      : t('kanban.workspaceSidebar.prFilterNoPr'),
              }))}
              onChange={onPrFilterChange}
              icon={GitPullRequestIcon}
              label={t('kanban.workspaceSidebar.prFilterLabel')}
            />
            {hasActiveFilters && (
              <div className="self-end">
                <PrimaryButton
                  variant="tertiary"
                  value={t('kanban.clearFilters')}
                  actionIcon={XIcon}
                  onClick={onClearFilters}
                />
              </div>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function toTimestamp(value: string | undefined): number | null {
  if (!value) {
    return null;
  }

  const timestamp = new Date(value).getTime();
  return Number.isNaN(timestamp) ? null : timestamp;
}

function getWorkspaceSortTimestamp(
  workspace: Workspace,
  sortBy: WorkspaceSortBy
): number | null {
  if (sortBy === 'updated_at') {
    return toTimestamp(workspace.latestProcessCompletedAt);
  }

  return toTimestamp(workspace.createdAt);
}

export function WorkspacesSidebarContainer({
  onScrollToBottom = () => {},
}: WorkspacesSidebarContainerProps) {
  const {
    workspaceId: selectedWorkspaceId,
    activeWorkspaces: rawActiveWorkspaces,
    archivedWorkspaces,
    isWorkspacesListLoading,
    isCreateMode,
    selectWorkspace,
    navigateToCreate,
  } = useWorkspaceContext();

  // Optimistic pinned order: applied immediately on drop so the user sees
  // the new order without waiting for the API round-trip. Cleared by an
  // effect once the server data converges.
  const [optimisticPinnedOrder, setOptimisticPinnedOrder] = useState<
    string[] | null
  >(null);

  const activeWorkspaces = useMemo(() => {
    if (!optimisticPinnedOrder) return rawActiveWorkspaces;
    const orderMap = new Map(
      optimisticPinnedOrder.map((id, i) => [id, i] as const)
    );
    return rawActiveWorkspaces.map((w) => {
      const optIdx = orderMap.get(w.id);
      if (optIdx !== undefined) {
        return { ...w, isPinned: true, pinOrder: optIdx };
      }
      // Workspaces not in the optimistic list are unpinned by definition.
      if (w.isPinned) {
        return { ...w, isPinned: false, pinOrder: undefined };
      }
      return w;
    });
  }, [rawActiveWorkspaces, optimisticPinnedOrder]);

  // Drop the optimistic overlay once the WS stream / summary refetch reflects
  // the same pinned order on the server.
  useEffect(() => {
    if (!optimisticPinnedOrder) return;
    const serverOrder = rawActiveWorkspaces
      .filter((w) => w.isPinned)
      .sort((a, b) => (a.pinOrder ?? 0) - (b.pinOrder ?? 0))
      .map((w) => w.id);
    if (
      serverOrder.length === optimisticPinnedOrder.length &&
      serverOrder.every((id, i) => id === optimisticPinnedOrder[i])
    ) {
      setOptimisticPinnedOrder(null);
    }
  }, [rawActiveWorkspaces, optimisticPinnedOrder]);

  const isMobile = useIsMobile();
  const { hosts: remoteCloudHosts } = useRemoteCloudHostsAppBarModel();
  const { hostId: routeHostId } = useParams({ strict: false });
  const appNavigation = useAppNavigation();
  const location = useLocation();
  const isRoutinesActive = location.pathname.startsWith('/routines');
  const isAgentsActive = location.pathname.startsWith('/agents');
  const setMobileActiveTab = useUiPreferencesStore((s) => s.setMobileActiveTab);
  const searchQuery = useUiPreferencesStore((s) => s.sidebarSearchQuery);
  const [showArchive, setShowArchive] = usePersistedExpanded(
    PERSIST_KEYS.workspacesSidebarArchived,
    false
  );
  const [isAccordionLayout, setAccordionLayout] = usePersistedExpanded(
    PERSIST_KEYS.workspacesSidebarAccordionLayout,
    true
  );
  const enableFlatGrouping = useUiPreferencesStore((s) => s.enableFlatGrouping);
  const enableAccordionGrouping = useUiPreferencesStore(
    (s) => s.enableAccordionGrouping
  );

  const layoutMode: WorkspaceLayoutMode = enableAccordionGrouping
    ? isAccordionLayout
      ? 'accordion'
      : enableFlatGrouping
        ? 'flat'
        : 'folder'
    : enableFlatGrouping
      ? 'flat'
      : 'folder';
  const toggleLayoutMode = () => setAccordionLayout(!isAccordionLayout);

  // Workspace sidebar filters + sort (state preserved behind the scenes;
  // UI entry points are hidden in default mode).
  const workspaceFilters = useUiPreferencesStore((s) => s.workspaceFilters);
  const workspaceSort = useUiPreferencesStore((s) => s.workspaceSort);

  // Remote data for project filter (all orgs) — hidden in default mode.
  const { workspaces: remoteWorkspaces } = useUserContext();

  // Map local workspace ID → remote project ID
  const remoteProjectByLocalId = useMemo(() => {
    const map = new Map<string, string>();
    for (const rw of remoteWorkspaces) {
      if (rw.local_workspace_id) {
        map.set(rw.local_workspace_id, rw.project_id);
      }
    }
    return map;
  }, [remoteWorkspaces]);

  // Theme (footer control panel — tri-state: light/dark/system).
  const { theme, setTheme } = useTheme();
  const { updateAndSaveConfig } = useUserSystem();
  const handleSetTheme = useCallback(
    (next: ThemeMode) => {
      setTheme(next);
      void updateAndSaveConfig({ theme: next });
    },
    [setTheme, updateAndSaveConfig]
  );

  // Pagination state for infinite scroll
  const [displayLimit, setDisplayLimit] = useState(PAGE_SIZE);

  // Reset display limit when search, filter, or sort state changes
  useEffect(() => {
    setDisplayLimit(PAGE_SIZE);
  }, [searchQuery, showArchive, workspaceFilters, workspaceSort]);

  const searchLower = searchQuery.toLowerCase();
  const isSearching = searchQuery.length > 0;

  // Apply sidebar filters (project + PR), then search
  const filteredActiveWorkspaces = useMemo(() => {
    let result = activeWorkspaces;

    // Project filter
    if (workspaceFilters.projectIds.length > 0) {
      const includeNoProject =
        workspaceFilters.projectIds.includes(NO_PROJECT_ID);
      const realProjectIds = workspaceFilters.projectIds.filter(
        (id) => id !== NO_PROJECT_ID
      );
      result = result.filter((ws) => {
        const projectId = remoteProjectByLocalId.get(ws.id);
        if (!projectId) return includeNoProject;
        return realProjectIds.includes(projectId);
      });
    }

    // PR filter
    if (workspaceFilters.prFilter === 'has_pr') {
      result = result.filter((ws) => !!ws.prStatus);
    } else if (workspaceFilters.prFilter === 'no_pr') {
      result = result.filter((ws) => !ws.prStatus);
    }

    // Search filter
    if (searchLower) {
      result = result.filter(
        (ws) =>
          ws.name.toLowerCase().includes(searchLower) ||
          ws.branch.toLowerCase().includes(searchLower)
      );
    }

    return result;
  }, [activeWorkspaces, workspaceFilters, remoteProjectByLocalId, searchLower]);

  const filteredArchivedWorkspaces = useMemo(() => {
    let result = archivedWorkspaces;

    if (workspaceFilters.projectIds.length > 0) {
      const includeNoProject =
        workspaceFilters.projectIds.includes(NO_PROJECT_ID);
      const realProjectIds = workspaceFilters.projectIds.filter(
        (id) => id !== NO_PROJECT_ID
      );
      result = result.filter((ws) => {
        const projectId = remoteProjectByLocalId.get(ws.id);
        if (!projectId) return includeNoProject;
        return realProjectIds.includes(projectId);
      });
    }

    if (workspaceFilters.prFilter === 'has_pr') {
      result = result.filter((ws) => !!ws.prStatus);
    } else if (workspaceFilters.prFilter === 'no_pr') {
      result = result.filter((ws) => !ws.prStatus);
    }

    if (searchLower) {
      result = result.filter(
        (ws) =>
          ws.name.toLowerCase().includes(searchLower) ||
          ws.branch.toLowerCase().includes(searchLower)
      );
    }

    return result;
  }, [
    archivedWorkspaces,
    workspaceFilters,
    remoteProjectByLocalId,
    searchLower,
  ]);

  const sortWorkspaces = useCallback(
    (workspaces: Workspace[]) =>
      [...workspaces].sort((a, b) => {
        // Always keep pinned workspaces at the top.
        if (a.isPinned !== b.isPinned) {
          return a.isPinned ? -1 : 1;
        }

        // Within the pinned set, honor user-defined pin_order.
        if (a.isPinned && b.isPinned) {
          const ao = a.pinOrder ?? 0;
          const bo = b.pinOrder ?? 0;
          if (ao !== bo) return ao - bo;
        }

        const aTimestamp = getWorkspaceSortTimestamp(a, workspaceSort.sortBy);
        const bTimestamp = getWorkspaceSortTimestamp(b, workspaceSort.sortBy);

        // Workspaces without the selected timestamp are always sorted first.
        if (aTimestamp === null && bTimestamp === null) {
          return a.name.localeCompare(b.name);
        }
        if (aTimestamp === null) {
          return -1;
        }
        if (bTimestamp === null) {
          return 1;
        }

        if (aTimestamp === bTimestamp) {
          return a.name.localeCompare(b.name);
        }

        return workspaceSort.sortOrder === 'asc'
          ? aTimestamp - bTimestamp
          : bTimestamp - aTimestamp;
      }),
    [workspaceSort.sortBy, workspaceSort.sortOrder]
  );

  const sortedActiveWorkspaces = useMemo(
    () => sortWorkspaces(filteredActiveWorkspaces),
    [filteredActiveWorkspaces, sortWorkspaces]
  );

  const sortedArchivedWorkspaces = useMemo(
    () => sortWorkspaces(filteredArchivedWorkspaces),
    [filteredArchivedWorkspaces, sortWorkspaces]
  );

  // Apply pagination (only when not searching)
  const paginatedActiveWorkspaces = useMemo(
    () =>
      isSearching
        ? sortedActiveWorkspaces
        : sortedActiveWorkspaces.slice(0, displayLimit),
    [sortedActiveWorkspaces, displayLimit, isSearching]
  );

  const paginatedArchivedWorkspaces = useMemo(
    () =>
      isSearching
        ? sortedArchivedWorkspaces
        : sortedArchivedWorkspaces.slice(0, displayLimit),
    [sortedArchivedWorkspaces, displayLimit, isSearching]
  );

  // Partition paginated active list into { pinned, byFolder }.
  const { pinnedWorkspaces, folderGroups } = useMemo(() => {
    const pinned: typeof paginatedActiveWorkspaces = [];
    const groups = new Map<string, WorkspacesSidebarFolderGroup>();

    for (const ws of paginatedActiveWorkspaces) {
      if (ws.isPinned) {
        pinned.push(ws);
        continue;
      }
      const primary = ws.primaryRepo;
      if (!primary) {
        // Defensive: workspace with no attached repo. Skip and warn.
        // (Under multi-folder-sessions invariant this should not happen.)
        console.warn(
          `[sidebar] Workspace ${ws.id} has no primary repo; skipping folder grouping.`
        );
        continue;
      }
      // Key by path: two local checkouts of one repository differ by path and
      // by nothing else the sidebar shows.
      const existing = groups.get(primary.path);
      if (existing) {
        existing.sessions.push(ws);
      } else {
        groups.set(primary.path, {
          repoId: primary.id,
          repoPath: primary.path,
          displayName: primary.displayName || primary.name,
          sessions: [ws],
        });
      }
    }

    const qualifiers = repoGroupQualifiers(
      Array.from(groups.values(), (group) => ({
        path: group.repoPath,
        label: group.displayName,
      }))
    );

    const folderGroupsArr = Array.from(groups.values(), (group) => ({
      ...group,
      qualifier: qualifiers.get(group.repoPath),
    })).sort(
      (a, b) =>
        a.displayName.localeCompare(b.displayName) ||
        (a.qualifier ?? '').localeCompare(b.qualifier ?? '')
    );
    return { pinnedWorkspaces: pinned, folderGroups: folderGroupsArr };
  }, [paginatedActiveWorkspaces]);

  // Check if there are more workspaces to load
  const hasMoreWorkspaces = showArchive
    ? sortedArchivedWorkspaces.length > displayLimit
    : sortedActiveWorkspaces.length > displayLimit;

  // Handle scroll to load more
  const handleLoadMore = useCallback(() => {
    if (!isSearching && hasMoreWorkspaces) {
      setDisplayLimit((prev) => prev + PAGE_SIZE);
    }
  }, [isSearching, hasMoreWorkspaces]);

  // Read persisted draft for sidebar placeholder
  const { scratch: draftScratch } = useScratch(
    ScratchType.DRAFT_WORKSPACE,
    DRAFT_WORKSPACE_ID
  );

  // Extract draft title from persisted scratch
  const persistedDraftTitle = useMemo(() => {
    const scratchData: DraftWorkspaceData | undefined =
      draftScratch?.payload?.type === 'DRAFT_WORKSPACE'
        ? draftScratch.payload.data
        : undefined;

    if (!scratchData?.message?.trim()) return undefined;
    const { title } = splitMessageToTitleDescription(
      scratchData.message.trim()
    );
    return title || 'New Workspace';
  }, [draftScratch]);

  // Handle workspace selection.
  // - If already in this cell (focused first cell): scroll to bottom.
  // - If mounted in another cell of the grid: just focus that cell (don't
  //   navigate, since the URL tracks the first cell only).
  // - Otherwise: navigate the URL, which routes through WorkspaceProvider
  //   into setFirstCellSession (replacing cell #1).
  const handleSelectWorkspace = useCallback(
    (id: string) => {
      if (id === selectedWorkspaceId) {
        onScrollToBottom();
      } else {
        const grid = useSessionGridStore.getState().grid;
        const mountedCell = grid.groups
          .flatMap((g) => g.cells)
          .find((c) => c.sessionId === id);
        const isFirstCell = grid.groups[0]?.cells[0]?.sessionId === id;
        if (mountedCell && !isFirstCell) {
          useSessionGridStore.getState().focusCell(mountedCell.id);
        } else {
          selectWorkspace(id);
        }
      }
      if (isMobile) {
        setMobileActiveTab('chat');
      }
    },
    [
      selectedWorkspaceId,
      selectWorkspace,
      onScrollToBottom,
      isMobile,
      setMobileActiveTab,
    ]
  );

  const handleAddWorkspace = useCallback(() => {
    navigateToCreate();
    if (isMobile) {
      setMobileActiveTab('chat');
    }
  }, [navigateToCreate, isMobile, setMobileActiveTab]);

  const handleOpenRoutines = useCallback(() => {
    appNavigation.routines?.goToRoutines();
    if (isMobile) {
      setMobileActiveTab('chat');
    }
  }, [appNavigation, isMobile, setMobileActiveTab]);

  const handleOpenAgents = useCallback(() => {
    appNavigation.goToAgents();
    if (isMobile) {
      setMobileActiveTab('chat');
    }
  }, [appNavigation, isMobile, setMobileActiveTab]);

  const setPendingFolderSeed = useFolderSeedStore((s) => s.setPending);
  const handleCreateInFolder = useCallback(
    (repoId: string) => {
      setPendingFolderSeed(repoId);
      navigateToCreate();
      if (isMobile) {
        setMobileActiveTab('chat');
      }
    },
    [setPendingFolderSeed, navigateToCreate, isMobile, setMobileActiveTab]
  );

  const handleOpenWorkspaceActions = useCallback((workspaceId: string) => {
    CommandBarDialog.show({
      page: 'workspaceActions',
      workspaceId,
    });
  }, []);

  const sidebarPersistKeys: WorkspacesSidebarPersistKeys = {
    raisedHand: PERSIST_KEYS.workspacesSidebarRaisedHand,
    notRunning: PERSIST_KEYS.workspacesSidebarNotRunning,
    running: PERSIST_KEYS.workspacesSidebarRunning,
  };

  const activeRemoteHost = useMemo(() => {
    if (remoteCloudHosts.length === 0 || !routeHostId) {
      return null;
    }

    return remoteCloudHosts.find((host) => host.id === routeHostId) ?? null;
  }, [routeHostId, remoteCloudHosts]);

  const handleOpenRemoteHostSettings = useCallback(() => {
    void SettingsDialog.show({
      initialSection: 'relay',
      ...(routeHostId ? { initialState: { hostId: routeHostId } } : {}),
    });
  }, [routeHostId]);

  // Set of currently-pinned ids (used to mark the drag as a pinned drag,
  // which drives the "release to unpin" affordance).
  const pinnedIds = useMemo(
    () => new Set(activeWorkspaces.filter((w) => w.isPinned).map((w) => w.id)),
    [activeWorkspaces]
  );

  const queryClient = useQueryClient();

  // Drop on something other than a known target with no successful drop
  // unpins the workspace. Triggered by onDragEnd below.
  const handleUnpin = useCallback(
    async (workspaceId: string) => {
      // Snap UI: pill leaves the pinned section immediately.
      const newOrder = pinnedWorkspaces
        .filter((w) => w.id !== workspaceId)
        .map((w) => w.id);
      setOptimisticPinnedOrder(newOrder);
      try {
        await workspacesApi.update(workspaceId, { pinned: false });
        queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
        // Also refresh the per-workspace record so the kebab menu's
        // PinWorkspace action label flips from "Unpin" → "Pin".
        queryClient.invalidateQueries({
          queryKey: workspaceRecordKeys.byId(workspaceId),
        });
      } catch (err) {
        console.warn('Unpin via drag failed', err);
        setOptimisticPinnedOrder(null);
      }
    },
    [pinnedWorkspaces, queryClient]
  );

  // Make every pill a drag source. The grid's per-cell drop overlay reads
  // `usePillDragStore` to know what's being dragged; the actual drop
  // (split / open-in-split / pin) is performed by the drop target.
  const getWorkspaceDragProps = useCallback(
    (workspaceId: string) => ({
      draggable: !isMobile,
      onDragStart: (e: React.DragEvent<HTMLDivElement>) => {
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('application/x-vibe-pill', workspaceId);
        usePillDragStore
          .getState()
          .setDragging(workspaceId, pinnedIds.has(workspaceId));
      },
      onDragEnd: () => {
        const state = usePillDragStore.getState();
        const wasPinned = state.draggingIsPinned;
        const droppedInPin = state.droppedInPinSection;
        state.setDragging(null);
        // Any release of a pinned pill *outside* the Pinned section
        // unpins it. Drops on a cell's drop-half still split / open-in-split
        // and *also* unpin (the pill is now somewhere else, no need to
        // keep it pinned).
        if (wasPinned && !droppedInPin) {
          void handleUnpin(workspaceId);
        }
      },
    }),
    [isMobile, pinnedIds, handleUnpin]
  );

  const handlePinAreaHover = useCallback((over: boolean) => {
    usePillDragStore.getState().setOverDropTarget(over);
  }, []);

  // Sidebar pill state derived from the session-grid:
  // - openInGridWorkspaceIds = every cell's sessionId (gives pill background)
  // - focusedSessionId       = the focused cell's sessionId (gives bright + bold)
  const grid = useSessionGridStore((s) => s.grid);
  // When the anchor cell is hosting a slot (create-mode composer or routines
  // pages) instead of its workspace, exclude the anchor cell's workspace from
  // the active/focused pill sets — the workspace isn't on screen.
  const anchorSlotActive = isCreateMode || isRoutinesActive || isAgentsActive;
  const openInGridWorkspaceIds = useMemo(() => {
    const anchorId = grid.groups[0]?.cells[0]?.id;
    return new Set(
      grid.groups
        .flatMap((g) => g.cells)
        .filter((c) => !(anchorSlotActive && c.id === anchorId))
        .map((c) => c.sessionId)
        .filter(Boolean)
    );
  }, [grid, anchorSlotActive]);
  const focusedSessionId = useMemo(() => {
    const anchorId = grid.groups[0]?.cells[0]?.id;
    if (anchorSlotActive && grid.focusedCellId === anchorId) return null;
    return (
      grid.groups
        .flatMap((g) => g.cells)
        .find((c) => c.id === grid.focusedCellId)?.sessionId ?? null
    );
  }, [grid, anchorSlotActive]);

  // Drop on a pinned-section slot atomically rewrites the pinned set to
  // the given order. Setting droppedInPinSection synchronously prevents
  // the source pill's onDragEnd (which fires *after* this handler) from
  // interpreting the release as "unpin". Skip the network round-trip when
  // the order is unchanged (e.g. drop on own slot).
  const handleReorderPins = useCallback(
    async (orderedIds: string[]) => {
      // Clear the drag store now (instead of waiting for onDragEnd): when an
      // unpinned pill becomes pinned, the optimistic projection moves it into
      // the PinnedSection on the next render — which can unmount the original
      // source element before its onDragEnd fires, leaving draggingWorkspaceId
      // stuck and the freshly-pinned pill incorrectly dimmed as the source.
      usePillDragStore.getState().setDragging(null);

      // Compare against the order PinnedSection actually rendered.
      const displayed = pinnedWorkspaces.map((w) => w.id);
      const unchanged =
        displayed.length === orderedIds.length &&
        displayed.every((id, i) => id === orderedIds[i]);
      if (unchanged) return;
      // Snap UI to the new order immediately so the user doesn't wait for
      // the network round-trip. Cleared once server data converges.
      setOptimisticPinnedOrder(orderedIds);
      try {
        await workspacesApi.reorderPins(orderedIds);
        queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
        // Refresh per-workspace records for every workspace whose pinned
        // state may have flipped (newly pinned + previously pinned). The
        // kebab menu reads workspace.pinned from this cache to label the
        // PinWorkspace action; without this, freshly drag-pinned pills
        // still show "Pin session" and freshly drag-unpinned pills still
        // show "Unpin session".
        const affected = new Set<string>([...orderedIds, ...displayed]);
        for (const id of affected) {
          queryClient.invalidateQueries({
            queryKey: workspaceRecordKeys.byId(id),
          });
        }
      } catch (err) {
        console.warn('Reorder pins failed', err);
        setOptimisticPinnedOrder(null);
      }
    },
    [pinnedWorkspaces, queryClient]
  );

  const draggingWorkspaceId = useDraggingWorkspaceId();

  // Action items lifted from the now-hidden top navbar.
  // Top of sidebar: full left group (sidebar toggle).
  // Bottom of sidebar: only command bar + settings from the right group.
  const actionCtx = useActionVisibilityContext();
  const { executeAction } = useActions();
  const handleExecuteAction = useCallback(
    (action: ActionDefinition) => {
      executeAction(action);
    },
    [executeAction]
  );

  const topActionItems: ActionNavbarItem[] = useMemo(
    () => [...NavbarActionGroups.left],
    []
  );

  const renderActionItems = useCallback(
    (items: ActionNavbarItem[]) =>
      items
        .filter(
          (item): item is ActionDefinition =>
            !('type' in item) &&
            isActionVisible(item, actionCtx) &&
            !isSpecialIcon(getActionIcon(item, actionCtx))
        )
        .map((action) => {
          const icon = getActionIcon(action, actionCtx);
          if (isSpecialIcon(icon)) return null;
          const tooltip = getActionTooltip(action, actionCtx);
          const enabled = isActionEnabled(action, actionCtx);
          const active = isActionActive(action, actionCtx);
          return (
            <IconButton
              key={action.id}
              icon={icon}
              onClick={() => handleExecuteAction(action)}
              disabled={!enabled}
              aria-label={tooltip}
              title={tooltip}
              className={active ? 'text-normal' : ''}
            />
          );
        }),
    [actionCtx, handleExecuteAction]
  );

  const topActions = renderActionItems(topActionItems);

  const handleOpenSettings = useCallback(() => {
    void SettingsDialog.show();
  }, []);
  const handleViewArchive = useCallback(() => {
    setShowArchive(true);
  }, [setShowArchive]);

  return (
    <>
      <WorkspacesSidebar
        workspaces={paginatedActiveWorkspaces}
        totalWorkspacesCount={activeWorkspaces.length}
        archivedWorkspaces={paginatedArchivedWorkspaces}
        isLoading={isWorkspacesListLoading}
        selectedWorkspaceId={focusedSessionId ?? selectedWorkspaceId ?? null}
        onSelectWorkspace={handleSelectWorkspace}
        onAddWorkspace={handleAddWorkspace}
        onCreateInFolder={handleCreateInFolder}
        onOpenRoutines={appNavigation.routines ? handleOpenRoutines : undefined}
        isRoutinesActive={isRoutinesActive}
        onOpenAgents={handleOpenAgents}
        isAgentsActive={isAgentsActive}
        isCreateMode={isCreateMode}
        draftTitle={persistedDraftTitle}
        onSelectCreate={navigateToCreate}
        showArchive={showArchive}
        onShowArchiveChange={setShowArchive}
        layoutMode={layoutMode}
        onToggleLayoutMode={toggleLayoutMode}
        enableFlatGrouping={enableFlatGrouping}
        enableAccordionGrouping={enableAccordionGrouping}
        folderGroups={folderGroups}
        pinnedWorkspaces={pinnedWorkspaces}
        onLoadMore={handleLoadMore}
        hasMoreWorkspaces={hasMoreWorkspaces && !isSearching}
        onOpenWorkspaceActions={handleOpenWorkspaceActions}
        persistKeys={sidebarPersistKeys}
        activeRemoteHost={activeRemoteHost}
        onOpenRemoteHostSettings={handleOpenRemoteHostSettings}
        getWorkspaceDragProps={getWorkspaceDragProps}
        onReorderPins={handleReorderPins}
        onPinAreaHover={handlePinAreaHover}
        draggingWorkspaceId={draggingWorkspaceId}
        openInGridWorkspaceIds={openInGridWorkspaceIds}
        topActions={
          <>
            {topActions}
            <NavbarSidebarSearchSlot />
          </>
        }
        bottomActions={
          <>
            <ControlPanelMenu
              theme={theme}
              onSetTheme={handleSetTheme}
              onOpenSettings={handleOpenSettings}
              onViewArchive={handleViewArchive}
            />
            {/*
              Hidden: command bar + Settings buttons (Settings now lives in
              the ControlPanelMenu; command palette is intentionally stripped
              from the footer for now).

              Hidden: dev "Refresh sessions" escape hatch — uncomment to
              recover the local pinned-order reset + summary refetch button.

              <IconButton
                icon={ArrowClockwiseIcon}
                onClick={() => {
                  setOptimisticPinnedOrder(null);
                  queryClient.invalidateQueries({
                    queryKey: workspaceSummaryKeys.all,
                  });
                }}
                aria-label="Refresh sessions (dev)"
                title="Refresh sessions (dev) — drops local state and refetches"
              />
            */}
          </>
        }
      />
      <UnpinDragIndicator />
    </>
  );
}
