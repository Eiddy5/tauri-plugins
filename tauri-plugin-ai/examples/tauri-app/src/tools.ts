import { inject, Injectable } from '@angular/core'
import {
  AiError,
  Module,
  Tool,
  type ToolContext,
} from 'tauri-plugin-ai-api'

@Injectable({ providedIn: 'root' })
export class TextService {
  echo(text: string) {
    return { text, length: [...text].length }
  }
}

@Injectable()
@Module('text')
export class TextTools {
  private readonly service = inject(TextService)

  @Tool({
    name: 'echo',
    description:
      '通过 Angular Service 返回文本及字符数，验证注解、DI 和实例方法绑定。',
    annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    inputSchema: {
      type: 'object',
      properties: { text: { type: 'string', minLength: 1, maxLength: 2000 } },
      required: ['text'],
      additionalProperties: false,
    },
    outputSchema: {
      type: 'object',
      properties: { text: { type: 'string' }, length: { type: 'integer' } },
      required: ['text', 'length'],
      additionalProperties: false,
    },
  })
  echo(input: { text: string }) {
    return this.service.echo(input.text)
  }
}

@Injectable()
@Module('math')
export class MathTools {
  @Tool({
    name: 'add',
    description: '将两个数相加，返回结构化结果。字符串参数会被 Schema 拒绝。',
    annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    inputSchema: {
      type: 'object',
      properties: { a: { type: 'number' }, b: { type: 'number' } },
      required: ['a', 'b'],
      additionalProperties: false,
    },
    outputSchema: {
      type: 'object',
      properties: { sum: { type: 'number' } },
      required: ['sum'],
      additionalProperties: false,
    },
  })
  add(input: { a: number; b: number }) {
    return { sum: input.a + input.b }
  }
}

@Injectable()
@Module('system')
export class SystemTools {
  @Tool({
    name: 'wait',
    description: '等待指定毫秒数。可主动取消；超过 1.5 秒时由框架终止等待。',
    annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    policy: { timeoutMs: 1500 },
    inputSchema: {
      type: 'object',
      properties: { delayMs: { type: 'integer', minimum: 0, maximum: 10000 } },
      required: ['delayMs'],
      additionalProperties: false,
    },
    outputSchema: {
      type: 'object',
      properties: { waitedMs: { type: 'integer' } },
      required: ['waitedMs'],
      additionalProperties: false,
    },
  })
  wait(
    input: { delayMs: number },
    context: ToolContext,
  ): Promise<{ waitedMs: number }> {
    return new Promise((resolve, reject) => {
      const abort = () => {
        clearTimeout(timer)
        reject(new AiError('CANCELLED', '等待已取消'))
      }
      const timer = setTimeout(() => {
        context.signal.removeEventListener('abort', abort)
        resolve({ waitedMs: input.delayMs })
      }, input.delayMs)
      context.signal.addEventListener('abort', abort, { once: true })
      if (context.signal.aborted) abort()
    })
  }

  @Tool({
    name: 'fail',
    description: '主动返回一个可识别的业务错误，演示稳定错误码。',
    annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    inputSchema: { type: 'object', additionalProperties: false },
    outputSchema: { type: 'object', additionalProperties: false },
  })
  fail(): never {
    throw new AiError('DEMO_ERROR', '这是一个主动触发的示例错误。')
  }
}
