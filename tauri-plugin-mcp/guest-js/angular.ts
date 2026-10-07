import {
  DestroyRef,
  inject,
  makeEnvironmentProviders,
  provideAppInitializer,
  type EnvironmentProviders,
  type Type,
} from '@angular/core'
import { McpRuntime } from './runtime'
import type { RuntimeOptions } from './types'

export interface AngularRuntimeOptions extends RuntimeOptions {
  modules: Type<unknown>[]
}

export function provideMcpRuntime(
  options: AngularRuntimeOptions,
): EnvironmentProviders {
  return makeEnvironmentProviders([
    {
      provide: McpRuntime,
      useFactory: () => {
        const runtime = new McpRuntime(options)
        const destroyRef = inject(DestroyRef)
        for (const module of options.modules)
          runtime.registerModule(inject(module) as object)
        destroyRef.onDestroy(() => {
          void runtime.stop()
        })
        return runtime
      },
    },
    provideAppInitializer(() => inject(McpRuntime).start()),
  ])
}
