// Forge for Claude Code. Loaded into every Claude started in a Forge terminal (Forge
// sets CLAUDE_CODE_PLUGIN_DIRS there), whether typed by hand or opened from the sidebar;
// nothing is installed globally and the repository stays clean.
//
// - A team of subagents on cheaper models, and a standing instruction to delegate to
//   them: the main session keeps its context and its prompt cache, the heavy reading
//   is billed to Haiku.
// - Reports every turn's tokens by model to Forge (subagents included), so Forge can
//   show where the tokens go.
// - A band above the prompt with the project's Guard status, plans and the
//   session's tokens by model.
// - /plans, /guard and /remember answered by Forge's CLI, without a model call.
import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Active, Band, Limit, ModelTokens } from '../types'

const band = atom({ plugin: 'forge', key: 'band' } as const, null as Band | null)
const tokens = atom({ plugin: 'forge', key: 'tokens' } as const, {} as ModelTokens)
const limits = atom({ plugin: 'forge', key: 'limits' } as const, [] as Limit[])
const active = atom({ plugin: 'forge', key: 'active' } as const, {} as Active)
const tasks = atom({ plugin: 'forge', key: 'tasks' } as const, {} as Record<string, string>)

/** Models always listed in the band, in this order (others are added when used). */
const MODELS = ['opus', 'sonnet', 'haiku']

/** The plan windows shown in the band, in order, with their short labels. */
const WINDOWS: [string, string][] = [['five_hour', '5h'], ['seven_day', 'week']]
const BAR_CELLS = 12

/** Filled cells of a bar: any use shows at least one. */
const cells = (percent: number) =>
  percent > 0 ? Math.max(1, Math.round((percent / 100) * BAR_CELLS)) : 0

/** "in 3h" / "in 2d" until an ISO time; empty when unknown or past. */
function resetsIn(at: string | undefined, now: number) {
  const ms = at ? Date.parse(at) - now : Number.NaN
  if (!(ms > 0)) return ''
  const hours = Math.round(ms / 3_600_000)
  return hours >= 48 ? `in ${Math.round(hours / 24)}d` : `in ${Math.max(1, hours)}h`
}

/** "2 in progress · 3 backlog" (older Forge: "N ideas pending"). */
function plansLabel(status: Band) {
  if (status.plans_in_progress === undefined || status.backlog === undefined) {
    return { text: `${status.ideas} ${status.ideas === 1 ? 'idea' : 'ideas'} pending`, busy: status.ideas > 0 }
  }
  const parts = [
    status.plans_in_progress > 0 ? `${status.plans_in_progress} in progress` : '',
    status.backlog > 0 ? `${status.backlog} backlog` : '',
  ].filter(Boolean)
  return { text: parts.length ? parts.join(' · ') : 'no plans', busy: status.plans_in_progress > 0 }
}

/** Theme colors (they follow the user's Claude theme). */
const MODEL_COLOR: Record<string, string> = { opus: 'planMode', sonnet: 'suggestion', haiku: 'success', fable: 'claude' }
const GUARD_COLOR: Record<string, string> = { ready: 'success', warnings: 'warning', blocked: 'error' }

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

const RESEARCHER = `You are a research assistant working for a senior engineer.
Look things up outside the codebase with the connected tools (Slack, Linear, ClickUp,
email, calendar, Drive, docs) or the web, exactly as asked.
- Search, open the relevant threads or items, and read what matters.
- Reply briefly: the answer first, then who said what and when, with links.
- Do not post, send, create or change anything unless explicitly asked.`

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
- forge:researcher (fast model): anything outside the code through the connected tools
  or the web, such as "did X reply?", "what's the status of this ticket?", "find the
  thread about…". It searches and reads; you get back a short summary with links.
- forge:reviewer: running test suites, type checks or builds and interpreting their
  failures; reviewing a finished change.
- forge:architect (strongest model): only for genuinely hard design decisions or bugs
  that resisted the obvious fixes.
Keep doing the actual editing yourself.

Forge plans: the user's plans and backlog live in Forge, reachable through the forge MCP
tools (plans_list, plan_add, plan_update). When the user asks to save or put a plan "in
Forge" / "en planes de Forge", call plan_add with its phases (a branch only where it
deserves one, and tasks); never look for or write plan files. Ideas for later: plan_add
without phases (backlog). While working a plan, mark the phase in_progress, tick tasks
and mark it done.`

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
      { name: 'researcher', model: 'haiku', prompt: RESEARCHER, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Fast, cheap researcher for anything outside the code: Slack, Linear, ClickUp, email, calendar, Drive, docs and the web through the connected tools. Returns a short summary with links. Read-only.' },
      { name: 'reviewer', model: 'sonnet', prompt: REVIEWER, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Runs tests, type checks and builds and explains failures; reviews a finished change. Does not edit files.' },
      { name: 'architect', model: 'opus', prompt: ARCHITECT, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Strongest model, for hard design decisions, risky migrations, security-sensitive code and stubborn bugs. Returns a recommendation and a plan.' },
    ]
    for (const agent of agents) {
      await $.agent.register(agent)
    }
    await $.command.register({ name: 'plans', description: "This project's Forge plans and backlog", argumentHint: '[add <text> | show <id>]' })
    await $.command.register({ name: 'guard', description: 'Check the commit with Forge Guard' })
    await $.command.register({ name: 'remember', description: 'Save a note to the project memory', argumentHint: '<text>' })
    void refreshBand($)
    void $.session.usage().then(u => update($, limits, () => u.rateLimits)).catch(() => undefined)
    return next(e)
  })

  // Plan usage windows: pushed after each turn and when a window moves a point.
  on('session.measure', async ($, e, next) => {
    if (e.changed.includes('rateLimits')) await update($, limits, () => e.rateLimits)
    return next(e)
  })

  // The delegation policy, added to the main session's system prompt.
  on('prompt.compose', async ($, e, next) => {
    const result = await next(e)
    return { sections: [...result.sections, { id: 'forge-team', text: DELEGATION, scope: 'session' as const }] }
  })

  // The built-in explorer runs on Haiku too unless the model asked for another; the
  // task is remembered so the band can say what each running subagent is doing.
  // If this hook fails, the subagent is spawned unchanged.
  on('agent.spawn', async ($, e, next) => {
    const result = await next(e.subagentType === 'Explore' && !e.model ? { ...e, model: 'haiku' } : e)
    const id = result.agentId
    if (id) {
      const name = e.subagentType.replace(/^forge:/, '').toLowerCase()
      // Only cosmetic: a failure here must not spawn the subagent a second time.
      await update($, tasks, all => ({ ...all, [id]: e.description ? `${name}: ${e.description}` : name })).catch(
        () => undefined,
      )
    }
    return result
  }).catch(($, e, next) => next(e))

  // Who is working right now, for the band: from a loop's first model request (main or
  // a subagent) until its turn completes. The request itself passes through untouched.
  on('turn.step', async function* ($, e, next) {
    const key = e.agentId ?? 'main'
    const model = alias(e.model)
    try {
      const names = await read($, tasks)
      await update($, active, all =>
        all[key]?.model === model ? all : { ...all, [key]: { model, task: key === 'main' ? '' : names[key] ?? 'subagent' } },
      )
    } catch {
      // Only the band depends on this: never hold up the request.
    }
    return yield* next(e)
  })

  on('turn.complete', async ($, e, next) => {
    const result = await next(e)
    const key = e.agentId ?? 'main'
    await update($, active, ({ [key]: _done, ...rest }) => rest)
    if (e.usage) {
      // Cache reads cost a fraction of new tokens: counted apart so the band is honest.
      const fresh = e.usage.input_tokens + e.usage.output_tokens + e.usage.cache_creation_input_tokens
      const cache = e.usage.cache_read_input_tokens
      const model = alias(e.usage.model)
      await update($, tokens, all => {
        const before = all[model] ?? { fresh: 0, cache: 0 }
        return { ...all, [model]: { fresh: before.fresh + fresh, cache: before.cache + cache } }
      })
      void forge($, ['agent-usage'], JSON.stringify({ ...e.usage, model, subagent: Boolean(e.agentId) }))
    }
    // The main loop answered (not an intermediate turn that only called tools): tell
    // Forge, which notifies while it is in the background.
    if (!e.agentId && e.reason === 'answer' && e.answer.trim()) {
      const summary = e.answer.split('\n').find(line => line.trim())?.trim().slice(0, 140) ?? ''
      void forge($, ['agent-event', 'stop'], JSON.stringify({ message: summary, origin: 'Claude Code' }))
      void refreshBand($)
    }
    return result
  })

  // Claude needs the user (a permission prompt, waiting for input): tell Forge.
  on('classic.Notification', async ($, e, next) => {
    void forge($, ['agent-event', 'waiting'], JSON.stringify({ message: e.message, origin: 'Claude Code' }))
    return next(e)
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const status = await read($, band)
    const spent = await read($, tokens)
    const working = Object.entries(await read($, active))
    // Fixed order so nothing jumps around while it updates; unused models stay dim.
    const models = [...MODELS, ...Object.keys(spent).filter(m => !MODELS.includes(m))]
    const used = Object.entries(spent)
    const busy = (model: string) => working.some(([, w]) => w.model === model)
    const subagents = working.filter(([key]) => key !== 'main')
    const now = Date.now()
    const windows = await read($, limits)
    const plan = WINDOWS.flatMap(([kind, label]) => {
      const w = windows.find(l => l.kind === kind)
      if (!w) return []
      const percent = Math.max(0, Math.min(100, Math.round(w.percentUsed)))
      return [{ label, percent, reset: resetsIn(w.resetsAt, now) }]
    })
    if (e.props.hasSurvey || (status === null && used.length === 0 && working.length === 0 && plan.length === 0))
      return next(e)
    const { Text } = $.ui.resolve(e)
    // One line of text with colored spans: it reads left to right and, when narrow,
    // wraps like any sentence instead of stacking words in columns.
    const sep = <Text dimColor> · </Text>
    return (
      <Text>
        <Text color="claude" bold>Forge</Text>
        {status ? sep : null}
        {status ? <Text dimColor>Guard </Text> : null}
        {status ? <Text color={GUARD_COLOR[status.guard] ?? 'subtle'}>{status.guard}</Text> : null}
        {status ? sep : null}
        {status ? (
          <Text color={plansLabel(status).busy ? 'warning' : 'subtle'}>{plansLabel(status).text}</Text>
        ) : null}
        {models.map(model => {
          const n = spent[model] ?? { fresh: 0, cache: 0 }
          const atWork = busy(model)
          if (!atWork && n.fresh === 0) {
            return <Text key={model} dimColor>{` · ${model} 0`}</Text>
          }
          // The model at work right now: a filled dot and its name in bold.
          return (
            <Text key={model}>
              <Text dimColor> · </Text>
              {atWork ? <Text color={MODEL_COLOR[model] ?? 'text'}>● </Text> : null}
              <Text color={MODEL_COLOR[model] ?? 'text'} bold={atWork}>{model}</Text>
              {` ${short(n.fresh)}`}
              {n.cache > 0 ? <Text dimColor>{` (+${short(n.cache)} cache)`}</Text> : null}
            </Text>
          )
        })}
        {subagents.map(([key, w]) => (
          <Text key={key}>
            <Text dimColor> · </Text>
            <Text color={MODEL_COLOR[w.model] ?? 'text'}>{'↳ '}</Text>
            <Text>{w.task.length > 48 ? `${w.task.slice(0, 47)}…` : w.task}</Text>
            <Text dimColor>{` (${w.model})`}</Text>
          </Text>
        ))}
        {plan.map(({ label, percent, reset }) => (
          <Text key={label}>
            <Text dimColor> · {label} </Text>
            <Text color={percent >= 80 ? 'error' : percent >= 50 ? 'warning' : 'success'}>
              {'━'.repeat(cells(percent))}
            </Text>
            <Text dimColor>{'─'.repeat(BAR_CELLS - cells(percent))}</Text>
            {` ${percent}%`}
            {percent >= 80 && reset ? <Text dimColor>{` resets ${reset}`}</Text> : null}
          </Text>
        ))}
      </Text>
    )
  })

  on('command.run', { command: 'plans' }, async ($, e) => {
    const [action, ...rest] = e.args.trim().split(/\s+/)
    const text = rest.join(' ')
    if (action === 'add' && text) return run($, ['plans', 'add', text])
    if (action === 'show' && text) return run($, ['plans', 'show', text])
    return run($, ['plans'])
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
