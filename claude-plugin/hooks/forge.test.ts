import type { AgentSpawnInput, CommandRunInput, RenderPropsOf, TurnCompleteInput, TurnUsage } from 'claude-code'
import { expect, mock, test } from 'claude-code/testing'

// The engine fills in the fields a test leaves out (ids, origin, parent model…).
const spawn = (fields: Partial<AgentSpawnInput>) => fields as AgentSpawnInput
const command = (fields: Partial<CommandRunInput>) => fields as CommandRunInput
type AbovePromptProps = RenderPropsOf['AbovePrompt']
const turn = (fields: Partial<TurnCompleteInput>) =>
  ({ reason: 'answer', isAborted: false, durationMs: 1, turnId: 't', ...fields }) as TurnCompleteInput
const usage = (model: string, input: number): TurnUsage => ({
  model, input_tokens: input, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0,
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
  const result = await $.command.run(command({ command: 'ideas', args: '' }))
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

test('the band shows the session tokens by model', async ($, on) => {
  on('turn.complete', ($, e) => ({ text: e.answer })) // the engine's side of the event
  // Two finished turns: the main loop on Opus and a subagent on Haiku.
  await $.turn.complete(turn({ answer: 'Done.', usage: usage('claude-opus-5-5', 45_000) }))
  await $.turn.complete(turn({ answer: 'src/a.ts', agentId: 'a1', usage: usage('claude-haiku-5-5', 120_000) }))
  const drawn = await $.ui.mount({
    plugin: 'forge',
    surface: 'terminal',
    component: 'AbovePrompt',
    props: { hasSurvey: false, isWorking: false } as AbovePromptProps,
  })
  expect(JSON.stringify(await drawn.drawn())).toContain('session haiku 120k · opus 45k')
})
