// The anthology mods' contract with Claude Code (plugin.json names it as "types"): the named
// values the mods keep in the session with `$.state`, one key per value. Every `$.state` call must
// name a key declared here.
declare module 'claude-code' {
  interface PluginState {
    anthology: {
      /** strict delegation: the lead's tiny-fix budget this turn, the file edited and how many edits */
      delegation: { file: string; edits: number }
      /** strict delegation: whether the system prompt carries the coordinator section */
      coordinator: boolean
    }
  }
}
