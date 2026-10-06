import type { ValidateFunction } from 'ajv'
import { AiError, errorData } from './errors'
import { compileSchema, jsonCopy, LIMITS, validateDefinition, validateValue } from './schema'
import type { BridgeEvent, Caller, Completion, JsonValue, RuntimeTransport, ToolOptions } from './types'

export interface BrowserOptions {
  authorize?: (caller: Caller, tool: Readonly<ToolOptions>) => boolean
}
interface Pending {
  name: string
  expires: number
  output: ValidateFunction
  finish: (completion: Completion) => void
}
const caller: Caller = Object.freeze({ principal: 'browser:local', source: 'local' })

/** Standalone development adapter, explicitly separate from desktop IPC. */
export class BrowserTransport implements RuntimeTransport {
  readonly mode = 'browser' as const
  private sessionId?: string
  private receive?: (event: BridgeEvent) => void
  private readonly pending = new Map<string, Pending>()
  private definitions: ToolOptions[] = []
  private validators = new Map<string, { input: ValidateFunction; output: ValidateFunction }>()
  constructor(private readonly options: BrowserOptions = {}) {}

  async connect(definitions: ToolOptions[], receive: (event: BridgeEvent) => void) {
    if (this.sessionId) throw new AiError('BUSY', 'Runtime is already connected')
    jsonCopy(definitions, 'INVALID_DEFINITION')
    const validators = new Map<string, { input: ValidateFunction; output: ValidateFunction }>()
    if (definitions.length > LIMITS.maxTools) throw new AiError('INVALID_DEFINITION', 'Too many tools')
    for (const definition of definitions) {
      validateDefinition(definition)
      if (validators.has(definition.name)) throw new AiError('DUPLICATE_TOOL', 'Duplicate tool')
      validators.set(definition.name, { input: compileSchema(definition.inputSchema), output: compileSchema(definition.outputSchema) })
    }
    this.validators = validators
    this.definitions = JSON.parse(JSON.stringify(definitions))
    this.sessionId = crypto.randomUUID()
    this.receive = receive
    return { tools: this.visibleTools(), pendingCount: 0 }
  }

  private authorize(tool: ToolOptions): void {
    if (tool.policy?.permissions?.length && !this.options.authorize)
      throw new AiError('UNAUTHORIZED', 'This tool requires a host authorizer')
    try {
      if (this.options.authorize && this.options.authorize(caller, JSON.parse(JSON.stringify(tool))) !== true)
        throw new AiError('UNAUTHORIZED', 'Host denied access')
    }
    catch (error) {
      throw error instanceof AiError ? error : new AiError('UNAUTHORIZED', 'Host denied access')
    }
  }
  private visibleTools(): ToolOptions[] {
    return this.definitions.filter((tool) => {
      try { this.authorize(tool); return true } catch { return false }
    }).map((tool) => JSON.parse(JSON.stringify(tool)))
  }

  invoke(requestId: string, name: string, arguments_: JsonValue, signal?: AbortSignal): Promise<JsonValue> {
    const sessionId = this.sessionId
    if (!sessionId) return Promise.reject(new AiError('NOT_READY', 'Runtime is not started'))
    const started = performance.now()
    const emit = (state: 'started' | 'completed' | 'failed', errorCode?: string): void => {
      this.receive?.({ type: 'state', sessionId, pendingCount: this.pending.size, event: {
        requestId, name, caller, source: caller.source, state,
        durationMs: Math.max(0, Math.round(performance.now() - started)),
        ...(errorCode ? { errorCode } : {}),
      } })
    }
    try {
      if (signal?.aborted) throw new AiError('CANCELLED', 'Cancelled before dispatch')
      if (this.pending.has(requestId)) throw new AiError('DUPLICATE_REQUEST', 'Request is already running')
      const tool = this.definitions.find((tool) => tool.name === name)
      if (!tool) throw new AiError('NOT_FOUND', 'Unknown tool: ' + name)
      const timeout = tool.policy?.timeoutMs ?? LIMITS.timeoutMs
      const expires = started + timeout
      this.authorize(tool)
      const validators = this.validators.get(name)!
      const input = validateValue(validators.input, arguments_, 'INVALID_ARGUMENT')
      if (this.pending.size >= LIMITS.maxPending ||
        [...this.pending.values()].filter((p) => p.name === name).length >= (tool.policy?.maxConcurrency ?? LIMITS.maxPending))
        throw new AiError('BUSY', 'Too many active calls')
      if (performance.now() >= expires) throw new AiError('TIMEOUT', 'Deadline expired before dispatch')
      const output = validators.output
      return new Promise((resolve, reject) => {
        let timer: ReturnType<typeof setTimeout>
        const finish = (completion: Completion): void => {
          if (!this.pending.delete(requestId)) return
          clearTimeout(timer)
          signal?.removeEventListener('abort', abort)
          if (completion.status === 'error') {
            const error = completion.error
            this.receive?.({ type: 'cancel', sessionId, requestId, error })
            emit('failed', error.code)
            reject(new AiError(error.code, error.message, error.details))
          } else { emit('completed'); resolve(completion.result) }
        }
        const abort = (): void => finish({ status: 'error', error: new AiError('CANCELLED', 'Call cancelled', { outcome: 'unknown' }).toJSON() })
        this.pending.set(requestId, { name, expires, output, finish })
        signal?.addEventListener('abort', abort, { once: true })
        timer = setTimeout(() => finish({ status: 'error', error: new AiError('TIMEOUT', 'Tool execution timed out', { outcome: 'unknown' }).toJSON() }), Math.max(0, expires - performance.now()))
        emit('started')
        if (this.pending.has(requestId)) {
          this.receive?.({ type: 'call', sessionId, requestId, name, arguments: input, caller,
            deadline: Date.now() + Math.max(0, expires - performance.now()),
          })
        }
      })
    } catch (error) {
      const controlled = errorData(error)
      emit('failed', controlled.code)
      return Promise.reject(controlled)
    }
  }

  async resolve(sessionId: string, requestId: string, completion: Completion): Promise<void> {
    if (this.sessionId !== sessionId) throw new AiError('STALE_SESSION', 'Bridge session is not current')
    const pending = this.pending.get(requestId)
    if (!pending) throw new AiError('STALE_REQUEST', 'Request already ended')
    try {
      if (performance.now() >= pending.expires)
        throw new AiError('TIMEOUT', 'Tool execution timed out', { outcome: 'unknown' })
      jsonCopy(completion, 'INVALID_RESULT')
      if (completion.status === 'success')
        completion = { status: 'success', result: validateValue(pending.output, completion.result, 'INVALID_RESULT') }
      if (performance.now() >= pending.expires)
        throw new AiError('TIMEOUT', 'Tool execution timed out', { outcome: 'unknown' })
      pending.finish(completion)
    } catch (error) { pending.finish({ status: 'error', error: errorData(error).toJSON() }) }
  }

  async disconnect(): Promise<void> {
    for (const pending of this.pending.values())
      pending.finish({ status: 'error', error: new AiError('NOT_READY', 'Runtime disconnected', { outcome: 'unknown' }).toJSON() })
    this.sessionId = undefined
    this.receive = undefined
    this.definitions = []
    this.validators.clear()
  }
}
