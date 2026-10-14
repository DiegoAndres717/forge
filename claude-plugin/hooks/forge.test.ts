import type { AgentSpawnInput, CommandRunInput, RenderPropsOf, TurnCompleteInput, TurnStepInput, TurnUsage } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'

// The engine fills in the fields a test leaves out (ids, origin, parent model…).
const spawn = (fields: Partial<AgentSpawnInput>) => fields as AgentSpawnInput
const command = (fields: Partial<CommandRunInput>) => fields as CommandRunInput
type AbovePromptProps = RenderPropsOf['AbovePrompt']
const turn = (fields: Partial<TurnCompleteInput>) =>
  ({ reason: 'answer', isAborted: false, durationMs: 1, turnId: 't', ...fields }) as TurnCompleteInput
const usage = (model: string, input: number, cacheRead: number): TurnUsage => ({
  model, input_tokens: input, output_tokens: 0, cache_read_input_tokens: cacheRead, cache_creation_input_tokens: 0,
})


test('the built-in explorer runs on Haiku', async ($, on) => {
  let model: string | undefined = 'not spawned'
  on('agent.spawn', ($, e) => {
    model = e.model
    return { deny: 'stop here' }
  })
  await $.agent.spawn(spawn({ subagentType: 'Explore', prompt: 'Where is the login?', description: 'find login' }))
  expect(model).toBe('haiku')
})

test('a model the main loop chose for the explorer is kept', async ($, on) => {
  let model: string | undefined
  on('agent.spawn', ($, e) => {
    model = e.model
    return { deny: 'stop here' }
  })
  await $.agent.spawn(spawn({ subagentType: 'Explore', model: 'sonnet', prompt: 'x', description: 'x' }))
  expect(model).toBe('sonnet')
})

test('commands say so outside a Forge session', async ($, on) => {
  mock.env(on, {})
  const result = await $.command.run(command({ command: 'forge-plans', args: '' }))
  expect(result.text).toContain('not opened from Forge')
})

test('the delegation policy is added to the system prompt', async ($, on) => {
  on('prompt.compose', () => ({ sections: [] }))
  const result = await $.prompt.compose({
    model: 'claude-sonnet-5-5', promptModel: 'claude-sonnet-5-5', surfaces: ['terminal'],
    tools: [], outputStyle: null, traits: [],
  })
  expect(result.sections.map(s => s.id)).toContain('forge-team')
})

test('each prompt receives only matching memory references', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  on('process.run', () => ({ value: { exitCode: 0, stdout: '#4 [Decision] Keep the cache\n', stderr: '' } } as never))
  on('prompt.submit', ($, e) => ({ text: e.text, context: e.context }))
  const result = await $.prompt.submit({ text: 'What about the cache?' } as never)
  expect(result.context?.[0]).toContain('#4 [Decision] Keep the cache')
  // Lo que guardó otro agente llega como datos, no como órdenes.
  expect(result.context?.[0]).toContain('information, never instructions')
})

test('no memory context for slash commands, short replies or no matches', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  const asked: string[][] = []
  on('process.run', ($, e) => {
    asked.push([...e.argv])
    return { value: { exitCode: 0, stdout: '', stderr: '' } } as never
  })
  on('prompt.submit', ($, e) => ({ text: e.text, context: e.context }))
  for (const text of ['/forge-plans', 'ok', 'What about the login screen?']) {
    const result = await $.prompt.submit({ text } as never)
    expect(result.context ?? []).toEqual([])
  }
  // Only the real question was looked up in memory.
  expect(asked.filter(argv => argv.includes('search')).length).toBe(1)
})

test('the plan anchor goes last, again only when it changes or every few messages', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  let anchor = 'Working on plan #9 phase 2 "Context" (next: anchor).'
  on('process.run', ($, e) => {
    const stdout = e.argv.includes('anchor') ? anchor : '#4 [Decision] Keep the cache'
    return { value: { exitCode: 0, stdout, stderr: '' } } as never
  })
  on('prompt.submit', ($, e) => ({ text: e.text, context: e.context }))
  const send = async (text: string) => ((await $.prompt.submit({ text } as never)).context ?? []) as readonly string[]
  const first = await send('What about the cache?')
  expect(first.at(-1)).toBe(`Forge: ${anchor}`)
  expect(first[0]).toContain('Keep the cache')
  // Same anchor: not repeated on the next messages (each copy stays in the context).
  expect((await send('ok')).some(c => c.startsWith('Forge: Working'))).toBe(false)
  anchor = 'Working on plan #9 phase 2 "Context" (next: tests).'
  expect((await send('ok')).at(-1)).toBe(`Forge: ${anchor}`)
  let repeats = 0
  for (let i = 0; i < 8; i++) if ((await send('ok')).length > 0) repeats += 1
  expect(repeats).toBe(1)
})

test('a compaction updates the single last-session note', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  const sent: { argv: string[]; stdin?: string }[] = []
  on('process.run', ($, e) => {
    sent.push({ argv: [...e.argv], stdin: (e.init as { stdin?: string } | undefined)?.stdin })
    return { value: { exitCode: 0, stdout: '#9\n', stderr: '' } } as never
  })
  on('session.compact', () => ({
    messages: [
      { role: 'user', text: 'Summary — done: A. Next: B.', toolUses: [] },
      { role: 'assistant', text: 'OK', toolUses: [] },
    ],
  }))
  await $.session.compact({ trigger: 'manual', messages: [{ role: 'user', text: 'long chat', toolUses: [] }] })
  for (let i = 0; i < 50 && sent.length === 0; i++) await Promise.resolve()
  expect(sent[0]?.argv.slice(1, 3)).toEqual(['memory', 'session'])
  expect(sent[0]?.stdin).toBe('Summary — done: A. Next: B.')
})

test('ask first adds its instruction only when it is on', async ($, on) => {
  on('prompt.compose', () => ({ sections: [] }))
  const compose = () => $.prompt.compose({
    model: 'claude-sonnet-5-5', promptModel: 'claude-sonnet-5-5', surfaces: ['terminal'],
    tools: [], outputStyle: null, traits: [],
  })
  mock.env(on, { FORGE_ASK_FIRST: '1' })
  const ids = (await compose()).sections.map(s => s.id)
  expect(ids).toContain('forge-ask-first')
  expect(ids).toContain('forge-team')
  expect(ids).not.toContain('forge-tests-first')
})

test('tests first adds its instruction only when it is on', async ($, on) => {
  on('prompt.compose', () => ({ sections: [] }))
  mock.env(on, { FORGE_TESTS_FIRST: '1' })
  const result = await $.prompt.compose({
    model: 'claude-sonnet-5-5', promptModel: 'claude-sonnet-5-5', surfaces: ['terminal'],
    tools: [], outputStyle: null, traits: [],
  })
  const section = result.sections.find(s => s.id === 'forge-tests-first')
  expect(section?.text).toContain('fails for the right reason')
  expect(result.sections.map(s => s.id)).not.toContain('forge-ask-first')
})

test('the band brands Forge with its symbol and an orange gradient', async ($, on) => {
  // The band only draws when it has something to show: a plan usage window.
  on('session.measure', ($, e) => ({ changed: e.changed }))
  await $.session.measure({
    context: { percent: 10 } as never,
    rateLimits: [{ kind: 'five_hour', percentUsed: 20 }],
    changed: ['rateLimits'],
  })
  const drawn = await $.ui.mount({
    plugin: 'forge',
    surface: 'terminal',
    component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })
  const text = JSON.stringify(await drawn.drawn())
  expect(text).toContain('◆ ')
  expect(text).toContain('#ff6a00')
  expect(text).toContain('#ffc66b')
  // The name itself in the brand orange, not in the status colours (error, warning…).
  expect(text).toContain('{"color":"#ff6a00","bold":true},"children":["F"]')
})

test('a Forge session writes one compact project header', async ($, on) => {
  mock.env(on, { FORGE_PROJECT: '/work/forge' })
  // The engine's side of what the session start asks for.
  on('session.start', ($, e) => ({ cwd: e.cwd }) as never)
  on('agent.register', () => ({ value: {} }) as never)
  on('command.register', () => ({ value: {} }) as never)
  on('process.run', () => ({ value: { exitCode: 0, stdout: 'feat/mod-brand\n', stderr: '' } }) as never)
  let header = ''
  on('ui.log', async ($, e, next) => {
    header = e.text
    return next(e)
  })
  await $.session.start({ cwd: '/work/forge', surface: 'terminal', isInteractive: true })
  // Written in the background, so the start never waits for git.
  for (let i = 0; i < 50 && !header; i++) await Promise.resolve()
  expect(header).toBe('◆ Forge · forge · feat/mod-brand')
})

test('the band shows new and cached tokens by model', async ($, on) => {
  on('turn.complete', ($, e) => ({ text: e.answer })) // the engine's side of the event
  // Two finished turns: the main loop on Opus and a subagent on Haiku.
  await $.turn.complete(turn({ answer: 'Done.', usage: usage('claude-opus-5-5', 45_000, 2_700_000) }))
  await $.turn.complete(turn({ answer: 'src/a.ts', agentId: 'a1', usage: usage('claude-haiku-5-5', 120_000, 0) }))
  const drawn = await $.ui.mount({
    plugin: 'forge',
    surface: 'terminal',
    component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })
  const text = JSON.stringify(await drawn.drawn())
  // New tokens by model in a fixed order (nothing jumps while it updates); cache reads apart.
  expect(text.indexOf('opus')).toBeLessThan(text.indexOf('haiku'))
  expect(text).toContain(' · sonnet 0') // an unused model stays listed, dim
  expect(text).toContain('120k')
  expect(text).toContain('45k')
  expect(text).toContain('(+2.7M cache)')
  // One line of text (spans inside a Text), never a box of pieces that wraps in columns.
  expect(text.startsWith('{"type":"Text"')).toBe(true)
})

test('the band shows the plan usage windows as bars', async ($, on) => {
  on('session.measure', ($, e) => ({ changed: e.changed })) // the engine's side
  const resetsAt = new Date(Date.now() + 3 * 3_600_000).toISOString()
  await $.session.measure({
    context: { percent: 10 } as never,
    rateLimits: [
      { kind: 'five_hour', percentUsed: 85, resetsAt },
      { kind: 'seven_day', percentUsed: 3 },
    ],
    changed: ['rateLimits'],
  })
  const drawn = await $.ui.mount({
    plugin: 'forge',
    surface: 'terminal',
    component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })
  const text = JSON.stringify(await drawn.drawn())
  expect(text).toContain(' 85%')
  expect(text).toContain(' 3%')
  expect(text).toContain('━'.repeat(10)) // 85% of 12 cells
  expect(text).toContain('resets in 3h') // only past 80%
  expect(text).toContain('"error"') // red past 80%
  // 3% still fills one cell, so any use shows.
  expect(text).toContain('"━"')
})

test('the band shows which model is working right now and on what', async ($, on) => {
  // The engine's side: the spawn starts subagent a1, the request and the turn finish.
  on('agent.spawn', () => ({ model: 'haiku', agentId: 'a1' }))
  on('turn.step', async function* ($, e) {
    return { turnId: e.turnId, index: e.index, answer: "", toolUses: [] } as never
  })
  on('turn.complete', ($, e) => ({ text: e.answer }))
  const mount = async () =>
    JSON.stringify(
      await (
        await $.ui.mount({
          plugin: 'forge',
          surface: 'terminal',
          component: 'AbovePrompt',
          props: { hasSurvey: false, isWorking: true } as AbovePromptProps,
        })
      ).drawn(),
    )
  await $.agent.spawn(spawn({ subagentType: 'forge:explorer', prompt: 'x', description: 'find the login' }))
  const step = (fields: Partial<TurnStepInput>) =>
    ({ turnId: 't', index: 0, messageCount: 1, ...fields }) as TurnStepInput
  // A streaming event runs as it is read.
  for (const fields of [{ model: 'claude-opus-5-5' }, { model: 'claude-haiku-5-5', agentId: 'a1' }]) {
    for await (const _chunk of $.turn.step(step(fields))) {
      // no chunks from the stub
    }
  }
  const working = await mount()
  expect(working).toContain('● ')
  expect(working).toContain('explorer: find the login')
  expect(working).toContain(' (haiku)')
  // Opus works on its first turn: no "opus 0" (tokens add up when the turn completes).
  expect(working).not.toContain('opus 0')
  // The subagent finishes: its task leaves the band.
  await $.turn.complete(turn({ answer: 'src/login.ts', agentId: 'a1', usage: usage('claude-haiku-5-5', 9_000, 0) }))
  const after = await mount()
  expect(after).not.toContain('explorer: find the login')
  expect(after).toContain('9k')
})

test('outside Forge the first message carries no Forge context', async ($, on) => {
  mock.env(on, {})
  on('prompt.context', () => ({ blocks: [{ name: 'currentDate', text: 'today' }] }))
  const result = await $.prompt.context({ blocks: [] })
  expect(result.blocks.map(b => b.name)).toEqual(['currentDate'])
})

test('the band names the Forge account when there are several', async ($, on) => {
  mock.env(on, { FORGE_ACCOUNT: 'Trabajo' })
  on('session.measure', ($, e) => ({ changed: e.changed }))
  await $.session.measure({
    context: { percent: 10 } as never,
    rateLimits: [{ kind: 'five_hour', percentUsed: 20 }],
    changed: ['rateLimits'],
  })
  const drawn = await $.ui.mount({
    plugin: 'forge',
    surface: 'terminal',
    component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })
  expect(JSON.stringify(await drawn.drawn())).toContain(' · Trabajo')
})

test('an exhausted usage window tells Forge once so it can offer another account', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  const sent: string[][] = []
  on('process.run', ($, e) => {
    sent.push([...e.argv])
    return { value: { exitCode: 0, stdout: '', stderr: '' } } as never
  })
  on('session.measure', ($, e) => ({ changed: e.changed }))
  const measure = (percentUsed: number) =>
    $.session.measure({
      context: { percent: 10 } as never,
      rateLimits: [{ kind: 'five_hour', percentUsed, resetsAt: '2026-10-10T20:00:00Z' }],
      changed: ['rateLimits'],
    })
  await measure(90)
  await measure(100)
  await measure(100)
  expect(sent.some(argv => argv.includes('agent-limits'))).toBe(true) // Forge keeps the latest
  const limits = sent.filter(argv => argv.includes('limit'))
  expect(limits.length).toBe(1)
  expect(limits[0]?.slice(0, 3)).toEqual(['/forge', 'agent-event', 'limit'])
})

test('/forge-effort changes the main loop effort and the band shows it', async ($, on) => {
  let sent: unknown
  on('turn.step', async function* ($, e) {
    if (!e.agentId) sent = e.effort
    return { turnId: e.turnId, index: e.index, answer: '', toolUses: [] } as never
  })
  const step = (fields: Partial<TurnStepInput>) =>
    ({ turnId: 't', index: 0, messageCount: 1, model: 'claude-opus-5-5', effort: 'medium', ...fields }) as TurnStepInput
  const run = async (fields: Partial<TurnStepInput> = {}) => {
    for await (const _chunk of $.turn.step(step(fields))) {
      // no chunks from the stub
    }
  }
  expect((await $.command.run(command({ command: 'forge-effort', args: 'turbo' }))).exitCode).toBe(1)
  await $.command.run(command({ command: 'forge-effort', args: 'high' }))
  await run()
  expect(sent).toBe('high')
  const drawn = JSON.stringify(await (await $.ui.mount({
    plugin: 'forge', surface: 'terminal', component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: true } as AbovePromptProps,
  })).drawn())
  expect(drawn).toContain(' high')
  // A model without effort keeps going without one; auto goes back to the session's.
  sent = 'unset'
  await run({ effort: undefined })
  expect(sent).toBeUndefined()
  await $.command.run(command({ command: 'forge-effort', args: 'auto' }))
  await run()
  expect(sent).toBe('medium')
})

test('the main loop gets one nudge to delegate after several reads in a row', async ($, on) => {
  on('tool.call', () => ({ result: {}, context: [] }) as never)
  const read = (agentId?: string) => $.tool.call({ tool: 'Read', file_path: '/x', agentId } as never)
  const nudges: number[] = []
  for (let i = 0; i < 6; i++) {
    const r = (await read()) as { context?: readonly string[] }
    nudges.push((r.context ?? []).filter(c => c.includes('forge:explorer')).length)
  }
  expect(nudges).toEqual([0, 0, 1, 0, 0, 0])
  // A subagent reading is what it is for: no nudge.
  for (let i = 0; i < 5; i++) {
    const r = (await read('a1')) as { context?: readonly string[] }
    expect((r.context ?? []).length).toBe(0)
  }
})

test('shell searches count as exploring, other commands reset the streak', async ($, on) => {
  on('tool.call', () => ({ result: {}, context: [] }) as never)
  const bash = async (command: string) => {
    const r = (await $.tool.call({ tool: 'Bash', command } as never)) as { context?: readonly string[] }
    return (r.context ?? []).filter(c => c.includes('forge:explorer')).length
  }
  expect(await bash('cargo test')).toBe(0)
  expect(await bash('rg -n "fn main" src')).toBe(0)
  expect(await bash("sed -n '1,80p' src/main.rs")).toBe(0)
  expect(await bash('cd crates && grep -rn Store .')).toBe(1)
  // Building resets the streak: three more searches before the next nudge.
  expect(await bash('cargo build')).toBe(0)
  expect(await bash('cat README.md')).toBe(0)
})

test('the main loop running a whole suite or build itself is nudged once to forge:reviewer', async ($, on) => {
  on('tool.call', () => ({ result: {}, context: [] }) as never)
  on('prompt.submit', ($, e) => ({ text: e.text, context: e.context }))
  const bash = async (command: string, agentId?: string) => {
    const r = (await $.tool.call({ tool: 'Bash', command, agentId } as never)) as { context?: readonly string[] }
    return (r.context ?? []).filter(c => c.includes('forge:reviewer')).length
  }
  expect(await bash('git status')).toBe(0)
  expect(await bash('cargo test --workspace --no-fail-fast')).toBe(1)
  // Once per message of yours: the second run isn't nagged.
  expect(await bash('npx tsc --noEmit')).toBe(0)
  await $.prompt.submit({ text: 'next' } as never)
  expect(await bash('cd web && npm run build')).toBe(1)
  // The reviewer itself runs suites: that is its job.
  await $.prompt.submit({ text: 'again' } as never)
  expect(await bash('pytest', 'a1')).toBe(0)
})

test('the band shows how much exploration was delegated', async ($, on) => {
  on('tool.call', () => ({ result: {}, context: [] }) as never)
  on('session.compact', () => ({ messages: [{ role: 'user', text: 'resumen', toolUses: [] }] }))
  const read = (agentId?: string) => $.tool.call({ tool: 'Read', file_path: '/a', agentId } as never)
  await read()
  await read()
  for (const _ of [1, 2, 3, 4]) await read('explorer-1')
  const drawn = JSON.stringify(await (await $.ui.mount({
    plugin: 'forge', surface: 'terminal', component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })).drawn())
  // 4 de 6 lecturas las hizo un subagente: lo que se deja de releer en cada turno.
  expect(drawn).toContain('delegated 67%')
  // Un rg por la shell también es explorar; tras compactar se cuenta de nuevo.
  await $.tool.call({ tool: 'Bash', command: 'rg login src', agentId: 'explorer-1' } as never)
  await $.session.compact({ trigger: 'manual', messages: [{ role: 'user', text: 'resumen', toolUses: [] }] } as never)
  for (const _ of [1, 2, 3, 4, 5]) await read()
  const after = JSON.stringify(await (await $.ui.mount({
    plugin: 'forge', surface: 'terminal', component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })).drawn())
  // Sin reiniciar serían 5 de 12 (42 %): cuenta solo lo de después.
  expect(after).toContain('delegated 0%')
})

test('Forge offers to compact by tokens, and again each step', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  const offers: string[] = []
  on('process.run', ($, e) => {
    if (e.argv.includes('compact')) offers.push((e.init as { stdin?: string } | undefined)?.stdin ?? '')
    return { value: { exitCode: 0, stdout: '', stderr: '' } } as never
  })
  on('session.measure', ($, e) => ({ changed: e.changed }))
  const measure = (percent: number) =>
    $.session.measure({ context: { percent, window: 1_000_000 } as never, rateLimits: [], changed: ['context'] })
  // En una ventana de 1M se ofrece desde 400k, no al 60 % (600k).
  await measure(39)
  expect(offers.length).toBe(0)
  await measure(41)
  expect(offers.length).toBe(1)
  expect(offers[0]).toContain('410k')
  await measure(45)
  expect(offers.length).toBe(1)
  // Si se dejó para luego, vuelve a ofrecerse 100k más tarde.
  await measure(51)
  expect(offers.length).toBe(2)
})

test('the band warns when the context is getting full', async ($, on) => {
  on('session.measure', ($, e) => ({ changed: e.changed }))
  await $.session.measure({
    context: { percent: 78, window: 200_000 } as never,
    rateLimits: [],
    changed: ['context'],
  })
  const drawn = JSON.stringify(await (await $.ui.mount({
    plugin: 'forge', surface: 'terminal', component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })).drawn())
  expect(drawn).toContain('context 78%')
  expect(drawn).toContain('/compact')
})

test('on Windows a dangerous Bash command asks first; others keep their verdict', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge', FORGE_GUARD_COMMANDS: '1' })
  const asked: string[][] = []
  on('process.run', ($, e) => {
    asked.push([...e.argv])
    const danger = e.argv.includes('danger') && e.argv.some(a => a.includes('--force'))
    return { value: { exitCode: danger ? 0 : 1, stdout: danger ? 'reescribe la historia del remoto' : '', stderr: '' } } as never
  })
  on('tool.check', () => ({ decision: 'allow' as const }))
  const check = (command: string) => $.tool.check({ tool: 'Bash', input: { command } } as never)
  const push = await check('git status && git push --force')
  expect(push.decision).toBe('ask')
  expect(push.reason).toContain('reescribe la historia')
  expect((await check('git status')).decision).toBe('allow')
  // No watched program in the line: Forge isn't even asked.
  const before = asked.length
  expect((await check('npm test')).decision).toBe('allow')
  expect(asked.length).toBe(before)
})

test('outside Windows the shims already guard Bash: Forge is not asked', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  const asked: string[][] = []
  on('process.run', ($, e) => {
    asked.push([...e.argv])
    return { value: { exitCode: 0, stdout: 'x', stderr: '' } } as never
  })
  on('tool.check', () => ({ decision: 'allow' as const }))
  const verdict = await $.tool.check({ tool: 'Bash', input: { command: 'git push --force' } } as never)
  expect(verdict.decision).toBe('allow')
  expect(asked.some(a => a.includes('danger'))).toBe(false)
})

test('reading a file with secrets asks first; other files keep their verdict', async ($, on) => {
  mock.env(on, { FORGE_BIN: '/forge' })
  const asked: string[][] = []
  on('process.run', ($, e) => {
    asked.push([...e.argv])
    const secret = e.argv.some(a => a.includes('.env') && !a.includes('.example'))
    return { value: { exitCode: secret ? 0 : 1, stdout: secret ? 'lee un archivo con secretos (.env)' : '', stderr: '' } } as never
  })
  on('tool.check', () => ({ decision: 'allow' as const }))
  const check = (tool: string, input: object) => $.tool.check({ tool, input } as never)
  const env = await check('Read', { file_path: '/p/.env' })
  expect(env.decision).toBe('ask')
  expect(env.reason).toContain('.env')
  expect((await check('Bash', { command: 'grep KEY .env' })).decision).toBe('ask')
  expect((await check('Read', { file_path: '/p/.env.example' })).decision).toBe('allow')
  // No hint in the path: Forge isn't even asked.
  const before = asked.length
  expect((await check('Read', { file_path: '/p/src/main.rs' })).decision).toBe('allow')
  expect(asked.length).toBe(before)
})
