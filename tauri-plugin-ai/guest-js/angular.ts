import {
  DestroyRef,
  inject,
  makeEnvironmentProviders,
  provideAppInitializer,
  type EnvironmentProviders,
  type Type,
} from '@angular/core'
import { AiRuntime } from './runtime'
import type { RuntimeOptions } from './types'

export interface AngularRuntimeOptions extends RuntimeOptions {
  modules: Type<unknown>[]
}

export function provideAiRuntime(
  options: AngularRuntimeOptions,
): EnvironmentProviders {
  return makeEnvironmentProviders([
    {
      provide: AiRuntime,
      useFactory: () => {
        const runtime = new AiRuntime(options)
        const destroyRef = inject(DestroyRef)
        for (const module of options.modules)
          runtime.registerModule(inject(module) as object)
        destroyRef.onDestroy(() => {
          void runtime.stop()
        })
        return runtime
      },
    },
    provideAppInitializer(() => inject(AiRuntime).start()),
  ])
}
