import { useCallback, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  PlusIcon,
  ArrowLeftIcon,
  StackIcon,
  SpinnerIcon,
  CaretDownIcon,
  LightningIcon,
  RobotIcon,
} from '@phosphor-icons/react';
import { useTranslation } from 'react-i18next';
import { cn } from '../lib/cn';
import { WorkspaceSummary } from './WorkspaceSummary';
import type { AppBarHostStatus } from './AppBar';
import {
  CollapsibleSectionHeader,
  type SectionAction,
} from './CollapsibleSectionHeader';

export type WorkspaceLayoutMode = 'folder' | 'flat' | 'accordion';

export interface WorkspacesSidebarWorkspace {
  id: string;
  name: string;
  filesChanged?: number;
  linesAdded?: number;
  linesRemoved?: number;
  isRunning?: boolean;
  isPinned?: boolean;
  hasPendingApproval?: boolean;
  hasRunningDevServer?: boolean;
  hasUnseenActivity?: boolean;
  latestProcessCompletedAt?: string;
  latestProcessStatus?: 'running' | 'completed' | 'failed' | 'killed';
  prStatus?: 'open' | 'merged' | 'closed' | 'unknown';
  primaryRepo?: {
    id: string;
    name: string;
    displayName: string;
    path: string;
  };
}

export interface WorkspacesSidebarFolderGroup {
  repoId: string;
  /** Repository path. The identity groups are keyed by. */
  repoPath: string;
  displayName: string;
  /**
   * Ancestor folders that tell this group apart from another one rendering the
   * same `displayName`. Absent when the label is already unique.
   */
  qualifier?: string;
  sessions: WorkspacesSidebarWorkspace[];
}

export interface WorkspacesSidebarPersistKeys {
  raisedHand: string;
  notRunning: string;
  running: string;
}

const DEFAULT_PERSIST_KEYS: WorkspacesSidebarPersistKeys = {
  raisedHand: 'workspaces-sidebar-raised-hand',
  notRunning: 'workspaces-sidebar-not-running',
  running: 'workspaces-sidebar-running',
};

const FOLDER_EXPANDED_KEY_PREFIX = 'workspaces-sidebar-folder-expanded:';

function readFolderExpanded(repoId: string): boolean {
  try {
    const raw = localStorage.getItem(`${FOLDER_EXPANDED_KEY_PREFIX}${repoId}`);
    if (raw === null) return true;
    return raw !== 'false';
  } catch {
    return true;
  }
}

function writeFolderExpanded(repoId: string, expanded: boolean): void {
  try {
    localStorage.setItem(
      `${FOLDER_EXPANDED_KEY_PREFIX}${repoId}`,
      expanded ? 'true' : 'false'
    );
  } catch {
    // localStorage may be unavailable
  }
}

export interface WorkspacesSidebarProps {
  workspaces: WorkspacesSidebarWorkspace[];
  totalWorkspacesCount: number;
  archivedWorkspaces?: WorkspacesSidebarWorkspace[];
  isLoading?: boolean;
  selectedWorkspaceId: string | null;
  onSelectWorkspace: (id: string) => void;
  onAddWorkspace?: () => void;
  /** Open the new-session page with a specific folder's repo pre-selected. */
  onCreateInFolder?: (repoId: string) => void;
  /** Whether we're in create mode */
  isCreateMode?: boolean;
  /** Title extracted from draft message (only shown when isCreateMode and non-empty) */
  draftTitle?: string;
  /** Handler to navigate back to create mode */
  onSelectCreate?: () => void;
  /** Whether to show archived workspaces */
  showArchive?: boolean;
  /** Handler for toggling archive view */
  onShowArchiveChange?: (show: boolean) => void;
  /** Active grouping mode. Defaults to 'folder'. */
  layoutMode?: WorkspaceLayoutMode;
  /** Handler for toggling between flat and accordion (only used when accordion flag is on). */
  onToggleLayoutMode?: () => void;
  /** Flag gates: when false (default), flat/accordion UI is suppressed entirely. */
  enableFlatGrouping?: boolean;
  enableAccordionGrouping?: boolean;
  /** Pre-grouped folder buckets for layoutMode='folder'. */
  folderGroups?: WorkspacesSidebarFolderGroup[];
  /** Pinned workspaces (lifted out of folder groups). */
  pinnedWorkspaces?: WorkspacesSidebarWorkspace[];
  /** Handler to load more workspaces on scroll */
  onLoadMore?: () => void;
  /** Whether there are more workspaces to load */
  hasMoreWorkspaces?: boolean;
  /** Callback for opening workspace actions */
  onOpenWorkspaceActions?: (workspaceId: string) => void;
  /** Persist keys for collapsible sections */
  persistKeys?: WorkspacesSidebarPersistKeys;
  activeRemoteHost?: {
    name: string;
    status: AppBarHostStatus;
  } | null;
  onOpenRemoteHostSettings?: () => void;
  /** Per-pill drag props (HTML5). Returning undefined disables drag for that pill. */
  getWorkspaceDragProps?: (workspaceId: string) =>
    | {
        draggable?: boolean;
        onDragStart?: (e: React.DragEvent<HTMLDivElement>) => void;
        onDragEnd?: (e: React.DragEvent<HTMLDivElement>) => void;
      }
    | undefined;
  /**
   * Called when a pill is dropped onto the Pinned section at a specific
   * slot. The consumer is expected to atomically reorder the pinned set
   * to exactly `orderedIds`. Workspaces not in the list are unpinned;
   * new ids are pinned.
   */
  onReorderPins?: (orderedIds: string[]) => void;
  /**
   * Fired during a pill drag whenever the cursor enters / leaves the
   * Pinned section. Lets the consumer drive a "release to unpin"
   * indicator while a pinned pill is being dragged outside the section.
   */
  onPinAreaHover?: (over: boolean) => void;
  /** Workspace id currently being dragged (drives pin reorder UI). */
  draggingWorkspaceId?: string | null;
  /**
   * Set of workspace ids currently open in any cell of the session grid.
   * Each pill in this set gets a background; the *focused* one (matched by
   * `selectedWorkspaceId`) additionally gets brighter + semibold.
   */
  openInGridWorkspaceIds?: Set<string>;
  /** Action row rendered at the top of the sidebar (above the new-session row). */
  topActions?: ReactNode;
  /** Action buttons rendered in the footer next to the archive toggle. */
  bottomActions?: ReactNode;
  /** Open the Routines page (rendered as a top-level nav row). */
  onOpenRoutines?: () => void;
  /** Whether the Routines nav row should render as active. */
  isRoutinesActive?: boolean;
  /** Open the Agents page (rendered as a top-level nav row). */
  onOpenAgents?: () => void;
  /** Whether the Agents nav row should render as active. */
  isAgentsActive?: boolean;
}

export interface WorkspacesSidebarReopenTagProps {
  active?: boolean;
  onHoverStart?: () => void;
  onHoverEnd?: () => void;
  ariaLabel?: string;
  className?: string;
}

export function WorkspacesSidebarReopenTag({
  active = false,
  onHoverStart,
  onHoverEnd,
  ariaLabel,
  className,
}: WorkspacesSidebarReopenTagProps) {
  return (
    <button
      type="button"
      onMouseEnter={onHoverStart}
      onMouseLeave={onHoverEnd}
      aria-label={ariaLabel ?? 'Preview sessions sidebar'}
      title={ariaLabel ?? 'Preview sessions sidebar'}
      className={cn(
        'group inline-flex h-24 w-2 items-center justify-center rounded-r-md bg-secondary/40 hover:bg-secondary/80 transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-brand focus-visible:ring-offset-2 cursor-e-resize',
        active && 'bg-secondary/80',
        className
      )}
    >
      <span className="grid grid-cols-1 gap-[2px] opacity-40 group-hover:opacity-80 transition-opacity">
        <span className="size-dot rounded-full bg-low" />
        <span className="size-dot rounded-full bg-low" />
        <span className="size-dot rounded-full bg-low" />
      </span>
    </button>
  );
}

function WorkspaceList({
  workspaces,
  selectedWorkspaceId,
  onSelectWorkspace,
  onOpenWorkspaceActions,
  getWorkspaceDragProps,
  openInGridWorkspaceIds,
}: {
  workspaces: WorkspacesSidebarWorkspace[];
  selectedWorkspaceId: string | null;
  onSelectWorkspace: (id: string) => void;
  onOpenWorkspaceActions: (workspaceId: string) => void;
  getWorkspaceDragProps?: WorkspacesSidebarProps['getWorkspaceDragProps'];
  openInGridWorkspaceIds?: ReadonlySet<string>;
}) {
  return (
    <>
      {workspaces.map((workspace) => (
        <WorkspaceSummary
          key={workspace.id}
          name={workspace.name}
          workspaceId={workspace.id}
          filesChanged={workspace.filesChanged}
          linesAdded={workspace.linesAdded}
          linesRemoved={workspace.linesRemoved}
          isActive={selectedWorkspaceId === workspace.id}
          isOpenInGrid={openInGridWorkspaceIds?.has(workspace.id)}
          isRunning={workspace.isRunning}
          isPinned={workspace.isPinned}
          hasPendingApproval={workspace.hasPendingApproval}
          hasRunningDevServer={workspace.hasRunningDevServer}
          hasUnseenActivity={workspace.hasUnseenActivity}
          latestProcessCompletedAt={workspace.latestProcessCompletedAt}
          latestProcessStatus={workspace.latestProcessStatus}
          prStatus={workspace.prStatus}
          summary
          onOpenWorkspaceActions={onOpenWorkspaceActions}
          onClick={() => onSelectWorkspace(workspace.id)}
          {...getWorkspaceDragProps?.(workspace.id)}
        />
      ))}
    </>
  );
}

function FolderGroup({
  group,
  selectedWorkspaceId,
  onSelectWorkspace,
  onOpenWorkspaceActions,
  getWorkspaceDragProps,
  openInGridWorkspaceIds,
  onCreateInFolder,
}: {
  group: WorkspacesSidebarFolderGroup;
  selectedWorkspaceId: string | null;
  onSelectWorkspace: (id: string) => void;
  onOpenWorkspaceActions: (workspaceId: string) => void;
  getWorkspaceDragProps?: WorkspacesSidebarProps['getWorkspaceDragProps'];
  openInGridWorkspaceIds?: ReadonlySet<string>;
  onCreateInFolder?: (repoId: string) => void;
}) {
  const { t } = useTranslation('common');
  const [expanded, setExpandedState] = useState(() =>
    readFolderExpanded(group.repoId)
  );

  const toggle = useCallback(() => {
    setExpandedState((prev) => {
      const next = !prev;
      writeFolderExpanded(group.repoId, next);
      return next;
    });
  }, [group.repoId]);

  return (
    <div className="flex flex-col">
      <div className="group flex items-center px-double py-half text-sm text-low opacity-60 hover:opacity-100 hover:text-normal transition-colors lowercase">
        <button
          type="button"
          onClick={toggle}
          title={group.repoPath}
          className="min-w-0 flex items-center gap-half text-left"
        >
          <span className="truncate">{group.displayName}</span>
          {group.qualifier && (
            /* The qualifier is secondary: it gives up width before the repo
               name does, so the name never truncates while a hint is intact. */
            <span className="truncate normal-case text-low/70 [flex-shrink:9999]">
              {group.qualifier}
            </span>
          )}
          <CaretDownIcon
            className={cn(
              'size-icon-xs shrink-0 opacity-0 group-hover:opacity-100 transition-transform',
              expanded ? 'rotate-0' : '-rotate-90'
            )}
            weight="bold"
          />
        </button>
        {onCreateInFolder && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onCreateInFolder(group.repoId);
            }}
            aria-label={t('sidebar.newSessionInFolder', {
              defaultValue: 'New session in {{folder}}',
              folder: group.qualifier
                ? `${group.displayName} (${group.qualifier})`
                : group.displayName,
            })}
            className="ml-auto shrink-0 pl-base opacity-0 group-hover:opacity-100 transition-opacity"
          >
            <PlusIcon className="size-icon-xs" weight="bold" />
          </button>
        )}
      </div>
      {expanded && (
        <div className="flex flex-col gap-[2px]">
          <WorkspaceList
            workspaces={group.sessions}
            selectedWorkspaceId={selectedWorkspaceId}
            onSelectWorkspace={onSelectWorkspace}
            onOpenWorkspaceActions={onOpenWorkspaceActions}
            getWorkspaceDragProps={getWorkspaceDragProps}
            openInGridWorkspaceIds={openInGridWorkspaceIds}
          />
        </div>
      )}
    </div>
  );
}

function NewSessionRow({ onAddWorkspace }: { onAddWorkspace?: () => void }) {
  const { t } = useTranslation('common');
  return (
    <button
      type="button"
      onClick={onAddWorkspace}
      className="w-full flex items-center gap-base px-double py-half text-base text-normal hover:bg-tertiary/60 transition-colors"
    >
      <PlusIcon className="size-icon-sm" weight="bold" />
      <span>{t('sidebar.newSession', { defaultValue: 'New session' })}</span>
    </button>
  );
}

function RoutinesNavRow({
  onOpenRoutines,
  isActive,
}: {
  onOpenRoutines?: () => void;
  isActive?: boolean;
}) {
  const { t } = useTranslation('common');
  return (
    <button
      type="button"
      onClick={onOpenRoutines}
      className={cn(
        'w-full flex items-center gap-base px-double py-half text-base text-normal hover:bg-tertiary/60 transition-colors',
        isActive && 'bg-tertiary/60'
      )}
    >
      <LightningIcon className="size-icon-sm" />
      <span>{t('routines.sidebar.nav', { defaultValue: 'Routines' })}</span>
    </button>
  );
}

function AgentsNavRow({
  onOpenAgents,
  isActive,
}: {
  onOpenAgents?: () => void;
  isActive?: boolean;
}) {
  const { t } = useTranslation('common');
  return (
    <button
      type="button"
      onClick={onOpenAgents}
      className={cn(
        'w-full flex items-center gap-base px-double py-half text-base text-normal hover:bg-tertiary/60 transition-colors',
        isActive && 'bg-tertiary/60'
      )}
    >
      <RobotIcon className="size-icon-sm" />
      <span>{t('agents.sidebar.nav', { defaultValue: 'Agents' })}</span>
    </button>
  );
}

/** Pixel height of an expanded drop slot (must accommodate one pill + gap). */
const PINNED_SLOT_OPEN_HEIGHT = 52;

function PinnedSection({
  pinnedWorkspaces,
  selectedWorkspaceId,
  onSelectWorkspace,
  onOpenWorkspaceActions,
  getWorkspaceDragProps,
  onReorderPins,
  onPinAreaHover,
  openInGridWorkspaceIds,
  draggingWorkspaceId,
}: {
  pinnedWorkspaces: WorkspacesSidebarWorkspace[];
  selectedWorkspaceId: string | null;
  onSelectWorkspace: (id: string) => void;
  onOpenWorkspaceActions: (workspaceId: string) => void;
  getWorkspaceDragProps?: WorkspacesSidebarProps['getWorkspaceDragProps'];
  onReorderPins?: WorkspacesSidebarProps['onReorderPins'];
  onPinAreaHover?: WorkspacesSidebarProps['onPinAreaHover'];
  openInGridWorkspaceIds?: ReadonlySet<string>;
  draggingWorkspaceId?: string | null;
}) {
  const { t } = useTranslation('common');
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const pillRefs = useRef<Array<HTMLDivElement | null>>([]);

  const sourceIndex = useMemo(() => {
    if (!draggingWorkspaceId) return -1;
    return pinnedWorkspaces.findIndex((w) => w.id === draggingWorkspaceId);
  }, [draggingWorkspaceId, pinnedWorkspaces]);

  const computeNewOrder = useCallback(
    (slotIdx: number): string[] | null => {
      if (!draggingWorkspaceId) return null;
      const filtered = pinnedWorkspaces
        .filter((w) => w.id !== draggingWorkspaceId)
        .map((w) => w.id);
      // Slots are between original positions; if source was before slotIdx,
      // its removal shifts the effective insertion point one earlier.
      const insertAt =
        sourceIndex !== -1 && slotIdx > sourceIndex ? slotIdx - 1 : slotIdx;
      const next = [...filtered];
      next.splice(insertAt, 0, draggingWorkspaceId);
      const current = pinnedWorkspaces.map((w) => w.id);
      if (
        next.length === current.length &&
        next.every((id, i) => id === current[i])
      ) {
        return null;
      }
      return next;
    },
    [draggingWorkspaceId, pinnedWorkspaces, sourceIndex]
  );

  // dropIndex equal to sourceIndex or sourceIndex+1 means "leave it where it
  // was" — don't render a visual gap there (it'd confuse the user).
  const showGapAt = useCallback(
    (slotIdx: number): boolean => {
      if (dropIndex !== slotIdx) return false;
      if (sourceIndex === -1) return true; // external drag (always informative)
      return slotIdx !== sourceIndex && slotIdx !== sourceIndex + 1;
    },
    [dropIndex, sourceIndex]
  );

  const handleSectionDragOver = (e: React.DragEvent<HTMLDivElement>) => {
    if (!onReorderPins || !draggingWorkspaceId) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    // Compute the insertion index from cursor Y vs each pill's vertical midpoint.
    let idx = pinnedWorkspaces.length;
    for (let i = 0; i < pinnedWorkspaces.length; i++) {
      const rect = pillRefs.current[i]?.getBoundingClientRect();
      if (!rect) continue;
      if (e.clientY < rect.top + rect.height / 2) {
        idx = i;
        break;
      }
    }
    setDropIndex(idx);
    onPinAreaHover?.(true);
  };

  const handleSectionDragLeave = (e: React.DragEvent<HTMLDivElement>) => {
    if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
    setDropIndex(null);
    onPinAreaHover?.(false);
  };

  const handleSectionDrop = (e: React.DragEvent<HTMLDivElement>) => {
    if (!onReorderPins || !draggingWorkspaceId) return;
    e.preventDefault();
    const slotIdx = dropIndex ?? pinnedWorkspaces.length;
    // Always emit (even on no-op self-drop) so the consumer can mark
    // "dropped in pin section" and prevent the source's onDragEnd from
    // interpreting the release as an unpin.
    const newOrder =
      computeNewOrder(slotIdx) ?? pinnedWorkspaces.map((w) => w.id);
    setDropIndex(null);
    // Don't call onPinAreaHover(false) here: the source pill's onDragEnd
    // (which fires right after) calls setDragging(null) and that resets
    // isOverDropTarget. Calling false here would briefly flash the
    // "release to unpin" indicator between drop and dragend.
    onReorderPins(newOrder);
  };

  // Empty pinned list: render a single stable structure regardless of
  // whether a drag is in progress. The two branches used to differ in
  // element type (<p> vs <div>), in attached drag handlers, and in
  // min-height — so flipping between them at dragstart relocated the
  // source pill's bounding box and Chrome cancelled the drag (the
  // observed "drag only works when there are pinned sessions" symptom).
  // Now: handlers always attached, fixed min-height, only the hint text
  // swaps with the drag state.
  if (pinnedWorkspaces.length === 0) {
    return (
      <div
        className="flex flex-col"
        onDragOver={handleSectionDragOver}
        onDragLeave={handleSectionDragLeave}
        onDrop={handleSectionDrop}
      >
        <div className="px-double py-half">
          <span className="text-sm text-low opacity-60">
            {t('sidebar.pinned.sectionHeader', { defaultValue: 'Pinned' })}
          </span>
        </div>
        <div
          className="px-double py-half text-sm text-low opacity-60 rounded-md transition-colors"
          style={{
            backgroundColor:
              dropIndex !== null ? 'rgba(59, 130, 246, 0.1)' : undefined,
            minHeight: PINNED_SLOT_OPEN_HEIGHT,
          }}
        >
          {draggingWorkspaceId
            ? t('sidebar.pinned.dropHint', {
                defaultValue: 'Drop here to pin',
              })
            : t('sidebar.pinned.emptyHint', {
                defaultValue: 'Pin sessions from their menu to keep them here.',
              })}
        </div>
      </div>
    );
  }

  return (
    <div
      className="flex flex-col"
      onDragOver={handleSectionDragOver}
      onDragLeave={handleSectionDragLeave}
      onDrop={handleSectionDrop}
    >
      <div className="px-double py-half">
        <span className="text-sm text-low opacity-60">
          {t('sidebar.pinned.sectionHeader', { defaultValue: 'Pinned' })}
        </span>
      </div>
      <div className="flex flex-col">
        {pinnedWorkspaces.map((workspace, i) => {
          const isSource = i === sourceIndex;
          return (
            <div key={workspace.id}>
              <div
                className="transition-[height] duration-150 ease-out"
                style={{ height: showGapAt(i) ? PINNED_SLOT_OPEN_HEIGHT : 0 }}
              />
              <div
                ref={(el) => {
                  pillRefs.current[i] = el;
                }}
                className="transition-opacity duration-150"
                style={{ opacity: isSource ? 0.3 : 1 }}
              >
                <WorkspaceSummary
                  name={workspace.name}
                  workspaceId={workspace.id}
                  filesChanged={workspace.filesChanged}
                  linesAdded={workspace.linesAdded}
                  linesRemoved={workspace.linesRemoved}
                  isActive={selectedWorkspaceId === workspace.id}
                  isOpenInGrid={openInGridWorkspaceIds?.has(workspace.id)}
                  isRunning={workspace.isRunning}
                  isPinned={workspace.isPinned}
                  hasPendingApproval={workspace.hasPendingApproval}
                  hasRunningDevServer={workspace.hasRunningDevServer}
                  hasUnseenActivity={workspace.hasUnseenActivity}
                  latestProcessCompletedAt={workspace.latestProcessCompletedAt}
                  latestProcessStatus={workspace.latestProcessStatus}
                  prStatus={workspace.prStatus}
                  summary
                  onOpenWorkspaceActions={onOpenWorkspaceActions}
                  onClick={() => onSelectWorkspace(workspace.id)}
                  {...getWorkspaceDragProps?.(workspace.id)}
                />
              </div>
              <div style={{ height: 2 }} />
            </div>
          );
        })}
        <div
          className="transition-[height] duration-150 ease-out"
          style={{
            height: showGapAt(pinnedWorkspaces.length)
              ? PINNED_SLOT_OPEN_HEIGHT
              : 0,
          }}
        />
      </div>
    </div>
  );
}

export function WorkspacesSidebar({
  workspaces,
  totalWorkspacesCount,
  archivedWorkspaces = [],
  isLoading = false,
  selectedWorkspaceId,
  onSelectWorkspace,
  onAddWorkspace,
  onCreateInFolder,
  // vibe-kanban: workspace create-mode UI — restore if inline-create returns
  // isCreateMode = false,
  draftTitle,
  // onSelectCreate,
  showArchive = false,
  onShowArchiveChange,
  layoutMode = 'folder',
  onToggleLayoutMode,
  enableFlatGrouping = false,
  enableAccordionGrouping = false,
  folderGroups = [],
  pinnedWorkspaces = [],
  onLoadMore,
  hasMoreWorkspaces = false,
  onOpenWorkspaceActions,
  persistKeys = DEFAULT_PERSIST_KEYS,
  activeRemoteHost = null,
  onOpenRemoteHostSettings,
  getWorkspaceDragProps,
  onReorderPins,
  onPinAreaHover,
  draggingWorkspaceId,
  openInGridWorkspaceIds,
  topActions,
  bottomActions,
  onOpenRoutines,
  isRoutinesActive,
  onOpenAgents,
  isAgentsActive,
}: WorkspacesSidebarProps) {
  const { t } = useTranslation(['tasks', 'common']);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const handleOpenWorkspaceActions = useCallback(
    (workspaceId: string) => {
      onOpenWorkspaceActions?.(workspaceId);
    },
    [onOpenWorkspaceActions]
  );

  // Handle scroll to load more
  const handleScroll = () => {
    if (!hasMoreWorkspaces || !onLoadMore) return;

    const container = scrollContainerRef.current;
    if (!container) return;

    const { scrollTop, scrollHeight, clientHeight } = container;
    // Load more when scrolled within 100px of the bottom
    if (scrollHeight - scrollTop - clientHeight < 100) {
      onLoadMore();
    }
  };

  // Categorize workspaces for accordion layout
  const { raisedHandWorkspaces, idleWorkspaces, runningWorkspaces } =
    useMemo(() => {
      // Running workspaces should stay in the "Running" section even if unseen.
      const needsAttention = (ws: WorkspacesSidebarWorkspace) =>
        ws.hasPendingApproval || (ws.hasUnseenActivity && !ws.isRunning);

      return {
        raisedHandWorkspaces: workspaces.filter((ws) => needsAttention(ws)),
        idleWorkspaces: workspaces.filter(
          (ws) => !ws.isRunning && !needsAttention(ws)
        ),
        runningWorkspaces: workspaces.filter(
          (ws) => ws.isRunning && !needsAttention(ws)
        ),
      };
    }, [workspaces]);

  const headerActions: SectionAction[] = enableAccordionGrouping
    ? [
        {
          icon: StackIcon,
          onClick: () => onToggleLayoutMode?.(),
          isActive: layoutMode === 'accordion',
        },
      ]
    : [];

  const resolvedMode: WorkspaceLayoutMode = showArchive
    ? 'flat'
    : layoutMode === 'accordion' && enableAccordionGrouping
      ? 'accordion'
      : layoutMode === 'flat' && enableFlatGrouping
        ? 'flat'
        : 'folder';

  return (
    <div className="w-full h-full bg-[#fdfdfc] dark:bg-secondary flex flex-col rounded-2xl border border-[#d4d4d4] dark:border-[#1e1e1e] overflow-hidden pt-base">
      {topActions && (
        <div className="px-double pb-half flex items-center gap-base">
          {topActions}
        </div>
      )}
      {/* Legacy header title row — only rendered when a mode-toggle is available */}
      {headerActions.length > 0 && (
        <CollapsibleSectionHeader
          title={t('common:workspaces.title')}
          collapsible={false}
          actions={headerActions}
          className="border-b"
        />
      )}

      {/* New Session row */}
      {!isLoading && !showArchive && (
        <NewSessionRow onAddWorkspace={onAddWorkspace} />
      )}

      {/* Routines nav row */}
      {!isLoading && !showArchive && onOpenRoutines && (
        <RoutinesNavRow
          onOpenRoutines={onOpenRoutines}
          isActive={isRoutinesActive}
        />
      )}

      {/* Agents nav row */}
      {!isLoading && !showArchive && onOpenAgents && (
        <AgentsNavRow onOpenAgents={onOpenAgents} isActive={isAgentsActive} />
      )}

      {activeRemoteHost && (
        <div className="px-base py-half">
          <div className="rounded-sm border border-border bg-panel/60 px-base py-half flex items-center justify-between gap-base">
            <div className="min-w-0">
              <p className="text-xs text-low uppercase tracking-wide">
                {t('common:workspaces.remoteHostLabel', {
                  defaultValue: 'Remote host',
                })}
              </p>
              <p className="text-sm text-high truncate">
                {activeRemoteHost.name}
              </p>
            </div>
            <div className="flex items-center gap-half shrink-0">
              <span
                className={cn(
                  'inline-flex h-2.5 w-2.5 rounded-full',
                  activeRemoteHost.status === 'online'
                    ? 'bg-success'
                    : activeRemoteHost.status === 'offline'
                      ? 'bg-low'
                      : 'bg-warning'
                )}
                aria-hidden="true"
              />
              {onOpenRemoteHostSettings && (
                <button
                  type="button"
                  onClick={onOpenRemoteHostSettings}
                  className="text-xs text-brand hover:underline"
                >
                  {t('common:workspaces.remoteHostManage', {
                    defaultValue: 'Manage',
                  })}
                </button>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Scrollable workspace list */}
      <div
        ref={scrollContainerRef}
        onScroll={handleScroll}
        className="flex-1 overflow-y-auto py-base"
      >
        {isLoading ? (
          <div className="flex h-full min-h-[220px] items-center justify-center px-base">
            <div className="flex items-center justify-center text-low">
              <SpinnerIcon className="size-6 animate-spin" weight="bold" />
            </div>
          </div>
        ) : showArchive ? (
          /* Archived workspaces view */
          <div className="flex flex-col gap-base">
            <span className="text-sm font-medium text-low px-base">
              {t('common:workspaces.archived')}
            </span>
            {archivedWorkspaces.length === 0 ? (
              <span className="text-sm text-low opacity-60 px-base">
                {t('common:workspaces.noArchived')}
              </span>
            ) : (
              archivedWorkspaces.map((workspace) => (
                <WorkspaceSummary
                  summary
                  key={workspace.id}
                  name={workspace.name}
                  workspaceId={workspace.id}
                  filesChanged={workspace.filesChanged}
                  linesAdded={workspace.linesAdded}
                  linesRemoved={workspace.linesRemoved}
                  isActive={selectedWorkspaceId === workspace.id}
                  isOpenInGrid={openInGridWorkspaceIds?.has(workspace.id)}
                  isRunning={workspace.isRunning}
                  isPinned={workspace.isPinned}
                  hasPendingApproval={workspace.hasPendingApproval}
                  hasRunningDevServer={workspace.hasRunningDevServer}
                  hasUnseenActivity={workspace.hasUnseenActivity}
                  latestProcessCompletedAt={workspace.latestProcessCompletedAt}
                  latestProcessStatus={workspace.latestProcessStatus}
                  prStatus={workspace.prStatus}
                  onOpenWorkspaceActions={handleOpenWorkspaceActions}
                  onClick={() => onSelectWorkspace(workspace.id)}
                  {...getWorkspaceDragProps?.(workspace.id)}
                />
              ))
            )}
          </div>
        ) : resolvedMode === 'accordion' ? (
          /* Accordion layout view (flag-gated) */
          <div className="flex flex-col gap-base">
            {/* Needs Attention section */}
            <CollapsibleSectionHeader
              title={t('common:workspaces.needsAttention')}
              persistKey={persistKeys.raisedHand}
              defaultExpanded={true}
            >
              <div className="flex flex-col gap-base py-half">
                {/* {draftTitle && (
                  <WorkspaceSummary
                    name={draftTitle}
                    isActive={isCreateMode}
                    isDraft={true}
                    onClick={onSelectCreate}
                  />
                )} */}
                {raisedHandWorkspaces.length === 0 && !draftTitle ? (
                  <span className="text-sm text-low opacity-60 pl-base">
                    {t('common:workspaces.noWorkspaces')}
                  </span>
                ) : (
                  <WorkspaceList
                    workspaces={raisedHandWorkspaces}
                    selectedWorkspaceId={selectedWorkspaceId}
                    onSelectWorkspace={onSelectWorkspace}
                    onOpenWorkspaceActions={handleOpenWorkspaceActions}
                    getWorkspaceDragProps={getWorkspaceDragProps}
                    openInGridWorkspaceIds={openInGridWorkspaceIds}
                  />
                )}
              </div>
            </CollapsibleSectionHeader>

            {/* Running section */}
            <CollapsibleSectionHeader
              title={t('common:workspaces.running')}
              persistKey={persistKeys.running}
              defaultExpanded={true}
            >
              <div className="flex flex-col gap-base py-half">
                {runningWorkspaces.length === 0 ? (
                  <span className="text-sm text-low opacity-60 pl-base">
                    {t('common:workspaces.noWorkspaces')}
                  </span>
                ) : (
                  <WorkspaceList
                    workspaces={runningWorkspaces}
                    selectedWorkspaceId={selectedWorkspaceId}
                    onSelectWorkspace={onSelectWorkspace}
                    onOpenWorkspaceActions={handleOpenWorkspaceActions}
                    getWorkspaceDragProps={getWorkspaceDragProps}
                    openInGridWorkspaceIds={openInGridWorkspaceIds}
                  />
                )}
              </div>
            </CollapsibleSectionHeader>

            {/* Idle section */}
            <CollapsibleSectionHeader
              title={t('common:workspaces.idle')}
              persistKey={persistKeys.notRunning}
              defaultExpanded={true}
            >
              <div className="flex flex-col gap-base py-half">
                {idleWorkspaces.length === 0 ? (
                  <span className="text-sm text-low opacity-60 pl-base">
                    {t('common:workspaces.noWorkspaces')}
                  </span>
                ) : (
                  <WorkspaceList
                    workspaces={idleWorkspaces}
                    selectedWorkspaceId={selectedWorkspaceId}
                    onSelectWorkspace={onSelectWorkspace}
                    onOpenWorkspaceActions={handleOpenWorkspaceActions}
                    getWorkspaceDragProps={getWorkspaceDragProps}
                    openInGridWorkspaceIds={openInGridWorkspaceIds}
                  />
                )}
              </div>
            </CollapsibleSectionHeader>
          </div>
        ) : resolvedMode === 'flat' ? (
          /* Active workspaces flat view (flag-gated) */
          <div className="flex flex-col gap-base">
            <div className="flex items-center justify-between px-base">
              <span className="text-sm font-medium text-low">
                {t('common:workspaces.active')}
              </span>
              <span className="text-xs text-low">{totalWorkspacesCount}</span>
            </div>
            {/* {draftTitle && (
              <WorkspaceSummary
                name={draftTitle}
                isActive={isCreateMode}
                isDraft={true}
                onClick={onSelectCreate}
              />
            )} */}
            {workspaces.map((workspace) => (
              <WorkspaceSummary
                key={workspace.id}
                name={workspace.name}
                workspaceId={workspace.id}
                filesChanged={workspace.filesChanged}
                linesAdded={workspace.linesAdded}
                linesRemoved={workspace.linesRemoved}
                isActive={selectedWorkspaceId === workspace.id}
                isOpenInGrid={openInGridWorkspaceIds?.has(workspace.id)}
                isRunning={workspace.isRunning}
                isPinned={workspace.isPinned}
                hasPendingApproval={workspace.hasPendingApproval}
                hasRunningDevServer={workspace.hasRunningDevServer}
                hasUnseenActivity={workspace.hasUnseenActivity}
                latestProcessCompletedAt={workspace.latestProcessCompletedAt}
                latestProcessStatus={workspace.latestProcessStatus}
                prStatus={workspace.prStatus}
                onOpenWorkspaceActions={handleOpenWorkspaceActions}
                onClick={() => onSelectWorkspace(workspace.id)}
                {...getWorkspaceDragProps?.(workspace.id)}
              />
            ))}
          </div>
        ) : (
          /* Default: Pinned + folder groups */
          <div className="flex flex-col gap-base">
            {/* {draftTitle && (
              <WorkspaceSummary
                name={draftTitle}
                isActive={isCreateMode}
                isDraft={true}
                onClick={onSelectCreate}
              />
            )} */}
            <PinnedSection
              pinnedWorkspaces={pinnedWorkspaces}
              selectedWorkspaceId={selectedWorkspaceId}
              onSelectWorkspace={onSelectWorkspace}
              onOpenWorkspaceActions={handleOpenWorkspaceActions}
              getWorkspaceDragProps={getWorkspaceDragProps}
              onReorderPins={onReorderPins}
              onPinAreaHover={onPinAreaHover}
              draggingWorkspaceId={draggingWorkspaceId}
              openInGridWorkspaceIds={openInGridWorkspaceIds}
            />
            {folderGroups.map((group) => (
              <FolderGroup
                key={group.repoId}
                group={group}
                selectedWorkspaceId={selectedWorkspaceId}
                onSelectWorkspace={onSelectWorkspace}
                onOpenWorkspaceActions={handleOpenWorkspaceActions}
                getWorkspaceDragProps={getWorkspaceDragProps}
                openInGridWorkspaceIds={openInGridWorkspaceIds}
                onCreateInFolder={onCreateInFolder}
              />
            ))}
          </div>
        )}
      </div>

      {/* Footer: back-to-active arrow when in archive view + bottom actions (control panel) */}
      <div className="p-double flex items-center gap-base">
        {showArchive && (
          <button
            onClick={() => onShowArchiveChange?.(false)}
            aria-label={t('common:workspaces.backToActive')}
            title={t('common:workspaces.backToActive')}
            className="inline-flex h-7 w-7 items-center justify-center rounded text-low hover:text-normal hover:bg-tertiary/60 transition-colors"
          >
            <ArrowLeftIcon className="size-icon-xs" />
          </button>
        )}
        <div className="ml-auto flex items-center gap-base">
          {bottomActions}
        </div>
      </div>
    </div>
  );
}
