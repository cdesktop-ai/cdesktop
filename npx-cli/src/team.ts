/*
 * `cdesktop team` subcommand — coordinate with teammates over HTTP.
 *
 * Talks to the locally-running cdesktop backend at the port written to
 * `${TMPDIR}/cdesktop/cdesktop.port` (same mechanism as `cdesktop-mcp`).
 * Caller session is identified by `CDESKTOP_SESSION_ID`, injected by the
 * server at executor spawn (see `crates/local-deployment/src/container.rs`).
 *
 * Implements four subcommands per the MVP plan:
 *   spawn   — lead-only; create a teammate session
 *   send    — message a peer in the same workspace
 *   manager: message the lead session without looking up its id
 *   list    — show team roster (id, name, executor, created_at)
 *
 * `transcript` was descoped — see `plans/agent-teams-mvp.md` v1.5
 * candidates. Peers coordinate by sending request/response messages; the
 * UI is the place to read another peer's full transcript.
 */

import fs from "fs";
import os from "os";
import path from "path";

// Loosely-typed JSON value used as the upper bound on response payloads.
// Routes are pinned to concrete shapes at the call site.
type Jsonish = unknown;

const PORT_FILENAME = "cdesktop.port";
const PORT_DIR_NAME = "cdesktop";

function tmpDir(): string {
  // Match `std::env::temp_dir()` semantics on Rust side (port_file.rs).
  return process.env.TMPDIR || process.env.TEMP || process.env.TMP || os.tmpdir();
}

function readPort(): number {
  const portPath = path.join(tmpDir(), PORT_DIR_NAME, PORT_FILENAME);
  let raw: string;
  try {
    raw = fs.readFileSync(portPath, "utf8");
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    fatal(
      `Cannot read cdesktop port file at ${portPath}: ${msg}\n` +
        `Is the cdesktop backend running? Start it with \`npx cdesktop\`.`,
    );
  }
  // The file is either a JSON `{ "main_port": <u16>, ... }` blob written by
  // newer servers, or a bare integer line written by the legacy code path.
  // See `crates/utils/src/port_file.rs::read_port_info`.
  const trimmed = raw.trim();
  try {
    const parsed = JSON.parse(trimmed) as { main_port?: number };
    if (parsed && typeof parsed.main_port === "number") return parsed.main_port;
  } catch {
    /* fall through to bare-integer parse */
  }
  const port = Number.parseInt(trimmed, 10);
  if (!Number.isFinite(port) || port <= 0 || port > 65535) {
    fatal(`Invalid port-file contents: '${trimmed}'`);
  }
  return port;
}

function callerSessionId(): string {
  const id = process.env.CDESKTOP_SESSION_ID;
  if (!id || id.length === 0) {
    fatal(
      "CDESKTOP_SESSION_ID is not set. This command must be run from a " +
        "cdesktop-spawned executor session — the env var is injected by the " +
        "backend at executor spawn. If you are testing outside cdesktop, set " +
        "the env var manually.",
    );
  }
  return id;
}

function fatal(msg: string): never {
  process.stderr.write(`error: ${msg}\n`);
  process.exit(1);
}

function readPromptArg(prompt: string | undefined, promptFile: string | undefined): string | undefined {
  if (prompt === undefined && promptFile === undefined) return undefined;
  if (prompt !== undefined && promptFile !== undefined) {
    fatal("--prompt and --prompt-file are mutually exclusive");
  }
  if (prompt === "-") return readStdin();
  if (promptFile === "-") return readStdin();
  if (promptFile) {
    try {
      return fs.readFileSync(promptFile, "utf8");
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      fatal(`Cannot read prompt file ${promptFile}: ${msg}`);
    }
  }
  return prompt;
}

function readMessageArg(message: string | undefined, messageFile: string | undefined): string {
  if (message === undefined && messageFile === undefined) {
    fatal("--message or --message-file is required for `send`");
  }
  if (message !== undefined && messageFile !== undefined) {
    fatal("--message and --message-file are mutually exclusive");
  }
  if (message === "-" || messageFile === "-") return readStdin();
  if (messageFile) {
    try {
      return fs.readFileSync(messageFile, "utf8");
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      fatal(`Cannot read message file ${messageFile}: ${msg}`);
    }
  }
  return message as string;
}

function readStdin(): string {
  try {
    return fs.readFileSync(0, "utf8");
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    fatal(`Cannot read from stdin: ${msg}`);
  }
}

async function apiPost<TReq, TRes extends Jsonish>(
  pathSegment: string,
  body: TReq,
  extraHeaders?: Record<string, string>,
): Promise<TRes> {
  const port = readPort();
  const url = `http://127.0.0.1:${port}/api${pathSegment}`;
  let res: Response;
  try {
    res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...(extraHeaders ?? {}) },
      body: JSON.stringify(body),
    });
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    fatal(`HTTP request to ${url} failed: ${msg}`);
  }
  return await unwrap<TRes>(res, url);
}

async function apiGet<TRes extends Jsonish>(pathSegment: string): Promise<TRes> {
  const port = readPort();
  const url = `http://127.0.0.1:${port}/api${pathSegment}`;
  let res: Response;
  try {
    res = await fetch(url);
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    fatal(`HTTP request to ${url} failed: ${msg}`);
  }
  return await unwrap<TRes>(res, url);
}

type ApiEnvelope<T> = { success?: boolean; data?: T; message?: string };

async function unwrap<T extends Jsonish>(res: Response, url: string): Promise<T> {
  const text = await res.text();
  let body: ApiEnvelope<T> | null = null;
  try {
    body = text ? (JSON.parse(text) as ApiEnvelope<T>) : null;
  } catch {
    /* non-JSON response */
  }
  if (!res.ok) {
    const message = body?.message ?? text ?? `HTTP ${res.status}`;
    fatal(`Request failed (${res.status}) at ${url}: ${message}`);
  }
  if (body && body.success === false) {
    fatal(`Backend rejected request at ${url}: ${body.message ?? "unknown error"}`);
  }
  return (body?.data ?? body) as T;
}

interface SpawnFlags {
  name?: string;
  prompt?: string;
  promptFile?: string;
  executor?: string;
  variant?: string;
  model?: string;
  reasoning?: string;
  provider?: string;
  json?: boolean;
}

async function cmdSpawn(flags: SpawnFlags): Promise<void> {
  if (!flags.name || flags.name.trim().length === 0) {
    fatal("--name is required");
  }
  const prompt = readPromptArg(flags.prompt, flags.promptFile);

  // Only attach an `executor_config` when at least one identity/override
  // flag was passed. Otherwise the server inherits the lead's config.
  const overrides: Record<string, unknown> = {};
  if (flags.executor) overrides.executor = flags.executor;
  if (flags.variant) overrides.variant = flags.variant;
  if (flags.model) overrides.model_id = flags.model;
  if (flags.reasoning) overrides.reasoning_id = flags.reasoning;

  const body: Record<string, unknown> = { name: flags.name };
  if (prompt !== undefined) body.prompt = prompt;
  if (Object.keys(overrides).length > 0) body.executor_config = overrides;
  if (flags.provider) body.selected_provider_id = flags.provider;

  const caller = callerSessionId();
  const data = await apiPost<typeof body, { session_id: string }>(
    `/sessions/${encodeURIComponent(caller)}/teammates`,
    body,
  );

  if (flags.json) {
    process.stdout.write(`${JSON.stringify(data)}\n`);
  } else {
    process.stdout.write(`spawned ${flags.name} (session ${data.session_id})\n`);
  }
}

interface SendFlags {
  message?: string;
  messageFile?: string;
  json?: boolean;
}

interface ListedExecutionProcess {
  id: string;
  status: string;
  run_reason: string;
}

interface PendingApproval {
  execution_process_id: string;
}

async function interruptTarget(target: string): Promise<string[]> {
  const path = `/execution-processes?session_id=${encodeURIComponent(target)}`;
  let processes = await apiGet<ListedExecutionProcess[]>(path);
  const running = processes.filter(
    (process) => process.status === "running" && process.run_reason !== "devserver",
  );
  const pending = await apiGet<PendingApproval[]>("/approvals");
  const targetProcessIds = new Set(processes.map((process) => process.id));
  if (pending.some((approval) => targetProcessIds.has(approval.execution_process_id))) {
    fatal(
      `target session ${target} has a pending approval or question; resolve it before steering`,
    );
  }
  for (const process of running) {
    await apiPost(`/execution-processes/${encodeURIComponent(process.id)}/stop`, {});
  }
  if (running.length === 0) return [];

  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    processes = await apiGet<ListedExecutionProcess[]>(path);
    if (
      !processes.some(
        (process) => process.status === "running" && process.run_reason !== "devserver",
      )
    ) {
      return running.map((process) => process.id);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  fatal(`target session ${target} did not stop within 30 seconds`);
}

async function cmdSend(target: string | undefined, flags: SendFlags): Promise<void> {
  if (!target) fatal("usage: cdesktop team send <session-id> --message-file <path>");
  const prompt = readMessageArg(flags.message, flags.messageFile);
  const caller = callerSessionId();
  if (target === caller) fatal("a session cannot steer itself");
  const interrupted = await interruptTarget(target);
  // Omit executor_config + selected_provider_id so the server inherits the
  // recipient's last config (see `latest_executor_config_for_session`).
  const body = { prompt };
  // The from-session header is informational — the server emits a
  // `team_message_sent` telemetry event when it sees the header. UI
  // follow-ups omit it (UI sends are not team-coordination events).
  const data = await apiPost<typeof body, { id: string }>(
    `/sessions/${encodeURIComponent(target)}/follow-up`,
    body,
    { "x-cdesktop-from-session": caller },
  );
  if (flags.json) {
    process.stdout.write(`${JSON.stringify({ data, interrupted })}\n`);
  } else {
    const detail = interrupted.length > 0 ? ` after interrupting ${interrupted.length} turn(s)` : "";
    process.stdout.write(`steered ${target}${detail}\n`);
  }
}

async function cmdManager(flags: SendFlags): Promise<void> {
  const caller = callerSessionId();
  const me = await apiGet<ListedSession>(`/sessions/${encodeURIComponent(caller)}`);
  const peers = await apiGet<ListedSession[]>(
    `/sessions?workspace_id=${encodeURIComponent(me.workspace_id)}`,
  );
  const manager = [...peers].sort((a, b) => a.created_at.localeCompare(b.created_at))[0];
  if (!manager) fatal("current workspace has no lead session");
  if (manager.id === caller) {
    fatal("current session is already the workspace lead; there is no parent manager to contact");
  }
  return cmdSend(manager.id, flags);
}

interface ListFlags {
  json?: boolean;
}

interface ListedSession {
  id: string;
  workspace_id: string;
  name: string | null;
  executor: string | null;
  created_at: string;
}

async function cmdList(flags: ListFlags): Promise<void> {
  const caller = callerSessionId();
  const me = await apiGet<ListedSession>(`/sessions/${encodeURIComponent(caller)}`);
  const peers = await apiGet<ListedSession[]>(
    `/sessions?workspace_id=${encodeURIComponent(me.workspace_id)}`,
  );

  // Sort by created_at ASC so the lead (oldest) is row 0.
  const sorted = [...peers].sort((a, b) => a.created_at.localeCompare(b.created_at));

  if (flags.json) {
    const enriched = sorted.map((s, idx) => ({
      ...s,
      role: idx === 0 ? "lead" : "teammate",
      is_self: s.id === caller,
    }));
    process.stdout.write(`${JSON.stringify(enriched, null, 2)}\n`);
    return;
  }

  // Plain-text table: ROLE  NAME  EXECUTOR  ID  (* marks self)
  const rows: string[] = [];
  rows.push(["ROLE", "NAME", "EXECUTOR", "ID"].join("\t"));
  for (let i = 0; i < sorted.length; i++) {
    const s = sorted[i];
    const role = i === 0 ? "lead" : "teammate";
    const name = s.id === caller ? `${s.name ?? "(unnamed)"} *` : s.name ?? "(unnamed)";
    rows.push([role, name, s.executor ?? "-", s.id].join("\t"));
  }
  process.stdout.write(rows.join("\n") + "\n");
}

function printTeamHelp(): void {
  process.stdout.write(
    [
      "cdesktop team — coordinate with teammates in the same workspace.",
      "",
      "Usage:",
      "  cdesktop team spawn --name <name> [--prompt <text> | --prompt-file <path>] \\",
      "                     [--executor <CLAUDE_CODE|CODEX|...>] [--variant <name>] \\",
      "                     [--model <id>] [--reasoning <low|medium|high|max>] \\",
      "                     [--provider <uuid>] [--json]",
      "      Spawn a teammate (lead-only). Inherits caller's executor + provider",
      "      when overrides are omitted. Cross-executor spawn requires --model and",
      "      --provider.",
      "",
      "  cdesktop team send <session-id> --message <text> | --message-file <path> [--json]",
      "      Interrupt only that peer's running turn, then send an immediate follow-up.",
      "      Does NOT swap the recipient's",
      "      model or provider — they keep their own config.",
      "",
      "  cdesktop team manager --message <text> | --message-file <path> [--json]",
      "      Immediately steer the workspace lead with a question, blocker,",
      "      status update, or completion notice without looking up its id.",
      "",
      "  cdesktop team list [--json]",
      "      List the current workspace's team roster. Oldest session = lead.",
      "      Your own session is marked with `*`.",
      "",
      "Env:",
      "  CDESKTOP_SESSION_ID   Caller session id. Injected by cdesktop at",
      "                        executor spawn; required for every subcommand.",
      "",
      "Port discovery:",
      "  Reads ${TMPDIR}/cdesktop/cdesktop.port written by the running backend.",
    ].join("\n") + "\n",
  );
}

export async function runTeam(args: string[]): Promise<void> {
  const sub = args[0];
  if (!sub || sub === "-h" || sub === "--help" || sub === "help") {
    printTeamHelp();
    return;
  }
  const rest = args.slice(1);

  // Minimal hand-rolled flag parser. `cac` is available as a dependency but
  // adding a nested instance for three subcommands would inflate the bundle;
  // the surface is small enough that direct parsing is clearer.
  const flags = parseFlags(rest);

  switch (sub) {
    case "spawn":
      return cmdSpawn(flags);
    case "send": {
      const positional = flags._.shift();
      return cmdSend(positional, flags);
    }
    case "manager":
      return cmdManager(flags);
    case "list":
      return cmdList(flags);
    default:
      fatal(`unknown subcommand '${sub}'. Try \`cdesktop team --help\`.`);
  }
}

interface ParsedFlags {
  _: string[];
  name?: string;
  prompt?: string;
  promptFile?: string;
  message?: string;
  messageFile?: string;
  executor?: string;
  variant?: string;
  model?: string;
  reasoning?: string;
  provider?: string;
  json?: boolean;
}

const FLAG_NAMES: Array<[string, keyof ParsedFlags]> = [
  ["--name", "name"],
  ["--prompt", "prompt"],
  ["--prompt-file", "promptFile"],
  ["--message", "message"],
  ["--message-file", "messageFile"],
  ["--executor", "executor"],
  ["--variant", "variant"],
  ["--model", "model"],
  ["--reasoning", "reasoning"],
  ["--provider", "provider"],
];

function parseFlags(args: string[]): ParsedFlags {
  const out: ParsedFlags = { _: [] };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === "--json") {
      out.json = true;
      continue;
    }
    const eqIdx = arg.indexOf("=");
    const head = eqIdx >= 0 ? arg.slice(0, eqIdx) : arg;
    const inlineValue = eqIdx >= 0 ? arg.slice(eqIdx + 1) : undefined;
    const match = FLAG_NAMES.find(([flag]) => flag === head);
    if (match) {
      const [, key] = match;
      const value = inlineValue !== undefined ? inlineValue : args[++i];
      if (value === undefined) fatal(`${head} requires a value`);
      (out as unknown as Record<string, unknown>)[key] = value;
      continue;
    }
    if (arg.startsWith("--")) fatal(`unknown flag '${arg}'`);
    out._.push(arg);
  }
  return out;
}
