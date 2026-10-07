import { provideZonelessChangeDetection } from '@angular/core'
import { bootstrapApplication } from '@angular/platform-browser'
import { isTauri } from '@tauri-apps/api/core'
import { provideMcpRuntime } from 'tauri-plugin-mcp-api/angular'
import { TauriTransport } from 'tauri-plugin-mcp-api/tauri'
import { AppComponent } from './app'
import { MathTools, SystemTools, TextTools } from './tools'

bootstrapApplication(AppComponent, {
  providers: [
    provideZonelessChangeDetection(),
    { provide: TextTools, useFactory: () => new TextTools() },
    MathTools,
    SystemTools,
    provideMcpRuntime({
      modules: [TextTools, MathTools, SystemTools],
      ...(isTauri() ? { transport: new TauriTransport() } : {}),
    }),
  ],
}).catch((error) => {
  console.error(error)
  const message = document.createElement('pre')
  message.textContent = 'Runtime 启动失败：' + String(error)
  document.body.append(message)
})
