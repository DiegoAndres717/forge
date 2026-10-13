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
// - /forge-plans, /forge-guard and /forge-remember answered by Forge's CLI, without a model
//   call (the prefix keeps them together: typing /forge lists all three).
import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Active, Band, Limit, ModelTokens } from '../types'

const band = atom({ plugin: 'forge', key: 'band' } as const, null as Band | null)
const tokens = atom({ plugin: 'forge', key: 'tokens' } as const, {} as ModelTokens)
/** Usage windows already reported to Forge as exhausted (`kind:resetsAt`). */
const limitsReported = new Set<string>()
const limits = atom({ plugin: 'forge', key: 'limits' } as const, [] as Limit[])
const active = atom({ plugin: 'forge', key: 'active' } as const, {} as Active)
const tasks = atom({ plugin: 'forge', key: 'tasks' } as const, {} as Record<string, string>)
const context = atom({ plugin: 'forge', key: 'context' } as const, null as number | null)
const effort = atom({ plugin: 'forge', key: 'effort' } as const, '')

/** Effort levels Claude Code takes, from least to most. */
const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'] as const
type Effort = (typeof EFFORTS)[number]
const isEffort = (v: string): v is Effort => (EFFORTS as readonly string[]).includes(v)

/** Context fuller than this: the band suggests /compact before it gets expensive. */
// Quality drops well before the window is full: warn early enough to compact in time.
const CONTEXT_WARN = 60
// The plan anchor is repeated when it changes or every few messages: each copy stays in
// the context, so not on every turn.
const ANCHOR_EVERY = 8
let lastAnchor = ''
let sinceAnchor = 0
// Quick filter before asking Forge (`forge secret`) whether a call reads a file with
// secrets; Forge decides. Same idea as the reader shims of the other agents.
// Windows: Claude Code's shell can rebuild PATH and skip Guard's shims, so on a Bash call
// that names a watched program the mod asks Forge (`forge danger`) as a second line.
const DANGER_HINT = /\b(git|rm|docker|psql|mysql|mariadb|dropdb)(\.exe)?\b/i
const SECRET_HINT = /\.env|\.ssh|\.aws|\.gnupg|\.kube|\.docker|gh\/hosts|gcloud|\.pem|\.key\b|\.p12|\.pfx|id_rsa|id_ed25519|id_ecdsa|\.netrc|\.pgpass|\.npmrc|\.pypirc|credentials|keychain|\.kdbx/

/** Searching and reading tools: after this many in a row the main loop is nudged to
 * hand the exploration to forge:explorer (cheaper, and keeps the context clean). */
const EXPLORE_TOOLS = new Set(['Read', 'Grep', 'Glob'])
// Measured on real sessions: most exploring goes through the shell (rg, grep, sed -n,
// cat…), not Read, so those commands count too.
const EXPLORE_SHELL = /^\s*(?:cd\s+\S+\s*&&\s*)?(?:rg|grep|egrep|find|fd|ls|tree|cat|head|tail|sed\s+-n|wc|awk)\b/
const EXPLORE_NUDGE_AT = 3
const explores = (e: { tool: string }) =>
  EXPLORE_TOOLS.has(e.tool) ||
  (e.tool === 'Bash' && 'command' in e && typeof e.command === 'string' && EXPLORE_SHELL.test(e.command))
let exploring = 0

/** Whole suites, builds and type checks: long output that stays in the context, the
 * reviewer's job. Once per message of yours the main loop is reminded to hand it over. */
// ponytail: matches by command; a single targeted test (`cargo test name`) gets the same
// one reminder, which says to ignore it for the test being fixed.
const CHECK_SHELL = /(?:^|&&|;)\s*(?:cargo\s+(?:test|build|clippy|check)|(?:npm|pnpm|yarn|bun)\s+(?:run\s+)?(?:test|build|typecheck|lint)|npx\s+(?:-p\s+\S+\s+)?tsc|tsc|pytest|go\s+(?:test|build|vet)|gradle|\.\/gradlew|mvn|xcodebuild|swift\s+(?:test|build))\b/
let checkNudged = false

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

// Same text as the MCP server's tests-first instruction (crates/forge-core/src/mcp.rs).
const TESTS_FIRST = 'Tests-first mode: for every behavior change or bug fix, first write a test that fails for the right reason and run it to see it fail, then write the smallest code that makes it pass, run it again to see it pass, and only then refactor. Say which test you added and show both runs. Skip it only for changes with nothing to test (docs, config, styling) and say so.'

const DELEGATION = `Forge team (subagents on cheaper models). Use them to save tokens
and keep your context clean:
- forge:explorer (fast model): any "where is…", "what does … do", finding files,
  symbols or call sites, or reading and summarizing several files. Start with it
  whenever understanding something will take more than two searches or reads, shell
  ones included (rg, grep, find, cat, sed -n): its results stay out of your context,
  which every later turn re-reads. You get back a short answer with paths.
- forge:researcher (fast model): anything outside the code through the connected tools
  or the web, such as "did X reply?", "what's the status of this ticket?", "find the
  thread about…". It searches and reads; you get back a short summary with links.
- forge:reviewer (cheaper model): full test suites, type checks and builds, and the
  review of a finished change before you commit. Their long output stays in its
  context, not yours; you get back what failed and why. Run only the single test you
  are fixing yourself.
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

/** Forge's orange (the app icon), a little lighter letter by letter. */
const BRAND = ['#ff6a00', '#ff7f0a', '#ff9f0a', '#ffb340', '#ffc66b']

/** One short header line when the session was opened from Forge: project and branch. */
async function sessionHeader($: EngineInterface, cwd: string) {
  const project = await $.env.get('FORGE_PROJECT')
  if (!project) return
  const name = project.split(/[\\/]/).filter(Boolean).at(-1) ?? project
  const branch = await $.process.run(['git', 'branch', '--show-current'], { cwd, timeoutMs: 5_000 })
    .then(result => result.exitCode === 0 ? result.stdout.trim() : '')
    .catch(() => '')
  $.ui.log(`◆ Forge · ${name}${branch ? ` · ${branch}` : ''}`)
}

/** Runs Forge's CLI for this project; null when the session was not opened by Forge. */
async function forge($: EngineInterface, args: string[], stdin?: string, timeoutMs = 300_000) {
  // Never throws: most calls are fire-and-forget and must not break a turn.
  try {
    const bin = await $.env.get('FORGE_BIN')
    if (!bin) return null
    const project = await $.env.get('FORGE_PROJECT')
    const argv = [bin, ...args, ...(project ? ['--project', project] : [])]
    return await $.process.run(argv, { stdin, timeoutMs })
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
      { name: 'explorer', model: 'haiku', effort: 'low', prompt: EXPLORER, tools: ['Read', 'Grep', 'Glob', 'Bash'],
        description: 'Fast, cheap code explorer: where things are, what code does, finding files, symbols and call sites, reading and summarizing several files. Returns a short answer with paths.' },
      { name: 'researcher', model: 'haiku', effort: 'low', prompt: RESEARCHER, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Fast, cheap researcher for anything outside the code: Slack, Linear, ClickUp, email, calendar, Drive, docs and the web through the connected tools. Returns a short summary with links. Read-only.' },
      { name: 'reviewer', model: 'sonnet', effort: 'medium', prompt: REVIEWER, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Runs tests, type checks and builds and explains failures; reviews a finished change. Does not edit files.' },
      { name: 'architect', model: 'opus', effort: 'high', prompt: ARCHITECT, disallowedTools: ['Edit', 'Write', 'NotebookEdit'],
        description: 'Strongest model, for hard design decisions, risky migrations, security-sensitive code and stubborn bugs. Returns a recommendation and a plan.' },
    ]
    for (const agent of agents) {
      await $.agent.register(agent)
    }
    await $.command.register({ name: 'forge-plans', description: "Forge: this project's plans and backlog", argumentHint: '[add <text> | show <id>]' })
    await $.command.register({ name: 'forge-guard', description: 'Forge: check the commit with Project Guard' })
    await $.command.register({ name: 'forge-remember', description: 'Forge: save a note to Forge Memory', argumentHint: '<text>' })
    await $.command.register({ name: 'forge-effort', description: 'Forge: how hard Claude thinks in this session', argumentHint: '[low | medium | high | xhigh | max | auto]' })
    await $.command.register({ name: 'forge-memory', description: 'Forge: search project memory without a model call', argumentHint: '<text>' })
    void sessionHeader($, e.cwd) // `git` may be slow: never hold up the session start
    void refreshBand($)
    void $.session
      .usage()
      .then(async u => {
        await update($, limits, () => u.rateLimits)
        if (u.rateLimits?.length) void forge($, ['agent-limits'], JSON.stringify(u.rateLimits))
      })
      .catch(() => undefined)
    return next(e)
  })

  // Plan usage windows: pushed after each turn and when a window moves a point.
  on('session.measure', async ($, e, next) => {
    if (e.changed.includes('context') && typeof e.context?.percent === 'number') {
      const percent = Math.round(e.context.percent)
      await update($, context, () => percent)
    }
    if (e.changed.includes('rateLimits')) {
      await update($, limits, () => e.rateLimits)
      // Forge keeps each account's latest windows, to show them all in Settings.
      if (e.rateLimits?.length) void forge($, ['agent-limits'], JSON.stringify(e.rateLimits))
      // A window ran out: Forge offers to carry on with another account (once per window).
      for (const w of e.rateLimits ?? []) {
        const key = `${w.kind}:${w.resetsAt ?? ''}`
        if (w.percentUsed >= 100 && !limitsReported.has(key)) {
          limitsReported.add(key)
          void forge($, ['agent-event', 'limit'], JSON.stringify({ message: `${w.kind} limit reached`, origin: 'Claude Code' }))
        }
      }
    }
    return next(e)
  })

  // Project memory at the start of every conversation (and again after compaction or
  // /clear): key decisions and conventions, Guard rules and the plans in progress, so
  // Claude starts knowing them instead of having to remember to look them up.
  on('prompt.context', async ($, e, next) => {
    const result = await next(e)
    const out = await forge($, ['context'])
    const text = out && out.exitCode === 0 ? out.stdout.trim() : ''
    if (!text) return result
    const block = {
      name: 'forge',
      text: `${text}\n\nFrom Forge (this project's memory and plans). Search more with memory_search and plans_list; save new decisions and fixed bugs with memory_save.`,
    }
    return { ...result, blocks: [...result.blocks, block] }
  })

  // A local search per message adds only references (a few titles): the model asks for
  // the detail with memory_get when it needs it. Slash commands and one-word replies
  // carry nothing to look up; it never waits more than 2 s.
  // Files with secrets (.env, ~/.ssh, credentials…): the person decides each time, with
  // Claude Code's own permission dialog.
  on('tool.check', async ($, e, next) => {
    const verdict = await next(e)
    if (verdict.decision === 'deny') return verdict
    const input = (e.input ?? {}) as { file_path?: unknown; path?: unknown; command?: unknown }
    if (e.tool === 'Bash' && typeof input.command === 'string' && DANGER_HINT.test(input.command)
      && await $.env.get('FORGE_GUARD_COMMANDS').catch(() => undefined) === '1') {
      const out = await forge($, ['danger', '--command', input.command], undefined, 2_000)
      if (out && out.exitCode === 0) return { decision: 'ask' as const, reason: `Forge: ${out.stdout.trim()}` }
    }
    const target = e.tool === 'Bash' ? input.command : e.tool === 'Read' || e.tool === 'Grep' ? input.file_path ?? input.path : undefined
    if (typeof target !== 'string' || !SECRET_HINT.test(target)) return verdict
    const out = await forge($, ['secret', e.tool === 'Bash' ? '--command' : '--path', target], undefined, 2_000)
    if (!out || out.exitCode !== 0) return verdict
    return { decision: 'ask' as const, reason: `Forge: ${out.stdout.trim()}` }
  })

  // The plan anchor goes last, right before the model answers: what sits at the end of a
  // long context is what it attends to best ("lost in the middle").
  on('prompt.submit', async ($, e, next) => {
    exploring = 0
    checkNudged = false
    const text = e.text.trim()
    if (text.startsWith('/')) return next(e)
    const [found, anchored] = await Promise.all([
      text.length < 12 ? null : forge($, ['memory', 'search', '--brief', text], undefined, 2_000),
      forge($, ['anchor'], undefined, 2_000),
    ])
    const memories = found && found.exitCode === 0 ? found.stdout.trim() : ''
    const anchor = anchored && anchored.exitCode === 0 ? anchored.stdout.trim() : ''
    sinceAnchor += 1
    const repeat = anchor !== '' && (anchor !== lastAnchor || sinceAnchor >= ANCHOR_EVERY)
    const context = [
      ...(e.context ?? []),
      ...(memories ? [`Relevant Forge Memory:\n${memories}`] : []),
      ...(repeat ? [`Forge: ${anchor}`] : []),
    ]
    if (repeat) [lastAnchor, sinceAnchor] = [anchor, 0]
    return next(context.length === (e.context ?? []).length ? e : { ...e, context })
  })

  on('session.compact', async ($, e, next) => {
    const result = await next(e)
    const messages = 'messages' in result ? result.messages : undefined
    // The summary is the longest text left after compacting (it may come as a user turn).
    // ponytail: by length; read a marker if the engine ever labels the summary.
    const summary = [...(messages ?? [])].sort((a, b) => b.text.length - a.text.length)[0]?.text.trim()
    // One note per project, updated each time (see `forge memory session`).
    if (summary && e.trigger !== 'precompute') void forge($, ['memory', 'session'], summary)
    return result
  })

  // The delegation policy, added to the main session's system prompt.
  on('prompt.compose', async ($, e, next) => {
    const result = await next(e)
    const sections = [...result.sections, { id: 'forge-team', text: DELEGATION, scope: 'session' as const }]
    // Never lose the delegation policy because the variable can't be read.
    if (await $.env.get('FORGE_ASK_FIRST').catch(() => undefined) === '1') {
      sections.push({ id: 'forge-ask-first', text: 'Before planning a large request, ask up to three questions to clarify what is essential. Once answered, create the plan with plan_add before writing code.', scope: 'session' as const })
    }
    if (await $.env.get('FORGE_TESTS_FIRST').catch(() => undefined) === '1') {
      sections.push({ id: 'forge-tests-first', text: TESTS_FIRST, scope: 'session' as const })
    }
    return { sections }
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
    // /forge-effort changes the main loop's effort (only on a model that takes one).
    let step = e
    try {
      const chosen = key === 'main' ? await read($, effort) : ''
      if (chosen && isEffort(chosen) && e.effort !== undefined) step = { ...e, effort: chosen }
    } catch {
      // Without the store the step goes as it came.
    }
    const level = typeof step.effort === 'string' ? step.effort : undefined
    try {
      const names = await read($, tasks)
      await update($, active, all =>
        all[key]?.model === model && all[key]?.effort === level
          ? all
          : { ...all, [key]: { model, effort: level, task: key === 'main' ? '' : names[key] ?? 'subagent' } },
      )
    } catch {
      // Only the band depends on this: never hold up the request.
    }
    return yield* next(step)
  })

  // The main loop searching and reading file after file itself: one reminder (that only
  // the model reads) to hand the rest to forge:explorer. Counted per message of yours.
  on('tool.call', async ($, e, next) => {
    const result = await next(e)
    if (e.agentId || 'deny' in result) return result
    if (!checkNudged && e.tool === 'Bash' && 'command' in e && typeof e.command === 'string' && CHECK_SHELL.test(e.command)) {
      checkNudged = true
      exploring = 0
      return {
        ...result,
        context: [
          ...(result.context ?? []),
          'Forge: whole test suites, builds and type checks are forge:reviewer\'s job (cheaper model, and their long output stays out of your context). Hand it the next full run and the review of the finished change; run only the single test you are fixing yourself.',
        ],
      }
    }
    if (!explores(e)) {
      exploring = 0
      return result
    }
    exploring += 1
    if (exploring !== EXPLORE_NUDGE_AT) return result
    return {
      ...result,
      context: [
        ...(result.context ?? []),
        'Forge: you have searched or read several files yourself in a row. Delegate the rest of this exploration to forge:explorer (cheaper, and it keeps your context clean); keep doing the editing yourself.',
      ],
    }
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
    const main = working.find(([key]) => key === 'main')?.[1]
    const ctx = await read($, context)
    const now = Date.now()
    const windows = await read($, limits)
    // The Forge account this session runs with (only set when there are several).
    const account = await $.env.get('FORGE_ACCOUNT').catch(() => undefined)
    const plan = WINDOWS.flatMap(([kind, label]) => {
      const w = windows.find(l => l.kind === kind)
      if (!w) return []
      const percent = Math.max(0, Math.min(100, Math.round(w.percentUsed)))
      return [{ label, percent, reset: resetsIn(w.resetsAt, now) }]
    })
    const full = ctx !== null && ctx >= CONTEXT_WARN
    if (e.props.hasSurvey || (status === null && used.length === 0 && working.length === 0 && plan.length === 0 && !full))
      return next(e)
    const { Text } = $.ui.resolve(e)
    // One line of text with colored spans: it reads left to right and, when narrow,
    // wraps like any sentence instead of stacking words in columns.
    const sep = <Text dimColor> · </Text>
    return (
      <Text>
        <Text color={BRAND[0]} bold>◆ </Text>
        {[...'Forge'].map((letter, i) => (
          <Text key={`brand-${i}`} color={BRAND[i]} bold>{letter}</Text>
        ))}
        {account ? <Text color="claude">{` · ${account}`}</Text> : null}
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
              {/* The main loop's effort, next to its model while it works. */}
              {atWork && main?.model === model && main.effort ? <Text dimColor>{` ${main.effort}`}</Text> : null}
              {/* Working on its first turn: no "0" (tokens add up when a turn completes). */}
              {n.fresh > 0 ? ` ${short(n.fresh)}` : null}
              {n.cache > 0 ? <Text dimColor>{` (+${short(n.cache)} cache)`}</Text> : null}
            </Text>
          )
        })}
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
        {full ? (
          <Text>
            <Text dimColor> · </Text>
            <Text color={ctx >= 85 ? 'error' : 'warning'}>{`context ${ctx}%`}</Text>
            <Text dimColor> /compact</Text>
          </Text>
        ) : null}
        {/* Subagents at work go last: a long task wraps alone, the usage bars stay put. */}
        {subagents.map(([key, w]) => (
          <Text key={key}>
            <Text dimColor> · </Text>
            <Text color={MODEL_COLOR[w.model] ?? 'text'}>{'↳ '}</Text>
            <Text>{w.task.length > 36 ? `${w.task.slice(0, 35)}…` : w.task}</Text>
            <Text dimColor>{` (${w.model})`}</Text>
          </Text>
        ))}
      </Text>
    )
  })

  on('command.run', { command: 'forge-plans' }, async ($, e) => {
    const [action, ...rest] = e.args.trim().split(/\s+/)
    const text = rest.join(' ')
    if (action === 'add' && text) return run($, ['plans', 'add', text])
    if (action === 'show' && text) return run($, ['plans', 'show', text])
    return run($, ['plans'])
  })

  on('command.run', { command: 'forge-guard' }, async $ => {
    const result = await run($, ['guard', 'commit'])
    void refreshBand($)
    return result
  })

  on('command.run', { command: 'forge-remember' }, async ($, e) => {
    const text = e.args.trim()
    if (!text) return { text: 'Usage: /forge-remember <text>', exitCode: 1 }
    return run($, ['memory', 'add', 'note', text.slice(0, 60), text])
  })

  on('command.run', { command: 'forge-effort' }, async ($, e) => {
    const level = e.args.trim().toLowerCase()
    if (!level) {
      const now = await read($, effort)
      return { text: now ? `Effort in this session: ${now} (auto to go back).` : 'Effort: the session\'s own. Choose: low, medium, high, xhigh or max.', exitCode: 0 }
    }
    if (level === 'auto') {
      await update($, effort, () => '')
      return { text: 'Effort back to the session\'s own.', exitCode: 0 }
    }
    if (!isEffort(level)) return { text: `Unknown effort "${level}". Use low, medium, high, xhigh, max or auto.`, exitCode: 1 }
    await update($, effort, () => level)
    return { text: `Effort in this session: ${level}. It applies from your next message.`, exitCode: 0 }
  })

  on('command.run', { command: 'forge-memory' }, async ($, e) => {
    const text = e.args.trim()
    return run($, ['memory', 'search', text])
  })
}
