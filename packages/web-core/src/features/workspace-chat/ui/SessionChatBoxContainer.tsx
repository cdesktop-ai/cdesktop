import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { useDropzone } from 'react-dropzone';
import {
  type AskUserQuestionItem,
  BaseAgentCapability,
  type Session,
  type BaseCodingAgent,
  ExecutionProcessStatus,
} from 'shared/types';
import { AgentIcon } from '@/shared/components/AgentIcon';
import { useHostId } from '@/shared/providers/HostIdProvider';
import { workspaceSessionKeys } from '@/shared/hooks/workspaceSessionKeys';
import { useWorkspaceExecution } from '@/shared/hooks/useWorkspaceExecution';
import { useWorkspaceRepo } from '@/shared/hooks/useWorkspaceRepo';
import { useUserSystem } from '@/shared/hooks/useUserSystem';
import WYSIWYGEditor from '@/shared/components/WYSIWYGEditor';
import { VoiceInputButton } from '@/shared/components/VoiceInputButton';
import { useApprovalFeedbackOptional } from '../model/contexts/ApprovalFeedbackContext';
import { useMessageEditContext } from '../model/contexts/MessageEditContext';
import { useEntries, useTokenUsage } from '../model/contexts/EntriesContext';
import { useExecutionProcesses } from '@/shared/hooks/useExecutionProcesses';
import { isAgentDefaultModelId } from '@/shared/lib/agentDefaultModel';
import { useReviewOptional } from '@/shared/hooks/useReview';
import { useActions } from '@/shared/hooks/useActions';
import { useTodos } from '../model/hooks/useTodos';
import { getLatestConfigFromProcesses } from '@/shared/lib/executor';
import { useExecutorConfig } from '@/shared/hooks/useExecutorConfig';
import { useModelSelectorConfig } from '@/shared/hooks/useExecutorDiscovery';
import {
  SHOW_INNER_SESSION_SWITCHER,
  SHOW_RESOLVE_CONFLICTS,
} from '@/shared/lib/cdesktopFlags';
import { useSessionMessageEditor } from '../model/hooks/useSessionMessageEditor';
import { useSessionQueueInteraction } from '../model/hooks/useSessionQueueInteraction';
import { useSessionSend } from '../model/hooks/useSessionSend';
import { useSessionAttachments } from '../model/hooks/useSessionAttachments';
import { useMessageEditRetry } from '../model/hooks/useMessageEditRetry';
import { useBranchStatus } from '@/shared/hooks/useBranchStatus';
import { useWorkspaceBranch } from '../model/hooks/useWorkspaceBranch';
import { useApprovalMutation } from '../model/hooks/useApprovalMutation';
import { useApprovals } from '@/shared/hooks/useApprovals';
import { ResolveConflictsDialog } from '@/shared/dialogs/tasks/ResolveConflictsDialog';
import { workspaceSummaryKeys } from '@/shared/hooks/workspaceSummaryKeys';
import { buildAgentPrompt } from '@/shared/lib/promptMessage';
import { formatDateShortWithTime } from '@/shared/lib/date';
import { toPrettyCase } from '@/shared/lib/string';
import {
  SessionChatBox,
  type ExecutionStatus,
  type SessionChatBoxEditorRenderProps,
} from '@vibe/ui/components/SessionChatBox';
import { ModelSelectorContainer } from '@/shared/components/ModelSelectorContainer';
import { TeamPillRowContainer } from './TeamPillRowContainer';
import { ProviderModelPicker } from '@/shared/components/ProviderModelPicker';
import {
  useWorkspacePickerSelection,
  writeLastUsed,
  resolveDefaultSelection,
} from '@/shared/hooks/useWorkspacePickerSelection';
import { useProviders } from '@/shared/hooks/useProviders';
import { useWorkspacePanelLayout } from '@/shared/stores/useUiPreferencesStore';
import { useInspectModeStore } from '../model/store/useInspectModeStore';
import { Actions } from '@/shared/actions';
import {
  isSpecialIcon,
  getActionTooltip,
  isActionEnabled,
  isActionVisible,
  type ActionDefinition,
} from '@/shared/types/actions';
import { SettingsDialog } from '@/shared/dialogs/settings/SettingsDialog';
import { useActionVisibilityContext } from '@/shared/hooks/useActionVisibilityContext';
import { PrCommentsDialog } from '@/shared/dialogs/tasks/PrCommentsDialog';
import type { NormalizedComment } from '@vibe/ui/components/pr-comment-node';
import { useAppNavigation } from '@/shared/hooks/useAppNavigation';
import { sessionsApi } from '@/shared/lib/api';
import { RenameSessionDialog } from '@vibe/ui/components/RenameSessionDialog';
import type { TurnNavigationItem } from '@vibe/ui/components/TurnNavigationPopup';

/** Compute execution status from boolean flags */
function computeExecutionStatus(params: {
  isInFeedbackMode: boolean;
  isInEditMode: boolean;
  isStopping: boolean;
  isQueueLoading: boolean;
  isSendingFollowUp: boolean;
  isQueued: boolean;
  isAttemptRunning: boolean;
}): ExecutionStatus {
  if (params.isInFeedbackMode) return 'feedback';
  if (params.isInEditMode) return 'edit';
  if (params.isStopping) return 'stopping';
  if (params.isQueueLoading) return 'queue-loading';
  if (params.isSendingFollowUp) return 'sending';
  if (params.isQueued) return 'queued';
  if (params.isAttemptRunning) return 'running';
  return 'idle';
}

type ResponseMode = 'call' | 'chat';

function addResponseModeInstruction(prompt: string, mode: ResponseMode) {
  if (mode === 'call') {
    return `${prompt}\n\n[Réponds en phrases courtes, adaptées à une conversation orale.]`;
  }
  return `${prompt}\n\n[Réponds de façon détaillée et structurée, sauf si une réponse courte suffit.]`;
}

/** Shared props across all modes */
interface SharedProps {
  /** Available sessions for this workspace */
  sessions: Session[];
  /** Number of files changed in current session */
  filesChanged: number;
  /** Number of lines added */
  linesAdded: number;
  /** Number of lines removed */
  linesRemoved: number;
  /** Callback to scroll to previous user message */
  onScrollToPreviousMessage: () => void;
  /** Callback to scroll to bottom of conversation */
  onScrollToBottom: (behavior?: 'auto' | 'smooth') => void;
  /** Callback to scroll to a specific user message by patchKey */
  onScrollToUserMessage: (patchKey: string) => void;
  /** Returns the patchKey of the user message currently visible in the viewport */
  getActiveTurnPatchKey?: () => string | null;
  /** Disable the "view code" click handler (for VS Code extension) */
  disableViewCode: boolean;
  /** Replace diff stats with an "Open Workspace" button in header */
  showOpenWorkspaceButton: boolean;
}

/** Props for existing session mode */
interface ExistingSessionProps extends SharedProps {
  mode: 'existing-session';
  /** The current session */
  session: Session;
  /** Called when a session is selected */
  onSelectSession: (sessionId: string) => void;
  /** Callback to start new session mode */
  onStartNewSession: (() => void) | undefined;
}

/** Props for new session mode */
interface NewSessionProps extends SharedProps {
  mode: 'new-session';
  /** Workspace ID for creating new sessions */
  workspaceId: string;
  /** Called when a session is selected */
  onSelectSession: (sessionId: string) => void;
}

/** Props for placeholder mode (no workspace selected) */
interface PlaceholderProps extends SharedProps {
  mode: 'placeholder';
}

type SessionChatBoxContainerProps =
  | ExistingSessionProps
  | NewSessionProps
  | PlaceholderProps;

export function SessionChatBoxContainer(props: SessionChatBoxContainerProps) {
  const { t } = useTranslation('common');
  const [responseMode, setResponseMode] = useState<ResponseMode>(() =>
    (localStorage.getItem('freeclaude-response-mode') ??
      localStorage.getItem('cdesktop-response-mode')) === 'call'
      ? 'call'
      : 'chat'
  );
  useEffect(() => {
    localStorage.setItem('freeclaude-response-mode', responseMode);
  }, [responseMode]);
  const handledQuotaProcessIds = useRef(new Set<string>());
  const quotaHistoryInitialized = useRef(false);
  const quotaFailuresBySession = useRef(new Map<string, Set<string>>());
  const [quotaNotice, setQuotaNotice] = useState<string | null>(null);
  const {
    mode,
    sessions,
    filesChanged,
    linesAdded,
    linesRemoved,
    onScrollToPreviousMessage,
    onScrollToBottom,
    onScrollToUserMessage,
    getActiveTurnPatchKey,
    disableViewCode = false,
    showOpenWorkspaceButton,
  } = props;

  // Extract mode-specific values
  const session = mode === 'existing-session' ? props.session : undefined;
  const workspaceId =
    mode === 'existing-session'
      ? props.session.workspace_id
      : mode === 'new-session'
        ? props.workspaceId
        : undefined;
  const isNewSessionMode = mode === 'new-session';
  const onSelectSession =
    mode === 'placeholder' ? undefined : props.onSelectSession;
  const onStartNewSession =
    mode === 'existing-session' ? props.onStartNewSession : undefined;

  const sessionId = session?.id;
  const queryClient = useQueryClient();
  const hostId = useHostId();

  const handleRenameSession = useCallback(
    (targetSessionId: string, currentName: string) => {
      void RenameSessionDialog.show({
        currentName,
        onRename: async (newName: string) => {
          await sessionsApi.update(targetSessionId, { name: newName });
          void queryClient.invalidateQueries({
            queryKey: workspaceSessionKeys.byWorkspace(workspaceId, hostId),
          });
        },
      });
    },
    [queryClient, hostId, workspaceId]
  );
  const appNavigation = useAppNavigation();

  const { executeAction } = useActions();
  const actionCtx = useActionVisibilityContext();
  const { togglePanel } = useWorkspacePanelLayout(workspaceId);

  const handleViewCode = useCallback(() => {
    togglePanel('changes');
  }, [togglePanel]);

  const handleOpenWorkspace = useCallback(() => {
    if (!workspaceId) return;
    appNavigation.goToWorkspace(workspaceId);
  }, [appNavigation, workspaceId]);

  // Get entries early to extract pending approval for scratch key
  const { entries } = useEntries();
  const tokenUsageInfo = useTokenUsage();

  // Extract user messages for turn navigation
  const userMessageTurns: TurnNavigationItem[] = useMemo(() => {
    let turnNumber = 0;
    return entries
      .filter(
        (entry) =>
          entry.type === 'NORMALIZED_ENTRY' &&
          entry.content.entry_type.type === 'user_message'
      )
      .map((entry) => {
        turnNumber++;
        return {
          patchKey: entry.patchKey,
          content:
            entry.type === 'NORMALIZED_ENTRY' ? entry.content.content : '',
          turnNumber,
        };
      });
  }, [entries]);

  // Execution state
  const {
    isAttemptRunning,
    stopExecution,
    isStopping,
    processes,
    isLoading: isExecutionLoading,
  } = useWorkspaceExecution(workspaceId);

  // Approvals state
  const { getPendingForProcess } = useApprovals();

  // Get pending approval from running processes
  const pendingApproval = useMemo(() => {
    const runningProcesses = processes.filter(
      (p) => p.status === ExecutionProcessStatus.running
    );
    for (const proc of runningProcesses) {
      const info = getPendingForProcess(proc.id);
      if (info) {
        let questions: AskUserQuestionItem[] | undefined;
        for (const entry of entries) {
          if (entry.type !== 'NORMALIZED_ENTRY') continue;
          const entryType = entry.content.entry_type;
          if (
            entryType.type === 'tool_use' &&
            entryType.status.status === 'pending_approval' &&
            entryType.status.approval_id === info.approval_id &&
            entryType.action_type.action === 'ask_user_question'
          ) {
            questions = entryType.action_type.questions;
            break;
          }
        }
        return {
          approvalId: info.approval_id,
          timeoutAt: info.timeout_at,
          executionProcessId: info.execution_process_id,
          questions,
        };
      }
    }
    return null;
  }, [processes, getPendingForProcess, entries]);

  // Use approval_id as scratch key when pending approval exists to avoid
  // prefilling approval response with queued follow-up message
  const scratchId = useMemo(() => {
    if (pendingApproval?.approvalId) {
      return pendingApproval.approvalId;
    }
    return isNewSessionMode ? workspaceId : sessionId;
  }, [pendingApproval?.approvalId, isNewSessionMode, workspaceId, sessionId]);

  // Get repos for file search
  const { repos } = useWorkspaceRepo(workspaceId);
  const repoIds = repos.map((r) => r.id);
  const hasGitRepo = repos.some((r) => r.is_git);

  // Approval feedback context
  const feedbackContext = useApprovalFeedbackOptional();
  const isInFeedbackMode = !!feedbackContext?.activeApproval;

  // Message edit context
  const editContext = useMessageEditContext();
  const isInEditMode = editContext.isInEditMode;

  // Get todos from entries
  const { todos, inProgressTodo } = useTodos(entries);

  // Review comments context (optional - only available when ReviewProvider wraps this)
  const reviewContext = useReviewOptional();
  const reviewMarkdown = useMemo(
    () => reviewContext?.generateReviewMarkdown() ?? '',
    [reviewContext]
  );
  const hasReviewComments = (reviewContext?.comments.length ?? 0) > 0;

  // Approval mutation for approve/deny/answer actions
  const {
    approveAsync,
    denyAsync,
    answerAsync,
    isApproving,
    isDenying,
    isAnswering,
    denyError,
    answerError,
  } = useApprovalMutation();

  // Branch status for edit retry and conflict detection
  const { data: branchStatus } = useBranchStatus(workspaceId);

  // Derive conflict state from branch status
  const hasConflicts = useMemo(() => {
    return (
      branchStatus?.some((r) => (r.conflicted_files?.length ?? 0) > 0) ?? false
    );
  }, [branchStatus]);

  const conflictedFilesCount = useMemo(() => {
    return (
      branchStatus?.reduce(
        (sum, r) => sum + (r.conflicted_files?.length ?? 0),
        0
      ) ?? 0
    );
  }, [branchStatus]);

  // Get workspace branch for conflict resolution dialog
  const { branch: attemptBranch } = useWorkspaceBranch(workspaceId);

  // Find the first repo with conflicts (for the resolve dialog)
  const repoWithConflicts = useMemo(
    () =>
      branchStatus?.find(
        (r) => r.is_rebase_in_progress || (r.conflicted_files?.length ?? 0) > 0
      ),
    [branchStatus]
  );

  const handleResolveConflicts = useCallback(() => {
    // v1 (hide-inner-sessions): also defensive — the button that calls this
    // is hidden via `stats.onResolveConflicts: undefined` below when the
    // flag is off, but guard here too so no programmatic caller slips
    // through.
    if (!SHOW_RESOLVE_CONFLICTS) return;
    if (!workspaceId || !repoWithConflicts) return;
    ResolveConflictsDialog.show({
      workspaceId,
      conflictOp: repoWithConflicts.conflict_op ?? 'rebase',
      sourceBranch: attemptBranch,
      targetBranch: repoWithConflicts.target_branch_name,
      conflictedFiles: repoWithConflicts.conflicted_files ?? [],
      repoName: repoWithConflicts.repo_name,
    });
  }, [workspaceId, repoWithConflicts, attemptBranch]);

  // User profiles, config preference, and latest executor from processes
  const { profiles, config, capabilities } = useUserSystem();

  // Fetch processes from last session to get full profile (only in new session mode)
  const lastSessionId = isNewSessionMode ? sessions?.[0]?.id : undefined;
  const { executionProcesses: lastSessionProcesses } =
    useExecutionProcesses(lastSessionId);

  // Compute latestConfig: current processes > last session processes > session metadata
  const latestConfig = useMemo(() => {
    // Current session's processes take priority (full ExecutorConfig)
    const fromProcesses = getLatestConfigFromProcesses(processes);
    if (fromProcesses) return fromProcesses;

    // Try full config from last session's processes
    const fromLastSession = getLatestConfigFromProcesses(lastSessionProcesses);
    if (fromLastSession) return fromLastSession;

    // Fallback: just executor from session metadata
    const lastSessionExecutor = sessions?.[0]?.executor;
    if (lastSessionExecutor) {
      return {
        executor: lastSessionExecutor as BaseCodingAgent,
      };
    }

    return null;
  }, [processes, lastSessionProcesses, sessions]);

  const needsExecutorSelection =
    isNewSessionMode || (!session?.executor && !latestConfig?.executor);

  // Message editor state
  const {
    localMessage,
    setLocalMessage,
    scratchData,
    isScratchLoading,
    hasInitialValue,
    saveToScratch,
    clearDraft,
    cancelDebouncedSave,
    handleMessageChange,
  } = useSessionMessageEditor({ scratchId });

  // Ref to access current message value for attachment handler
  const localMessageRef = useRef(localMessage);
  useEffect(() => {
    localMessageRef.current = localMessage;
  }, [localMessage]);

  // Attachment handling - insert markdown when attachments are uploaded
  const handleInsertMarkdown = useCallback(
    (markdown: string) => {
      const currentMessage = localMessageRef.current;
      const newMessage = currentMessage.trim()
        ? `${currentMessage}\n\n${markdown}`
        : markdown;
      setLocalMessage(newMessage);
    },
    [setLocalMessage]
  );

  // Auto-paste component context from inspect mode
  const pendingComponentMarkdown = useInspectModeStore(
    (s) => s.pendingComponentMarkdown
  );
  const clearPendingComponentMarkdown = useInspectModeStore(
    (s) => s.clearPendingComponentMarkdown
  );

  useEffect(() => {
    if (pendingComponentMarkdown) {
      handleInsertMarkdown(pendingComponentMarkdown);
      clearPendingComponentMarkdown();
    }
  }, [
    pendingComponentMarkdown,
    handleInsertMarkdown,
    clearPendingComponentMarkdown,
  ]);

  const { uploadFiles, localAttachments, clearUploadedAttachments } =
    useSessionAttachments(workspaceId, sessionId, handleInsertMarkdown);

  // Unified executor + variant + model selector options resolution
  const {
    executorConfig,
    effectiveExecutor,
    selectedVariant,
    executorOptions,
    variantOptions,
    presetOptions,
    setExecutor: handleExecutorChange,
    setVariant: setSelectedVariant,
    setOverrides: setExecutorOverrides,
  } = useExecutorConfig({
    profiles,
    lastUsedConfig: latestConfig,
    scratchConfig: scratchData?.executor_config ?? undefined,
    configExecutorProfile: config?.executor_profile,
    onPersist: (cfg) => void saveToScratch(localMessageRef.current, cfg),
  });

  const supportsContextUsage =
    !!effectiveExecutor &&
    capabilities?.[effectiveExecutor]?.includes(
      BaseAgentCapability.CONTEXT_USAGE
    );

  // Navigate to agent settings to customise variants
  const handleCustomise = () => {
    SettingsDialog.show({ initialSection: 'agents' });
  };

  // Queue interaction
  const {
    isQueued,
    queuedMessage,
    queuedConfig,
    isQueueLoading,
    queueMessage,
    cancelQueue,
    refreshQueueStatus,
  } = useSessionQueueInteraction({ sessionId });

  // Send actions
  const {
    selectedProviderId,
    selectedModelId,
    selectedReasoningId,
    preferredEffortId,
    setSelection,
    setPreferredEffort,
  } = useWorkspacePickerSelection(workspaceId);

  // Legacy / never-picked sessions: seed from last-used (or the active agent's
  // first canonical model) so the pill matches what will actually run on send.
  const { data: providers = [] } = useProviders();
  const { config: agentModelConfig } =
    useModelSelectorConfig(effectiveExecutor);
  const agentDefaultModels = useMemo(
    () =>
      (agentModelConfig?.models ?? []).map((m) => ({
        id: m.id,
        displayName: m.name,
        ownedBy: null,
      })),
    [agentModelConfig]
  );
  useEffect(() => {
    if (selectedProviderId || selectedModelId) return;
    if (effectiveExecutor && agentDefaultModels.length === 0) return;
    const resolved = resolveDefaultSelection(providers, agentDefaultModels);
    if (!resolved) return;
    setSelection(resolved.providerId, resolved.modelId, resolved.reasoningId);
    setPreferredEffort(resolved.preferredEffortId);
  }, [
    providers,
    selectedProviderId,
    selectedModelId,
    effectiveExecutor,
    agentDefaultModels,
    setSelection,
    setPreferredEffort,
  ]);

  const {
    send,
    isSending,
    error: sendError,
    clearError,
  } = useSessionSend({
    sessionId,
    workspaceId,
    isNewSessionMode,
    onSelectSession,
    executorConfig,
    selectedProviderId,
    selectedModelId,
    selectedReasoningId,
  });

  useEffect(() => {
    if (isExecutionLoading) return;
    if (!quotaHistoryInitialized.current) {
      for (const process of processes) {
        if (process.status !== ExecutionProcessStatus.running) {
          handledQuotaProcessIds.current.add(process.id);
        }
      }
      quotaHistoryInitialized.current = true;
      return;
    }
    if (
      !sessionId ||
      !effectiveExecutor ||
      isAttemptRunning ||
      isSending ||
      isStopping
    ) {
      return;
    }

    const failedProcess = [...processes]
      .reverse()
      .find(
        (process) =>
          process.run_reason === 'codingagent' &&
          process.status === ExecutionProcessStatus.failed &&
          !handledQuotaProcessIds.current.has(process.id)
      );
    if (!failedProcess) return;

    const processEntries = entries.filter(
      (entry) => entry.executionProcessId === failedProcess.id
    );
    const diagnosticText = processEntries
      .filter(
        (entry) =>
          entry.type === 'STDERR' ||
          (entry.type === 'NORMALIZED_ENTRY' &&
            entry.content.entry_type.type === 'error_message')
      )
      .map((entry) => entry.content)
      .join('\n');
    const quotaFailure =
      /\b429\b|rate.?limit|too many requests|quota|resource_exhausted|insufficient_quota|credits? exhausted/i.test(
        diagnosticText
      );
    if (!quotaFailure) return;

    handledQuotaProcessIds.current.add(failedProcess.id);
    const failedModels =
      quotaFailuresBySession.current.get(sessionId) ?? new Set();
    if (selectedProviderId && selectedModelId) {
      failedModels.add(`${selectedProviderId}/${selectedModelId}`);
    }
    quotaFailuresBySession.current.set(sessionId, failedModels);

    const hasCredentialsForAgent = (provider: (typeof providers)[number]) => {
      if (provider.kind === 'Default') return true;
      if (provider.apiKey) return true;
      switch (effectiveExecutor) {
        case 'CLAUDE_CODE':
          return !!provider.claude.apiKey;
        case 'CODEX':
          return !!provider.codex.apiKey;
        case 'OPENCODE':
          return !!provider.opencode.apiKey;
        case 'GEMINI':
          return !!provider.gemini.apiKey;
        case 'HERMES':
          return !!provider.hermes.apiKey;
        case 'DEEPSEEK_TUI':
          return !!provider.deepseekTui.apiKey;
        default:
          return false;
      }
    };

    const fallback = (agentModelConfig?.models ?? [])
      .filter((model) => !!model.provider_id)
      .map((model) => {
        const provider = providers.find(
          (item) => item.id === model.provider_id
        );
        return provider ? { model, provider } : null;
      })
      .find(
        (candidate) =>
          !!candidate &&
          candidate.provider.enabled &&
          (candidate.provider.kind === 'Default' ||
            candidate.provider.perAgentEnabled[effectiveExecutor] === true) &&
          hasCredentialsForAgent(candidate.provider) &&
          !failedModels.has(`${candidate.provider.id}/${candidate.model.id}`) &&
          (candidate.provider.id !== selectedProviderId ||
            candidate.model.id !== selectedModelId)
      );

    if (!fallback) {
      setQuotaNotice(
        'Le fournisseur a signalé une limite. Connecte un autre modèle dans Paramètres → Fournisseurs pour reprendre.'
      );
      return;
    }

    const lastRequest = [...processEntries]
      .reverse()
      .find(
        (entry) =>
          entry.type === 'NORMALIZED_ENTRY' &&
          entry.content.entry_type.type === 'user_message'
      );
    const handoffPrompt = [
      'The previous model run stopped because its provider reported a rate or quota limit. Continue the same task in this session. The project files and completed edits are shared; inspect their current state and do not repeat completed work.',
      lastRequest?.type === 'NORMALIZED_ENTRY'
        ? `Original request: ${lastRequest.content.content}`
        : '',
      'The previous provider transcript and command diagnostics were not transferred. Review the shared project state to determine what is already complete; avoid repeating completed work.',
      'First state briefly what is already complete and what remains, then continue the task.',
    ]
      .filter(Boolean)
      .join('\n\n');
    const nextProviderId = fallback.provider.id;
    const nextModelId = fallback.model.id;
    setSelection(nextProviderId, nextModelId, null);
    setQuotaNotice(
      `Limite atteinte. Reprise avec ${fallback.provider.name} · ${fallback.model.name}… La demande initiale est transmise; les journaux restent locaux.`
    );
    void send(addResponseModeInstruction(handoffPrompt, responseMode), {
      providerId: nextProviderId,
      modelId: nextModelId,
    }).then((success) => {
      if (!success) {
        setQuotaNotice(
          `Le transfert vers ${fallback.provider.name} a échoué. Vérifie sa connexion puis relance la tâche.`
        );
      }
    });
  }, [
    sessionId,
    isExecutionLoading,
    effectiveExecutor,
    isAttemptRunning,
    isSending,
    isStopping,
    processes,
    entries,
    agentModelConfig,
    providers,
    selectedModelId,
    selectedProviderId,
    selectedModelId,
    setSelection,
    send,
    responseMode,
  ]);

  const handleSend = useCallback(async () => {
    const { prompt: basePrompt, isSlashCommand } = buildAgentPrompt(
      localMessage,
      [reviewMarkdown]
    );
    const prompt = isSlashCommand
      ? basePrompt
      : addResponseModeInstruction(basePrompt, responseMode);

    onScrollToBottom('auto');

    const success = await send(prompt);
    if (success) {
      if (selectedProviderId && selectedModelId) {
        writeLastUsed({
          providerId: selectedProviderId,
          modelId: selectedModelId,
          preferredEffortId,
        });
      }
      cancelDebouncedSave();
      setLocalMessage('');
      clearUploadedAttachments();
      await clearDraft();
      if (!isSlashCommand) {
        reviewContext?.clearComments();
      }
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          onScrollToBottom('auto');
        });
      });
    }
  }, [
    onScrollToBottom,
    send,
    localMessage,
    reviewMarkdown,
    cancelDebouncedSave,
    setLocalMessage,
    clearUploadedAttachments,
    clearDraft,
    reviewContext,
    selectedProviderId,
    selectedModelId,
    preferredEffortId,
    responseMode,
  ]);

  // Track previous process count for queue refresh
  const prevProcessCountRef = useRef(processes.length);

  // Refresh queue status when execution stops or new process starts
  useEffect(() => {
    const prevCount = prevProcessCountRef.current;
    prevProcessCountRef.current = processes.length;

    if (!workspaceId) return;

    if (!isAttemptRunning) {
      refreshQueueStatus();
      return;
    }

    if (processes.length > prevCount) {
      refreshQueueStatus();
    }
  }, [isAttemptRunning, workspaceId, processes.length, refreshQueueStatus]);

  // Queue message handler
  const handleQueueMessage = useCallback(async () => {
    // Allow queueing if there's a message OR review comments, and we have a config
    if ((!localMessage.trim() && !reviewMarkdown) || !executorConfig) return;

    const { prompt: basePrompt } = buildAgentPrompt(localMessage, [
      reviewMarkdown,
    ]);
    const prompt = addResponseModeInstruction(basePrompt, responseMode);

    // Picker is authoritative for model_id / reasoning_id — overlay before queue.
    const effectiveConfig = {
      ...executorConfig,
      // The "agent default" sentinel from ProviderModelPicker maps to null
      // so the spawn applier skips the `--model` flag.
      model_id: isAgentDefaultModelId(selectedModelId)
        ? null
        : (selectedModelId ?? executorConfig.model_id ?? null),
      reasoning_id: selectedReasoningId ?? executorConfig.reasoning_id ?? null,
    };

    cancelDebouncedSave();
    await saveToScratch(localMessage, effectiveConfig);
    await queueMessage(prompt, effectiveConfig);

    // Clear local state after queueing (same as handleSend)
    setLocalMessage('');
    clearUploadedAttachments();
    reviewContext?.clearComments();
  }, [
    localMessage,
    reviewMarkdown,
    executorConfig,
    selectedModelId,
    selectedReasoningId,
    queueMessage,
    cancelDebouncedSave,
    saveToScratch,
    setLocalMessage,
    clearUploadedAttachments,
    reviewContext,
    responseMode,
  ]);

  // Editor change handler
  const handleEditorChange = useCallback(
    (value: string) => {
      if (isQueued) cancelQueue();
      if (executorConfig) {
        handleMessageChange(value, executorConfig);
      } else {
        setLocalMessage(value);
      }
      if (sendError) clearError();
    },
    [
      isQueued,
      cancelQueue,
      handleMessageChange,
      executorConfig,
      sendError,
      clearError,
      setLocalMessage,
    ]
  );

  // Handle feedback submission
  const handleSubmitFeedback = useCallback(async () => {
    if (!feedbackContext || !localMessage.trim()) return;
    try {
      await feedbackContext.submitFeedback(localMessage);
      cancelDebouncedSave();
      setLocalMessage('');
      await clearDraft();
    } catch {
      // Error is handled in context
    }
  }, [
    feedbackContext,
    localMessage,
    cancelDebouncedSave,
    setLocalMessage,
    clearDraft,
  ]);

  // Handle cancel feedback mode
  const handleCancelFeedback = useCallback(() => {
    feedbackContext?.exitFeedbackMode();
  }, [feedbackContext]);

  // Handle cancel queue - restore message to editor
  const handleCancelQueue = useCallback(async () => {
    if (queuedMessage) {
      setLocalMessage(queuedMessage);
    }
    if (queuedConfig) {
      setExecutorOverrides(queuedConfig);
    }
    await cancelQueue();
  }, [
    queuedMessage,
    queuedConfig,
    setLocalMessage,
    setExecutorOverrides,
    cancelQueue,
  ]);

  // Message edit retry mutation
  const editRetryMutation = useMessageEditRetry(sessionId ?? '', () => {
    // On success, clear edit mode and reset editor
    editContext.cancelEdit();
    cancelDebouncedSave();
    setLocalMessage('');
  });

  const areAttachmentInputsDisabled =
    mode === 'placeholder' ||
    isQueued ||
    isSending ||
    isStopping ||
    !!feedbackContext?.isSubmitting ||
    editRetryMutation.isPending ||
    isApproving ||
    isDenying ||
    isAnswering;

  const onDrop = useCallback(
    (acceptedFiles: File[]) => {
      if (acceptedFiles.length > 0) {
        uploadFiles(acceptedFiles);
      }
    },
    [uploadFiles]
  );

  const { getRootProps, getInputProps, isDragActive } = useDropzone({
    onDrop,
    disabled: areAttachmentInputsDisabled,
    noClick: true,
    noKeyboard: true,
  });

  // Handle edit submission
  const handleSubmitEdit = useCallback(async () => {
    if (!editContext.activeEdit || !localMessage.trim() || !executorConfig)
      return;
    const effectiveConfig = {
      ...executorConfig,
      // The "agent default" sentinel from ProviderModelPicker maps to null
      // so the spawn applier skips the `--model` flag.
      model_id: isAgentDefaultModelId(selectedModelId)
        ? null
        : (selectedModelId ?? executorConfig.model_id ?? null),
      reasoning_id: selectedReasoningId ?? executorConfig.reasoning_id ?? null,
    };
    editRetryMutation.mutate({
      message: localMessage,
      executorConfig: effectiveConfig,
      executionProcessId: editContext.activeEdit.processId,
      branchStatus,
      processes,
    });
  }, [
    editContext.activeEdit,
    localMessage,
    executorConfig,
    selectedModelId,
    selectedReasoningId,
    branchStatus,
    processes,
    editRetryMutation,
  ]);

  // Handle cancel edit mode
  const handleCancelEdit = useCallback(() => {
    editContext.cancelEdit();
    setLocalMessage('');
  }, [editContext, setLocalMessage]);

  // Populate editor with original message when entering edit mode
  const prevEditRef = useRef(editContext.activeEdit);
  useEffect(() => {
    if (editContext.activeEdit && !prevEditRef.current) {
      // Just entered edit mode - populate with original message
      setLocalMessage(editContext.activeEdit.originalMessage);
    }
    prevEditRef.current = editContext.activeEdit;
  }, [editContext.activeEdit, setLocalMessage]);

  // Handle inserting PR comments into the message editor
  const handleInsertPrComments = useCallback(async () => {
    if (!workspaceId) return;
    const repoId = repos[0]?.id;
    if (!repoId) return;

    const result = await PrCommentsDialog.show({
      workspaceId: workspaceId,
      repoId,
    });
    if (result.comments.length > 0) {
      const markdownBlocks = result.comments.map((comment) => {
        const payload: NormalizedComment = {
          id:
            comment.comment_type === 'general'
              ? comment.id
              : comment.id.toString(),
          comment_type: comment.comment_type,
          author: comment.author,
          body: comment.body,
          created_at: comment.created_at,
          url: comment.url,
          ...(comment.comment_type === 'review' && {
            path: comment.path,
            line: comment.line != null ? Number(comment.line) : null,
            diff_hunk: comment.diff_hunk,
          }),
        };
        return '```gh-comment\n' + JSON.stringify(payload, null, 2) + '\n```';
      });
      handleInsertMarkdown(markdownBlocks.join('\n\n'));
    }
  }, [workspaceId, repos, handleInsertMarkdown]);

  // Toolbar actions handler
  const handleToolbarAction = useCallback(
    (action: ActionDefinition) => {
      if (action.requiresTarget && workspaceId) {
        executeAction(action, workspaceId);
      } else {
        executeAction(action);
      }
    },
    [executeAction, workspaceId]
  );

  // Define which actions appear in the toolbar
  const toolbarActionsList = useMemo(
    () =>
      [Actions.StartReview].filter((action) =>
        isActionVisible(action, actionCtx)
      ),
    [actionCtx]
  );

  const toolbarActionItems = useMemo(
    () =>
      toolbarActionsList.flatMap((action) => {
        if (isSpecialIcon(action.icon)) {
          return [];
        }

        const label = action.label;

        return [
          {
            id: action.id,
            icon: action.icon,
            label: label(),
            tooltip: getActionTooltip(action, actionCtx),
            disabled: !isActionEnabled(action, actionCtx),
            onClick: () => handleToolbarAction(action),
          },
        ];
      }),
    [toolbarActionsList, actionCtx, handleToolbarAction]
  );

  // Handle approve action
  const handleApprove = useCallback(async () => {
    if (!pendingApproval) return;

    // Exit feedback mode if active
    feedbackContext?.exitFeedbackMode();

    try {
      await approveAsync({
        approvalId: pendingApproval.approvalId,
        executionProcessId: pendingApproval.executionProcessId,
      });

      // Invalidate workspace summary cache to update sidebar
      queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
      onScrollToBottom();
    } catch {
      // Error is handled by mutation
    }
  }, [
    pendingApproval,
    feedbackContext,
    approveAsync,
    queryClient,
    onScrollToBottom,
  ]);

  // Handle request changes (deny with feedback)
  const handleRequestChanges = useCallback(async () => {
    if (!pendingApproval || !localMessage.trim()) return;

    try {
      await denyAsync({
        approvalId: pendingApproval.approvalId,
        executionProcessId: pendingApproval.executionProcessId,
        reason: localMessage.trim(),
      });
      cancelDebouncedSave();
      setLocalMessage('');
      await clearDraft();

      // Invalidate workspace summary cache to update sidebar
      queryClient.invalidateQueries({ queryKey: workspaceSummaryKeys.all });
      onScrollToBottom();
    } catch {
      // Error is handled by mutation
    }
  }, [
    pendingApproval,
    localMessage,
    denyAsync,
    cancelDebouncedSave,
    setLocalMessage,
    clearDraft,
    queryClient,
    onScrollToBottom,
  ]);

  // Handle AskUserQuestion answer submission
  const handleAnswerQuestion = useCallback(
    async (answers: Array<{ question: string; answer: string[] }>) => {
      if (!pendingApproval) return;

      try {
        await answerAsync({
          approvalId: pendingApproval.approvalId,
          executionProcessId: pendingApproval.executionProcessId,
          answers,
        });
        queryClient.invalidateQueries({
          queryKey: workspaceSummaryKeys.all,
        });
        onScrollToBottom();
      } catch {
        // Error is handled by mutation
      }
    },
    [pendingApproval, answerAsync, queryClient, onScrollToBottom]
  );

  // Check if approval is timed out
  const isApprovalTimedOut = pendingApproval
    ? new Date() > new Date(pendingApproval.timeoutAt)
    : false;

  const status = computeExecutionStatus({
    isInFeedbackMode,
    isInEditMode,
    isStopping,
    isQueueLoading,
    isSendingFollowUp: isSending,
    isQueued,
    isAttemptRunning,
  });

  // During loading, render with empty editor to preserve container UI
  // In approval mode, don't show queued message - it's for follow-up, not approval response
  const editorValue = useMemo(() => {
    if (isScratchLoading || !hasInitialValue) return '';
    if (pendingApproval) return localMessage;
    return queuedMessage ?? localMessage;
  }, [
    isScratchLoading,
    hasInitialValue,
    pendingApproval,
    queuedMessage,
    localMessage,
  ]);

  const renderEditor = useCallback(
    ({
      focusKey,
      placeholder,
      value,
      onChange,
      onCmdEnter,
      disabled,
      repoIds,
      executor,
      onPasteFiles,
      localAttachments,
    }: SessionChatBoxEditorRenderProps<BaseCodingAgent>) => (
      <div className="flex min-w-0 items-end gap-2">
        <div className="min-w-0 flex-1">
          <WYSIWYGEditor
            key={focusKey}
            placeholder={placeholder}
            value={value}
            onChange={onChange}
            onCmdEnter={onCmdEnter}
            disabled={disabled}
            className="min-h-double max-h-[50vh] overflow-y-auto"
            repoIds={repoIds}
            executor={executor}
            sessionId={sessionId}
            autoFocus
            onPasteFiles={onPasteFiles}
            localAttachments={localAttachments}
            sendShortcut={config?.send_message_shortcut}
          />
        </div>
        {!disabled && onChange && (
          <VoiceInputButton value={value} onChange={onChange} />
        )}
      </div>
    ),
    [config?.send_message_shortcut, sessionId]
  );

  const modelSelectorNode = effectiveExecutor ? (
    <div className="flex items-center gap-2">
      {quotaNotice && (
        <span
          role="status"
          className="max-w-64 truncate text-xs text-amber-600 dark:text-amber-400"
          title={quotaNotice}
        >
          {quotaNotice}
        </span>
      )}
      <div
        className="flex items-center rounded-md border p-0.5"
        role="group"
        aria-label={t('composer.responseMode.label')}
      >
        {(['call', 'chat'] as const).map((modeOption) => (
          <button
            key={modeOption}
            type="button"
            aria-pressed={responseMode === modeOption}
            title={t(`composer.responseMode.${modeOption}Description`)}
            aria-label={t(`composer.responseMode.${modeOption}Label`)}
            onClick={() => setResponseMode(modeOption)}
            className={`rounded px-2.5 py-1.5 text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand ${responseMode === modeOption ? 'bg-accent font-medium text-foreground' : 'text-muted-foreground hover:bg-secondary hover:text-foreground'}`}
          >
            {t(`composer.responseMode.${modeOption}`)}
          </button>
        ))}
      </div>
      <ProviderModelPicker
        selectedProviderId={selectedProviderId}
        selectedModelId={selectedModelId}
        selectedReasoningId={selectedReasoningId}
        preferredEffortId={preferredEffortId}
        activeAgent={effectiveExecutor}
        onManageProviders={() =>
          SettingsDialog.show({ initialSection: 'providers' })
        }
        onSelectionChange={(providerId, modelId, reasoningId) =>
          setSelection(providerId, modelId, reasoningId)
        }
        onPreferredEffortChange={setPreferredEffort}
      />
    </div>
  ) : undefined;

  const modelSelectorLeftNode = effectiveExecutor ? (
    <ModelSelectorContainer
      slot="left"
      agent={effectiveExecutor}
      workspaceId={workspaceId}
      sessionId={sessionId}
      onAdvancedSettings={handleCustomise}
      presets={variantOptions}
      selectedPreset={selectedVariant}
      onPresetSelect={setSelectedVariant}
      onOverrideChange={setExecutorOverrides}
      executorConfig={executorConfig}
      presetOptions={presetOptions}
    />
  ) : undefined;

  // In placeholder mode, render a disabled version to maintain visual structure
  if (mode === 'placeholder') {
    return (
      <SessionChatBox<BaseCodingAgent>
        status="idle"
        showSessionSwitcher={SHOW_INNER_SESSION_SWITCHER}
        renderEditor={renderEditor}
        repoIds={repoIds}
        tokenUsageInfo={tokenUsageInfo}
        supportsContextUsage={false}
        formatExecutorLabel={toPrettyCase}
        formatSessionDate={(createdAt) =>
          formatDateShortWithTime(
            createdAt instanceof Date ? createdAt.toISOString() : createdAt
          )
        }
        renderAgentIcon={(executor, className) => (
          <AgentIcon
            agent={executor as BaseCodingAgent | null | undefined}
            className={className}
          />
        )}
        editor={{
          value: '',
          onChange: () => {},
        }}
        actions={{
          onSend: () => {},
          onQueue: () => {},
          onCancelQueue: () => {},
          onStop: () => {},
          onPasteFiles: () => {},
        }}
        session={{
          sessions: [],
          selectedSessionId: undefined,
          onSelectSession: () => {},
          isNewSessionMode: false,
          onNewSession: undefined,
        }}
        stats={{
          filesChanged: 0,
          linesAdded: 0,
          linesRemoved: 0,
        }}
        onViewCode={disableViewCode ? undefined : handleViewCode}
      />
    );
  }

  return (
    <SessionChatBox<BaseCodingAgent>
      status={status}
      showSessionSwitcher={SHOW_INNER_SESSION_SWITCHER}
      onViewCode={disableViewCode ? undefined : handleViewCode}
      onOpenWorkspace={
        showOpenWorkspaceButton && workspaceId ? handleOpenWorkspace : undefined
      }
      onScrollToPreviousMessage={onScrollToPreviousMessage}
      userMessageTurns={userMessageTurns}
      onScrollToUserMessage={onScrollToUserMessage}
      getActiveTurnPatchKey={getActiveTurnPatchKey}
      renderEditor={renderEditor}
      repoIds={repoIds}
      tokenUsageInfo={tokenUsageInfo}
      supportsContextUsage={supportsContextUsage}
      formatExecutorLabel={toPrettyCase}
      formatSessionDate={(createdAt) =>
        formatDateShortWithTime(
          createdAt instanceof Date ? createdAt.toISOString() : createdAt
        )
      }
      renderAgentIcon={(executor, className) => (
        <AgentIcon
          agent={executor as BaseCodingAgent | null | undefined}
          className={className}
        />
      )}
      editor={{
        value: editorValue,
        onChange: handleEditorChange,
      }}
      actions={{
        onSend: handleSend,
        onQueue: handleQueueMessage,
        onCancelQueue: handleCancelQueue,
        onStop: stopExecution,
        onPasteFiles: uploadFiles,
      }}
      session={{
        sessions,
        selectedSessionId: sessionId,
        onSelectSession: onSelectSession ?? (() => {}),
        isNewSessionMode: needsExecutorSelection,
        onNewSession: onStartNewSession,
        onRenameSession: handleRenameSession,
      }}
      toolbarActions={{
        items: toolbarActionItems,
      }}
      onPrCommentClick={
        actionCtx.hasOpenPR ? handleInsertPrComments : undefined
      }
      stats={{
        filesChanged: hasGitRepo ? filesChanged : undefined,
        linesAdded: hasGitRepo ? linesAdded : undefined,
        linesRemoved: hasGitRepo ? linesRemoved : undefined,
        hasConflicts,
        conflictedFilesCount,
        onResolveConflicts: SHOW_RESOLVE_CONFLICTS
          ? handleResolveConflicts
          : undefined,
      }}
      error={sendError}
      agent={effectiveExecutor}
      todos={todos}
      inProgressTodo={inProgressTodo}
      executor={
        needsExecutorSelection
          ? {
              selected: effectiveExecutor,
              options: executorOptions,
              onChange: handleExecutorChange,
            }
          : undefined
      }
      feedbackMode={
        feedbackContext
          ? {
              isActive: isInFeedbackMode,
              onSubmitFeedback: handleSubmitFeedback,
              onCancel: handleCancelFeedback,
              isSubmitting: feedbackContext.isSubmitting,
              error: feedbackContext.error,
              isTimedOut: feedbackContext.isTimedOut,
            }
          : undefined
      }
      approvalMode={
        pendingApproval && !pendingApproval.questions
          ? {
              isActive: true,
              onApprove: handleApprove,
              onRequestChanges: handleRequestChanges,
              isSubmitting: isApproving || isDenying,
              isTimedOut: isApprovalTimedOut,
              error: denyError?.message ?? null,
            }
          : undefined
      }
      askQuestionMode={
        pendingApproval?.questions
          ? {
              isActive: true,
              questions: pendingApproval.questions,
              onSubmitAnswers: handleAnswerQuestion,
              isSubmitting: isAnswering,
              isTimedOut: isApprovalTimedOut,
              error: answerError?.message ?? null,
            }
          : undefined
      }
      editMode={{
        isActive: isInEditMode,
        onSubmitEdit: handleSubmitEdit,
        onCancel: handleCancelEdit,
        isSubmitting: editRetryMutation.isPending,
      }}
      reviewComments={
        hasReviewComments && reviewContext
          ? {
              count: reviewContext.comments.length,
              previewMarkdown: reviewMarkdown,
              onClear: reviewContext.clearComments,
            }
          : undefined
      }
      localAttachments={localAttachments}
      dropzone={{ getRootProps, getInputProps, isDragActive }}
      modelSelector={modelSelectorNode}
      modelSelectorLeft={modelSelectorLeftNode}
      teamPillRow={
        workspaceId ? (
          <TeamPillRowContainer
            workspaceId={workspaceId}
            currentSessionId={sessionId}
            onSelectSession={onSelectSession ?? (() => {})}
            lastUsedConfig={latestConfig ?? null}
            configExecutorProfile={config?.executor_profile ?? null}
            visible={!needsExecutorSelection}
          />
        ) : undefined
      }
    />
  );
}
