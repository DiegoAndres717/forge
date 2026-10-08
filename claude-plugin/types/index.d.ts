/** Project status shown in the band above the prompt (from `forge status --json`). */
export type Band = {
  guard: string
  ideas: number
  memories: number
}

/** Tokens this session used, by model alias (main loop and subagents together). */
export type ModelTokens = Record<string, number>

declare module 'claude-code' {
  interface PluginState {
    forge: { band: Band | null; tokens: ModelTokens }
  }
}
