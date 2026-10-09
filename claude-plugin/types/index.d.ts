/** Project status shown in the band above the prompt (from `forge status --json`). */
export type Band = {
  guard: string
  ideas: number
  memories: number
}

/** Tokens this session used by one model: new ones and the ones read from the prompt cache. */
export type Used = { fresh: number; cache: number }

/** Tokens this session used, by model alias (main loop and subagents together). */
export type ModelTokens = Record<string, Used>

/** A plan usage window (`five_hour`, `seven_day`) as Claude Code reports it. */
export type Limit = { kind: string; percentUsed: number; resetsAt?: string }

declare module 'claude-code' {
  interface PluginState {
    forge: { band: Band | null; tokens: ModelTokens; limits: Limit[] }
  }
}
