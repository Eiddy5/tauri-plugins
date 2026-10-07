import { Channel, invoke } from '@tauri-apps/api/core'
import { McpError } from './errors'
import type { BridgeEvent, Completion, ErrorData, JsonValue, RuntimeSnapshot, RuntimeTransport, ToolOptions } from './types'

function ipcError(error: unknown): McpError {
  if (error && typeof error === 'object' && 'code' in error && 'message' in error) {
    const data = error as ErrorData
    return new McpError(data.code, data.message, data.details)
  }
  return new McpError('TRANSPORT_ERROR', 'Tauri runtime request failed')
}

/** Only this entry imports Tauri. Desktop calls always enter Rust. */
export class TauriTransport implements RuntimeTransport {
  readonly mode = 'tauri' as const
  private sessionId?: string
  private channel?: Channel<BridgeEvent>
  private readonly localCalls = new Map<string, { aborted: boolean }>()

  async connect(definitions: ToolOptions[], receive: (event: BridgeEvent) => void): Promise<RuntimeSnapshot> {
    if (this.channel) throw new McpError('BUSY', 'Runtime is already connected')
    const queued: BridgeEvent[] = []
    const channel = new Channel<BridgeEvent>()
    this.channel = channel
    const deliver = (event: BridgeEvent): void => {
      if (event.sessionId !== this.sessionId) return
      // An abort may race IPC admission. Retry when Rust confirms admission.
      if (event.type === 'state' && event.event.state === 'started' &&
        this.localCalls.get(event.event.requestId)?.aborted)
        this.cancel(event.event.requestId)
      receive(event)
    }
    channel.onmessage = (event) => {
      if (this.channel !== channel) return
      if (!this.sessionId) queued.push(event)
      else deliver(event)
    }
    try {
      const response = await invoke<{ sessionId: string }>('plugin:mcp|runtime_connect', {
        protocolVersion: 2, definitions, onEvent: channel,
      })
      this.sessionId = response.sessionId
      for (const event of queued) deliver(event)
      return await invoke<RuntimeSnapshot>('plugin:mcp|runtime_ready', { sessionId: response.sessionId })
    } catch (error) { throw ipcError(error) }
  }

  async invoke(requestId: string, name: string, arguments_: JsonValue, signal?: AbortSignal): Promise<JsonValue> {
    if (!this.sessionId) throw new McpError('NOT_READY', 'Runtime is not connected')
    if (signal?.aborted) throw new McpError('CANCELLED', 'Cancelled before dispatch')
    const call = { aborted: false }
    const abort = (): void => { call.aborted = true; this.cancel(requestId) }
    this.localCalls.set(requestId, call)
    signal?.addEventListener('abort', abort, { once: true })
    try {
      return await invoke<JsonValue>('plugin:mcp|runtime_invoke', { sessionId: this.sessionId, requestId, name, arguments: arguments_ })
    } catch (error) { throw ipcError(error) }
    finally {
      signal?.removeEventListener('abort', abort)
      this.localCalls.delete(requestId)
    }
  }
  private cancel(requestId: string): void {
    void invoke('plugin:mcp|runtime_cancel', { requestId }).catch(() => { /* Admission or terminal state may win this race. */ })
  }
  async resolve(sessionId: string, requestId: string, completion: Completion): Promise<void> {
    if (sessionId !== this.sessionId) return
    await invoke('plugin:mcp|runtime_resolve', { sessionId, requestId, completion })
  }
  async disconnect(): Promise<void> {
    const sessionId = this.sessionId
    // Keep the channel alive until Rust has revoked admission and sent cancels.
    try {
      if (sessionId) await invoke('plugin:mcp|runtime_disconnect', { sessionId })
    } finally { this.sessionId = undefined; this.channel = undefined }
  }
}
