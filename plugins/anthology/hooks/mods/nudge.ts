// Parallel work reminder (stub): the real part replaces this file. It is one part of the shared
// `agent.report` hook that index.ts registers, so it exports a function and registers nothing.
import type { AgentReportInput, AgentReportResult, EngineInterface } from 'claude-code'

export async function nudgeReport(_$: EngineInterface, _e: AgentReportInput, r: AgentReportResult): Promise<AgentReportResult> {
  return r
}
