// The anthology mods' contract with Claude Code (plugin.json names it as "types"): the named
// values the mods keep in the session with `$.state`, one key per mod. Every `$.state` call must
// name a key declared here.
declare module 'claude-code' {
  interface PluginState {
    anthology: {
      /** strict delegation: the lead's tiny-fix budget this turn, the file edited and how many edits */
      delegation: { file: string; edits: number }
      /**
       * alerts and status pane (hooks/mods/alerts.tsx): what the pane draws. A pane reads it while
       * it draws, so a write redraws it, and a hot reload keeps it. Times are epoch milliseconds.
       * Change the shape tag on its atom when a row's fields change.
       */
      status: Shaped<{
        /** agents that started and have not reported; `type` is `anthology:implementer` or `general-purpose` */
        running: { id: string; type: string; description: string; startedAt: number }[]
        /** the last agents whose run ended, newest first; `startedAt` is absent when the start was not seen */
        finished: {
          id: string
          type: string
          description: string
          startedAt?: number
          endedAt: number
          status: 'completed' | 'failed' | 'stopped'
        }[]
        /**
         * permission prompts not yet answered; `key` ties one to the tool call that ends it. Only the
         * tool's name is kept, never what the call does (no command, path or URL).
         */
        prompts: {
          key: string
          tool: string
          /** the agent that asks and its type; absent for the lead. A subagent's row ends with its report */
          agentId?: string
          agent?: string
          at: number
        }[]
      }>
    }
  }
}
