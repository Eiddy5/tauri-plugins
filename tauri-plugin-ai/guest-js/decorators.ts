import { AiError } from './errors'
import type { DefinedTool, ToolContext, ToolOptions } from './types'

const modules = new WeakMap<Function, string>()
const methods = new WeakMap<object, Map<string | symbol, ToolOptions>>()

/** Legacy TypeScript decorators: enable experimentalDecorators. */
export function Module(namespace: string): ClassDecorator {
  return (target) => {
    modules.set(target, namespace)
  }
}

export function Tool(options: ToolOptions): MethodDecorator {
  return (target, key, descriptor) => {
    if (typeof descriptor?.value !== 'function')
      throw new AiError(
        'INVALID_DEFINITION',
        '@Tool requires a method and experimentalDecorators: true',
      )
    let declarations = methods.get(target)
    if (!declarations) methods.set(target, (declarations = new Map()))
    declarations.set(key, options)
  }
}

export function defineTool<I, O>(
  options: ToolOptions & {
    handler: (input: I, context: ToolContext) => O | Promise<O>
  },
): DefinedTool {
  return {
    ...options,
    handler: (input, context) => options.handler(input as I, context),
  }
}

export function toolsFromInstance(instance: object): DefinedTool[] {
  const namespace = modules.get(instance.constructor)
  if (!namespace || !/^[a-zA-Z0-9_-]+$/.test(namespace)) {
    throw new AiError(
      'INVALID_DEFINITION',
      'Module requires a valid @Module namespace',
    )
  }
  const result: DefinedTool[] = []
  const seen = new Set<string | symbol>()
  for (
    let prototype = Object.getPrototypeOf(instance);
    prototype && prototype !== Object.prototype;
    prototype = Object.getPrototypeOf(prototype)
  ) {
    for (const [key, options] of methods.get(prototype) ?? []) {
      if (seen.has(key)) continue
      seen.add(key)
      const handler = (instance as Record<string | symbol, unknown>)[key]
      if (typeof handler !== 'function')
        throw new AiError('INVALID_DEFINITION', 'Tool method is missing')
      result.push({
        ...options,
        name: namespace + '.' + options.name,
        handler: (input, context) => handler.call(instance, input, context),
      })
    }
  }
  if (!result.length)
    throw new AiError('INVALID_DEFINITION', 'Module has no @Tool methods')
  return result
}
