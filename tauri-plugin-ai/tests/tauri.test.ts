import { test } from 'node:test'
import assert from 'node:assert/strict'
import type { Channel } from '@tauri-apps/api/core'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { AiRuntime, defineTool, AiError } from '../guest-js/index'
import { TauriTransport } from '../guest-js/tauri'
import type { BridgeEvent, Completion } from '../guest-js/types'

const definition = {
  name: 'test.echo', description: 'Echo',
  inputSchema: { type: 'object' }, outputSchema: { type: 'object' },
}
function withWindow(): () => void {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'window')
  Object.defineProperty(globalThis, 'window', { value: { crypto }, configurable: true })
  return () => {
    clearMocks()
    if (previous) Object.defineProperty(globalThis, 'window', previous)
    else Reflect.deleteProperty(globalThis, 'window')
  }
}

test('Tauri adapter performs v2 registration and ready, and invoke returns through IPC', async () => {
  const restore = withWindow()
  let channel!: Channel<BridgeEvent>
  let finish!: (value: unknown) => void
  let requestId = ''
  const commands: string[] = []
  const runtime = new AiRuntime({ transport: new TauriTransport() }).register(defineTool({
    ...definition,
    handler: (input, context) => {
      assert.equal(context.caller.principal, 'app:trusted')
      assert.equal(context.source, 'local')
      return input
    },
  }))
  mockIPC((command, payload) => {
    const args = payload as Record<string, unknown>
    commands.push(command)
    switch (command) {
      case 'plugin:ai|runtime_connect':
        assert.equal(args.protocolVersion, 2)
        channel = args.onEvent as Channel<BridgeEvent>
        return { sessionId: 's' }
      case 'plugin:ai|runtime_ready':
        assert.equal(args.sessionId, 's')
        assert.equal(runtime.isStarted, false)
        return { tools: [definition], pendingCount: 0 }
      case 'plugin:ai|runtime_invoke': {
        assert.equal(args.caller, undefined)
        assert.equal(args.source, undefined)
        requestId = args.requestId as string
        const promise = new Promise((resolve) => { finish = resolve })
        channel.onmessage({ type: 'call', sessionId: 's', requestId, name: args.name as string,
          arguments: args.arguments, deadline: Date.now() + 1000,
          caller: { principal: 'app:trusted', source: 'local' },
        })
        return promise
      }
      case 'plugin:ai|runtime_resolve': {
        const completion = args.completion as Completion
        assert.equal(args.requestId, requestId)
        assert.equal(completion.status, 'success')
        if (completion.status === 'success') finish(completion.result)
        return
      }
      case 'plugin:ai|runtime_disconnect': return
      default: throw new Error('Unexpected IPC command: ' + command)
    }
  })
  try {
    await runtime.start()
    assert.deepEqual(await runtime.invoke('test.echo', { value: 42 }), { value: 42 })
    await runtime.stop()
    assert.deepEqual(commands, [
      'plugin:ai|runtime_connect', 'plugin:ai|runtime_ready',
      'plugin:ai|runtime_invoke', 'plugin:ai|runtime_resolve', 'plugin:ai|runtime_disconnect',
    ])
  } finally { restore() }
})

test('Tauri cancellation is retried when abort precedes native admission', async () => {
  const restore = withWindow()
  let channel!: Channel<BridgeEvent>
  let requestId = ''
  let admitted = false
  let cancels = 0
  let rejectCall!: (error: unknown) => void
  const runtime = new AiRuntime({ transport: new TauriTransport() })
  mockIPC(async (command, payload) => {
    const args = payload as Record<string, unknown>
    switch (command) {
      case 'plugin:ai|runtime_connect':
        channel = args.onEvent as Channel<BridgeEvent>
        return { sessionId: 's' }
      case 'plugin:ai|runtime_ready': return { tools: [], pendingCount: 0 }
      case 'plugin:ai|runtime_invoke':
        requestId = args.requestId as string
        return new Promise((_resolve, reject) => { rejectCall = reject })
      case 'plugin:ai|runtime_cancel':
        cancels++
        if (!admitted) throw { code: 'STALE_REQUEST', message: 'Not yet admitted' }
        await Promise.resolve()
        rejectCall({ code: 'CANCELLED', message: 'Call cancelled' })
        return
      case 'plugin:ai|runtime_disconnect': return
      default: throw new Error('Unexpected IPC command: ' + command)
    }
  })
  try {
    await runtime.start()
    const controller = new AbortController()
    const call = runtime.invoke('test.echo', {}, { signal: controller.signal })
    const assertion = assert.rejects(call, (e: unknown) => e instanceof AiError && e.code === 'CANCELLED')
    controller.abort()
    admitted = true
    channel.onmessage({ type: 'state', sessionId: 's', pendingCount: 1, event: {
      requestId, name: 'test.echo', state: 'started', source: 'local', durationMs: 0,
      caller: { principal: 'app:trusted', source: 'local' },
    } })
    await assertion
    assert.equal(cancels, 2)
    await runtime.stop()
  } finally { restore() }
})
