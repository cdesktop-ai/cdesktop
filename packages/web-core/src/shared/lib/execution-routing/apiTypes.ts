/** Display-safe metered approval contract from cdesktop 96960fbe. */
export type MeteredApprovalPolicy = 'auto' | 'ask' | 'never';
export type MeteredApprovalState =
  | 'pending'
  | 'approved'
  | 'denied'
  | 'auto_started'
  | 'blocked';

export interface MeteredApproval {
  id: string;
  session_command_id: string;
  policy: MeteredApprovalPolicy;
  state: MeteredApprovalState;
  account_alias: string | null;
  reason: string | null;
  execution_process_id: string | null;
  created_at: string;
  resolved_at: string | null;
}

export interface MeteredApprovalResponseRequest {
  approved: boolean;
  reason?: string;
}
