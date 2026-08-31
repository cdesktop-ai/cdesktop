Lane E cdesktop dashboard shell handoff

Head SHA: `61f5f010d345a6fb5d55a6065c09ce6f37f733d6`
Base SHA: `41d37b261ada0d03b73e82cfd59d1fa39140a61b` (`origin/cdt/13da-cdesktop-format`)
PR: #6 `https://github.com/clarkipeng/cdesktop/pull/6` (draft, stacked on `cdt/13da-cdesktop-format`)

Owned changed paths: `packages/local-web/src/app/navigation/AppNavigation.ts`, `packages/local-web/src/routeTree.gen.ts`, `packages/local-web/src/routes/_app._shell.agents.tsx`, `packages/ui/src/components/WorkspacesSidebar.tsx`, `packages/web-core/src/i18n/locales/en/common.json`, `packages/web-core/src/i18n/locales/en/settings.json`, `packages/web-core/src/i18n/locales/es/common.json`, `packages/web-core/src/i18n/locales/es/settings.json`, `packages/web-core/src/i18n/locales/fr/common.json`, `packages/web-core/src/i18n/locales/fr/settings.json`, `packages/web-core/src/i18n/locales/ja/common.json`, `packages/web-core/src/i18n/locales/ja/settings.json`, `packages/web-core/src/i18n/locales/ko/common.json`, `packages/web-core/src/i18n/locales/ko/settings.json`, `packages/web-core/src/i18n/locales/zh-Hans/common.json`, `packages/web-core/src/i18n/locales/zh-Hans/settings.json`, `packages/web-core/src/i18n/locales/zh-Hant/common.json`, `packages/web-core/src/i18n/locales/zh-Hant/settings.json`, `packages/web-core/src/pages/workspaces/WorkspacesSidebarContainer.tsx`, `packages/web-core/src/shared/components/execution-routing/ExecutionRoutingSummary.tsx`, `packages/web-core/src/shared/dialogs/settings/settings/ExecutionRoutingSettingsSection.tsx`, `packages/web-core/src/shared/dialogs/settings/settings/settingsRegistry.tsx`, `packages/web-core/src/shared/lib/execution-routing/fixtures.ts`, `packages/web-core/src/shared/lib/routes/appNavigation.ts`.

Fixture module: `packages/web-core/src/shared/lib/execution-routing/fixtures.ts`
Fixture exports: `ExecutionRoutingExecutor`, `ExecutionRoutingBillingClass`, `MeteredFallbackPolicy`, `SelectionStatus`, `RouteHealthStatus`, `ExecutionRoutingRouteFixture`, `ExecutionRoutingSettingsFixture`, `ExecutionRoutingSelectionFixture`, `AgentRouteFixture`, `ExecutionRoutingFixture`, `executionRoutingFixture`, `getExecutionRoutingRouteById`.

Replace fixtures at these seams only after A1/B contract is real: settings read/update; ordered route inventory/status; selection result and safe trace; agent route bindings/status; metered approval outcome; cooldown reset/clear; retry/backoff events; route reorder/edit persistence.

Deliberately not done: no backend wiring, no Rust, no migrations/schema, no generated shared TS type edits, no provider/auth/secret resolution, no real account credential fields, no Lane B guessing, no standalone SightMesh pool-page work, no PR #5 backend CI repair.

Assumptions to revalidate with B: real API preserves subscription-first route order and metered `auto|ask|never`; route status can expose safe nullable account aliases but never credentials/auth bindings; UI can consume a stable `resolved|approval_needed|blocked` selection shape plus display-safe trace lines.
