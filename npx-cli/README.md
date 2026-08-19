![](https://github.com/cdesktop-ai/cdesktop/raw/main/packages/public/cdesktop-hero.png)

<h1 align="center">cdesktop</h1>

<p align="center">An open-source alternative to Claude Code Desktop.</p>

<p align="center">
  <strong>English</strong> | <a href="https://github.com/cdesktop-ai/cdesktop/blob/main/README.zh-Hans.md">简体中文</a>
</p>
<!-- <p align="center">
  <a href="https://www.npmjs.com/package/cdesktop"><img alt="npm" src="https://img.shields.io/npm/v/cdesktop?style=flat-square" /></a>
  <a href="https://github.com/cdesktop-ai/cdesktop/blob/main/.github/workflows/publish.yml"><img alt="Build status" src="https://img.shields.io/github/actions/workflow/status/cdesktop-ai/cdesktop/.github%2Fworkflows%2Fpublish.yml" /></a>
  <a href="https://deepwiki.com/cdesktop-ai/cdesktop"><img src="https://deepwiki.com/badge.svg" alt="Ask DeepWiki"></a>
</p> -->

<p align="center">
  <video src="https://github.com/user-attachments/assets/d57bb67f-185b-4e19-b386-64406578c8df" controls></video>
</p>

## SightMesh fork

This fork keeps cdesktop as the visible local UI while adding the control surfaces required by [SightMesh](https://github.com/clarkipeng/sightmesh):

- a native snapshot API for pending plan and tool approvals;
- approval responses bound to the exact execution process so a stale or mismatched response cannot consume a request;
- Codex app-server and event protocol types updated to 0.147;
- current Claude and Codex model shortcuts, Codex `max` reasoning, and exact free-form model IDs;
- a native `cdesktop team manager` command for teammate questions, blockers, status, and completion messages;
- per-session immediate steering that leaves peer sessions and dev servers running;
- one bounded normalized-log snapshot API for compact fleet inspection;
- one-form submission for every question in a multi-question request;
- the upstream websocket approval stream and ordinary cdesktop UI remain compatible.

Build and install this fork locally:

```bash
git clone https://github.com/clarkipeng/cdesktop.git
cd cdesktop
pnpm install --frozen-lockfile
pnpm run build:npx
npm install --global ./npx-cli
```

The CLI package and backend `/api/info` version include a `-sightmesh` suffix so both the installed command and active runtime can be distinguished from upstream. Local release builds explicitly disable the Sentry upload plugin. The fork does not copy or rotate Claude or Codex credentials. It launches the locally authenticated agent CLIs in the same way as upstream.

## Sponsors

Want your logo featured here? [Get in touch.](mailto:onlylakehouse@163.com)

## Overview

cdesktop is an open-source alternative to Anthropic's [Claude Code Desktop](https://code.claude.com/docs/en/desktop-quickstart). It's a web UI for 5 coding agents — Claude Code, Codex, Gemini CLI, OpenCode, and Hermes — wrapping each CLI as a child process on your machine, with your code, transcripts, and worktrees all kept on disk.

The layout is modeled after the Code tab of Anthropic's official desktop app: a sessions sidebar, a transcript with an integrated terminal and diff viewer, and a right-side pane for plan, files, and app preview. Unlike the official app, cdesktop is local-only, agent-agnostic, and provider-agnostic — pick an agent, pick a provider from the built-in catalog or wire up your own.

**As of May 7, 2026, Anthropic's Claude Code Desktop no longer accepts third-party model names.** cdesktop fully supports third-party providers and models.

- **5 coding agents in one UI** — Claude Code, Codex, Gemini, OpenCode, Hermes; pick per session, transcripts stay separate per agent
- **Plug in any provider in one click** — 20+ built-in presets (OpenRouter, AWS Bedrock, DeepSeek, Kimi, ModelScope, MiniMax, Nvidia, …) or add a custom `ANTHROPIC_BASE_URL` endpoint; switch providers and tune reasoning effort per session
- **Agent teams** — spawn teammates that share your workspace and divide work; mix agents and models per teammate; lead delegates via `npx cdesktop team spawn`
- **Run sessions side by side** — split the workspace into up to 4 cells and drag any session into a new cell
- **Switch between sessions instantly** — no reload between threads; transcripts stay where you left them
- **Routines** — schedule recurring agent runs (hourly, daily, weekdays, weekly) or save manual-fire templates; each run spawns its own workspace you can open and review
- **Optional Git worktrees** — opt in per project for an isolated branch per session, or work directly in the folder; non-Git directories work too
- **Review diffs and leave inline comments** — send feedback directly to the agent without leaving the UI
- **Preview your app** — built-in browser with devtools, inspect mode, and device emulation
- **Create pull requests and merge** — open PRs with AI-generated descriptions, review on GitHub, and merge
- **Speaks your language** — built-in support for English, Simplified/Traditional Chinese, Spanish, French, Japanese, and Korean
- **Works on your phone** — fully responsive UI; check progress, review diffs, and send follow-ups from any device
- **Runs in your browser today** — start with `npx cdesktop` and open it in any modern browser; a Tauri desktop build is wired up but not yet shipped

> **Beta software.** Expect bugs and rough edges. Please [file issues](https://github.com/cdesktop-ai/cdesktop/issues) when you hit them.

## Installation

```bash
npx cdesktop
```

## Release asset contract

Each published CLI package is pinned to one GitHub release tag and one binary
manifest SHA-256. Its bundled downloader fetches `manifest.json` from that
exact release asset directory, verifies the manifest bytes first, and then
downloads flat binary assets named:

```text
cdesktop-PLATFORM.zip
cdesktop-mcp-PLATFORM.zip
cdesktop-review-PLATFORM.zip
```

Supported platforms are `linux-x64`, `linux-arm64`, `windows-x64`,
`windows-arm64`, `macos-x64`, and `macos-arm64`.

The prerelease also includes the npm tarball as a sibling release asset.
`manifest.json` records SHA-256 and byte size for every binary zip only, and
the CLI validates cached and fresh downloads against the verified manifest
before extraction. The npm tarball checksum is intentionally outside the binary
manifest so downstream release locks can pin the package URL and digest without
creating a hash cycle.

For private mirrors, set `CDESKTOP_RELEASE_ASSET_BASE_URL` to the exact
directory containing `manifest.json` and the flat asset files. The mirror must
preserve the release asset names and the exact manifest bytes embedded into the
published package.

## Roadmap

- **Desktop app build** — ship Tauri installers for macOS, Windows, and Linux
- **Voice input** — push-to-talk dictation for hands-free prompts
- **Files panel** — browse the full project file tree, not just the session working directory
- **Performance optimization** — faster cold start, smaller bundle, lower idle CPU
- **Skill browser** — in-app discovery and one-click install for slash commands and skills

## Support

File bug reports and feature requests at [cdesktop-ai/cdesktop/issues](https://github.com/cdesktop-ai/cdesktop/issues).

## Contributing

Please raise ideas and changes in [GitHub Discussions](https://github.com/cdesktop-ai/cdesktop/discussions) before opening a PR, so we can align on implementation details and roadmap fit.

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (latest stable)
- [Node.js](https://nodejs.org/) (>=20)
- [pnpm](https://pnpm.io/) (>=8)

Additional development tools:

```bash
cargo install cargo-watch
cargo install sqlx-cli
```

Install dependencies:

```bash
pnpm i
```

### Running the dev server

```bash
pnpm run dev
```

This will start the backend and web app. A blank DB will be copied from the `dev_assets_seed` folder.

### Building the web app

To build just the web app:

```bash
cd packages/local-web
pnpm run build
```

### Build from source (macOS)

1. Run `./local-build.sh`
2. Test with `cd npx-cli && node bin/cli.js`

### Environment Variables

The following environment variables can be configured at build time or runtime:

| Variable                   | Type       | Default                 | Description                                                                                                             |
| -------------------------- | ---------- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `POSTHOG_API_KEY`          | Build-time | Empty                   | PostHog analytics API key (disables analytics if empty)                                                                 |
| `POSTHOG_API_ENDPOINT`     | Build-time | Empty                   | PostHog analytics endpoint (disables analytics if empty)                                                                |
| `PORT`                     | Runtime    | Auto-assign             | **Production**: Server port. **Dev**: Frontend port (backend uses PORT+1)                                               |
| `BACKEND_PORT`             | Runtime    | `0` (auto-assign)       | Backend server port (dev mode only, overrides PORT+1)                                                                   |
| `FRONTEND_PORT`            | Runtime    | `3000`                  | Frontend dev server port (dev mode only, overrides PORT)                                                                |
| `HOST`                     | Runtime    | `127.0.0.1`             | Backend server host                                                                                                     |
| `MCP_HOST`                 | Runtime    | Value of `HOST`         | MCP server connection host (use `127.0.0.1` when `HOST=0.0.0.0` on Windows)                                             |
| `MCP_PORT`                 | Runtime    | Value of `BACKEND_PORT` | MCP server connection port                                                                                              |
| `DISABLE_WORKTREE_CLEANUP` | Runtime    | Not set                 | Disable all git worktree cleanup including orphan and expired workspace cleanup (for debugging)                         |
| `CDT_ALLOWED_ORIGINS`      | Runtime    | Not set                 | Comma-separated list of origins that are allowed to make backend API requests (e.g., `https://my-cdesktop.example.com`) |

**Build-time variables** must be set when running `pnpm run build`. **Runtime variables** are read when the application starts.

#### Self-Hosting with a Reverse Proxy or Custom Domain

When running cdesktop behind a reverse proxy (e.g., nginx, Caddy, Traefik) or on a custom domain, you must set the `CDT_ALLOWED_ORIGINS` environment variable. Without this, the browser's Origin header won't match the backend's expected host, and API requests will be rejected with a 403 Forbidden error.

Set it to the full origin URL(s) where your frontend is accessible:

```bash
# Single origin
CDT_ALLOWED_ORIGINS=https://cdesktop.example.com

# Multiple origins (comma-separated)
CDT_ALLOWED_ORIGINS=https://cdesktop.example.com,https://cdesktop-staging.example.com
```

### Remote Deployment

When running cdesktop on a remote server (e.g., via systemctl, Docker, or cloud hosting), you can configure your editor to open projects via SSH:

1. **Access via tunnel**: Use Cloudflare Tunnel, ngrok, or similar to expose the web UI
2. **Configure remote SSH** in Settings → Editor Integration:
   - Set **Remote SSH Host** to your server hostname or IP
   - Set **Remote SSH User** to your SSH username (optional)
3. **Prerequisites**:
   - SSH access from your local machine to the remote server
   - SSH keys configured (passwordless authentication)
   - VSCode Remote-SSH extension

When configured, the "Open in VSCode" buttons will generate URLs like `vscode://vscode-remote/ssh-remote+user@host/path` that open your local editor and connect to the remote server.

See the [documentation](https://cdesktop.ai) for detailed setup instructions.

## License

Apache License 2.0 — see [`LICENSE`](https://github.com/cdesktop-ai/cdesktop/blob/main/LICENSE).

cdesktop is a derivative work of [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban) (Apache 2.0). The provider preset catalog is derived from [farion1231/cc-switch](https://github.com/farion1231/cc-switch) (MIT). See [`NOTICE`](https://github.com/cdesktop-ai/cdesktop/blob/main/NOTICE) for full attribution.
