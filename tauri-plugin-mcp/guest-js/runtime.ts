import { toolsFromInstance } from './decorators'
import { McpError } from './errors'
import { BrowserTransport } from './browser'
import { JsExecutor } from './executor'
import { ToolRegistry } from './registry'
import { jsonCopy } from './schema'
import type { BridgeEvent, DefinedTool, JsonValue, RuntimeEvent, RuntimeOptions, RuntimeTransport, ToolOptions } from './types'

/** Application facade. Every invoke crosses the selected runtime adapter. */
export class McpRuntime {
  private readonly registry = new ToolRegistry()
  private readonly executor: JsExecutor
  private readonly transport: RuntimeTransport
  private readonly listeners = new Set<(event: RuntimeEvent) => void>()
  private accepted?: ToolOptions[]
  private pending = 0
  private stateVersion = 0
  private started = false
  private starting?: Promise<void>
  private stopping?: Promise<void>

  constructor(options: RuntimeOptions = {}) {
    this.transport = options.transport ?? new BrowserTransport()
    this.executor = new JsExecutor(this.registry, options.beforeCall)
  }

  registerModule(instance: object): this { return this.register(...toolsFromInstance(instance)) }
  register(...tools: DefinedTool[]): this {
    if (this.started || this.starting || this.stopping)
      throw new McpError('BUSY', 'Register tools before starting')
    this.registry.register(tools)
    return this
  }
  get tools(): ToolOptions[] { return this.accepted ? JSON.parse(JSON.stringify(this.accepted)) : this.registry.definitions }
  get mode(): RuntimeTransport['mode'] { return this.transport.mode }
  get isStarted(): boolean { return this.started }
  get pendingCount(): number { return this.pending }
  onEvent(listener: (event: RuntimeEvent) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  async start(): Promise<void> {
    if (this.stopping) await this.stopping
    if (this.started) return
    if (this.starting) return this.starting
    this.starting = (async () => {
      try {
        const stateVersion = this.stateVersion
        const snapshot = await this.transport.connect(this.registry.definitions, (event) => this.receive(event))
        this.accepted = snapshot.tools
        if (this.stateVersion === stateVersion) this.pending = snapshot.pendingCount
        this.started = true
      } catch (error) {
        await this.transport.disconnect().catch(() => {})
        this.executor.stop()
        throw error
      } finally { this.starting = undefined }
    })()
    return this.starting
  }

  async stop(): Promise<void> {
    if (this.stopping) return this.stopping
    this.stopping = (async () => {
      try {
        await this.starting?.catch(() => {})
        this.started = false
        await this.transport.disconnect()
      } finally {
        this.executor.stop()
        this.accepted = undefined
        this.pending = 0
        this.stopping = undefined
      }
    })()
    return this.stopping
  }

  async invoke(name: string, arguments_: unknown, options: { signal?: AbortSignal } = {}): Promise<JsonValue> {
    if (!this.started || this.stopping) throw new McpError('NOT_READY', 'Runtime is not started')
    return this.transport.invoke(crypto.randomUUID(), name, jsonCopy(arguments_, 'INVALID_ARGUMENT'), options.signal)
  }

  private receive(event: BridgeEvent): void {
    if (event.type === 'state') {
      this.stateVersion++
      this.pending = event.pendingCount
      for (const listener of this.listeners) {
        try { listener(JSON.parse(JSON.stringify(event.event))) } catch { /* Observers cannot change a call. */ }
      }
    } else {
      this.executor.receive(event, (completion) => this.transport.resolve(event.sessionId, event.requestId, completion))
    }
  }
}
