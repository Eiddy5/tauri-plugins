import { AiError, errorData } from './errors'
import { ToolRegistry } from './registry'
import { validateValue } from './schema'
import type { BridgeEvent, Completion, RuntimeOptions, ToolContext } from './types'

/** Executes admitted calls. The adapter owns authorization and terminal state. */
export class JsExecutor {
  private readonly active = new Map<string, (error: AiError) => void>()
  constructor(private readonly registry: ToolRegistry, private readonly beforeCall: RuntimeOptions['beforeCall']) {}

  stop(): void {
    for (const cancel of this.active.values())
      cancel(new AiError('NOT_READY', 'Execution host stopped', { outcome: 'unknown' }))
  }

  receive(event: Exclude<BridgeEvent, { type: 'state' }>, complete: (completion: Completion) => Promise<void>): void {
    if (event.type === 'cancel') {
      this.active.get(event.requestId)?.(new AiError(event.error.code, event.error.message, event.error.details))
      return
    }
    if (this.active.has(event.requestId)) return
    const controller = new AbortController()
    const remaining = Math.max(0, event.deadline - Date.now())
    const expires = performance.now() + remaining
    const caller = Object.freeze({ ...event.caller })
    const context: ToolContext = Object.freeze({
      requestId: event.requestId, source: caller.source, caller,
      deadline: event.deadline, signal: controller.signal,
    })
    let settled = false
    let timer: ReturnType<typeof setTimeout>
    const finish = (completion: Completion): void => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      this.active.delete(event.requestId)
      if (completion.status === 'error') controller.abort(new AiError(completion.error.code, completion.error.message, completion.error.details))
      void complete(completion).catch(() => { /* Rust may already have ended this request. */ })
    }
    const fail = (error: AiError): void => finish({ status: 'error', error: error.toJSON() })
    const checkDeadline = (): void => {
      if (performance.now() >= expires)
        throw new AiError('TIMEOUT', 'Tool execution timed out', { outcome: 'unknown' })
    }
    this.active.set(event.requestId, fail)
    timer = setTimeout(() => fail(new AiError('TIMEOUT', 'Tool execution timed out', { outcome: 'unknown' })), remaining)
    void Promise.resolve().then(async () => {
      if (settled) return
      checkDeadline()
      const binding = this.registry.get(event.name)
      if (!binding) throw new AiError('NOT_FOUND', 'Unknown tool binding')
      const input = validateValue(binding.input, event.arguments, 'INVALID_ARGUMENT')
      await this.beforeCall?.(JSON.parse(JSON.stringify(binding.definition)), input, context)
      if (settled) return
      checkDeadline()
      const result = await binding.handler(input, context)
      if (settled) return
      checkDeadline()
      const value = validateValue(binding.output, result, 'INVALID_RESULT')
      checkDeadline()
      finish({ status: 'success', result: value })
    }).catch((error) => fail(errorData(error)))
  }
}
