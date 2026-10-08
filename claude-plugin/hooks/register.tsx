// Forge for Claude Code. Loaded by Forge into every Claude session it opens
// (--plugin-dir), so nothing is installed globally and the repository stays clean.
//
// - A team of subagents on cheaper models, and a standing instruction to delegate to
//   them: the main session keeps its context and its prompt cache, the heavy reading
//   is billed to Haiku.
// - Reports every turn's tokens by model to Forge (subagents included), so Forge can
//   show where the tokens go.
// - A band above the prompt with the project's Guard status, pending ideas and the
//   session's tokens by model.
// - /ideas, /guard and /remember answered by Forge's CLI, without a model call.
import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Band, ModelTokens } from '../types'

const band = atom({ plugin: 'forge', key: 'band' } as const, null as Band | null)
const tokens = atom({ plugin: 'forge', key: 'tokens' } as const, {} as ModelTokens)

const EXPLORER = `You are a fast code explorer working for a senior engineer.
Answer exactly what you were asked about the codebase: where something lives, what a
piece of code does, which files are involved, how something is wired.
- Search with Grep and Glob first; read only the files that matter.
- Reply briefly: the answer first, then file paths with line numbers (path:line).
- Do not modify anything. Do not propose changes unless asked.`

const REVIEWER = `You are a careful reviewer working for a senior engineer.
Run what you are asked (tests, type checks, linters, builds), read the failures and
report the cause, not the full log.
- Quote only the relevant lines of output.
- When reviewing a change, list concrete problems with path:line and why they matter.
- Do not modify files.`

const ARCHITECT = `You are a principal engineer consulted for the hard parts: system
design, data model changes, risky migrations, security-sensitive code and bugs that
resisted the obvious fixes.
- Read what you need to be sure; reason about trade-offs explicitly.
- Reply with a concrete recommendation and a short step-by-step plan.
- Do not modify files.`

const DELEGATION = `Forge team (subagents on cheaper models). Use them to save tokens
and keep your context clean:
- forge:explorer (fast model): any "where is…", "what does … do", finding files,
  symbols or call sites, or reading and summarizing several files. Delegate instead of
  reading many files yourself; you get back a short answer with paths.
- forge:reviewer: running test suites, type checks or builds and interpreting their
  failures; reviewing a finished change.
- forge:architect (strongest model): only for genuinely hard design decisions or bugs
  that resisted the obvious fixes.
Keep doing the actual editing yourself.`

/** "claude-opus-5-5" → "opus"; unknown ids are kept. */
const alias = (model: string) =>
  ['haiku', 'sonnet', 'opus', 'fable'].find(name => model.includes(name)) ?? model

const short = (n: number) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : `${n}`

/** Runs Forge's CLI for this project; null when the session was not opened by Forge. */
async function forge($: EngineInterface, args: string[], stdin?: string) {
  // Never throws: most calls are fire-and-forget and must not break a turn.
  try {
    const bin = await $.env.get('FORGE_BIN')
    if (!bin) return null
    const project = await $.env.get('FORGE_PROJECT')
    const argv = [bin, ...args, ...(project ? ['--project', project] : [])]
    return await $.process.run(argv, { stdin, timeoutMs: 300_000 })
  } catch {
    return null
  }
}

/** Project status for the band, from `forge status --json`. */
async function refreshBand($: EngineInterface) {
  const out = await forge($, ['status', '--json'])
  if (!out || out.exitCode !== 0) return
  try {
    const status = JSON.parse(out.stdout) as Band
    await update($, band, () => status)
  } catch {
    // Older Forge without `status --json`: no band.
  }
}

/** A command answered by Forge's CLI output (no model call). */
async function run($: EngineInterface, args: string[]) {
  const out = await forge($, args)
  if (!out) return { text: 'This session was not opened from Forge.', exitCode: 1 }
  return { text: (out.stdout + out.stderr).trim() || 'Done.', exitCode: out.exitCode }
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const agents = [
      { name: 'explorer', model: 'haiku', prompt: EXPLORER, tools: ['Read', 'Grep', 'Glob', 'Bash'],
        description: 'Fast, cheap code explorer: where things are, what code does, finding files, symbols and call sites, reading and summarizing several files. Returns a short answer with paths.' },
      { name: 'reviewer', model: 'sonnet', prompt: REVIEWER, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Runs tests, type checks and builds and explains failures; reviews a finished change. Does not edit files.' },
      { name: 'architect', model: 'opus', prompt: ARCHITECT, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Strongest model, for hard design decisions, risky migrations, security-sensitive code and stubborn bugs. Returns a recommendation and a plan.' },
    ]
    for (const agent of agents) {
      await $.agent.register(agent)
    }
    await $.command.register({ name: 'ideas', description: 'Forge ideas for this project', argumentHint: '[add <text> | done <id>]' })
    await $.command.register({ name: 'guard', description: 'Check the commit with Forge Guard' })
    await $.command.register({ name: 'remember', description: 'Save a note to the project memory', argumentHint: '<text>' })
    void refreshBand($)
    return next(e)
  })

  // The delegation policy, added to the main session's system prompt.
  on('prompt.compose', async ($, e, next) => {
    const result = await next(e)
    return { sections: [...result.sections, { id: 'forge-team', text: DELEGATION, scope: 'session' as const }] }
  })

  // The built-in explorer runs on Haiku too unless the model asked for another.
  // If this hook fails, the subagent is spawned unchanged.
  on('agent.spawn', ($, e, next) =>
    next(e.subagentType === 'Explore' && !e.model ? { ...e, model: 'haiku' } : e),
  ).catch(($, e, next) => next(e))

  on('turn.complete', async ($, e, next) => {
    const result = await next(e)
    if (e.usage) {
      const total =
        e.usage.input_tokens + e.usage.output_tokens + e.usage.cache_read_input_tokens + e.usage.cache_creation_input_tokens
      const model = alias(e.usage.model)
      await update($, tokens, all => ({ ...all, [model]: (all[model] ?? 0) + total }))
      void forge($, ['agent-usage'], JSON.stringify({ ...e.usage, model, subagent: Boolean(e.agentId) }))
    }
    // The main loop answered (not an intermediate turn that only called tools): tell
    // Forge, which notifies while it is in the background.
    if (!e.agentId && e.reason === 'answer' && e.answer.trim()) {
      const summary = e.answer.split('\n').find(line => line.trim())?.trim().slice(0, 140) ?? ''
      void forge($, ['agent-event', 'stop'], JSON.stringify({ message: summary }))
      void refreshBand($)
    }
    return result
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const status = await read($, band)
    const used = Object.entries(await read($, tokens))
      .sort((a, b) => b[1] - a[1])
      .map(([model, n]) => `${model} ${short(n)}`)
      .join(' · ')
    if (e.props.hasSurvey || (status === null && !used)) return next(e)
    const parts = ['Forge']
    if (status) {
      parts.push(`Guard ${status.guard}`, `${status.ideas === 1 ? '1 idea' : `${status.ideas} ideas`} pending`)
    }
    if (used) parts.push(`session ${used}`)
    const { Text } = $.ui.resolve(e)
    return <Text dimColor>{parts.join(' · ')}</Text>
  })

  on('command.run', { command: 'ideas' }, async ($, e) => {
    const [action, ...rest] = e.args.trim().split(/\s+/)
    const text = rest.join(' ')
    if (action === 'add' && text) return run($, ['ideas', 'add', text])
    if (action === 'done' && text) return run($, ['ideas', 'done', text])
    return run($, ['ideas'])
  })

  on('command.run', { command: 'guard' }, async $ => {
    const result = await run($, ['guard', 'commit'])
    void refreshBand($)
    return result
  })

  on('command.run', { command: 'remember' }, async ($, e) => {
    const text = e.args.trim()
    if (!text) return { text: 'Usage: /remember <text>', exitCode: 1 }
    return run($, ['memory', 'add', 'note', text.slice(0, 60), text])
  })
}
