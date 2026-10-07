import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createEnvironmentInjector } from '@angular/core'
import { provideMcpRuntime } from '../guest-js/angular'
import {
  McpError,
  McpRuntime,
  BrowserTransport,
  defineTool,
  Module,
  Tool,
  type BridgeEvent,
  type Completion,
  type RuntimeTransport,
  type ToolContext,
} from '../guest-js/index'

const inputSchema = {
  type: 'object',
  properties: { value: { type: 'string' } },
  required: ['value'],
  additionalProperties: false,
}
const outputSchema = {
  type: 'object',
  properties: { value: { type: 'string' } },
  required: ['value'],
  additionalProperties: false,
}
const definition = {
  name: 'test.echo',
  description: 'Echo a value',
  inputSchema,
  outputSchema,
}
const isCode = (code: string) => (error: unknown) =>
  error instanceof McpError && error.code === code

@Module('service')
class ServiceTools {
  constructor(private readonly prefix: string) {}
  @Tool({ ...definition, name: 'echo' })
  echo(input: { value: string }) {
    return { value: this.prefix + input.value }
  }
}

test('Module and Tool preserve the DI instance and tool metadata', async () => {
  const runtime = new McpRuntime().registerModule(new ServiceTools('bound:'))
  assert.equal(runtime.tools[0].name, 'service.echo')
  await runtime.start()
  assert.deepEqual(await runtime.invoke('service.echo', { value: 'hello' }), {
    value: 'bound:hello',
  })
  await runtime.stop()
})

test('registration is atomic and metadata cannot mutate the registry', () => {
  const runtime = new McpRuntime()
  const tool = defineTool({ ...definition, handler: (input) => input })
  assert.throws(() => runtime.register(tool, tool), isCode('DUPLICATE_TOOL'))
  assert.equal(runtime.tools.length, 0)
  runtime.register(tool)
  runtime.tools[0].name = 'changed'
  assert.equal(runtime.tools[0].name, 'test.echo')
})

test('invalid arguments never reach the handler', async () => {
  let calls = 0
  const runtime = new McpRuntime().register(
    defineTool({
      ...definition,
      handler: (input) => {
        calls++
        return input
      },
    }),
  )
  await runtime.start()
  await assert.rejects(
    runtime.invoke('test.echo', { value: 1 }),
    isCode('INVALID_ARGUMENT'),
  )
  assert.equal(calls, 0)
  await assert.rejects(runtime.invoke('missing', {}), isCode('NOT_FOUND'))
  await runtime.stop()
})

test('invalid outputs and private exceptions produce controlled errors', async () => {
  const runtime = new McpRuntime().register(
    defineTool({ ...definition, handler: () => ({ value: 1 }) }),
    defineTool({
      ...definition,
      name: 'test.fail',
      handler: () => {
        throw new Error('private secret')
      },
    }),
  )
  await runtime.start()
  await assert.rejects(
    runtime.invoke('test.echo', { value: 'a' }),
    isCode('INVALID_RESULT'),
  )
  await assert.rejects(
    runtime.invoke('test.fail', { value: 'a' }),
    (error: McpError) =>
      error.code === 'HANDLER_ERROR' && !error.message.includes('secret'),
  )
  await runtime.stop()
})

test('timeout aborts the handler and ignores its late result', async () => {
  let signal: AbortSignal | undefined
  const events: string[] = []
  const runtime = new McpRuntime().register(
    defineTool({
      ...definition,
      policy: { timeoutMs: 15 },
      handler: async (input, context) => {
        signal = context.signal
        await new Promise((resolve) => setTimeout(resolve, 50))
        return input
      },
    }),
  )
  runtime.onEvent((event) => events.push(event.state))
  await runtime.start()
  await assert.rejects(
    runtime.invoke('test.echo', { value: 'a' }),
    isCode('TIMEOUT'),
  )
  assert.equal(signal?.aborted, true)
  await new Promise((resolve) => setTimeout(resolve, 60))
  assert.deepEqual(events, ['started', 'failed'])
  assert.equal(runtime.pendingCount, 0)
  await runtime.stop()
})

test('cancellation, stop and restart release pending calls', async () => {
  let started!: () => void
  let ready = new Promise<void>((resolve) => {
    started = resolve
  })
  const runtime = new McpRuntime().register(
    defineTool({
      ...definition,
      handler: () => {
        started()
        return new Promise(() => {})
      },
    }),
  )
  await runtime.start()
  const controller = new AbortController()
  const call = runtime.invoke(
    'test.echo',
    { value: 'a' },
    { signal: controller.signal },
  )
  const assertion = assert.rejects(call, isCode('CANCELLED'))
  await ready
  controller.abort()
  await assertion
  assert.equal(runtime.pendingCount, 0)
  ready = new Promise<void>((resolve) => {
    started = resolve
  })
  const next = assert.rejects(
    runtime.invoke('test.echo', { value: 'b' }),
    isCode('NOT_READY'),
  )
  await ready
  await runtime.stop()
  await next
  await assert.rejects(
    runtime.invoke('test.echo', { value: 'c' }),
    isCode('NOT_READY'),
  )
  await runtime.start()
  assert.equal(runtime.isStarted, true)
  await runtime.stop()
})

test('permissions are enforced by the adapter and cannot be granted by beforeCall', async () => {
  let called = false
  const tool = defineTool({
    ...definition,
    policy: { permissions: ['example:read'] },
    handler: (input: unknown) => {
      called = true
      return input
    },
  })
  const runtime = new McpRuntime({ beforeCall: () => {} }).register(tool)
  await runtime.start()
  await assert.rejects(
    runtime.invoke('test.echo', { value: 'a' }),
    isCode('UNAUTHORIZED'),
  )
  assert.equal(called, false)
  assert.deepEqual(runtime.tools, [])
  await runtime.stop()
  const allowed = new McpRuntime({
    transport: new BrowserTransport({ authorize: (caller, tool) => {
      assert.equal(caller.source, 'local')
      assert.deepEqual(tool.policy?.permissions, ['example:read'])
      return true
    } }),
  }).register(tool)
  await allowed.start()
  assert.deepEqual(await allowed.invoke('test.echo', { value: 'a' }), {
    value: 'a',
  })
  await allowed.stop()
})

test('desktop invoke crosses the transport and never executes a local fallback', async () => {
  const transport = new TestTransport()
  let executions = 0
  const runtime = new McpRuntime({ transport }).register(defineTool({
    ...definition,
    handler: () => { executions++; throw new Error('must only execute a dispatch') },
  }))
  await runtime.start()
  assert.deepEqual(await runtime.invoke('test.echo', { value: 'from-rust' }), { value: 'from-rust' })
  assert.equal(transport.invokeCount, 1)
  transport.invoke = async () => { throw new McpError('UNAUTHORIZED', 'Host denied access') }
  await assert.rejects(runtime.invoke('test.echo', { value: 'denied' }), isCode('UNAUTHORIZED'))
  assert.equal(executions, 0)
  await runtime.stop()
})

test('start becomes ready only after the adapter handshake and stop waits for it', async () => {
  const transport = new TestTransport()
  let ready!: () => void
  let disconnected = 0
  transport.connect = async () => {
    transport.connectCount++
    await new Promise<void>((resolve) => { ready = resolve })
    return { tools: [], pendingCount: 0 }
  }
  transport.disconnect = async () => { disconnected++ }
  const runtime = new McpRuntime({ transport })
  const starting = runtime.start()
  const duplicate = runtime.start()
  assert.equal(runtime.isStarted, false)
  await assert.rejects(runtime.invoke('missing', {}), isCode('NOT_READY'))
  assert.throws(() => runtime.register(), isCode('BUSY'))
  const stopping = runtime.stop()
  ready()
  await Promise.all([starting, duplicate, stopping])
  assert.equal(transport.connectCount, 1)
  assert.equal(disconnected, 1)
  assert.equal(runtime.isStarted, false)
})

test('synchronous handler blocking past the deadline cannot publish success', async () => {
  let called = false
  let signal: AbortSignal | undefined
  const runtime = new McpRuntime().register(defineTool({
    ...definition, policy: { timeoutMs: 80 },
    handler: (input, context) => {
      called = true
      signal = context.signal
      const until = performance.now() + 120
      while (performance.now() < until) { /* Simulate synchronous business work. */ }
      return input
    },
  }))
  const states: string[] = []
  runtime.onEvent((event) => states.push(event.state))
  await runtime.start()
  await assert.rejects(runtime.invoke('test.echo', { value: 'late' }), isCode('TIMEOUT'))
  assert.equal(called, true)
  assert.equal((signal?.reason as McpError).code, 'TIMEOUT')
  assert.deepEqual(states, ['started', 'failed'])
  await runtime.stop()
})

test('a ready snapshot cannot overwrite a newer channel state', async () => {
  const transport = new TestTransport()
  transport.connect = async (_definitions, receive) => {
    receive({ type: 'state', sessionId: 's', pendingCount: 1, event: {
      requestId: 'during-ready', name: 'test.echo', state: 'started', source: 'mcp', durationMs: 0,
      caller: { principal: 'mcp:local', source: 'mcp' },
    } })
    return { tools: [], pendingCount: 0 }
  }
  const runtime = new McpRuntime({ transport })
  await runtime.start()
  assert.equal(runtime.pendingCount, 1)
  await runtime.stop()
})

test('pre-dispatch rejection is included in runtime events', async () => {
  const runtime = new McpRuntime().register(defineTool({ ...definition, handler: (input) => input }))
  const errors: string[] = []
  runtime.onEvent((event) => { if (event.errorCode) errors.push(event.errorCode) })
  await runtime.start()
  await assert.rejects(runtime.invoke('test.echo', { value: 7 }), isCode('INVALID_ARGUMENT'))
  await assert.rejects(runtime.invoke('missing', {}), isCode('NOT_FOUND'))
  assert.deepEqual(errors, ['INVALID_ARGUMENT', 'NOT_FOUND'])
  await runtime.stop()
})

test('cancellation from an admission observer never dispatches a handler', async () => {
  const controller = new AbortController()
  let executions = 0
  const runtime = new McpRuntime().register(defineTool({ ...definition,
    handler: (input) => { executions++; return input },
  }))
  runtime.onEvent((event) => { if (event.state === 'started') controller.abort() })
  await runtime.start()
  await assert.rejects(runtime.invoke('test.echo', { value: 'cancelled' }, { signal: controller.signal }), isCode('CANCELLED'))
  await Promise.resolve()
  assert.equal(executions, 0)
  assert.equal(runtime.pendingCount, 0)
  await runtime.stop()
})

test('Angular binds existing useValue and useFactory providers', async () => {
  for (const provider of [
    { provide: ServiceTools, useValue: new ServiceTools('value:') },
    { provide: ServiceTools, useFactory: () => new ServiceTools('factory:') },
  ]) {
    const injector = createEnvironmentInjector([provider, provideMcpRuntime({ modules: [ServiceTools] })], null!)
    const runtime = injector.get(McpRuntime)
    await runtime.start()
    const expected = injector.get(ServiceTools).echo({ value: 'kept' })
    assert.deepEqual(await runtime.invoke('service.echo', { value: 'kept' }), expected)
    await runtime.stop()
    injector.destroy()
  }
})

test('retired fields and malformed policies are rejected without aliases', () => {
  for (const patch of [
    { readOnly: true }, { timeoutMs: 100 }, { permission: 'old:read' },
    { annotations: [] }, { policy: [] }, { policy: { maxConcurrency: null } },
    { policy: { permissions: ['same', 'same'] } },
  ]) {
    const runtime = new McpRuntime()
    assert.throws(() => runtime.register(defineTool({ ...definition, ...patch, handler: (input: unknown) => input } as never)), isCode('INVALID_DEFINITION'))
    assert.equal(runtime.tools.length, 0)
  }
})

test('external schema references and non-JSON output are rejected', async () => {
  const runtime = new McpRuntime()
  assert.throws(
    () =>
      runtime.register(
        defineTool({
          ...definition,
          inputSchema: { type: 'object', $ref: 'https://example.com/schema' },
          handler: (input) => input,
        }),
      ),
    isCode('INVALID_DEFINITION'),
  )
  runtime.register(
    defineTool({ ...definition, handler: () => ({ value: undefined }) }),
  )
  await runtime.start()
  await assert.rejects(
    runtime.invoke('test.echo', { value: 'a' }),
    isCode('INVALID_RESULT'),
  )
  await runtime.stop()
})

class TestTransport implements RuntimeTransport {
  readonly mode = 'tauri' as const
  invokeCount = 0
  async invoke(_requestId: string, _name: string, args: import('../guest-js/types').JsonValue) {
    this.invokeCount++
    return args
  }
  receive!: (event: BridgeEvent) => void
  replies: Completion[] = []
  connectCount = 0
  async connect(_definitions: unknown, receive: (event: BridgeEvent) => void) {
    this.receive = receive
    this.connectCount++
    return { tools: [], pendingCount: 0 }
  }
  async resolve(
    _sessionId: string,
    _requestId: string,
    completion: Completion,
  ) {
    this.replies.push(completion)
  }
  async disconnect() {}
}

test('bridge dispatch uses the same annotated methods and source context', async () => {
  const transport = new TestTransport()
  let context: ToolContext | undefined
  const runtime = new McpRuntime({ transport }).register(
    defineTool({
      ...definition,
      handler: (input, ctx) => {
        context = ctx
        return input
      },
    }),
  )
  await Promise.all([runtime.start(), runtime.start()])
  assert.equal(transport.connectCount, 1)
  transport.receive({
    type: 'call',
    sessionId: 's',
    requestId: 'mcp-call',
    name: 'test.echo',
    arguments: { value: 'mcp' },
    deadline: Date.now() + 1000,
    caller: { principal: 'mcp:local', source: 'mcp' },
  })
  transport.receive({
    type: 'call',
    sessionId: 's',
    requestId: 'mcp-call',
    name: 'test.echo',
    arguments: { value: 'duplicate' },
    deadline: Date.now() + 1000,
    caller: { principal: 'mcp:local', source: 'mcp' },
  })
  await new Promise((resolve) => setTimeout(resolve, 10))
  assert.equal(context?.source, 'mcp')
  assert.deepEqual(transport.replies, [
    { status: 'success', result: { value: 'mcp' } },
  ])
  await runtime.stop()
})

test('capacity is bounded and stop drains all accepted calls', async () => {
  const runtime = new McpRuntime().register(
    defineTool({ ...definition, handler: () => new Promise(() => {}) }),
  )
  await runtime.start()
  const calls = Array.from({ length: 64 }, () =>
    assert.rejects(
      runtime.invoke('test.echo', { value: 'wait' }),
      isCode('NOT_READY'),
    ),
  )
  assert.equal(runtime.pendingCount, 64)
  await assert.rejects(
    runtime.invoke('test.echo', { value: 'overflow' }),
    isCode('BUSY'),
  )
  await runtime.stop()
  await Promise.all(calls)
  assert.equal(runtime.pendingCount, 0)
})

test('a failed transport connection can be retried', async () => {
  const transport = new TestTransport()
  let attempts = 0
  transport.connect = async () => {
    if (++attempts === 1) throw new McpError('NOT_READY', 'Unavailable')
    return { tools: [], pendingCount: 0 }
  }
  const runtime = new McpRuntime({ transport }).register(
    defineTool({ ...definition, handler: (input) => input }),
  )
  await assert.rejects(runtime.start(), isCode('NOT_READY'))
  assert.equal(runtime.isStarted, false)
  await runtime.start()
  assert.equal(attempts, 2)
  assert.deepEqual(await runtime.invoke('test.echo', { value: 'retry' }), {
    value: 'retry',
  })
  await runtime.stop()
})

test('malformed metadata from JavaScript callers gets a stable error', () => {
  for (const patch of [
    { description: null },
    { inputSchema: null },
    { policy: { timeoutMs: null } },
    { annotations: { readOnlyHint: 'true' } },
    { policy: { permissions: [''] } },
    { description: '中'.repeat(1400) },
  ]) {
    const runtime = new McpRuntime()
    assert.throws(
      () =>
        runtime.register(
          defineTool({
            ...definition,
            ...patch,
            handler: (input: unknown) => input,
          } as never),
        ),
      isCode('INVALID_DEFINITION'),
    )
    assert.equal(runtime.tools.length, 0)
  }
})
